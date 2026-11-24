//! AE2's network storage on the server (stage 3): the cells of a grid's drives and ME chests mounted as one
//! `NetworkStorage`, the terminals' viewers and the items streamed to them, and what their clicks do.
//!
//! The logic is the upstream port's (`ae2-horizon/mixins/ae2/me.rs`: `with_storage`, `storage_open`/`storage_close`,
//! `stream_terminals`, `terminal_action`, `crafting_terminal_take`, the ME chest's terminal), on the fork's API:
//! - cells are read from and written to the drives' slots through `BlockEntityRef::item_slot`/`set_item_slot` in a
//!   transaction (the block entity a hook has out of the world comes in as `own`);
//! - a click on the item grid is a menu button (`menu_button`, with the player and the cursor stack); a shift-click
//!   from the inventory is taken by `menu_quick_move` and made at the level's tick end, through `ItemPlace::PlayerSlot`
//!   in a transaction (the hook can't say the whole stack moved);
//! - the list goes to the viewer on the `ae2:items` channel (only what changed, at most 29 000 bytes a message), and
//!   the crafting terminal's result on `ae2:extras`;
//! - the crafting grid's result is worked out by the game's own recipe matcher (`LevelRef::recipe_for`).
//!
//! Multiplayer rules (upstream PLAN.md Round 24): a click acts only for a player who has that terminal's menu open,
//! in this level and in reach; clicks are rate limited; the crafting result is no slot: taking it works the recipe
//! out on the server again.

use crate::cells::*;
use crate::core::{Actionable, AeKey, BasicCellInventory, KeyCounter, KeyType};
use crate::grid::{MeStorage, NetworkStorage};
use crate::util::*;
use mod_api::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

pub(crate) const NET_ITEMS: &str = "ae2:items";
pub(crate) const NET_EXTRAS: &str = "ae2:extras";

/// Menu buttons of the terminals (the grid's are `index << 3 | action`).
pub(crate) const RIGHT: i32 = 1;
pub(crate) const SHIFT: i32 = 2;
pub(crate) const MOVE_REGION: i32 = 4;
pub(crate) const ROLL_DOWN: i32 = 5;
pub(crate) const ROLL_UP: i32 = 6;
pub(crate) const CREATIVE_DUPLICATE: i32 = 7;
/// A click on the grid where no item is (the cursor's stack goes in).
pub(crate) const EMPTY_CELL: usize = (1 << 23) - 1;
/// CraftingTermMenu: clear the grid to the network / to the inventory; take the result (shift: as many as it makes).
pub(crate) const CT_STASH: i32 = (1 << 27) | 1;
pub(crate) const CT_STASH_PLAYER: i32 = (1 << 27) | 2;
pub(crate) const CT_TAKE: i32 = (1 << 27) | 3;
pub(crate) const CT_TAKE_ALL: i32 = (1 << 27) | 4;

/// The crafting terminal's container: slot 0 the insert slot, 1..10 the grid, 10 the result (no slot), then view cells.
pub(crate) const CT_GRID: std::ops::Range<usize> = 1..10;
pub(crate) const VIEW_CELLS: usize = 5;

/// Most messages a viewer is sent a tick (the rest wait): the game takes 32 a player a tick.
const MESSAGES_PER_TICK: usize = 8;
const MESSAGE_BYTES: usize = 29_000;
/// Most of a key's data one entry sends.
const ENTRY_DATA_BYTES: usize = 8 * 1024;
/// Menu buttons a player may send a second (a click is one; a held key repeats).
const BUTTONS_PER_SECOND: u32 = 60;

pub(crate) fn is_terminal(block: &str) -> bool {
    matches!(block, "ae2:terminal" | "ae2:crafting_terminal" | "ae2:pattern_encoding_terminal")
}

pub(crate) fn is_holder(block: &str) -> bool {
    matches!(block, "ae2:drive" | "ae2:chest")
}

pub(crate) fn holder_slots(block: &str) -> u32 {
    if block == "ae2:drive" { 10 } else { 1 }
}

// ---- A mounted cell

/// What a drive or chest slot holds as storage: a storage cell's inventory, or a creative cell (AE2's
/// CreativeCellInventory: what its partition names, without end).
pub(crate) enum Cell {
    Basic(BasicCellInventory),
    Creative(Vec<AeKey>),
    /// A storage bus's container (stage 4), mounted for one call: what it held when mounted, its changes written
    /// back through the face when unmounted (`parts::ExtStore`).
    External(crate::parts::ExtStore),
    /// A formation plane (FormationPlanePart's storage), mounted for one call like a storage bus.
    Formation(crate::planes::Formation),
    /// fixA: a storage bus on an unconfigured ME interface: that interface's network (`NetMount`).
    Net(Box<NetMount>),
}

impl MeStorage for Cell {
    fn insert(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
        match self {
            Cell::Basic(b) => b.insert(what, amount, mode),
            Cell::Creative(keys) => if keys.contains(what) { amount } else { 0 },
            Cell::External(e) => e.insert(what, amount, mode),
            Cell::Formation(f) => f.insert(what, amount, mode),
            Cell::Net(n) => n.insert(what, amount, mode),
        }
    }
    fn extract(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
        match self {
            Cell::Basic(b) => b.extract(what, amount, mode),
            Cell::Creative(keys) => if keys.contains(what) { amount } else { 0 },
            Cell::External(e) => e.extract(what, amount, mode),
            Cell::Formation(f) => f.extract(what, amount, mode),
            Cell::Net(n) => n.extract(what, amount, mode),
        }
    }
    fn available_stacks(&self, out: &mut KeyCounter) {
        match self {
            Cell::Basic(b) => b.available_stacks(out),
            Cell::Creative(keys) => keys.iter().for_each(|k| out.add(k, i64::from(i32::MAX))),
            Cell::External(e) => e.available_stacks(out),
            Cell::Formation(f) => f.available_stacks(out),
            Cell::Net(n) => n.available_stacks(out),
        }
    }
    fn is_preferred_for(&self, what: &AeKey) -> bool {
        match self {
            Cell::Basic(b) => MeStorage::is_preferred_for(b, what),
            Cell::Creative(keys) => keys.contains(what),
            Cell::External(e) => e.is_preferred_for(what),
            Cell::Formation(f) => f.is_preferred_for(what),
            Cell::Net(n) => n.is_preferred_for(what),
        }
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// A grid's (or a chest's own) mounted cells: where each is (level, holder, slot; fixA: the level, for grids over
/// several levels) with the id and version written on it.
#[derive(Default)]
pub(crate) struct Mounted {
    storage: NetworkStorage<Cell>,
    cells: Vec<(&'static str, Pos, usize, i64, i64)>,
}

/// One viewer's list as last sent.
#[derive(Default)]
pub(crate) struct Sent {
    keys: Vec<(AeKey, i64)>,
    index: HashMap<AeKey, usize>,
    free: Vec<usize>,
    generation: u64,
    full_after: u64,
    pending: VecDeque<Vec<u8>>,
    extras: u64,
}

/// A player with a terminal (or drive, chest) menu open.
pub(crate) struct Viewer {
    pub(crate) dim: &'static str,
    pub(crate) pos: Pos,
    pub(crate) block: String,
    /// The ME chest's own terminal (its cell only).
    pub(crate) chest_term: bool,
    /// The menu's window id.
    window: Option<i32>,
    /// A wireless terminal's (`wireless`): no menu and no reach to the block; the wireless session checks the item,
    /// the range and the power instead (`wireless::session_ok`).
    pub(crate) remote: bool,
    /// The store's tick when it opened.
    opened_tick: u64,
    sent: Sent,
}


#[derive(Default)]
pub(crate) struct Store {
    pub(crate) viewers: HashMap<u128, Viewer>,
    /// Mounted storage by level and key (a grid's first holder; a chest's own cell is keyed apart).
    mounted: HashMap<(&'static str, Pos, bool), Mounted>,
    /// Cells unmounted lately, by id: version, inventory, tick (read again when the version moved on).
    cache: HashMap<i64, (i64, Cell, u64)>,
    next_cell_id: i64,
    /// A counter per storage key, bumped when what it holds changed (terminals list again).
    generation: HashMap<(&'static str, Pos, bool), u64>,
    /// Stacks for players' inventories (a shift-click out of the terminal), what doesn't fit goes back.
    gives: Vec<(u128, &'static str, Pos, ItemStack)>,
    /// Shift-clicks into a terminal not stored yet: player, level, terminal, chest, inventory slot, the stack clicked,
    /// how much the network said it takes. Stored at the player's tick end once the slot shows the items left it.
    moves: Vec<PendingMove>,
    /// Holders whose cells' looks changed: their update tag goes to clients at the tick's end.
    pub(crate) syncs: HashSet<(&'static str, Pos)>,
    /// Holders' cell looks as last synced.
    looks: HashMap<(&'static str, Pos), Vec<String>>,
    rates: HashMap<u128, (std::time::Instant, u32)>,
    tick: u64,
    /// fixF: cells written by `close` in a level other than their holder's (the host writes a block entity only in
    /// the level in place): put on their holders at that level's next tick (`flush_held`), read in their stead until
    /// then (`held_over`).
    held: HashMap<CellAt, ItemStack>,
}

#[derive(Clone)]
pub(crate) struct PendingMove {
    player: u128,
    dim: &'static str,
    pos: Pos,
    chest: bool,
    slot: i32,
    stack: ItemStack,
    accept: u32,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

pub(crate) fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Store::default))
}

pub(crate) fn forget_all() {
    *STORE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// A holder's priority changed (`priority`): the level's grids mount their cells again, each with its holder's
/// priority (what the cells hold was written onto them after each use, so nothing is lost).
pub(crate) fn remount(dim: &'static str) {
    with_store(|s| s.mounted.retain(|(d, _, chest), _| *d != dim || *chest));
}

// ---- Opening storage

/// Where storage is asked for: a grid (by any of its nodes), or an ME chest's own cell.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Grid(Pos),
    Chest(Pos),
}

/// A block entity out of the world during this hook (its position and fields): read from there.
pub(crate) type Own<'a> = Option<(Pos, &'a mut Fields)>;

fn read_slot(host: &dyn Host, level: LevelRef, own: &Own<'_>, pos: Pos, slot: usize) -> Option<ItemStack> {
    match own {
        Some((p, e)) if *p == pos => e.items.get(slot).cloned().flatten(),
        _ => crate::bus::item_slot(host, level, pos, slot),
    }
}

// fixA: a grid's storage over every level its quantum bridges reach (`net::grid_devices_all`), storage buses over a
// cached container (`ext_cache`) or over an ME interface's whole network (InterfaceLogic.getInventory without
// config: "recursive networking"), and AE2's NetworkStorage.mountsInUse: a network asked again while it is being
// asked (a loop of buses and interfaces) answers nothing, and one already mounted in this call isn't mounted twice.

/// A network as `open` found it, until `close`.
pub(crate) struct Opened {
    /// The store's key for the mounted cells (the first holder's level and position; a chest's own cell apart).
    mkey: (&'static str, Pos, bool),
    /// The guard's name for the network (its lowest level and root; a chest's own cell apart).
    ident: (&'static str, Pos, bool),
    mounted: Mounted,
    found: Vec<(&'static str, Pos, usize, ItemStack)>,
    list: Vec<(&'static str, Pos, String)>,
    remounted: bool,
    volatile: Vec<u64>,
}

/// The networks open on this thread: the chain being asked now, and every one mounted since the outer call began.
#[derive(Default)]
struct Guard {
    depth: u32,
    chain: Vec<(&'static str, Pos, bool)>,
    seen: HashSet<(&'static str, Pos, bool)>,
}

thread_local! {
    static GUARD: std::cell::RefCell<Guard> = std::cell::RefCell::new(Guard::default());
}

/// A storage bus on an unconfigured interface: the interface's network, through the bus's filter, access and
/// priority (StorageBusPart's MEInventoryHandler over InterfaceLogic's network storage).
pub(crate) struct NetMount {
    filter: crate::parts::BusFilter,
    inner: Box<Opened>,
}

impl MeStorage for NetMount {
    fn insert(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
        if amount <= 0 || self.filter.access == 1 || !self.filter.allows(what) {
            return 0;
        }
        // The outer network notes what came in for its CPUs; the inner one mustn't note it again.
        let was = crate::grid::CPU_WATCH.with_borrow_mut(|w| std::mem::replace(&mut w.quiet, true));
        let went = self.inner.mounted.storage.insert(what, amount, mode);
        crate::grid::CPU_WATCH.with_borrow_mut(|w| w.quiet = was);
        if self.filter.void { amount } else { went }
    }
    fn extract(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
        if amount <= 0 || self.filter.access == 2 || !(self.filter.no_extract_filter || self.filter.allows(what)) {
            return 0;
        }
        self.inner.mounted.storage.extract(what, amount, mode)
    }
    fn available_stacks(&self, out: &mut KeyCounter) {
        // fixG: MEInventoryHandler over the interface's network (no StorageAdapter, so no probe: that network already
        // lists only what its own buses report): Extractable Only with Filter On Extract lists nothing through an
        // insert-only bus and only the filter's keys otherwise; else all of it.
        let f = &self.filter;
        let filter_listed = !f.report_all && !f.no_extract_filter;
        if filter_listed && f.access == 2 {
            return;
        }
        let mut all = KeyCounter::new();
        self.inner.mounted.storage.available_stacks(&mut all);
        for (k, n) in all.iter() {
            if n > 0 && (!filter_listed || f.allows(k)) {
                out.add(k, n);
            }
        }
    }
    fn is_preferred_for(&self, what: &AeKey) -> bool {
        self.filter.prefers(what)
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// The holders (drives, chests) of the grid `pos` is on in every level it reaches, if it is online; the chest alone
/// for its own terminal.
fn holders(dim: &'static str, scope: Scope) -> Option<Vec<(&'static str, Pos, String)>> {
    match scope {
        Scope::Chest(pos) => {
            let (_, online, _) = crate::net::device_state(dim, pos);
            online.then(|| vec![(dim, pos, "ae2:chest".to_owned())])
        }
        Scope::Grid(pos) => {
            let mut list = crate::net::grid_devices_all(dim, pos, true)?;
            list.retain(|(_, _, b)| is_holder(b));
            list.sort();
            Some(list)
        }
    }
}

/// The own block entity's fields when `pos` in `d` is it.
fn own_is(own: &Own<'_>, host_dim: &'static str, d: &'static str, pos: Pos) -> bool {
    d == host_dim && matches!(own, Some((p, _)) if *p == pos)
}

/// Opens `scope`'s storage: its cells mounted (kept while they stay where they are), its storage buses' containers
/// and interfaces' networks, its formation planes. None when the network is off or already being asked.
pub(crate) fn open(host: &mut dyn ServerHost, dim: &'static str, scope: Scope, own: &mut Own<'_>) -> Option<Opened> {
    let host_dim = host.dimension();
    let list = holders(dim, scope)?;
    let chest = matches!(scope, Scope::Chest(_));
    let ident = match scope {
        Scope::Chest(p) => (dim, p, true),
        Scope::Grid(p) => {
            let (d, r) = crate::net::grid_key(dim, p).unwrap_or((dim, p));
            (d, r, false)
        }
    };
    if !guard_enter(ident) {
        return None;
    }
    let (kd, key) = list.first().map_or((dim, [i32::MIN; 3]), |h| (h.0, h.1));
    let mkey = (kd, key, chest);
    // The cells there now: where, which kind, the id and version written on each.
    let mut found: Vec<(&'static str, Pos, usize, ItemStack)> = Vec::new();
    for (d, pos, block) in &list {
        let Some(level) = host.level(d) else { continue };
        for slot in 0..holder_slots(block) as usize {
            let stack = if own_is(own, host_dim, d, *pos) { read_slot(&*host, level, own, *pos, slot) } else { read_slot(&*host, level, &None, *pos, slot) };
            // fixF: a write held for this slot (made from another level) is what the cell holds now.
            let stack = with_store(|s| held_over(&mut s.held, (*d, *pos, slot), stack, same_cell));
            if let Some(stack) = stack
                && is_cell_item(sid(&stack))
            {
                found.push((*d, *pos, slot, stack));
            }
        }
    }
    let tick = with_store(|s| s.tick);
    let mut mounted = with_store(|s| s.mounted.remove(&mkey)).unwrap_or_default();
    let ids: Vec<Option<(i64, i64)>> = found.iter().map(|(_, _, _, st)| cell_id(st)).collect();
    let same = mounted.cells.len() == found.len()
        && mounted.cells.iter().zip(found.iter().zip(&ids)).all(|(m, ((d, p, sl, _), id))| m.0 == *d && m.1 == *p && m.2 == *sl && *id == Some((m.3, m.4)));
    let mut remounted = false;
    if !same {
        debug(&format!("remount {:?}: had {} cells, found {}", key, mounted.cells.len(), found.len()));
        remounted = true;
        for (i, (_, _, _, cid, ver)) in mounted.cells.drain(..).enumerate() {
            if let Some(inv) = mounted.storage.unmount(i as u64) {
                with_store(|s| s.cache.insert(cid, (ver, inv, tick)));
            }
        }
        mounted.storage = NetworkStorage::new();
        for ((d, pos, slot, stack), id) in found.iter().zip(&ids) {
            let taken = |c: &Mounted, i: i64| c.cells.iter().any(|m| m.3 == i);
            let cached = id.filter(|(i, _)| !taken(&mounted, *i)).and_then(|(i, v)| {
                with_store(|s| match s.cache.remove(&i) {
                    Some((ver, inv, _)) if ver == v => Some((i, v, inv)),
                    _ => None,
                })
            });
            let (cid, ver, inv) = match cached {
                Some(c) => c,
                None => {
                    let inv = match cell_type(sid(stack)) {
                        Some(cell) => Cell::Basic(cell_inventory(stack, cell)),
                        None => Cell::Creative(cell_config(stack)),
                    };
                    match id.filter(|(i, _)| !taken(&mounted, *i)) {
                        Some((i, v)) => (i, v, inv),
                        // A new cell (or a copy of one mounted already): its own id, written below (version -1).
                        None => (new_cell_id(), -1, inv),
                    }
                }
            };
            let priority = match host.level(d) {
                Some(level) if own_is(own, host_dim, d, *pos) => holder_priority(&*host, level, own, *pos),
                Some(level) => holder_priority(&*host, level, &None, *pos),
                None => 0,
            };
            mounted.storage.mount(mounted.cells.len() as u64, priority, inv);
            mounted.cells.push((*d, *pos, *slot, cid, ver));
        }
        // Old cache entries go after a minute.
        with_store(|s| s.cache.retain(|_, (_, _, t)| tick.saturating_sub(*t) < 1200));
    }
    let mut volatile = Vec::new();
    if !chest {
        let devices = match scope {
            Scope::Grid(p) => crate::net::grid_devices_all(dim, p, true).unwrap_or_default(),
            Scope::Chest(_) => Vec::new(),
        };
        // Storage buses (StorageBusPart.getMEStorage): mounted volatile for this call, after the cells.
        for (i, (priority, ext)) in crate::parts::open_externals(host, &devices).into_iter().enumerate() {
            let id = EXTERNAL_IDS + i as u64;
            match ext {
                crate::parts::ExtOpen::Store(e) => mounted.storage.mount_volatile(id, priority, Cell::External(e)),
                crate::parts::ExtOpen::Interface { filter, dim: d, iface } => {
                    // The interface's own network, unless it is being asked already (a loop) or mounted once in
                    // this call (it counts once).
                    let Some(inner) = open(host, d, Scope::Grid(iface), &mut None) else { continue };
                    mounted.storage.mount_volatile(id, priority, Cell::Net(Box::new(NetMount { filter, inner: Box::new(inner) })));
                }
            }
            volatile.push(id);
        }
        // Formation planes, mounted the same way (their blocks placed and items thrown when unmounted); this
        // level's only (the planes act through this level's host).
        let here_devices: Vec<(Pos, String)> = devices.iter().filter(|d| d.0 == host_dim).map(|d| (d.1, d.2.clone())).collect();
        for (i, (priority, f)) in crate::planes::open_formations(host, host_dim, &here_devices).into_iter().enumerate() {
            let id = EXTERNAL_IDS + 4096 + i as u64;
            mounted.storage.mount_volatile(id, priority, Cell::Formation(f));
            volatile.push(id);
        }
    }
    Some(Opened { mkey, ident, mounted, found, list, remounted, volatile })
}

/// Closes what `open` opened: the volatile mounts unmounted (formation planes act, interfaces' networks close),
/// changed cells written back onto their items in their levels, the store's lists told. Whether anything changed.
pub(crate) fn close(host: &mut dyn ServerHost, o: Opened, own: &mut Own<'_>) -> bool {
    let host_dim = host.dimension();
    let Opened { mkey, ident, mut mounted, found, list, remounted, volatile } = o;
    let mut ext_changed = false;
    for id in volatile {
        match mounted.storage.unmount(id) {
            Some(Cell::External(ext)) => ext_changed |= crate::parts::close_external(host, ext),
            Some(Cell::Formation(f)) => crate::planes::close_formation(host, f),
            Some(Cell::Net(n)) => ext_changed |= close(host, *n.inner, &mut None),
            _ => {}
        }
    }
    // Cells that changed (or are new) are saved on their items, a new version each time.
    let mut changed = remounted || ext_changed;
    let mut writes: Vec<(&'static str, Pos, usize, ItemStack)> = Vec::new();
    for i in 0..mounted.cells.len() {
        let Some(Cell::Basic(cell)) = mounted.storage.get_mut(i as u64) else {
            let (d, pos, slot, cid, ver) = mounted.cells[i];
            if ver < 0 && let Some((_, _, _, st)) = found.iter().find(|(fd, p, s, _)| *fd == d && *p == pos && *s == slot) {
                let mut st = st.clone();
                set_cell_id(&mut st, cid, 0, 1);
                mounted.cells[i].4 = 0;
                writes.push((d, pos, slot, st));
            }
            continue;
        };
        let contents = cell.persist();
        let (d, pos, slot, cid, ver) = mounted.cells[i];
        if contents.is_none() && ver >= 0 {
            continue;
        }
        changed |= contents.is_some();
        let status = status_number(cell.status());
        let ver = ver + 1;
        mounted.cells[i].4 = ver;
        let Some((_, _, _, stack)) = found.iter().find(|(fd, p, s, _)| *fd == d && *p == pos && *s == slot) else { continue };
        let mut stack = stack.clone();
        if let Some(contents) = &contents {
            write_cell(&mut stack, contents);
        }
        set_cell_id(&mut stack, cid, ver, status);
        writes.push((d, pos, slot, stack));
    }
    for (d, pos, slot, stack) in writes {
        let wrote = if own_is(own, host_dim, d, pos) {
            if let Some((_, e)) = own.as_mut()
                && let Some(s) = e.items.get_mut(slot)
            {
                *s = Some(stack.clone());
            }
            true
        } else if d == host_dim
            && let Some(level) = host.level(d)
            && let Some(be) = host.block_entity(level, pos)
        {
            host.transaction(|host, tx| be.set_item_slot(host, tx, slot as u32, Some(stack.clone())).map(|_| ())).is_ok()
        } else {
            // fixF: a holder in another level can't be written from here (the host refuses it as stale): before, the
            // write was lost, the next `open` found the cell's old version on its item and mounted the old contents
            // again (what went in from here was gone, what came out came back). Held for that level's tick instead.
            false
        };
        if !wrote {
            debug(&format!("fixF: cell {d} {pos:?} slot {slot} held for its level's tick"));
        }
        with_store(|s| {
            after_write(&mut s.held, (d, pos, slot), stack, wrote);
            s.syncs.insert((d, pos));
        });
    }
    let chest = mkey.2;
    with_store(|s| {
        s.mounted.insert(mkey, mounted);
        if changed {
            // The grid's and each chest's own lists both show these cells.
            *s.generation.entry(mkey).or_insert(0) += 1;
            for (d, pos, block) in &list {
                if block == "ae2:chest" {
                    *s.generation.entry((d, *pos, true)).or_insert(0) += 1;
                }
                *s.generation.entry((d, [i32::MIN; 3], false)).or_insert(0) += 1;
            }
            if chest {
                for g in s.generation.iter_mut().filter(|((d, _, c), _)| *d == mkey.0 && !*c) {
                    *g.1 += 1;
                }
            }
        }
    });
    guard_leave(ident);
    changed
}

// ---- fixF: cells written from another level

/// fixF: a cell's place: its holder's level and position, and the slot.
pub(crate) type CellAt = (&'static str, Pos, usize);

/// fixF: what a holder's slot holds as `open` sees it: a write held for it (`close` in another level) over what the
/// slot still shows while it is the same cell (`same(read, held)`); a held write whose cell has left the slot is
/// dropped. An unreadable slot (`None`: its chunk unloaded) keeps the held write for later.
pub(crate) fn held_over<S: Clone>(held: &mut HashMap<CellAt, S>, at: CellAt, read: Option<S>, same: impl Fn(&S, &S) -> bool) -> Option<S> {
    let read = read?;
    let Some(h) = held.get(&at) else { return Some(read) };
    if same(&read, h) {
        return Some(h.clone());
    }
    held.remove(&at);
    Some(read)
}

/// fixF: a cell `close` wrote (`wrote`: onto its holder now) or holds for its holder's level.
pub(crate) fn after_write<S>(held: &mut HashMap<CellAt, S>, at: CellAt, stack: S, wrote: bool) {
    if wrote {
        held.remove(&at);
    } else {
        held.insert(at, stack);
    }
}

/// fixF: whether `held` is a later write of the cell `read` shows: the same item, and the same cell id with a version
/// no older (or a new cell, `read` without an id yet).
fn same_cell(read: &ItemStack, held: &ItemStack) -> bool {
    read.item() == held.item()
        && match (cell_id(read), cell_id(held)) {
            (Some(r), Some(h)) => r.0 == h.0 && h.1 >= r.1,
            (None, _) => true,
            (Some(_), None) => false,
        }
}

/// fixF: the writes held for this level's holders put on them (its tick, before its block entities and players
/// use the cells). One whose cell has left its slot is dropped; one in an unloaded chunk waits.
pub(crate) fn flush_held(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let list: Vec<(CellAt, ItemStack)> = with_store(|s| s.held.iter().filter(|(a, _)| a.0 == dim).map(|(a, st)| (*a, st.clone())).collect());
    for ((d, pos, slot), stack) in list {
        if !level.is_loaded(&*host, real(pos)) {
            continue;
        }
        let done = match crate::bus::item_slot(&*host, level, pos, slot) {
            Some(now) if same_cell(&now, &stack) => crate::bus::set_item_slot(host, level, pos, slot, Some(stack)),
            _ => {
                debug(&format!("fixF: cell {d} {pos:?} slot {slot} left its holder before its held write"));
                true
            }
        };
        if done {
            with_store(|s| {
                s.held.remove(&(d, pos, slot));
                s.syncs.insert((d, pos));
            });
        }
    }
}

/// A network is being opened: false when it is open already up the chain (a loop) or was mounted once in this call.
fn guard_enter(ident: (&'static str, Pos, bool)) -> bool {
    GUARD.with_borrow_mut(|g| {
        if g.chain.is_empty() {
            g.seen.clear();
        }
        if g.chain.contains(&ident) || g.seen.contains(&ident) {
            return false;
        }
        g.chain.push(ident);
        g.seen.insert(ident);
        true
    })
}

fn guard_leave(ident: (&'static str, Pos, bool)) {
    GUARD.with_borrow_mut(|g| {
        if let Some(i) = g.chain.iter().rposition(|c| *c == ident) {
            g.chain.remove(i);
        }
    });
}

/// Runs `f` on the storage of `scope`: `open`, `f`, `close`. None when the network is off (or already being asked:
/// AE2's mountsInUse).
pub(crate) fn with_storage<R>(
    host: &mut dyn ServerHost,
    scope: Scope,
    own: Own<'_>,
    f: impl FnOnce(&mut NetworkStorage<Cell>) -> R,
) -> Option<R> {
    let dim = host.dimension();
    with_storage_in(host, dim, scope, own, f)
}

/// fixD: `with_storage` on `scope` in level `dim`, whichever level the host is in (a crafting CPU in another level of
/// the grid: quantum bridges, `xlink`).
pub(crate) fn with_storage_in<R>(
    host: &mut dyn ServerHost,
    dim: &'static str,
    scope: Scope,
    mut own: Own<'_>,
    f: impl FnOnce(&mut NetworkStorage<Cell>) -> R,
) -> Option<R> {
    // CraftingServiceStorage: what goes in here is noted for the CPUs of this grid.
    let watch_grid = match scope {
        Scope::Grid(p) => crate::net::device_state(dim, p).0.unwrap_or(usize::MAX),
        Scope::Chest(_) => usize::MAX,
    };
    crate::grid::CPU_WATCH.with_borrow_mut(|w| {
        w.grid = watch_grid;
        w.dim = dim; // fixD
    });
    struct Depth;
    impl Drop for Depth {
        fn drop(&mut self) {
            GUARD.with_borrow_mut(|g| g.depth = g.depth.saturating_sub(1));
        }
    }
    GUARD.with_borrow_mut(|g| {
        if g.depth == 0 {
            // An outer call: nothing can be open (a panic may have left the chain behind).
            g.chain.clear();
        }
        g.depth += 1;
    });
    let _depth = Depth;
    let mut o = open(host, dim, scope, &mut own)?;
    // The storage buses move items live (`parts::ExtStore`), through this host.
    let result = crate::ext_cache::with_host(host, || f(&mut o.mounted.storage));
    close(host, o, &mut own);
    Some(result)
}

/// A storage bus saw its container change from outside: the level's terminals list again.
pub(crate) fn bump_generation(dim: &'static str) {
    with_store(|s| *s.generation.entry((dim, [i32::MIN; 3], false)).or_insert(0) += 1);
}

/// Storage ids of the storage buses' containers (after the cells').
const EXTERNAL_IDS: u64 = 1_000_000;

fn new_cell_id() -> i64 {
    with_store(|s| {
        if s.next_cell_id == 0 {
            s.next_cell_id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_nanos() as i64 & 0x3FFF_FFFF_FFFF_FFFF);
        }
        s.next_cell_id += 1;
        s.next_cell_id
    })
}

/// A drive's or chest's priority (IPriorityHost; its dialog comes with stage 4).
fn holder_priority(host: &dyn Host, level: LevelRef, own: &Own<'_>, pos: Pos) -> i32 {
    match own {
        Some((p, e)) if *p == pos => nbt_i64(&e.nbt, "priority") as i32,
        _ => crate::bus::nbt_at(host, level, pos).map_or(0, |n| nbt_i64(&n, "priority") as i32),
    }
}

/// What a storage key's list counts by (a change makes its terminals list again).
fn generation(dim: &'static str, scope: Scope) -> u64 {
    let dims: Vec<&'static str> = match scope {
        Scope::Grid(p) => crate::net::grid_parts(dim, p).into_iter().map(|x| x.0).collect(),
        Scope::Chest(_) => Vec::new(),
    };
    with_store(|s| match scope {
        Scope::Chest(pos) => s.generation.get(&(dim, pos, true)).copied().unwrap_or(0),
        // fixA: a grid over several levels counts every level's changes.
        Scope::Grid(_) => s.generation.iter().filter(|((d, _, c), _)| (*d == dim || dims.contains(d)) && !*c).map(|(_, g)| *g).sum(),
    })
}

// ---- Power (StorageHelper.poweredInsert / poweredExtraction)

/// How much of a key one AE moves (AEKeyType.transferFactor): fluids by the bucket.
fn transfer_factor(key: &AeKey) -> f64 {
    if key.key_type() == KeyType::Fluids { 1000.0 } else { 1.0 }
}

/// How much of `want` the grid's power can move: `availablePower * factor + 0.9`.
pub(crate) fn power_allows(dim: &'static str, pos: Pos, key: &AeKey, want: i64) -> i64 {
    if want <= 0 {
        return 0;
    }
    let stored = crate::net::grid_power(dim, pos).map_or(0.0, |(e, _)| e);
    let factor = transfer_factor(key);
    let need = want as f64 / factor * crate::config::config().power_usage_multiplier;
    if stored >= need { want } else { want.min((stored * factor + 0.9) as i64) }
}

pub(crate) fn pay_moved(dim: &'static str, pos: Pos, key: &AeKey, moved: i64) {
    if moved > 0 {
        crate::net::extract_power(dim, pos, moved as f64 / transfer_factor(key) * crate::config::config().power_usage_multiplier);
    }
}

/// A powered insert into `scope`'s storage: how much went in.
pub(crate) fn insert(host: &mut dyn ServerHost, scope: Scope, own: Own<'_>, key: &AeKey, amount: i64) -> i64 {
    crate::parts::note_max_stack(&*host, key);
    crate::planes::note_key(&*host, key);
    let dim = host.dimension();
    let at = scope_pos(scope);
    with_storage(host, scope, own, |st| {
        let fits = st.insert(key, amount, Actionable::Simulate);
        let want = power_allows(dim, at, key, fits);
        let done = if want > 0 { st.insert(key, want, Actionable::Modulate) } else { 0 };
        pay_moved(dim, at, key, done);
        done
    })
    .unwrap_or(0)
}

/// A powered extraction from `scope`'s storage: how much came out.
pub(crate) fn extract(host: &mut dyn ServerHost, scope: Scope, own: Own<'_>, key: &AeKey, amount: i64) -> i64 {
    let dim = host.dimension();
    let at = scope_pos(scope);
    with_storage(host, scope, own, |st| {
        let has = st.extract(key, amount, Actionable::Simulate);
        let want = power_allows(dim, at, key, has);
        let done = if want > 0 { st.extract(key, want, Actionable::Modulate) } else { 0 };
        pay_moved(dim, at, key, done);
        done
    })
    .unwrap_or(0)
}

/// How much of `key` `scope` would take (`insert`) or give (`!insert`), power aside.
pub(crate) fn simulate(host: &mut dyn ServerHost, scope: Scope, own: Own<'_>, key: &AeKey, amount: i64, insert: bool) -> i64 {
    crate::parts::note_max_stack(&*host, key);
    crate::planes::note_key(&*host, key);
    with_storage(host, scope, own, |st| if insert { st.insert(key, amount, Actionable::Simulate) } else { st.extract(key, amount, Actionable::Simulate) }).unwrap_or(0)
}

fn scope_pos(scope: Scope) -> Pos {
    match scope {
        Scope::Grid(p) | Scope::Chest(p) => p,
    }
}

/// What `scope` holds, most first.
pub(crate) fn contents(host: &mut dyn ServerHost, scope: Scope, own: Own<'_>) -> Vec<(AeKey, i64)> {
    let mut items: Vec<(AeKey, i64)> =
        with_storage(host, scope, own, |st| st.contents().iter().map(|(k, a)| (k.clone(), a)).filter(|(_, a)| *a > 0).collect()).unwrap_or_default();
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.id().cmp(b.0.id())));
    items
}

/// A terminal's insert slot (never shown): what the game routed there goes into the network, the rest stays.
pub(crate) fn flush_insert_slot(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let Some(stack) = fields.items.first().cloned().flatten() else { return };
    let key = key_of(&*host, &stack);
    let stored = insert(host, Scope::Grid(pos), None, &key, i64::from(stack.count()));
    debug(&format!("insert slot: {} stored of {}", stored, stack.count()));
    if stored > 0 {
        fields.items[0] = with_count(&stack, stack.count() - stored as u32);
    }
}

// ---- Cells' idle draw and looks

/// `StorageCell.getIdleDrain` of the cells in a drive or chest (DriveBlockEntity.updateState's power usage).
pub(crate) fn holder_idle(host: &dyn Host, level: LevelRef, pos: Pos, block: &str) -> f64 {
    if !is_holder(block) {
        return 0.0;
    }
    let Some(be) = host.block_entity(level, pos) else { return 0.0 };
    (0..holder_slots(block)).filter_map(|i| be.item_slot(host, i).ok().flatten()).map(|s| idle_drain(&s)).sum()
}

/// A drive's or chest's tick: its cells' looks, synced to clients when they change.
pub(crate) fn holder_tick(dim: &'static str, pos: Pos, block: &str, fields: &mut Fields) {
    let looks: Vec<String> = (0..holder_slots(block) as usize).map(|i| cell_look(fields.items.get(i).and_then(Option::as_ref))).collect();
    let tag = Tag::List(looks.iter().map(|l| Tag::String(java(l))).collect());
    if fields.nbt.get(&java("cells")) != Some(&tag) {
        fields.nbt.insert(java("cells"), tag);
    }
    with_store(|s| {
        if s.looks.get(&(dim, pos)) != Some(&looks) {
            s.looks.insert((dim, pos), looks);
            s.syncs.insert((dim, pos));
        }
    });
}

// ---- Viewers

/// A player opened a terminal, drive or chest menu.
pub(crate) fn opened(player: u128, window: Option<i32>, dim: &'static str, pos: Pos, block: &str, chest_term: bool) {
    debug(&format!("opened {block} at {pos:?} window {window:?}"));
    with_store(|s| {
        let opened_tick = s.tick;
        s.viewers.insert(player, Viewer { dim, pos, block: block.to_owned(), chest_term, window, remote: false, opened_tick, sent: Sent::default() });
    });
}

/// A player opened a wireless terminal: they view the grid `pos` (an access point of it) is on, as `block`'s screen.
pub(crate) fn opened_remote(player: u128, dim: &'static str, pos: Pos, block: &str) {
    with_store(|s| {
        let opened_tick = s.tick;
        s.viewers.insert(player, Viewer { dim, pos, block: block.to_owned(), chest_term: false, window: None, remote: true, opened_tick, sent: Sent::default() });
    });
}

/// The access point a wireless viewer reaches its grid through moved to another of the same grid.
pub(crate) fn set_remote_pos(player: u128, pos: Pos) {
    with_store(|s| {
        if let Some(v) = s.viewers.get_mut(&player).filter(|v| v.remote) {
            v.pos = pos;
        }
    });
}

/// Whether the player's viewer record is a wireless terminal's.
pub(crate) fn is_remote(player: u128) -> bool {
    with_store(|s| s.viewers.get(&player).is_some_and(|v| v.remote))
}

pub(crate) fn closed(player: u128) {
    with_store(|s| {
        s.viewers.remove(&player);
    });
}

/// `menu_closed`: only the menu the record is of (a closed menu's message can come after the next one opened).
/// The record closed, if this was its menu: its level, block position and block.
pub(crate) fn menu_closed(player: u128, window: i32) -> Option<(&'static str, Pos, String)> {
    debug(&format!("menu_closed window {window}"));
    with_store(|s| {
        // A window not known yet (the menu opens at the tick's end): a close within two ticks of the open is the
        // menu before it.
        let tick = s.tick;
        if s.viewers.get(&player).is_some_and(|v| !v.remote && match v.window {
            Some(w) => w == window,
            None => tick > v.opened_tick + 8,
        }) {
            return s.viewers.remove(&player).map(|v| (v.dim, v.pos, v.block));
        }
        None
    })
}

/// `player_container_close`: the player's record, if it is of the block at `pos` in `dim` (a hosted part's, of its
/// cable bus's block). Its level, record position and block.
pub(crate) fn container_closed(player: u128, dim: &str, pos: Pos) -> Option<(&'static str, Pos, String)> {
    with_store(|s| {
        if s.viewers.get(&player).is_some_and(|v| !v.remote && v.dim == dim && real(v.pos) == pos) {
            let v = s.viewers.remove(&player)?;
            debug(&format!("container closed: {} at {}", v.block, pos_text(v.pos)));
            return Some((v.dim, v.pos, v.block));
        }
        None
    })
}

/// AE2's `clearGridOnClose`: the crafting terminal's grid back into its network as the player closes it (what the
/// network doesn't take stays in the grid). Only the terminal at `pos` in the player's level, on an active network.
pub(crate) fn clear_grid_on_close(host: &mut dyn ServerHost, dim: &'static str, pos: Pos) {
    if host.dimension() != dim {
        return;
    }
    let Some(level) = here(&*host) else { return };
    if crate::net::block_id(&*host, level, pos) != "ae2:crafting_terminal" || !crate::net::device_state(dim, pos).2 {
        return;
    }
    for slot in CT_GRID {
        let Some(stack) = crate::bus::item_slot(&*host, level, pos, slot) else { continue };
        let key = key_of(&*host, &stack);
        let stored = insert(host, Scope::Grid(pos), None, &key, i64::from(stack.count()));
        if stored <= 0 {
            continue;
        }
        let rest = with_count(&stack, stack.count().saturating_sub(stored.min(i64::from(stack.count())) as u32));
        if !crate::bus::set_item_slot(host, level, pos, slot, rest.clone()) {
            // The grid wouldn't change: what went in comes back out, so nothing is made twice.
            let back = extract(host, Scope::Grid(pos), None, &key, stored);
            debug(&format!("clear grid: slot {slot} unchanged, {back} of {stored} taken back"));
        }
    }
}

/// Whether `player` has an ME chest open as its terminal (its cell slot hidden: `devices::hidden_slot`).
pub(crate) fn chest_term_open(player: u128) -> bool {
    with_store(|s| s.viewers.get(&player).is_some_and(|v| v.chest_term && v.block == "ae2:chest"))
}

/// The block a player's click acts on, if they still may (AE2's menus' `stillValid`): on, in this level, within 8
/// blocks of it, with a menu open. A stale record is dropped.
fn viewer_checked(host: &dyn Host, player: u128) -> Option<(Pos, String, bool)> {
    let (dim, pos, block, chest, opened, remote) =
        with_store(|s| s.viewers.get(&player).map(|v| (v.dim, v.pos, v.block.clone(), v.chest_term, v.opened_tick, v.remote)))?;
    if remote {
        // A wireless terminal: its session checked the item, the range and the power this tick.
        if dim != host.dimension() || !crate::wireless::session_ok(player) {
            closed(player);
            return None;
        }
        return Some((pos, block, false));
    }
    let ok = (|| -> Result<(), &'static str> {
        let p = host.player(player).ok_or("player gone")?;
        let level = p.entity().level(host).ok_or("no level")?;
        if dim != host.dimension() || Some(level) != here(host) {
            return Err("another level");
        }
        let at = p.entity().position(host).ok_or("no position")?;
        // A hosted part's record names its virtual position: the reach is to its cable bus's block.
        let near = real(pos);
        let d2: f64 = (0..3).map(|i| (at[i] - f64::from(near[i]) - 0.5).powi(2)).sum();
        if d2 > 64.0 {
            return Err("out of reach");
        }
        if crate::net::block_id(host, level, pos) != block {
            return Err("block gone");
        }
        // The menu itself: `menu_closed` drops the record (`container_menu` doesn't answer for every mod menu
        // type: none for the crafting terminal's, which has a result slot); here only its window is learnt.
        if let Some(id) = p.container_menu(host).and_then(|m| m.container_id(host)) {
            with_store(|s| {
                if let Some(v) = s.viewers.get_mut(&player)
                    && v.window.is_none()
                {
                    v.window = Some(id);
                }
            });
        }
        Ok(())
    })();
    if let Err(why) = ok {
        debug(&format!("viewer check {block} at {pos:?}: {why} (opened at {opened})"));
        closed(player);
        return None;
    }
    Some((pos, block, chest))
}

/// A menu button's player: the engine sends one only for the menu it has open (its `stillValid` checked), so here
/// the record names that block, in this level, within reach.
pub(crate) fn viewer_for_button(host: &dyn Host, player: u128) -> Option<(Pos, String, bool)> {
    let (dim, pos, block, chest, remote) = with_store(|s| s.viewers.get(&player).map(|v| (v.dim, v.pos, v.block.clone(), v.chest_term, v.remote)))?;
    if dim != host.dimension() {
        return None;
    }
    if remote {
        // A wireless terminal: no reach to the block; its session's checks stand in.
        return crate::wireless::session_ok(player).then_some((pos, block, false));
    }
    let at = host.player(player)?.entity().position(host)?;
    let d2: f64 = (0..3).map(|i| (at[i] - f64::from(pos[i]) - 0.5).powi(2)).sum();
    (d2 <= 64.0).then_some((pos, block, chest))
}

/// The key a player's terminal list has at `index` (as the server sent it).
pub(crate) fn viewer_key(player: u128, index: usize) -> Option<AeKey> {
    with_store(|s| s.viewers.get(&player).and_then(|v| v.sent.keys.get(index)).filter(|(_, n)| *n >= 0).map(|(k, _)| k.clone()))
}

pub(crate) fn rate_ok(player: u128) -> bool {
    with_store(|s| {
        if s.rates.len() > 4096 {
            s.rates.clear();
        }
        let now = std::time::Instant::now();
        let (since, n) = s.rates.entry(player).or_insert((now, 0));
        if now.duration_since(*since).as_secs() >= 1 {
            *since = now;
            *n = 0;
        }
        *n += 1;
        *n <= BUTTONS_PER_SECOND
    })
}

// ---- Clicks

/// `menu_button` on a terminal or the ME chest: the grid's clicks (MEStorageMenu.handleNetworkInteraction) and the
/// crafting terminal's buttons.
pub(crate) fn menu_button(host: &mut dyn ServerHost, player: u128, pos: Pos, id: i32, entity: &mut Fields, carried: &mut Option<ItemStack>) {
    debug(&format!("menu_button {id} at {pos:?} by {player:x} in {}", host.dimension()));
    if !rate_ok(player) {
        return;
    }
    let Some((at, block, chest)) = viewer_for_button(&*host, player) else {
        debug("  no viewer");
        return;
    };
    if at != pos {
        debug("  another block");
        return;
    }
    let dim = host.dimension();
    let (_, _, active) = crate::net::device_state(dim, pos);
    if !active {
        debug("  inactive");
        return;
    }
    let scope = if chest { Scope::Chest(pos) } else { Scope::Grid(pos) };
    // The chest's own cell is in `entity` (out of the world during this hook).
    let own_holder = chest || block == "ae2:chest";
    if block == "ae2:crafting_terminal" && (id == CT_TAKE || id == CT_TAKE_ALL) {
        crafting_take(host, player, pos, entity, carried, id == CT_TAKE_ALL);
        refresh(player);
        return;
    }
    if block == "ae2:crafting_terminal" && (id == CT_STASH || id == CT_STASH_PLAYER) {
        // CraftingTermMenu.clearCraftingGrid / clearToPlayerInventory.
        for slot in CT_GRID {
            let Some(stack) = entity.items.get_mut(slot).and_then(Option::take) else { continue };
            if id == CT_STASH_PLAYER {
                with_store(|s| s.gives.push((player, dim, pos, stack)));
                continue;
            }
            let key = key_of(&*host, &stack);
            let stored = insert(host, scope, None, &key, i64::from(stack.count()));
            entity.items[slot] = with_count(&stack, stack.count().saturating_sub(stored as u32));
        }
        refresh(player);
        return;
    }
    if id < 0 || id >= 1 << 26 {
        return;
    }
    let (index, action) = ((id >> 3) as usize, id & 7);
    let entry = with_store(|s| s.viewers.get(&player).and_then(|v| v.sent.keys.get(index).cloned()).filter(|(_, n)| *n >= 0));
    // Clicks with the chest's own entity: its cell is there.
    macro_rules! own {
        () => {
            if own_holder { Some((pos, &mut *entity)) } else { None }
        };
    }
    // A bucket on the cursor: a fluid in or out (AE2 Fabric's fluid terminal handling, a bucket at a time).
    if let Some(stack) = carried.clone() {
        let item = sid(&stack);
        if let Some(fluid) = bucket_fluid(item) {
            let key = AeKey::fluid(&fluid, "");
            if simulate(host, scope, own!(), &key, 1000, true) == 1000
                && insert(host, scope, own!(), &key, 1000) == 1000
            {
                swap_cursor(host, player, dim, pos, carried, &stack, "minecraft:bucket");
            }
            refresh(player);
            return;
        }
        if item == "minecraft:bucket"
            && let Some((key, _)) = entry.as_ref().filter(|(k, _)| k.key_type() == KeyType::Fluids)
        {
            let bucket = fluid_bucket(key.id());
            if item_key(&*host, &bucket).is_some()
                && simulate(host, scope, own!(), key, 1000, false) == 1000
                && extract(host, scope, own!(), key, 1000) == 1000
            {
                swap_cursor(host, player, dim, pos, carried, &stack, &bucket);
            }
            refresh(player);
            return;
        }
    }
    match action {
        ROLL_DOWN => {
            // Shift and the wheel down: one of the cursor's in.
            if let Some(stack) = carried.take() {
                let key = key_of(&*host, &stack);
                let n = insert(host, scope, own!(), &key, 1);
                *carried = with_count(&stack, stack.count() - n as u32);
            }
        }
        CREATIVE_DUPLICATE => {
            let creative = host.player(player).and_then(|p| p.is_creative(&*host)).unwrap_or(false);
            if creative
                && carried.is_none()
                && let Some((key, _)) = &entry
                && let Some(mut s) = stack_of(&*host, key, 1)
            {
                let most = max_stack(&*host, &s);
                let _ = s.set_count(most);
                *carried = Some(s);
            }
        }
        MOVE_REGION => {
            // Space-click: a stack for each of the inventory's 36 slots (what doesn't fit comes back).
            if let Some((key, _)) = entry.filter(|(k, _)| k.key_type() == KeyType::Items) {
                let Some(one) = stack_of(&*host, &key, 1) else { return };
                let most = i64::from(max_stack(&*host, &one));
                for _ in 0..36 {
                    let got = extract(host, scope, own!(), &key, most);
                    if got <= 0 {
                        break;
                    }
                    if let Some(s) = stack_of(&*host, &key, got as u32) {
                        with_store(|st| st.gives.push((player, dim, pos, s)));
                    }
                }
            }
        }
        ROLL_UP | 3 => {
            // Shift-right or shift and the wheel up: one onto the cursor (onto nothing or the same item, short of
            // a full stack).
            let Some((key, _)) = entry.filter(|(k, n)| k.key_type() == KeyType::Items && *n > 0) else { return };
            if let Some(c) = carried.as_ref()
                && (c.count() >= max_stack(&*host, c) || key_of(&*host, c) != key)
            {
                return;
            }
            if extract(host, scope, own!(), &key, 1) == 1 {
                *carried = match carried.take() {
                    Some(c) => with_count(&c, c.count() + 1),
                    None => stack_of(&*host, &key, 1),
                };
            }
        }
        _ => {
            let (right, shift) = (action & RIGHT != 0, action & SHIFT != 0);
            if let Some(stack) = carried.take() {
                // PICKUP_OR_SET_DOWN / SPLIT_OR_PLACE_SINGLE with a stack: all of it, or one, goes in.
                let key = key_of(&*host, &stack);
                let n = insert(host, scope, own!(), &key, if right { 1 } else { i64::from(stack.count()) });
                *carried = with_count(&stack, stack.count() - n as u32);
            } else if let Some((key, _)) = entry.filter(|(k, n)| k.key_type() == KeyType::Items && *n > 0) {
                let Some(one) = stack_of(&*host, &key, 1) else { return };
                let most = i64::from(max_stack(&*host, &one));
                let have = simulate(host, scope, own!(), &key, most, false);
                let want = if right && !shift { (have + 1) / 2 } else { have };
                let got = extract(host, scope, own!(), &key, want);
                if got > 0
                    && let Some(s) = stack_of(&*host, &key, got as u32)
                {
                    if shift {
                        // SHIFT_CLICK: into the inventory (at the tick's end); what doesn't fit goes back.
                        with_store(|st| st.gives.push((player, dim, pos, s)));
                    } else {
                        *carried = Some(s);
                    }
                }
            }
        }
    }
    refresh(player);
}

/// The cursor's bucket swapped for `to` (one of a stack: the rest stays, the new one goes to the inventory).
fn swap_cursor(host: &mut dyn ServerHost, player: u128, dim: &'static str, pos: Pos, carried: &mut Option<ItemStack>, stack: &ItemStack, to: &str) {
    let Some(new) = new_stack(&*host, to, 1) else { return };
    if stack.count() > 1 {
        *carried = with_count(stack, stack.count() - 1);
        with_store(|s| s.gives.push((player, dim, pos, new)));
    } else {
        *carried = Some(new);
    }
}

/// The viewer's list goes again at once (a click changed it).
fn refresh(player: u128) {
    with_store(|s| {
        if let Some(v) = s.viewers.get_mut(&player) {
            v.sent.generation = u64::MAX;
        }
    });
}

/// CraftingTermMenu's result taken (CraftingTermSlot.onTake): the recipe worked out again here; each ingredient
/// refilled from the network when it can be, else used from the grid; with shift as many as it makes (up to a stack's
/// worth of crafts) into the inventory.
fn crafting_take(host: &mut dyn ServerHost, player: u128, pos: Pos, entity: &mut Fields, carried: &mut Option<ItemStack>, all: bool) {
    let dim = host.dimension();
    let Some(level) = here(&*host) else { return };
    let Some(crafting) = Identifier::parse("minecraft:crafting").ok() else { return };
    let mut made: Option<AeKey> = None;
    for _ in 0..if all { 64 } else { 1 } {
        let cells: Vec<Option<ItemStack>> = CT_GRID.map(|i| entity.items.get(i).cloned().flatten()).collect();
        let input = RecipeInput::Crafting { width: 3, height: 3, slots: cells.clone() };
        let Ok(Some(hit)) = level.recipe_for(&*host, &crafting, &input) else { break };
        let result = hit.output.clone();
        let key = key_of(&*host, &result);
        if made.as_ref().is_some_and(|k| *k != key) {
            break;
        }
        if !all
            && let Some(c) = carried.as_ref()
            && (key_of(&*host, c) != key || c.count() + result.count() > max_stack(&*host, c))
        {
            break;
        }
        made = Some(key);
        for (i, slot) in CT_GRID.enumerate() {
            let Some(stack) = entity.items[slot].clone() else { continue };
            let one = key_of(&*host, &stack);
            if extract(host, Scope::Grid(pos), None, &one, 1) != 1 {
                entity.items[slot] = with_count(&stack, stack.count() - 1);
            }
            if let Some(Some(rem)) = hit.remainders.get(i) {
                if entity.items[slot].is_none() {
                    entity.items[slot] = Some(rem.clone());
                } else {
                    with_store(|s| s.gives.push((player, dim, pos, rem.clone())));
                }
            }
        }
        if all {
            with_store(|s| s.gives.push((player, dim, pos, result)));
        } else {
            *carried = match carried.take() {
                Some(c) => with_count(&c, c.count() + result.count()),
                None => Some(result),
            };
        }
    }
}

/// The crafting grid's result now (none when it makes nothing).
fn crafting_result(host: &dyn Host, level: LevelRef, pos: Pos) -> Option<ItemStack> {
    let be = host.block_entity(level, pos)?;
    let cells: Vec<Option<ItemStack>> = CT_GRID.map(|i| be.item_slot(host, i as u32).ok().flatten()).collect();
    if cells.iter().all(Option::is_none) {
        return None;
    }
    let crafting = Identifier::parse("minecraft:crafting").ok()?;
    level.recipe_for(host, &crafting, &RecipeInput::Crafting { width: 3, height: 3, slots: cells }).ok().flatten().map(|m| m.output)
}

/// The stand-in a shift-click leaves in the clicked slot when all of it went into the network (`menu_quick_move`'s
/// stack can't be empty); the player's inventory check takes it out the same tick (`sweep_moved`).
pub(crate) const MOVED: &str = "ae2:hz_moved";

/// `menu_quick_move` on a terminal: a shift-click from the inventory goes into the network (MEStorageMenu.
/// quickMoveStack), made in the hook with the player's state swapped in: the clicked stack is the hook's `stack`, which
/// keeps what didn't fit (or the stand-in). Other slots route as the game does.
pub(crate) fn quick_move(host: &mut dyn ServerHost, pos: Pos, slot: usize, container_slots: usize, entity: &mut Fields, source: &mut Option<ItemStack>) -> bool {
    if slot < container_slots {
        return false;
    }
    let Some(stack) = source.clone() else { return false };
    let stack = &stack;
    let dim = host.dimension();
    let acting = host.players().acting_player();
    let acting = acting.and_then(|p| p.entity().uuid(host));
    let is_term = |v: &Viewer| v.dim == dim && v.pos == pos && (is_terminal(&v.block) || v.chest_term);
    let found = with_store(|s| match acting {
        Some(p) => s.viewers.get(&p).filter(|v| is_term(v)).map(|v| (p, v.chest_term)),
        None => s.viewers.iter().find(|(_, v)| is_term(v)).map(|(p, v)| (*p, v.chest_term)),
    });
    debug(&format!("quick_move slot {slot} of {container_slots} at {pos:?}: acting {acting:?}, viewer {found:?}"));
    let Some((player, chest)) = found else { return false };
    let pattern = sid(stack) == "ae2:blank_pattern" || crate::autocraft::is_pattern(sid(stack));
    if pattern && here(&*host).is_some_and(|l| crate::net::block_id(&*host, l, pos) == "ae2:pattern_encoding_terminal") {
        return false;
    }
    if sid(stack) == MOVED {
        return true;
    }
    let scope = if chest { Scope::Chest(pos) } else { Scope::Grid(pos) };
    let own: Own<'_> = if chest { Some((pos, entity)) } else { None };
    let key = key_of(&*host, stack);
    let count = stack.count();
    // The game may still refuse the move after this hook (main applies the source stack later): so nothing goes into
    // the network here. The slot gives up what the network would take, and the player's tick end stores it once the
    // slot shows the items really left (`player_tick`); a refused move then stores nothing (no duplicate).
    let accept = (simulate(host, scope, own, &key, i64::from(count), true).max(0) as u32).min(count);
    let dim_now = host.dimension();
    let accept = power_allows(dim_now, pos, &key, i64::from(accept)).max(0) as u32;
    debug(&format!("quick_move: {accept} of {count} to store"));
    if accept == 0 {
        return true;
    }
    let n = (slot - container_slots) as i32;
    let inv = match n {
        0..27 => 9 + n,
        27..36 => n - 27,
        _ => return true,
    };
    with_store(|s| s.moves.push(PendingMove { player, dim, pos, chest, slot: inv, stack: stack.clone(), accept }));
    *source = with_count(stack, count - accept);
    true
}

/// `player_inventory_changed`: the stand-ins go (their items went into the network already).
pub(crate) fn sweep_moved(host: &mut dyn ServerHost, player: u128, stacks: &[Option<ItemStack>]) {
    let marked: Vec<(i32, ItemStack)> = stacks
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.as_ref().filter(|s| sid(s) == MOVED).map(|s| (i, s.clone())))
        .map(|(i, s)| (match i { 36..=39 => 100 + (i as i32 - 36), 40 => -106, _ => i as i32 }, s))
        .collect();
    for (slot, s) in marked {
        let place = ItemPlace::PlayerSlot { player, slot };
        let _ = host.transaction(|host, tx| match place.extract(host, tx, &ItemResource::of(&s), s.count()).unwrap_or(0) {
            n if n == s.count() => Ok(()),
            _ => Err(()),
        });
    }
}

/// `player_tick_post` (the player's state in place): stacks for their inventory from the terminal's shift-clicks.
pub(crate) fn player_tick(host: &mut dyn ServerHost, player: u128) {
    settle_moves(host, player);
    let gives: Vec<(u128, &'static str, Pos, ItemStack)> = with_store(|s| {
        if !s.gives.iter().any(|g| g.0 == player) {
            return Vec::new();
        }
        let (mine, rest) = std::mem::take(&mut s.gives).into_iter().partition(|g| g.0 == player);
        s.gives = rest;
        mine
    });
    for (player, dim, pos, stack) in gives {
        give(host, player, dim, pos, stack);
    }
}

/// The shift-clicks `quick_move` took: what left the slot goes into the network now (powered, as the menu's insert);
/// what no longer fits goes back to the player.
fn settle_moves(host: &mut dyn ServerHost, player: u128) {
    let moves: Vec<PendingMove> = with_store(|s| {
        if !s.moves.iter().any(|m| m.player == player) {
            return Vec::new();
        }
        let (mine, rest) = std::mem::take(&mut s.moves).into_iter().partition(|m| m.player == player);
        s.moves = rest;
        mine
    });
    for m in moves {
        // What the slot holds now of the clicked item: unchanged when the game refused the move.
        let now = match (ItemPlace::PlayerSlot { player, slot: m.slot }).stack_owned(host) {
            Some(s) if s.is_stackable_with(&m.stack) => s.count(),
            _ => 0,
        };
        let gone = m.stack.count().saturating_sub(now).min(m.accept);
        debug(&format!("settle_moves: slot {} lost {gone} of {} (accepted {})", m.slot, m.stack.count(), m.accept));
        if gone == 0 {
            continue;
        }
        let Some(moved) = with_count(&m.stack, gone) else { continue };
        let key = key_of(&*host, &moved);
        let stored = if host.dimension() == m.dim {
            let scope = if m.chest { Scope::Chest(m.pos) } else { Scope::Grid(m.pos) };
            insert(host, scope, None, &key, i64::from(gone)).max(0) as u32
        } else {
            0
        };
        if stored < gone
            && let Some(back) = with_count(&moved, gone - stored)
        {
            give(host, player, m.dim, m.pos, back);
        }
        refresh(player);
    }
}

/// A stack into the player's inventory (Inventory.add: onto the same item first, then an empty slot); what doesn't
/// fit goes back into the network, or at their feet.
fn give(host: &mut dyn ServerHost, player: u128, dim: &'static str, pos: Pos, stack: ItemStack) {
    let res = ItemResource::of(&stack);
    let mut left = stack.count();
    let selected = host.player(player).and_then(|p| p.selected_slot(&*host)).unwrap_or(0) as i32;
    let order: Vec<i32> = std::iter::once(selected).chain(0..36).collect();
    let current: Vec<(i32, Option<ItemStack>)> = order.iter().map(|s| (*s, ItemPlace::PlayerSlot { player, slot: *s }.stack_owned(host))).collect();
    let mut plan: Vec<(i32, u32)> = Vec::new();
    let most = max_stack(&*host, &stack);
    for pass in 0..2 {
        for (slot, now) in &current {
            if left == 0 || plan.iter().any(|(s, _)| s == slot) {
                continue;
            }
            let room = match now {
                Some(s) if pass == 0 && s.is_stackable_with(&stack) => most.saturating_sub(s.count()),
                None if pass == 1 => most,
                _ => 0,
            };
            let n = room.min(left);
            if n > 0 {
                plan.push((*slot, n));
                left -= n;
            }
        }
    }
    let _ = host.transaction(|host, tx| {
        for (slot, n) in &plan {
            if (ItemPlace::PlayerSlot { player, slot: *slot }).insert(host, tx, &res, *n).unwrap_or(0) != *n {
                return Err(());
            }
        }
        Ok(())
    });
    if left == 0 {
        return;
    }
    let Some(rest) = with_count(&stack, left) else { return };
    let key = key_of(&*host, &rest);
    let back = insert(host, Scope::Grid(pos), None, &key, i64::from(left)) as u32;
    if back < left
        && let Some(p) = host.player(player)
        && let Some(drop) = with_count(&rest, left - back)
    {
        let _ = p.drop_stack(host, drop, false);
    }
    let _ = dim;
}

// ---- The level's tick end: shift-clicks, gives, the lists

pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    // fixF: what this level's own tick held (a holder out of the world then) goes on now, not a tick later.
    flush_held(host, level);
    let tick = with_store(|s| {
        s.tick += 1;
        s.tick
    });
    let syncs: Vec<Pos> = with_store(|s| {
        let (here, rest): (Vec<_>, Vec<_>) = s.syncs.drain().partition(|(d, _)| *d == dim);
        s.syncs = rest.into_iter().collect();
        here.into_iter().map(|(_, p)| p).collect()
    });
    for p in syncs {
        host.block_entity_sync(p);
    }
    stream(host, level, tick);
}

/// Each viewer's list, when it changed: `#reset` on a new screen, then `index amount key` for an entry it hadn't,
/// `index amount` for a new amount, `index -1` when gone. The crafting terminal's result goes on `ae2:extras`.
fn stream(host: &mut dyn ServerHost, level: LevelRef, tick: u64) {
    let dim = host.dimension();
    let viewers: Vec<u128> = with_store(|s| s.viewers.iter().filter(|(_, v)| v.dim == dim).map(|(u, _)| *u).collect());
    for player in viewers {
        let Some((pos, block, chest)) = viewer_checked(&*host, player) else { continue };
        if !is_terminal(&block) && !chest {
            continue;
        }
        let (_, _, active) = crate::net::device_state(dim, pos);
        let scope = if chest { Scope::Chest(pos) } else { Scope::Grid(pos) };
        let remote = is_remote(player);
        if block == "ae2:crafting_terminal" {
            // A wireless crafting terminal's grid is on the item.
            let result = if remote { crate::wireless::grid_result(host, player) } else { crafting_result(&*host, level, pos) };
            let bytes = result.and_then(|r| r.to_nbt(&*host)).and_then(|c| Tag::Compound(c).to_network_bytes().ok()).unwrap_or_default();
            let hash = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                bytes.hash(&mut h);
                h.finish()
            };
            if with_store(|s| s.viewers.get(&player).is_some_and(|v| v.sent.extras != hash)) && crate::util::send_to_player(host, player, NET_EXTRAS, &bytes).is_ok() {
                with_store(|s| {
                    if let Some(v) = s.viewers.get_mut(&player) {
                        v.sent.extras = hash;
                    }
                });
            }
        }
        // The network's craftables (stage 5): listed with amount 0 when none is stored (AE2's craftable entries).
        let (craftables, pattern_gen) = if chest || !active { (Vec::new(), 0) } else { crate::autocraft::craftables(dim, pos) };
        let generation = generation(dim, scope) ^ u64::from(active) ^ pattern_gen.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let due = with_store(|s| s.viewers.get(&player).is_some_and(|v| v.sent.generation != generation || tick >= v.sent.full_after));
        if due {
            let mut items = if active { contents(host, scope, None) } else { Vec::new() };
            // The view cells' partitions (ViewCellItem.createItemFilter), set in the cell workbench.
            if let Some(range) = view_cell_range(&block).filter(|_| !remote) {
                let cells: Vec<ItemStack> = range.filter_map(|i| host.block_entity(level, pos).and_then(|b| b.item_slot(&*host, i as u32).ok().flatten())).collect();
                items = crate::parts::view_filter(&cells, items);
            }
            for k in craftables {
                if !items.iter().any(|(i, _)| *i == k) {
                    items.push((k, 0));
                }
            }
            with_store(|s| {
                let Some(v) = s.viewers.get_mut(&player) else { return };
                let v = &mut v.sent;
                let mut lines: Vec<String> = Vec::new();
                if v.generation == 0 && v.keys.is_empty() {
                    lines.push("#reset".to_owned());
                }
                lines.push(format!("#on {}", u8::from(active)));
                let mut now: HashSet<usize> = HashSet::with_capacity(items.len());
                for (key, amount) in items {
                    match v.index.get(&key) {
                        Some(&i) => {
                            now.insert(i);
                            if v.keys[i].1 != amount {
                                v.keys[i].1 = amount;
                                lines.push(format!("{i} {amount}"));
                            }
                        }
                        None => {
                            let i = v.free.pop().unwrap_or_else(|| {
                                v.keys.push((key.clone(), -1));
                                v.keys.len() - 1
                            });
                            let mut whole = key_to_string(&key);
                            if whole.len() > ENTRY_DATA_BYTES {
                                // Too big to send: drawn as its plain item.
                                whole = key_to_string(&AeKey::item(key.id(), "", key.fuzzy_search_value(), key.fuzzy_search_max_value()));
                            }
                            lines.push(format!("{i} {amount} {whole}"));
                            v.keys[i] = (key.clone(), amount);
                            v.index.insert(key, i);
                            now.insert(i);
                        }
                    }
                }
                for i in 0..v.keys.len() {
                    if v.keys[i].1 >= 0 && !now.contains(&i) {
                        let key = v.keys[i].0.clone();
                        v.index.remove(&key);
                        v.keys[i].1 = -1;
                        v.free.push(i);
                        lines.push(format!("{i} -1"));
                    }
                }
                let mut body = String::new();
                for line in lines {
                    if body.len() + line.len() + 1 > MESSAGE_BYTES {
                        v.pending.push_back(std::mem::take(&mut body).into_bytes());
                    }
                    body.push_str(&line);
                    body.push('\n');
                }
                if !body.is_empty() {
                    v.pending.push_back(body.into_bytes());
                }
                v.generation = generation.max(1);
                v.full_after = tick + 40;
            });
        }
        for _ in 0..MESSAGES_PER_TICK {
            let Some(msg) = with_store(|s| s.viewers.get_mut(&player).and_then(|v| v.sent.pending.pop_front())) else { break };
            debug(&format!("send {} bytes to {player:x}", msg.len()));
            if crate::util::send_to_player(host, player, NET_ITEMS, &msg).is_err() {
                with_store(|s| {
                    if let Some(v) = s.viewers.get_mut(&player) {
                        v.sent.pending.push_front(msg);
                    }
                });
                break;
            }
        }
    }
}

/// The cells in a terminal's view cell slots don't filter yet (their partition is set in the cell workbench, stage 4).
pub(crate) fn view_cell_range(block: &str) -> Option<std::ops::Range<usize>> {
    match block {
        "ae2:terminal" => Some(1..1 + VIEW_CELLS),
        "ae2:crafting_terminal" => Some(11..11 + VIEW_CELLS),
        "ae2:pattern_encoding_terminal" => Some(3..3 + VIEW_CELLS),
        _ => None,
    }
}

/// Which cell kinds an item slot takes (`menu_slot_rules`, no host there: the ids are mapped once by then).
pub(crate) fn slot_rules(menu: &str, slot: usize, stack: &ItemStack, may_place: &mut bool, max_stack: &mut u32) {
    let id = sid(stack);
    match menu {
        "ae2:drive" | "ae2:chest" => {
            *may_place = is_cell_item(id);
            *max_stack = 1;
        }
        "ae2:terminal" | "ae2:crafting_terminal" | "ae2:pattern_encoding_terminal" => {
            if view_cell_range(menu).is_some_and(|r| r.contains(&slot)) {
                *may_place = id == "ae2:view_cell";
                *max_stack = 1;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod fixa_tests {
    //! fixA: storage buses on interfaces of other networks (recursive networking) at the model level: the same
    //! guard and mounts `open` uses, over cells in memory.
    use super::*;
    use crate::core::{CellContents, CellType, CellUpgrades, FuzzyMode, StorageTier};
    use crate::parts::BusFilter;

    fn cell_with(items: &[(&AeKey, i64)]) -> Cell {
        let t = CellType::item_cell(StorageTier::ALL[0]);
        let mut c = BasicCellInventory::new(t, &[], FuzzyMode::IgnoreAll, CellUpgrades::default(), &CellContents::default());
        for (k, n) in items {
            assert_eq!(MeStorage::insert(&mut c, k, *n, Actionable::Modulate), *n);
        }
        Cell::Basic(c)
    }

    fn stone() -> AeKey {
        AeKey::item("minecraft:stone", "", 0, 0)
    }
    fn dirt() -> AeKey {
        AeKey::item("minecraft:dirt", "", 0, 0)
    }

    /// Networks by number, each with its cells, and buses (from, to the interface's network, filter): opened as `open`
    /// opens them (the guard, then each bus's network mounted under its filter).
    struct World {
        cells: Vec<Vec<(AeKey, i64)>>,
        buses: Vec<(usize, usize, BusFilter)>,
    }

    impl World {
        fn open(&self, net: usize) -> Option<Opened> {
            let ident = ("test", [net as i32, 0, 0], false);
            if !guard_enter(ident) {
                return None;
            }
            let mut mounted = Mounted::default();
            let list: Vec<(&AeKey, i64)> = self.cells[net].iter().map(|(k, n)| (k, *n)).collect();
            mounted.storage.mount(0, 0, cell_with(&list));
            let mut volatile = Vec::new();
            for (i, (from, to, filter)) in self.buses.iter().enumerate() {
                if *from != net {
                    continue;
                }
                let Some(inner) = self.open(*to) else { continue };
                let id = EXTERNAL_IDS + i as u64;
                mounted.storage.mount_volatile(id, 0, Cell::Net(Box::new(NetMount { filter: filter.clone(), inner: Box::new(inner) })));
                volatile.push(id);
            }
            Some(Opened { mkey: ident, ident, mounted, found: Vec::new(), list: Vec::new(), remounted: false, volatile })
        }

        fn close(o: Opened) {
            let Opened { ident, mut mounted, volatile, .. } = o;
            for id in volatile {
                if let Some(Cell::Net(n)) = mounted.storage.unmount(id) {
                    Self::close(*n.inner);
                }
            }
            guard_leave(ident);
        }

        fn with<R>(&self, net: usize, f: impl FnOnce(&mut NetworkStorage<Cell>) -> R) -> R {
            let mut o = self.open(net).expect("not open yet");
            let r = f(&mut o.mounted.storage);
            Self::close(o);
            r
        }
    }

    fn open_bus() -> BusFilter {
        BusFilter::default()
    }

    /// The bus's mount (its id is the first external one) asked directly: (insert, extract) simulated.
    fn through_bus(w: &World, key: &AeKey, n: i64) -> Option<(i64, i64)> {
        w.with(0, |st| st.get_mut(EXTERNAL_IDS).map(|c| (c.insert(key, n, Actionable::Simulate), c.extract(key, n, Actionable::Simulate))))
    }

    #[test]
    fn a_loop_of_buses_and_interfaces_counts_each_network_once() {
        // A's bus on B's interface, B's bus on A's: each sees both, once, and nothing recurses.
        let w = World { cells: vec![vec![(stone(), 10)], vec![(dirt(), 5)]], buses: vec![(0, 1, open_bus()), (1, 0, open_bus())] };
        let a = w.with(0, |st| st.contents());
        assert_eq!((a.get(&stone()), a.get(&dirt())), (10, 5));
        let b = w.with(1, |st| st.contents());
        assert_eq!((b.get(&stone()), b.get(&dirt())), (10, 5));
        // Out of A through its bus: B's dirt, exactly what B has.
        assert_eq!(w.with(0, |st| st.extract(&dirt(), 50, Actionable::Simulate)), 5);
        // A diamond (A sees B and C, C sees B): B still counts once.
        let w = World {
            cells: vec![vec![(stone(), 1)], vec![(dirt(), 7)], vec![]],
            buses: vec![(0, 1, open_bus()), (0, 2, open_bus()), (2, 1, open_bus())],
        };
        assert_eq!(w.with(0, |st| st.contents()).get(&dirt()), 7);
        // The guard is left clean: another call opens again.
        assert_eq!(w.with(0, |st| st.contents()).get(&dirt()), 7);
        // A bus on its own network's interface sees nothing more (AE2: mountsInUse).
        let w = World { cells: vec![vec![(stone(), 3)]], buses: vec![(0, 0, open_bus())] };
        assert_eq!(w.with(0, |st| st.contents()).get(&stone()), 3);
    }

    #[test]
    fn the_bus_rules_apply_to_the_network_behind_it() {
        // A whitelist of dirt: stone doesn't go through, dirt is listed.
        let mut only_dirt = open_bus();
        only_dirt.keys = vec![dirt()];
        let w = World { cells: vec![vec![], vec![(dirt(), 1)]], buses: vec![(0, 1, only_dirt)] };
        assert_eq!(through_bus(&w, &stone(), 3), Some((0, 0)));
        assert_eq!(through_bus(&w, &dirt(), 3), Some((3, 1)));
        assert_eq!(w.with(0, |st| st.contents()).get(&dirt()), 1);
        // Inverted (an Inverter Card): everything but dirt.
        let mut not_dirt = open_bus();
        not_dirt.keys = vec![dirt()];
        not_dirt.inverted = true;
        let w = World { cells: vec![vec![], vec![(dirt(), 1)]], buses: vec![(0, 1, not_dirt)] };
        assert_eq!(through_bus(&w, &stone(), 3), Some((3, 0)));
        assert_eq!(through_bus(&w, &dirt(), 3), Some((0, 0)));
        // Extract only: nothing goes in through it; insert only: nothing comes out or is listed.
        let mut extract_only = open_bus();
        extract_only.access = 1;
        let w = World { cells: vec![vec![], vec![(dirt(), 4)]], buses: vec![(0, 1, extract_only)] };
        assert_eq!(through_bus(&w, &dirt(), 9), Some((0, 4)));
        let mut insert_only = open_bus();
        insert_only.access = 2;
        let w = World { cells: vec![vec![], vec![(dirt(), 4)]], buses: vec![(0, 1, insert_only)] };
        assert_eq!(through_bus(&w, &dirt(), 9), Some((9, 0)));
        assert_eq!(w.with(0, |st| st.contents()).get(&dirt()), 0);
        // Moves through the bus are real moves in B's cells: in, then out, no more than there is.
        let w = World { cells: vec![vec![], vec![]], buses: vec![(0, 1, open_bus())] };
        let mut o = w.open(0).expect("open");
        let n = o.mounted.storage.get_mut(EXTERNAL_IDS).map(|c| c.insert(&dirt(), 20, Actionable::Modulate));
        assert_eq!(n, Some(20));
        let Some(Cell::Net(net)) = o.mounted.storage.get_mut(EXTERNAL_IDS) else { panic!("no bus") };
        assert_eq!(net.inner.mounted.storage.contents().get(&dirt()), 20);
        assert_eq!(o.mounted.storage.extract(&dirt(), 100, Actionable::Modulate), 20);
        assert_eq!(o.mounted.storage.extract(&dirt(), 100, Actionable::Modulate), 0);
        World::close(o);
        // A Void Card takes all that is offered.
        let mut void = open_bus();
        void.void = true;
        let w = World { cells: vec![vec![], vec![]], buses: vec![(0, 1, void)] };
        assert_eq!(through_bus(&w, &dirt(), 1 << 40), Some((1 << 40, 0)));
    }
}

#[cfg(test)]
mod fixf_tests {
    //! fixF: a crafting card's request from level A run by a CPU in level B (its provider and assembler there too) over
    //! a drive's cell in A, as the game does it: `open` reads the cell through `held_over` and mounts it again when the
    //! version on its item isn't the mounted one; `close` writes a new version onto the item in the level in place and
    //! holds it (`after_write`) from any other; the holder's level puts it on at its tick (`flush_held`). The CPU, the
    //! plan and the cell are AE2's (`execution`, `crafting`, `BasicCellInventory`).
    use super::*;
    use crate::core::{CellContents, CellType, CellUpgrades, FuzzyMode, GenericStack, StorageTier};
    use crate::crafting::{CalculationStrategy, Pattern, PatternBook, PatternInput, calculate};
    use crate::execution::{CpuNetwork, CraftingCpu};

    const A: &str = "test:overworld";
    const B: &str = "test:nether";
    const DRIVE: CellAt = (A, [219, 100, 200], 0);

    fn log() -> AeKey {
        AeKey::item("minecraft:oak_log", "", 0, 0)
    }
    fn planks() -> AeKey {
        AeKey::item("minecraft:oak_planks", "", 0, 0)
    }
    fn planks_pattern(kind: &str) -> Pattern {
        Pattern {
            definition: AeKey::item(kind, "planks", 0, 0),
            inputs: vec![PatternInput::of(log(), 1)],
            outputs: vec![GenericStack { what: planks(), amount: 4 }],
        }
    }
    fn empty_cell(saved: &CellContents) -> BasicCellInventory {
        BasicCellInventory::new(CellType::item_cell(StorageTier::ALL[0]), &[], FuzzyMode::IgnoreAll, CellUpgrades::default(), saved)
    }

    /// The cell's item in the drive's slot: its id, version and saved contents.
    #[derive(Clone, Debug, PartialEq)]
    struct Item {
        id: i64,
        ver: i64,
        contents: CellContents,
    }

    fn same(read: &Item, held: &Item) -> bool {
        read.id == held.id && held.ver >= read.ver
    }

    /// The drive in A with its cell, the cell mounted in memory, the writes held for A.
    struct World {
        slot: Option<Item>,
        mounted: Option<(i64, i64, BasicCellInventory)>,
        held: HashMap<CellAt, Item>,
        /// Before fixF a write from another level was lost (the host refuses it); with it, it is held.
        fixed: bool,
    }

    impl World {
        fn new(logs: i64) -> Self {
            let mut inv = empty_cell(&CellContents::default());
            assert_eq!(MeStorage::insert(&mut inv, &log(), logs, Actionable::Modulate), logs);
            World { slot: Some(Item { id: 7, ver: 0, contents: inv.persist().unwrap() }), mounted: None, held: HashMap::new(), fixed: true }
        }

        /// `open`, `f` on the cell, `close`, from a hook of level `here`.
        fn with<R>(&mut self, here: &'static str, f: impl FnOnce(&mut BasicCellInventory) -> R) -> R {
            let read = held_over(&mut self.held, DRIVE, self.slot.clone(), same).expect("the cell is in the drive");
            if !self.mounted.as_ref().is_some_and(|(id, ver, _)| (*id, *ver) == (read.id, read.ver)) {
                self.mounted = Some((read.id, read.ver, empty_cell(&read.contents)));
            }
            let (id, ver, inv) = self.mounted.as_mut().unwrap();
            let out = f(inv);
            if let Some(contents) = inv.persist() {
                *ver += 1;
                let item = Item { id: *id, ver: *ver, contents };
                let wrote = here == DRIVE.0;
                if wrote {
                    self.slot = Some(item.clone());
                }
                if self.fixed {
                    after_write(&mut self.held, DRIVE, item, wrote);
                }
            }
            out
        }

        /// `flush_held` at A's tick.
        fn flush(&mut self) {
            if let Some(h) = self.held.remove(&DRIVE)
                && self.slot.as_ref().is_some_and(|s| same(s, &h))
            {
                self.slot = Some(h);
            }
        }

        fn stored(&mut self, here: &'static str, k: &AeKey) -> i64 {
            self.with(here, |inv| {
                let mut all = KeyCounter::new();
                inv.available_stacks(&mut all);
                all.get(k)
            })
        }

        /// What the cell's item in the drive holds.
        fn on_disk(&self, k: &AeKey) -> i64 {
            let c = &self.slot.as_ref().unwrap().contents;
            c.keys.iter().zip(&c.amounts).filter(|(key, _)| *key == k).map(|(_, n)| *n).sum()
        }
    }

    /// The grid as a CPU's `CpuNet` sees it from a hook of level `here`: storage through `World::with`, one provider
    /// beside a free assembler (or machine) in B.
    struct Net<'a> {
        w: &'a mut World,
        here: &'static str,
        pushed: Vec<GenericStack>,
    }

    impl CpuNetwork for Net<'_> {
        fn extract(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
            self.w.with(self.here, |inv| MeStorage::extract(inv, what, amount, mode))
        }
        fn insert(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
            self.w.with(self.here, |inv| MeStorage::insert(inv, what, amount, mode))
        }
        fn provider_count(&self, _: &Pattern) -> usize {
            1
        }
        fn provider_busy(&self, _: &Pattern, _: usize) -> bool {
            false
        }
        fn push_pattern(&mut self, pattern: &Pattern, _: usize, _: &[KeyCounter]) -> bool {
            assert_eq!(self.here, B, "the provider is in B");
            self.pushed.extend(pattern.outputs.iter().cloned());
            true
        }
        fn extract_power(&mut self, amount: f64, _: Actionable) -> f64 {
            amount
        }
    }

    /// A's crafting card asking for planks from the CPU in B until the logs run out: (planks in the chest, planks in
    /// the drive, logs in the drive, crafts started).
    fn run(fixed: bool, processing: bool) -> (i64, i64, i64, usize) {
        let mut w = World::new(8);
        w.fixed = fixed;
        let mut book = PatternBook::new();
        book.add(planks_pattern(if processing { "ae2:processing_pattern" } else { "ae2:crafting_pattern" }), 0);
        // The 1k crafting storage in B.
        let mut cpu = CraftingCpu::new(1024, 0);
        let (mut chest, mut crafts) = (0, 0);
        for _ in 0..200 {
            // A's tick: its held writes go on; the export bus exports what the network has, or else its crafting card
            // asks for one (request_for_machine: CRAFT_LESS, the CPU in B, the ingredients taken from A).
            w.flush();
            let have = w.stored(A, &planks());
            if have > 0 {
                chest += w.with(A, |inv| MeStorage::extract(inv, &planks(), have, Actionable::Modulate));
            } else if !cpu.has_job() {
                let mut stock = KeyCounter::new();
                stock.add(&log(), w.stored(A, &log()));
                let plan = calculate(&book, &stock, &planks(), 1, CalculationStrategy::CraftLess).unwrap();
                if plan.simulation || plan.final_output.amount <= 0 {
                    // "Missing ingredients".
                    break;
                }
                cpu.submit(&plan, &mut Net { w: &mut w, here: A, pushed: Vec::new() }, false, None).unwrap();
                crafts += 1;
            }
            // B's tick (run_cpus): the CPU pushes, the craft comes back.
            let mut net = Net { w: &mut w, here: B, pushed: Vec::new() };
            cpu.tick(&mut net);
            for out in std::mem::take(&mut net.pushed) {
                if processing {
                    // The machine's output into the network in B (an import bus, an interface, the provider's return
                    // slots): stored, and the CPU counts its final output there (CraftingServiceStorage).
                    assert_eq!(net.insert(&out.what, out.amount, Actionable::Modulate), out.amount);
                    let want = cpu.waiting_for(&out.what).min(out.amount);
                    cpu.insert(&out.what, want, Actionable::Modulate, &mut net);
                } else {
                    // The assembler's craft: to the CPU, the rest stored (`CpuNet::store_rest`).
                    let took = cpu.insert(&out.what, out.amount, Actionable::Modulate, &mut net);
                    assert_eq!(net.insert(&out.what, out.amount - took, Actionable::Modulate), out.amount - took);
                }
            }
            // Nothing is left in a CPU without a job.
            assert!(cpu.has_job() || cpu.stored(&planks()) + cpu.stored(&log()) == 0);
        }
        w.flush();
        (chest, w.on_disk(&planks()), w.on_disk(&log()), crafts)
    }

    #[test]
    fn a_held_write_is_read_until_its_level_puts_it_on() {
        let mut held: HashMap<CellAt, (i64, i64)> = HashMap::new();
        let same = |r: &(i64, i64), h: &(i64, i64)| r.0 == h.0 && h.1 >= r.1;
        // Written from another level: held, and read over the old version still in the slot.
        after_write(&mut held, DRIVE, (7, 3), false);
        assert_eq!(held_over(&mut held, DRIVE, Some((7, 2)), same), Some((7, 3)));
        // An unloaded holder reads nothing: the write waits.
        assert_eq!(held_over(&mut held, DRIVE, None, same), None);
        assert!(held.contains_key(&DRIVE));
        // Written in its own level: nothing held any more.
        after_write(&mut held, DRIVE, (7, 4), true);
        assert!(held.is_empty());
        // Another cell put in the slot meanwhile: the held write isn't its.
        after_write(&mut held, DRIVE, (7, 5), false);
        assert_eq!(held_over(&mut held, DRIVE, Some((9, 0)), same), Some((9, 0)));
        assert!(held.is_empty());
    }

    #[test]
    fn a_crafting_card_request_with_its_cpu_in_another_level_keeps_every_item() {
        // The game before fixF, as found in game: eight crafts, the eight logs gone, no plank anywhere (each craft's
        // planks went into the mounted cell only; A's next look mounted it again from its item).
        assert_eq!(run(false, false), (0, 0, 0, 8));
        // With it: every craft's planks reach the export bus's chest, and logs and planks add up.
        let (chest, drive_planks, drive_logs, crafts) = run(true, false);
        assert_eq!((chest, drive_planks, drive_logs, crafts), (32, 0, 0, 8));
        assert_eq!(drive_logs + (chest + drive_planks) / 4, 8);
    }

    #[test]
    fn a_processing_pattern_in_another_level_keeps_every_item() {
        assert_eq!(run(false, true), (0, 0, 0, 8));
        assert_eq!(run(true, true), (32, 0, 0, 8));
    }

    #[test]
    fn a_terminal_job_on_a_cpu_in_another_level_keeps_its_output_in_storage() {
        // A player's job for 8 planks from a terminal in A, on the CPU in B: the planks stay in the drive in A.
        let mut w = World::new(2);
        let mut book = PatternBook::new();
        book.add(planks_pattern("ae2:crafting_pattern"), 0);
        let mut stock = KeyCounter::new();
        stock.add(&log(), w.stored(A, &log()));
        let plan = calculate(&book, &stock, &planks(), 8, CalculationStrategy::ReportMissingItems).unwrap();
        let mut cpu = CraftingCpu::new(1024, 0);
        cpu.submit(&plan, &mut Net { w: &mut w, here: A, pushed: Vec::new() }, false, Some(1)).unwrap();
        for _ in 0..20 {
            w.flush();
            let mut net = Net { w: &mut w, here: B, pushed: Vec::new() };
            cpu.tick(&mut net);
            for out in std::mem::take(&mut net.pushed) {
                let took = cpu.insert(&out.what, out.amount, Actionable::Modulate, &mut net);
                net.insert(&out.what, out.amount - took, Actionable::Modulate);
            }
        }
        assert!(!cpu.has_job());
        // Read from B before A's tick, and in A after it.
        assert_eq!(w.stored(B, &planks()), 8);
        w.flush();
        assert_eq!((w.on_disk(&planks()), w.on_disk(&log())), (8, 0));
        assert_eq!(w.stored(A, &planks()), 8);
    }
}
