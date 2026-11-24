//! AE2's parts and the cell workbench on the server (stage 4): import, export and storage buses, interfaces (the block
//! and the cable part), P2P tunnels (ME, item, fluid, FE, redstone, light), level emitters and the cell workbench.
//!
//! The logic is the upstream port's (`ae2-horizon/mixins/ae2/me.rs`: `bus_tick`/`run_buses`, `ExternalFluids`, the
//! interface's stocking, `p2p_click`/`p2p_input_tick`, the level emitter in `block_entity_tick`, `workbench_tick`), on
//! the fork's APIs:
//! - every bus, the storage bus and the P2P tunnels reach any mod's inventory through the fork's `ITEMS`, `FLUIDS` and
//!   `ENERGY` capabilities (`LevelRef::capability`, `ResourceHandler` inserts and extractions in `host.transaction`),
//!   not side-slot tables;
//! - an interface's slots, and a tunnel's front, answer `ITEMS`/`FLUIDS`/`ENERGY` to other mods through providers asked
//!   before the game's (`block_entity_capability_before_game`): an input hands what comes in to its outputs' fronts, an
//!   output reaches what is in front of its input (CapabilityP2PTunnelPart);
//! - redstone goes out through `BlockPart::block_signal` and `update_neighbors`; a light tunnel's output lights by its
//!   block state (`state_light`);
//! - a storage bus's container is mounted into the grid's `NetworkStorage` for one call (`storage::with_storage`): what
//!   it held when mounted, its changes written back through the face when unmounted (upstream storage_open/close).
//!
//! Multiplayer rules (upstream PLAN.md Round 24): a part's buttons act only for a player whose menu it is, in this level
//! and within 8 blocks, rate limited (`storage::viewer_for_button`, `rate_ok`); a level typed on the client is checked
//! and clamped on the server.

use crate::cells::*;
use crate::core::{Actionable, AeKey, FuzzyMode, KeyCounter, KeyType};
use crate::storage::{self, Scope};
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) const NET_PART: &str = "ae2:part";

// ---- Menu buttons (the part screens' left toolbars and ghost cells)

pub(crate) const BTN_REDSTONE: i32 = 1;
pub(crate) const BTN_SCHEDULE: i32 = 2;
pub(crate) const BTN_CRAFT_ONLY: i32 = 3;
pub(crate) const BTN_FUZZY: i32 = 4;
pub(crate) const BTN_CLEAR: i32 = 5;
pub(crate) const BTN_PARTITION: i32 = 6;
pub(crate) const BTN_REPORT: i32 = 7;
pub(crate) const BTN_EXTRACT_FILTER: i32 = 8;
pub(crate) const BTN_ACCESS: i32 = 9;
pub(crate) const BTN_EMIT_MODE: i32 = 11;
/// The level emitter's Settings.CRAFT_VIA_REDSTONE (with a Crafting Card, in the redstone mode's place).
pub(crate) const BTN_CRAFT_RS: i32 = 12;
pub(crate) const BTN_WB_COPY: i32 = 15;
/// The formation plane's Block Placement (Settings.PLACE_BLOCK): `drop_items` 0 places, 1 drops as items.
pub(crate) const BTN_PLACE: i32 = 20;
/// A right-click on a toolbar button: the setting before (SettingToggleButton).
pub(crate) const BACK: i32 = 1 << 8;
/// A ghost cell: `GHOST | cell << 3 | action` (0 left, 1 right, 2 wheel up, 3 wheel down).
pub(crate) const GHOST: i32 = 1 << 20;

/// The redstone card's modes (RedstoneMode): ignore, active with signal, without, once per pulse.
pub(crate) const REDSTONE_MODES: [&str; 3] = ["Active with signal", "Active without signal", "Active once per pulse"];

// ---- Kinds

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Import,
    Export,
    StorageBus,
    Interface,
    LevelEmitter,
    EnergyEmitter,
    P2p,
    Workbench,
}

pub(crate) fn kind(block: &str) -> Option<Kind> {
    Some(match block {
        "ae2:import_bus" => Kind::Import,
        "ae2:export_bus" => Kind::Export,
        // The formation plane: a storage bus's filter, cards and screen (FormationPlaneMenu has the same config).
        "ae2:storage_bus" | "ae2:formation_plane" => Kind::StorageBus,
        "ae2:interface" | "ae2:cable_interface" => Kind::Interface,
        "ae2:level_emitter" => Kind::LevelEmitter,
        "ae2:energy_level_emitter" => Kind::EnergyEmitter,
        "ae2:cell_workbench" => Kind::Workbench,
        b if is_p2p(b) => Kind::P2p,
        _ => return None,
    })
}

pub(crate) const P2P_KINDS: [&str; 6] =
    ["ae2:me_p2p_tunnel", "ae2:item_p2p_tunnel", "ae2:fluid_p2p_tunnel", "ae2:redstone_p2p_tunnel", "ae2:light_p2p_tunnel", "ae2:fe_p2p_tunnel"];

pub(crate) fn is_p2p(block: &str) -> bool {
    P2P_KINDS.contains(&block)
}

/// Blocks with a part screen (a menu of their own).
pub(crate) fn has_screen(block: &str) -> bool {
    kind(block).is_some_and(|k| k != Kind::P2p)
}

/// The content menu type of a part's container (`data/ae2/sunrise/menu/*.json`) and its slots.
pub(crate) fn menu_of(block: &str) -> Option<(&'static str, usize)> {
    Some(match kind(block)? {
        Kind::Import | Kind::Export => ("ae2:io_bus", 5),
        Kind::StorageBus => ("ae2:storage_bus", 5),
        Kind::Interface => ("ae2:interface", 10),
        Kind::LevelEmitter => ("ae2:level_emitter", 2),
        Kind::EnergyEmitter => ("ae2:energy_level_emitter", 1),
        Kind::Workbench => ("ae2:cell_workbench", 5),
        Kind::P2p => return None,
    })
}

/// Where a part's upgrade cards start in its container, and how many slots.
pub(crate) fn upgrade_slots(block: &str) -> (usize, usize) {
    match kind(block) {
        Some(Kind::Import | Kind::Export | Kind::StorageBus) => (0, 5),
        Some(Kind::Interface) => (9, 1),
        Some(Kind::LevelEmitter) => (1, 1),
        Some(Kind::Workbench) => (1, 4),
        _ => (0, 0),
    }
}

/// The cards a part takes, and how many of each (Upgrades.getMaxInstallable, the parts' `upgrades` in AE2's
/// UpgradesInit).
pub(crate) fn card_limit(block: &str, card: &str) -> usize {
    let k = kind(block);
    match (k, card) {
        (Some(Kind::Import | Kind::Export), "ae2:capacity_card") => 2,
        (Some(Kind::Import | Kind::Export), "ae2:speed_card") => 4,
        (Some(Kind::Import | Kind::Export), "ae2:fuzzy_card" | "ae2:redstone_card") => 1,
        (Some(Kind::Import), "ae2:inverter_card") => 1,
        (Some(Kind::Export), "ae2:crafting_card") => 1,
        (Some(Kind::StorageBus), "ae2:capacity_card") => 5,
        (Some(Kind::StorageBus), "ae2:fuzzy_card" | "ae2:inverter_card" | "ae2:void_card") => 1,
        (Some(Kind::Interface), "ae2:fuzzy_card" | "ae2:crafting_card") => 1,
        (Some(Kind::LevelEmitter), "ae2:fuzzy_card" | "ae2:crafting_card") => 1,
        _ => 0,
    }
}

pub(crate) fn is_card(id: &str) -> bool {
    id.starts_with("ae2:") && id.ends_with("_card") && id != "ae2:basic_card" && id != "ae2:advanced_card"
}

/// The cards of `card` that count (no more than the part takes).
fn cards(block: &str, fields: &Fields, card: &str) -> usize {
    let (from, n) = upgrade_slots(block);
    let have: usize = fields.items.iter().skip(from).take(n).flatten().filter(|s| sid(s) == card).map(|s| s.count() as usize).sum();
    have.min(card_limit(block, card))
}

/// `menu_slot_rules`: cards in the upgrade slots (one a slot), cells in the workbench's cell slot.
pub(crate) fn slot_rules(menu: &str, slot: usize, stack: &ItemStack, may_place: &mut bool, max_stack: &mut u32) {
    let id = sid(stack);
    let block = match menu {
        "ae2:io_bus" => "ae2:import_bus",
        "ae2:storage_bus" => "ae2:storage_bus",
        "ae2:interface" => "ae2:interface",
        "ae2:level_emitter" => "ae2:level_emitter",
        "ae2:cell_workbench" => "ae2:cell_workbench",
        _ => return,
    };
    if menu == "ae2:cell_workbench" && slot == 0 {
        // [tools] Portable cells, the matter cannon and the colour applicator too (ICellWorkbenchItem).
        *may_place = is_cell_item(id) || id == "ae2:view_cell" || crate::tool_cells::workbench_takes(id);
        *max_stack = 1;
        return;
    }
    let (from, n) = upgrade_slots(block);
    if slot >= from && slot < from + n {
        let any_bus = menu == "ae2:io_bus" && (card_limit("ae2:import_bus", id) > 0 || card_limit("ae2:export_bus", id) > 0);
        *may_place = if menu == "ae2:cell_workbench" { is_cell_card(id) } else { any_bus || card_limit(block, id) > 0 };
        *max_stack = 1;
    } else if menu == "ae2:level_emitter" && slot == 0 {
        *may_place = false;
    }
}

// ---- Sides

pub(crate) fn dir_of(d: Pos) -> Direction {
    match d {
        [0, -1, 0] => Direction::Down,
        [0, 1, 0] => Direction::Up,
        [0, 0, -1] => Direction::North,
        [0, 0, 1] => Direction::South,
        [-1, 0, 0] => Direction::West,
        _ => Direction::East,
    }
}

pub(crate) fn neg(d: Pos) -> Pos {
    [-d[0], -d[1], -d[2]]
}

fn facing(host: &dyn Host, level: LevelRef, pos: Pos) -> Pos {
    facing_dir(crate::bus::prop(host, level, pos, "facing").as_deref())
}

// ---- Ghost cells (a part's config: keys, and for the interface, amounts)

/// The config cells as saved: `key amount;key amount;...` (`ghosts`, as upstream; a key is `cells::key_to_string`).
pub(crate) fn ghosts(nbt: &Compound, n: usize) -> Vec<Option<(AeKey, i64)>> {
    let saved = match nbt.get(&java("ghosts")) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let mut cells: Vec<Option<(AeKey, i64)>> = saved
        .split(';')
        .map(|c| {
            let (k, a) = c.rsplit_once(' ')?;
            Some((key_from_string(k)?, a.parse().ok()?))
        })
        .collect();
    cells.resize(n, None);
    cells.truncate(n);
    cells
}

pub(crate) fn set_ghosts(nbt: &mut Compound, cells: &[Option<(AeKey, i64)>]) {
    let text: Vec<String> = cells.iter().map(|c| c.as_ref().map_or(String::new(), |(k, n)| format!("{} {n}", key_to_string(k)))).collect();
    nbt.insert(java("ghosts"), Tag::String(java(&text.join(";"))));
}

/// How many config cells a part has, and how many are open (capacity cards).
pub(crate) fn config_cells(block: &str, fields: &Fields) -> (usize, usize) {
    match kind(block) {
        Some(Kind::Import | Kind::Export) => (9, [1, 5, 9][cards(block, fields, "ae2:capacity_card").min(2)]),
        Some(Kind::StorageBus) => (63, (18 + 9 * cards(block, fields, "ae2:capacity_card")).min(63)),
        Some(Kind::Interface) => (9, 9),
        Some(Kind::LevelEmitter) => (1, 1),
        Some(Kind::Workbench) => (63, 63),
        _ => (0, 0),
    }
}

// ---- Filters

/// A bus's filter (AE2's config with its cards).
#[derive(Clone, Default)]
pub(crate) struct BusFilter {
    pub(crate) keys: Vec<AeKey>,
    pub(crate) fuzzy: bool,
    pub(crate) inverted: bool,
    pub(crate) mode: FuzzyMode,
    /// Settings.ACCESS: 0 both ways, 1 extract only, 2 insert only.
    pub(crate) access: i64,
    pub(crate) no_extract_filter: bool,
    pub(crate) report_all: bool,
    /// fixA: a Void Card (StorageBusPart: MEInventoryHandler.setVoidOverflow).
    pub(crate) void: bool,
}

impl BusFilter {
    /// `MEInventoryHandler.isPreferredStorageFor`: a whitelist naming it.
    pub(crate) fn prefers(&self, what: &AeKey) -> bool {
        !self.inverted && !self.keys.is_empty() && self.allows(what)
    }

    pub(crate) fn allows(&self, key: &AeKey) -> bool {
        if self.keys.is_empty() {
            return true;
        }
        let hit = self.keys.iter().any(|k| k == key || (self.fuzzy && k.fuzzy_equals(key, self.mode)));
        hit != self.inverted
    }
}

fn fuzzy_mode(nbt: &Compound) -> FuzzyMode {
    FuzzyMode::VALUES[nbt_i64(nbt, "fuzzy").clamp(0, 4) as usize]
}

fn bus_filter(block: &str, fields: &Fields) -> BusFilter {
    let (n, open) = config_cells(block, fields);
    BusFilter {
        keys: ghosts(&fields.nbt, n).into_iter().take(open).flatten().map(|(k, _)| k).collect(),
        fuzzy: cards(block, fields, "ae2:fuzzy_card") > 0,
        inverted: cards(block, fields, "ae2:inverter_card") > 0,
        mode: fuzzy_mode(&fields.nbt),
        access: nbt_i64(&fields.nbt, "access"),
        no_extract_filter: nbt_i64(&fields.nbt, "no_extract_filter") != 0,
        report_all: nbt_i64(&fields.nbt, "report_all") != 0,
        void: cards(block, fields, "ae2:void_card") > 0, // fixA
    }
}

// ---- Server state

#[derive(Default)]
pub(crate) struct PartState {
    /// Each import and export bus's tick rate and next tick (TickRates, TickTracker).
    rates: HashMap<(&'static str, Pos), (u64, u64)>,
    /// Which bus each `rates` (and `bursts`) entry is of (its block entity's `tick_token`): a bus placed where another
    /// stood starts at TickRates' first rate, as a new part's TickTracker does, not where the last one left off.
    rate_owners: HashMap<(&'static str, Pos), i64>,
    // ETA (agent eta): each import bus's runs in a row that took something (the run of them now, the last whole run
    // of them) and its target, for a job's time left (upstream's BUS_BURSTS; `import_buses`).
    bursts: HashMap<(&'static str, Pos), (u32, u32, Pos)>,
    /// Storage buses as their last tick saw them: filter and priority.
    storage_buses: HashMap<(&'static str, Pos), (BusFilter, i32)>,
    /// What a storage bus put in that its container didn't take after all: back into the network.
    leftovers: Vec<(&'static str, Pos, AeKey, i64)>,
    /// Redstone out of level emitters and redstone tunnel outputs, by level and position.
    signals: HashMap<(&'static str, Pos), i32>,
    /// Redstone into tunnel inputs (their strongest neighbour), and light at their fronts.
    signal_in: HashMap<(&'static str, Pos), i32>,
    light_in: HashMap<(&'static str, Pos), i32>,
    /// Blocks whose neighbours hear them again at the tick's end.
    updates: Vec<(&'static str, Pos)>,
    /// Tunnels' settings to write at their next tick (a memory card, an attunement): frequency, output.
    p2p_pending: HashMap<(&'static str, Pos), (i64, bool)>,
    /// Levels typed into emitters' screens, for their next tick.
    levels: HashMap<(&'static str, Pos), i64>,
    /// Each viewer's part screen as last sent.
    sent: HashMap<u128, u64>,
    /// Item stack limits by id (a storage bus fills an empty slot up to it).
    max_stacks: HashMap<String, i64>,
}

static STATE: Mutex<Option<PartState>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut PartState) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(PartState::default))
}

pub(crate) fn forget_all() {
    *BUS_CACHES.lock().unwrap_or_else(|e| e.into_inner()) = None; // fixA
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(crate) fn note_max_stack(host: &dyn Host, key: &AeKey) {
    if key.key_type() != KeyType::Items || with_state(|s| s.max_stacks.contains_key(key.id())) {
        return;
    }
    if let Some(stack) = stack_of(host, key, 1) {
        let most = i64::from(max_stack(host, &stack));
        with_state(|s| s.max_stacks.insert(key.id().to_owned(), most));
    }
}

fn max_stack_of(key: &AeKey) -> i64 {
    if key.key_type() == KeyType::Fluids {
        return i64::MAX;
    }
    with_state(|s| s.max_stacks.get(key.id()).copied()).unwrap_or(64)
}

fn net_tick(dim: &'static str) -> u64 {
    crate::net::with_net(dim, |n| n.tick)
}

// ---- Resources

pub(crate) fn item_resource(host: &dyn Host, key: &AeKey) -> Option<ItemResource> {
    stack_of(host, key, 1).map(|s| ItemResource::of(&s))
}

pub(crate) fn fluid_resource(host: &dyn Host, key: &AeKey) -> Option<FluidResource> {
    let k = host.fluid_key(key.id())?;
    Some(FluidResource::of(&FluidStack::new(k, 1000)))
}

pub(crate) fn fluid_id(host: &dyn Host, r: &FluidResource) -> Option<String> {
    match r.fluid() {
        FluidIdentity::Resolved(k) => k.id(host).map(str::to_owned),
        _ => None,
    }
}

// ---- The block entity's tick

/// A part's (or the workbench's) block entity tick, its fields out of the world.
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, block: &str, fields: &mut Fields) {
    let Some(k) = kind(block) else { return };
    let dim = host.dimension();
    let Some(level) = here(&*host) else { return };
    let (_, online, active) = crate::net::device_state(dim, pos);
    match k {
        Kind::Import | Kind::Export | Kind::StorageBus => {
            bus_data(host, level, pos, block, fields);
            if k == Kind::StorageBus {
                let filter = bus_filter(block, fields);
                let priority = nbt_i64(&fields.nbt, "priority") as i32;
                with_state(|s| s.storage_buses.insert((dim, pos), (filter, priority)));
                storage_bus_tick(host, level, dim, pos, active); // fixA: the cached container
            } else if active {
                io_bus(host, level, dim, pos, block, fields, k == Kind::Import);
            }
        }
        Kind::Interface => {
            fields.data.resize(2, 0);
            fields.data[0] = i32::from(active);
            fields.data[1] = if cards(block, fields, "ae2:fuzzy_card") > 0 { nbt_i64(&fields.nbt, "fuzzy") as i32 } else { -1 };
            if active && net_tick(dim) % 5 == 0 {
                interface_stock(host, pos, block, fields);
                crate::iface_fluids::tick(host, pos, block, fields);
            }
        }
        Kind::LevelEmitter | Kind::EnergyEmitter => emitter_tick(host, dim, pos, block, fields, active),
        Kind::P2p => p2p_tick(host, level, dim, pos, block, fields),
        Kind::Workbench => workbench_tick(host, fields),
    }
    let _ = online;
}

/// IOBusMenu's and StorageBusMenu's data (the screen's toolbar), and the redstone card's signal.
fn bus_data(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, block: &str, fields: &mut Fields) {
    let (_, open) = config_cells(block, fields);
    let rs_card = cards(block, fields, "ae2:redstone_card") > 0;
    let data = {
    let card = |c: &str| cards(block, fields, c) > 0;
    let n = |k: &str| nbt_i64(&fields.nbt, k) as i32;
    [
        open as i32,
        if card("ae2:redstone_card") { n("redstone") } else { -1 },
        n("schedule"),
        i32::from(card("ae2:inverter_card")),
        if card("ae2:crafting_card") { n("craft_only") } else { -1 },
        if card("ae2:fuzzy_card") { n("fuzzy") } else { -1 },
        // The formation plane's Settings.PLACE_BLOCK in the access slot (it has no access mode).
        if block == "ae2:formation_plane" { n("drop_items") } else { n("access") },
        n("no_extract_filter"),
        n("report_all"),
    ]
    };
    if fields.data.len() != 9 || fields.data[..] != data[..] {
        fields.data = data.to_vec();
    }
    if rs_card {
        let on = level.has_neighbor_signal(&*host, real(pos));
        let was = nbt_i64(&fields.nbt, "rs") != 0;
        if on && !was {
            set_nbt_i64(&mut fields.nbt, "pulse", 1);
        }
        if on != was {
            set_nbt_i64(&mut fields.nbt, "rs", i64::from(on));
        }
    }
}

/// Whether the bus works this time (its redstone card's mode), using up a pulse.
fn bus_enabled(block: &str, fields: &mut Fields) -> bool {
    if cards(block, fields, "ae2:redstone_card") == 0 {
        return true;
    }
    let on = nbt_i64(&fields.nbt, "rs") != 0;
    match nbt_i64(&fields.nbt, "redstone") {
        1 => !on,
        2 => {
            let pulse = nbt_i64(&fields.nbt, "pulse") != 0;
            if pulse {
                set_nbt_i64(&mut fields.nbt, "pulse", 0);
            }
            pulse
        }
        _ => on,
    }
}

// ---- Import and export buses

/// ImportBusPart / ExportBusPart: items (and fluids, a bucket for each item a run may move) through the face the bus
/// touches, at TickRates' pace (5 to 40 ticks import, 5 to 60 export; 2 faster for each run that moved something, 1
/// slower for each that didn't), as many a run as its speed cards allow (1, 8, 32, 64, 96).
fn io_bus(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str, pos: Pos, block: &str, fields: &mut Fields, import: bool) {
    let now = net_tick(dim);
    let (fastest, slowest) = if import { (5u64, 40u64) } else { (5u64, 60u64) };
    let mut token = nbt_i64(&fields.nbt, "tick_token");
    if token == 0 {
        token = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as i64) & 0x7FFF_FFFF_FFFF_FFFF) | 1;
        set_nbt_i64(&mut fields.nbt, "tick_token", token);
    }
    with_state(|s| {
        if s.rate_owners.insert((dim, pos), token) != Some(token) {
            s.rates.remove(&(dim, pos));
            s.bursts.remove(&(dim, pos));
        }
    });
    let (rate, due) = with_state(|s| *s.rates.entry((dim, pos)).or_insert(((fastest + slowest) / 2, now)));
    if now < due {
        return;
    }
    let slower = (rate + 1).min(slowest);
    with_state(|s| s.rates.insert((dim, pos), (slower, now + slower)));
    if !bus_enabled(block, fields) {
        with_state(|s| s.rates.insert((dim, pos), (slowest, now + slowest)));
        return;
    }
    let f = facing(&*host, level, pos);
    let target = add(pos, f);
    let face = dir_of(neg(f));
    // AE2's own blocks are no containers to a bus, but the interface (and a tunnel, and its machines).
    let tb = crate::net::block_id(&*host, level, target);
    if tb.starts_with("ae2:") && kind(&tb) != Some(Kind::Interface) && !is_p2p(&tb) && !crate::autocraft::is_provider(&tb) && crate::net::is_device(&tb) && !matches!(tb.as_str(), "ae2:inscriber" | "ae2:charger" | "ae2:molecular_assembler") {
        return;
    }
    let filter = bus_filter(block, fields);
    let per = [1i64, 8, 32, 64, 96][cards(block, fields, "ae2:speed_card").min(4)];
    let scope = Scope::Grid(pos);
    let moved = if import {
        import_items(host, level, pos, target, face, &filter, per) + import_fluids(host, level, pos, target, face, &filter, per)
    } else {
        let turn = nbt_i64(&fields.nbt, "turn");
        set_nbt_i64(&mut fields.nbt, "turn", turn + 1);
        // The Crafting Card (`craft_cards`): what doesn't go out is asked for; "craft only" exports only what was made.
        let crafting = cards(block, fields, "ae2:crafting_card") > 0;
        let craft_only = crafting && nbt_i64(&fields.nbt, "craft_only") != 0;
        if !craft_only {
            crate::craft_cards::forget_credit(dim, pos);
        }
        let mut keys = filter.keys.clone();
        if !keys.is_empty() {
            match nbt_i64(&fields.nbt, "schedule") {
                1 => {
                    let n = keys.len() as i64;
                    keys.rotate_left(turn.rem_euclid(n) as usize);
                }
                2 => {
                    let r = (now as usize).wrapping_mul(2_654_435_761) % keys.len();
                    keys.rotate_left(r);
                }
                _ => {}
            }
        }
        let mut moved = 0;
        for key in keys {
            if moved >= per {
                break;
            }
            let fluid = key.key_type() == KeyType::Fluids;
            // ExportBusPart: with a Fuzzy Card, the first of what matches its config in the network.
            let key = if filter.fuzzy && !fluid && !craft_only {
                storage::with_storage(host, scope, None, |st| st.contents().find_fuzzy(&key, filter.mode)).unwrap_or_default().into_iter().find(|(_, n)| *n > 0).map_or(key, |(k, _)| k)
            } else {
                key
            };
            // A run moves `per - moved` items, or that many buckets of a fluid (in mB).
            let unit = if fluid { 1000 } else { 1 };
            let want = (per - moved) * unit;
            let allowed = if craft_only { crate::craft_cards::credit(dim, pos, &key).min(want) } else { want };
            let went = if allowed <= 0 {
                0
            } else if fluid {
                export_fluid_mb(host, level, pos, target, face, &key, allowed)
            } else {
                export_items(host, level, pos, target, face, &key, allowed)
            };
            if craft_only {
                crate::craft_cards::spend(dim, pos, &key, went);
            }
            moved += (went + unit - 1) / unit;
            if went == 0 && crafting {
                crate::craft_cards::export_bus(host, level, pos, target, face, &key, want, craft_only);
            }
        }
        moved
    };
    if moved > 0 {
        let faster = rate.saturating_sub(2).max(fastest);
        with_state(|s| s.rates.insert((dim, pos), (faster, now + faster)));
    }
    // ETA (agent eta): how many runs in a row an import bus takes something (a machine making more than one stack a
    // craft keeps its bus at work for more than one run).
    if import {
        with_state(|s| {
            let b = s.bursts.entry((dim, pos)).or_insert((0, 0, target));
            let (run, last) = next_burst(b.0, b.1, moved > 0);
            *b = (run, last, target);
        });
    }
}

/// ETA (agent eta): an import bus's (run now, last whole run) after a run that did or didn't take something.
pub(crate) fn next_burst(now: u32, last: u32, worked: bool) -> (u32, u32) {
    if worked {
        (now.saturating_add(1), last)
    } else if now > 0 {
        (0, now)
    } else {
        (now, last)
    }
}

/// ETA (agent eta): the import buses of `dim` as their last run left them: (bus, target, interval, ticks to the next run
/// (below 0: due), run now, last whole run). The positions of buses since removed may linger; check they're on the grid.
pub(crate) fn import_buses(dim: &'static str) -> Vec<(Pos, Pos, u64, i64, u32, u32)> {
    let now = net_tick(dim) as i64;
    with_state(|s| {
        s.bursts
            .iter()
            .filter(|((d, _), _)| *d == dim)
            .map(|((_, bus), (run, last, target))| {
                let (rate, due) = s.rates.get(&(dim, *bus)).copied().unwrap_or((22, now as u64));
                (*bus, *target, rate, due as i64 - now, *run, *last)
            })
            .collect()
    })
}

/// Items out of the container in front into the network: how many.
fn import_items(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, target: Pos, face: Direction, filter: &BusFilter, per: i64) -> i64 {
    let Some(h) = level.capability(host, &ITEMS, target, Some(face)) else {
        debug(&format!("import {pos:?}: no ITEMS at {target:?} {face:?}"));
        return 0;
    };
    let scope = Scope::Grid(pos);
    let mut moved = 0;
    debug(&format!("import {pos:?}: {} slots at {target:?}", h.size(host)));
    for i in 0..h.size(host) {
        if moved >= per {
            break;
        }
        let Some(res) = h.resource(host, i) else { continue };
        let have = i64::from(h.amount(host, i));
        if have == 0 {
            continue;
        }
        let key = key_of(&*host, res.as_stack());
        if !filter.allows(&key) {
            continue;
        }
        let want = (per - moved).min(have);
        let room = storage::power_allows(host.dimension(), pos, &key, storage::simulate(host, scope, None, &key, want, true));
        if room <= 0 {
            continue;
        }
        let got = host.transaction(|host, tx| Ok::<u32, ()>(h.extract_at(host, tx, i, &res, room as u32).unwrap_or(0))).unwrap_or(0);
        debug(&format!("  slot {i}: {} x{have}, room {room}, got {got}", key.id()));
        if got == 0 {
            continue;
        }
        let stored = storage::insert(host, scope, None, &key, i64::from(got));
        if stored < i64::from(got) {
            let back = got - stored as u32;
            let _ = host.transaction(|host, tx| Ok::<u32, ()>(h.insert_at(host, tx, i, &res, back).unwrap_or(0)));
        }
        moved += stored;
    }
    moved
}

/// Fluids out of the tanks in front: a bucket for each item a run may move. In items moved (a bucket each).
fn import_fluids(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, target: Pos, face: Direction, filter: &BusFilter, per: i64) -> i64 {
    let Some(h) = level.capability(host, &FLUIDS, target, Some(face)) else { return 0 };
    let scope = Scope::Grid(pos);
    let mut moved_mb = 0i64;
    let most = per * 1000;
    for i in 0..h.size(host) {
        if moved_mb >= most {
            break;
        }
        let Some(res) = h.resource(host, i) else { continue };
        let have = i64::from(h.amount(host, i));
        let Some(id) = fluid_id(&*host, &res) else { continue };
        if have == 0 {
            continue;
        }
        let key = AeKey::fluid(&id, "");
        if !filter.allows(&key) {
            continue;
        }
        let want = (most - moved_mb).min(have);
        let room = storage::power_allows(host.dimension(), pos, &key, storage::simulate(host, scope, None, &key, want, true));
        if room <= 0 {
            continue;
        }
        let got = host.transaction(|host, tx| Ok::<u32, ()>(h.extract_at(host, tx, i, &res, room as u32).unwrap_or(0))).unwrap_or(0);
        debug(&format!("  slot {i}: {} x{have}, room {room}, got {got}", key.id()));
        if got == 0 {
            continue;
        }
        let stored = storage::insert(host, scope, None, &key, i64::from(got));
        if stored < i64::from(got) {
            let back = got - stored as u32;
            let _ = host.transaction(|host, tx| Ok::<u32, ()>(h.insert_at(host, tx, i, &res, back).unwrap_or(0)));
        }
        moved_mb += stored;
    }
    (moved_mb + 999) / 1000
}

/// How much of `res` the handler takes: an insert in a transaction that is then undone.
fn fits<R>(host: &mut dyn ServerHost, h: &Capability<dyn ResourceHandler<R>>, res: &R, amount: u32) -> u32 {
    host.transaction(|host, tx| Err::<(), u32>(h.insert(host, tx, res, amount).unwrap_or(0))).err().and_then(crate::aborted).unwrap_or(0)
}

fn export_items(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, target: Pos, face: Direction, key: &AeKey, want: i64) -> i64 {
    let Some(h) = level.capability(host, &ITEMS, target, Some(face)) else { return 0 };
    let Some(res) = item_resource(&*host, key) else { return 0 };
    let scope = Scope::Grid(pos);
    let have = storage::simulate(host, scope, None, key, want, false);
    if have <= 0 {
        return 0;
    }
    let room = i64::from(fits(host, &h, &res, have as u32)).min(have);
    if room <= 0 {
        return 0;
    }
    let got = storage::extract(host, scope, None, key, room);
    if got <= 0 {
        return 0;
    }
    let went = host.transaction(|host, tx| h.insert(host, tx, &res, got as u32).map_err(|_| ())).unwrap_or(0);
    if i64::from(went) < got {
        storage::insert(host, scope, None, key, got - i64::from(went));
    }
    i64::from(went)
}

/// Up to `want_mb` of a fluid out of the network into what the bus faces: the millibuckets that went.
fn export_fluid_mb(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, target: Pos, face: Direction, key: &AeKey, want_mb: i64) -> i64 {
    let Some(h) = level.capability(host, &FLUIDS, target, Some(face)) else { return 0 };
    let Some(res) = fluid_resource(&*host, key) else { return 0 };
    let scope = Scope::Grid(pos);
    let have = storage::simulate(host, scope, None, key, want_mb, false);
    if have <= 0 {
        return 0;
    }
    let room = i64::from(fits(host, &h, &res, have.min(i64::from(u32::MAX)) as u32)).min(have);
    if room <= 0 {
        return 0;
    }
    let got = storage::extract(host, scope, None, key, room);
    if got <= 0 {
        return 0;
    }
    let went = host.transaction(|host, tx| h.insert(host, tx, &res, got as u32).map_err(|_| ())).unwrap_or(0);
    if i64::from(went) < got {
        storage::insert(host, scope, None, key, got - i64::from(went));
    }
    i64::from(went)
}

// ---- The storage bus (fixA: a cached view of its container, `ext_cache`; or an ME interface's network)

/// A storage bus's cached container: its items and tanks as last seen, and the block entity they were read from.
#[derive(Default)]
pub(crate) struct BusCache {
    target: Option<(Pos, Option<BlockEntityRef>)>,
    items: crate::ext_cache::SlotCache<ItemResource>,
    fluids: crate::ext_cache::SlotCache<FluidResource>,
}

static BUS_CACHES: Mutex<Option<HashMap<(&'static str, Pos), BusCache>>> = Mutex::new(None);

fn with_caches<R>(f: impl FnOnce(&mut HashMap<(&'static str, Pos), BusCache>) -> R) -> R {
    let mut g = BUS_CACHES.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// One transaction's (or a sweep's) view of an item handler (`ext_cache::Slots`).
struct LiveItems<'a> {
    h: &'a Capability<dyn ResourceHandler<ItemResource>>,
    host: &'a mut dyn ServerHost,
    tx: Option<&'a TransactionContext>,
}

impl crate::ext_cache::Slots<ItemResource> for LiveItems<'_> {
    fn size(&mut self) -> usize {
        self.h.size(self.host)
    }
    fn amount(&mut self, i: usize) -> i64 {
        i64::from(self.h.amount(self.host, i))
    }
    fn resource(&mut self, i: usize) -> Option<ItemResource> {
        self.h.resource(self.host, i)
    }
    fn capacity(&mut self, i: usize, r: &ItemResource) -> i64 {
        i64::from(self.h.capacity(self.host, i, Some(r)))
    }
    fn empty_capacity(&mut self, _i: usize) -> i64 {
        0
    }
    fn insert(&mut self, i: usize, r: &ItemResource, n: i64) -> i64 {
        let Some(tx) = self.tx else { return 0 };
        i64::from(self.h.insert_at(self.host, tx, i, r, n.clamp(0, i64::from(u32::MAX)) as u32).unwrap_or(0))
    }
    fn extract(&mut self, i: usize, r: &ItemResource, n: i64) -> i64 {
        let Some(tx) = self.tx else { return 0 };
        i64::from(self.h.extract_at(self.host, tx, i, r, n.clamp(0, i64::from(u32::MAX)) as u32).unwrap_or(0))
    }
    fn can_extract(&mut self, i: usize, r: &ItemResource, n: i64) -> bool {
        // fixG: StorageAdapter's probe for EXTRACTABLE_ONLY: 1, then the whole amount, in a transaction always aborted.
        let h = self.h;
        let n = n.clamp(1, i64::from(u32::MAX)) as u32;
        self.host.transaction(|host, tx| Err::<(), bool>(h.extract_at(host, tx, i, r, 1).unwrap_or(0) > 0 || h.extract_at(host, tx, i, r, n).unwrap_or(0) > 0)).err().and_then(crate::aborted).unwrap_or(false)
    }
    fn key_of(&mut self, r: &ItemResource) -> AeKey {
        key_of(&*self.host, r.as_stack())
    }
    fn resource_of(&mut self, key: &AeKey) -> Option<ItemResource> {
        item_resource(&*self.host, key)
    }
}

/// The same for a fluid handler (amounts in millibuckets, as the bus always counted them).
struct LiveFluids<'a> {
    h: &'a Capability<dyn ResourceHandler<FluidResource>>,
    host: &'a mut dyn ServerHost,
    tx: Option<&'a TransactionContext>,
}

impl crate::ext_cache::Slots<FluidResource> for LiveFluids<'_> {
    fn size(&mut self) -> usize {
        self.h.size(self.host)
    }
    fn amount(&mut self, i: usize) -> i64 {
        i64::from(self.h.amount(self.host, i))
    }
    fn resource(&mut self, i: usize) -> Option<FluidResource> {
        self.h.resource(self.host, i)
    }
    fn capacity(&mut self, i: usize, r: &FluidResource) -> i64 {
        i64::from(self.h.capacity(self.host, i, Some(r)))
    }
    fn empty_capacity(&mut self, i: usize) -> i64 {
        let probe = self.host.fluid_key("minecraft:water").map(|k| FluidResource::of(&FluidStack::new(k, 1000)));
        probe.map_or(0, |p| i64::from(self.h.capacity(self.host, i, Some(&p))))
    }
    fn insert(&mut self, i: usize, r: &FluidResource, n: i64) -> i64 {
        let Some(tx) = self.tx else { return 0 };
        i64::from(self.h.insert_at(self.host, tx, i, r, n.clamp(0, i64::from(u32::MAX)) as u32).unwrap_or(0))
    }
    fn extract(&mut self, i: usize, r: &FluidResource, n: i64) -> i64 {
        let Some(tx) = self.tx else { return 0 };
        i64::from(self.h.extract_at(self.host, tx, i, r, n.clamp(0, i64::from(u32::MAX)) as u32).unwrap_or(0))
    }
    fn can_extract(&mut self, i: usize, r: &FluidResource, n: i64) -> bool {
        // fixG: as for items (1 mB, then the whole amount; aborted).
        let h = self.h;
        let n = n.clamp(1, i64::from(u32::MAX)) as u32;
        self.host.transaction(|host, tx| Err::<(), bool>(h.extract_at(host, tx, i, r, 1).unwrap_or(0) > 0 || h.extract_at(host, tx, i, r, n).unwrap_or(0) > 0)).err().and_then(crate::aborted).unwrap_or(false)
    }
    fn key_of(&mut self, r: &FluidResource) -> AeKey {
        AeKey::fluid(&fluid_id(&*self.host, r).unwrap_or_default(), "")
    }
    fn resource_of(&mut self, key: &AeKey) -> Option<FluidResource> {
        fluid_resource(&*self.host, key)
    }
}

/// What is in front of the bus at `bus` (a part's cell's neighbour), and the face it is reached through.
fn bus_target(host: &dyn Host, level: LevelRef, bus: Pos) -> (Pos, Direction, Pos) {
    let f = facing(host, level, bus);
    (add(real(bus), f), dir_of(neg(f)), f)
}

/// An ME interface the bus faces (the block, or the part on the facing side of a cable bus there), as a network node;
/// with whether it has config (InterfaceLogic.hasConfig: then only its slots are seen).
fn facing_interface(host: &dyn Host, level: LevelRef, target: Pos, f: Pos) -> Option<(Pos, bool)> {
    let tb = crate::net::block_id(host, level, target);
    let node = if kind(&tb) == Some(Kind::Interface) {
        target
    } else if crate::bus::is_cell_block(&level.block(host, target).and_then(|b| b.id(host).map(str::to_owned)).unwrap_or_default()) {
        let side = SIDES.iter().position(|d| *d == neg(f))?;
        let p = hosted(target, side);
        let pb = crate::net::block_id(host, level, p);
        (kind(&pb) == Some(Kind::Interface)).then_some(p)?
    } else {
        return None;
    };
    let configured = crate::bus::nbt_at(host, level, node).is_some_and(|n| ghosts(&n, 9).iter().any(Option::is_some));
    Some((node, configured))
}

/// The storage bus's tick (StorageBusPart.tickingRequest): a step of the sweep over its container, at its pace; a
/// change from outside lists the terminals again.
fn storage_bus_tick(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str, pos: Pos, active: bool) {
    if !active {
        return;
    }
    let (target, face, f) = bus_target(&*host, level, pos);
    if facing_interface(&*host, level, target, f).is_some_and(|(_, configured)| !configured) {
        with_caches(|c| c.remove(&(dim, pos)));
        return;
    }
    let mut cache = with_caches(|c| c.remove(&(dim, pos))).unwrap_or_default();
    let be = host.block_entity(level, target);
    if cache.target != Some((target, be)) {
        cache = BusCache { target: Some((target, be)), ..BusCache::default() };
    }
    let mut changed = false;
    match level.capability(host, &ITEMS, target, Some(face)) {
        Some(h) => changed |= cache.items.tick(&mut LiveItems { h: &h, host, tx: None }),
        None if cache.items.len() > 0 => {
            cache.items = Default::default();
            changed = true;
        }
        None => {}
    }
    match level.capability(host, &FLUIDS, target, Some(face)) {
        Some(h) => changed |= cache.fluids.tick(&mut LiveFluids { h: &h, host, tx: None }),
        None if cache.fluids.len() > 0 => {
            cache.fluids = Default::default();
            changed = true;
        }
        None => {}
    }
    if changed {
        storage::bump_generation(dim);
    }
    with_caches(|c| c.insert((dim, pos), cache));
}

/// A storage bus's container as mounted for one storage call: the cache (taken out of the bus's keeping) and the
/// handlers, moved through live (`ext_cache::host_do`).
pub(crate) struct ExtStore {
    dim: &'static str,
    bus: Pos,
    filter: BusFilter,
    /// Where its handlers are found (looked up for each live move: a handler can't be kept in the store).
    level: LevelRef,
    target: Pos,
    face: Direction,
    cache: BusCache,
    changed_outside: bool,
}

impl ExtStore {
    fn is_fluid(key: &AeKey) -> bool {
        key.key_type() == KeyType::Fluids
    }
}

impl crate::grid::MeStorage for ExtStore {
    fn insert(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
        if amount <= 0 || self.filter.access == 1 || !self.filter.allows(what) {
            return 0;
        }
        let most = max_stack_of(what);
        let went = if mode == Actionable::Simulate {
            if Self::is_fluid(what) { self.cache.fluids.insert(None, what, amount, most) } else { self.cache.items.insert(None, what, amount, most) }
        } else if Self::is_fluid(what) {
            let (level, target, face, cache) = (self.level, self.target, self.face, &mut self.cache.fluids);
            crate::ext_cache::host_do(|host| {
                let Some(h) = level.capability(host, &FLUIDS, target, Some(face)) else { return 0 };
                host.transaction(|host, tx| Ok::<i64, ()>(cache.insert(Some(&mut LiveFluids { h: &h, host, tx: Some(tx) }), what, amount, most))).unwrap_or(0)
            })
            .unwrap_or(0)
        } else {
            let (level, target, face, cache) = (self.level, self.target, self.face, &mut self.cache.items);
            crate::ext_cache::host_do(|host| {
                let Some(h) = level.capability(host, &ITEMS, target, Some(face)) else { return 0 };
                host.transaction(|host, tx| Ok::<i64, ()>(cache.insert(Some(&mut LiveItems { h: &h, host, tx: Some(tx) }), what, amount, most))).unwrap_or(0)
            })
            .unwrap_or(0)
        };
        // A Void Card takes what doesn't fit (MEInventoryHandler.voidOverflow).
        if self.filter.void { amount } else { went }
    }

    fn extract(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
        if amount <= 0 || self.filter.access == 2 || !(self.filter.no_extract_filter || self.filter.allows(what)) {
            return 0;
        }
        if mode == Actionable::Simulate {
            return if Self::is_fluid(what) { self.cache.fluids.extract(None, what, amount) } else { self.cache.items.extract(None, what, amount) };
        }
        let (level, target, face) = (self.level, self.target, self.face);
        if Self::is_fluid(what) {
            let cache = &mut self.cache.fluids;
            crate::ext_cache::host_do(|host| {
                let Some(h) = level.capability(host, &FLUIDS, target, Some(face)) else { return 0 };
                host.transaction(|host, tx| Ok::<i64, ()>(cache.extract(Some(&mut LiveFluids { h: &h, host, tx: Some(tx) }), what, amount))).unwrap_or(0)
            })
            .unwrap_or(0)
        } else {
            let cache = &mut self.cache.items;
            crate::ext_cache::host_do(|host| {
                let Some(h) = level.capability(host, &ITEMS, target, Some(face)) else { return 0 };
                host.transaction(|host, tx| Ok::<i64, ()>(cache.extract(Some(&mut LiveItems { h: &h, host, tx: Some(tx) }), what, amount))).unwrap_or(0)
            })
            .unwrap_or(0)
        }
    }

    fn available_stacks(&self, out: &mut KeyCounter) {
        // fixG: STORAGE_FILTER as AE2 has it. Extractable only (`!report_all`): the container's adapter lists only what
        // its probe could extract (StorageAdapter.extractableOnly, the slots' cached probes); with Filter On Extract
        // too, MEInventoryHandler lists nothing through an insert-only bus and only the filter's keys otherwise.
        let f = &self.filter;
        let ext_only = !f.report_all;
        let filter_listed = ext_only && !f.no_extract_filter;
        if filter_listed && f.access == 2 {
            return;
        }
        let allow = |k: &AeKey| !filter_listed || f.allows(k);
        self.cache.items.available(out, ext_only, allow);
        self.cache.fluids.available(out, ext_only, allow);
    }

    fn is_preferred_for(&self, what: &AeKey) -> bool {
        self.filter.prefers(what) || self.cache.items.holds(what) || self.cache.fluids.holds(what)
    }

    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// A storage bus's or formation plane's filter and priority, as its last tick saw them.
pub(crate) fn filter_of(dim: &'static str, pos: Pos) -> Option<(BusFilter, i32)> {
    with_state(|s| s.storage_buses.get(&(dim, pos)).cloned())
}

/// What a storage bus mounts: its container (cached), or an unconfigured interface's network (by level and node).
pub(crate) enum ExtOpen {
    Store(ExtStore),
    Interface { filter: BusFilter, dim: &'static str, iface: Pos },
}

/// The grid's storage buses (in every level it reaches), for `storage::open`: priority and what each mounts.
pub(crate) fn open_externals(host: &mut dyn ServerHost, devices: &[(&'static str, Pos, String)]) -> Vec<(i32, ExtOpen)> {
    let mut out = Vec::new();
    for (dim, bus, block) in devices {
        if block != "ae2:storage_bus" {
            continue;
        }
        let (dim, bus) = (*dim, *bus);
        let (_, _, active) = crate::net::device_state(dim, bus);
        if !active {
            continue;
        }
        let Some(level) = host.level(dim) else { continue };
        let Some((filter, priority)) = with_state(|s| s.storage_buses.get(&(dim, bus)).cloned()) else { continue };
        let (target, face, f) = bus_target(&*host, level, bus);
        let tb = crate::net::block_id(&*host, level, target);
        let iface = facing_interface(&*host, level, target, f);
        if iface.is_none() && tb.starts_with("ae2:") && !is_p2p(&tb) {
            continue;
        }
        if let Some((node, configured)) = iface {
            // polish: StorageBusPart's RECURSIVE advancement, on an interface.
            crate::advancements::due(dim, bus, crate::advancements::RECURSIVE);
            if !configured {
                // InterfaceLogic.getInventory without config: the interface's whole network.
                out.push((priority, ExtOpen::Interface { filter, dim, iface: node }));
                continue;
            }
        }
        // The cache the bus's ticks keep (taken while mounted; a bus that hasn't ticked yet starts empty).
        let Some(mut cache) = with_caches(|c| c.remove(&(dim, bus))) else { continue };
        let changed_outside = cache.items.take_changed() | cache.fluids.take_changed();
        out.push((priority, ExtOpen::Store(ExtStore { dim, bus, filter, level, target, face, cache, changed_outside })));
    }
    out
}

/// A storage bus's container unmounted: its cache back in the bus's keeping. Whether it changed from outside since
/// last mounted (terminals list again); the network's own moves were made live and listed by the store.
pub(crate) fn close_external(_host: &mut dyn ServerHost, ext: ExtStore) -> bool {
    let ExtStore { dim, bus, cache, changed_outside, .. } = ext;
    with_caches(|c| c.insert((dim, bus), cache));
    changed_outside
}

// ---- Interfaces

/// InterfaceLogic.updateStorage: each configured slot stocked to its amount from the network (fuzzy with a Fuzzy
/// Card), what is over it or not wanted (an unconfigured slot's) back into the network.
fn interface_stock(host: &mut dyn ServerHost, pos: Pos, block: &str, fields: &mut Fields) {
    if fields.items.len() < 10 {
        fields.items.resize(10, None);
    }
    let config = ghosts(&fields.nbt, 9);
    let scope = Scope::Grid(pos);
    let crafting = cards(block, fields, "ae2:crafting_card") > 0;
    for (i, want) in config.iter().enumerate() {
        let held = fields.items[i].clone();
        // What doesn't belong in the slot goes back first.
        if let Some(stack) = &held {
            let key = key_of(&*host, stack);
            let keep = match want {
                Some((k, n)) if *k == key => i64::from(stack.count()).min(*n),
                _ => 0,
            };
            let over = i64::from(stack.count()) - keep;
            if over > 0 {
                let stored = storage::insert(host, scope, None, &key, over);
                fields.items[i] = with_count(stack, stack.count() - stored as u32);
            }
        }
        let Some((key, amount)) = want else { continue };
        if key.key_type() != KeyType::Items {
            continue;
        }
        let have = fields.items[i].as_ref().map_or(0, |s| i64::from(s.count()));
        let Some(one) = stack_of(&*host, key, 1) else { continue };
        let target = (*amount).min(i64::from(max_stack(&*host, &one)));
        if have < target && (have == 0 || fields.items[i].as_ref().is_some_and(|s| key_of(&*host, s) == *key)) {
            let got = storage::extract(host, scope, None, key, target - have);
            if got > 0 {
                fields.items[i] = match fields.items[i].take() {
                    Some(s) => with_count(&s, s.count() + got as u32),
                    None => stack_of(&*host, key, got as u32),
                };
            }
            // InterfaceLogic.handleCrafting: with a Crafting Card, what the network didn't have is asked for.
            if crafting && have + got < target {
                crate::craft_cards::want(host.dimension(), pos, key, target - have - got, crate::craft_cards::Who::Interface);
            }
        }
    }
}

/// The interface's slots to other mods (InterfaceLogic's exposed inventory): the nine storage slots, each taking
/// only its configured item when it has one.
pub(crate) struct InterfaceItems {
    inner: Box<dyn ResourceHandler<ItemResource>>,
    config: Vec<Option<ItemResource>>,
}

impl ResourceHandler<ItemResource> for InterfaceItems {
    fn size(&self, host: &mut dyn ServerHost) -> usize {
        self.inner.size(host).min(9)
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        self.inner.resource(host, index)
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        self.inner.amount(host, index)
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, resource: Option<&ItemResource>) -> u32 {
        self.inner.capacity(host, index, resource)
    }
    fn is_valid(&self, host: &mut dyn ServerHost, index: usize, resource: &ItemResource) -> bool {
        match self.config.get(index) {
            Some(Some(want)) => want == resource,
            _ => self.inner.is_valid(host, index, resource),
        }
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        if index >= 9 || !self.is_valid(host, index, resource) {
            return Ok(0);
        }
        self.inner.insert_at(host, tx, index, resource, amount).map(|n| n.min(amount))
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        if index >= 9 {
            return Ok(0);
        }
        self.inner.extract_at(host, tx, index, resource, amount).map(|n| n.min(amount))
    }
}

/// ITEMS for an interface, asked before the game's (which, asked with no side, gives the slots themselves).
pub(crate) fn interface_items(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    at.side?;
    let inner = at.level.capability(host, &ITEMS, at.pos, None)?;
    interface_over(host, at.level, at.pos, Box::new(crate::bus::Slice { inner, base: 0, len: usize::MAX }))
}

/// An interface's exposed slots over `inner` (its own block's, or its side's run of a cable bus's).
pub(crate) fn interface_over(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, inner: Box<dyn ResourceHandler<ItemResource>>) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    let nbt = crate::bus::nbt_at(&*host, level, pos)?;
    let config = ghosts(&nbt, 9).into_iter().map(|c| c.and_then(|(k, _)| item_resource(&*host, &k))).collect();
    Some(Box::new(InterfaceItems { inner, config }))
}

// ---- Level emitters

/// StorageLevelEmitterPart / EnergyLevelEmitterPart: on while the network holds at least the level of the
/// configured key (all that fuzzy-matches with a Fuzzy Card; stored AE for the energy emitter), or below it in the
/// other redstone mode; off without a channel. Checked every 10 ticks.
fn emitter_tick(host: &mut dyn ServerHost, dim: &'static str, pos: Pos, block: &str, fields: &mut Fields, active: bool) {
    if let Some(level) = with_state(|s| s.levels.remove(&(dim, pos))) {
        set_nbt_i64(&mut fields.nbt, "level", level);
    }
    let energy = block == "ae2:energy_level_emitter";
    let size = if energy { 2 } else { 5 };
    fields.data.resize(size, 0);
    fields.data[0] = i32::from(active);
    fields.data[1] = nbt_i64(&fields.nbt, "below") as i32;
    if !energy {
        fields.data[2] = i32::from(cards(block, fields, "ae2:crafting_card") > 0);
        fields.data[3] = nbt_i64(&fields.nbt, "craft_redstone") as i32;
        fields.data[4] = if cards(block, fields, "ae2:fuzzy_card") > 0 { nbt_i64(&fields.nbt, "fuzzy") as i32 } else { -1 };
    }
    // The Crafting Card: "emit to craft" offers the configured key to the network's plans (`craft_cards`).
    let carded = !energy && cards(block, fields, "ae2:crafting_card") > 0;
    let configured = if energy { None } else { ghosts(&fields.nbt, 1).into_iter().next().flatten().map(|(k, _)| k) };
    if !energy {
        let emits = if carded && nbt_i64(&fields.nbt, "craft_redstone") != 0 { configured.clone() } else { None };
        crate::craft_cards::note_emitter(dim, pos, emits);
    }
    if net_tick(dim) % 10 != 0 && with_state(|s| s.signals.contains_key(&(dim, pos))) {
        return;
    }
    if carded {
        // StorageLevelEmitterPart.getDirectOutput: on while a CPU of the grid waits for the key (any job with none).
        let on = active && crate::autocraft::grid_waiting(dim, pos, configured.as_ref());
        set_signal(dim, pos, if on { 15 } else { 0 });
        crate::net::with_net(dim, |n| n.prop_sets.push((pos, "powered", on.to_string())));
        return;
    }
    let level = nbt_i64(&fields.nbt, "level");
    let reached = if !active {
        false
    } else if energy {
        crate::net::grid_power(dim, pos).is_some_and(|(e, _)| e >= level as f64)
    } else {
        match ghosts(&fields.nbt, 1).into_iter().next().flatten() {
            Some((key, _)) => {
                let fuzzy = (cards(block, fields, "ae2:fuzzy_card") > 0).then(|| fuzzy_mode(&fields.nbt));
                let held: i64 = storage::with_storage(host, Scope::Grid(pos), None, |st| {
                    let c = st.contents();
                    match fuzzy {
                        Some(mode) if key.supports_fuzzy_range_search() => c.find_fuzzy(&key, mode).iter().map(|(_, n)| *n).sum(),
                        _ => c.get(&key),
                    }
                })
                .unwrap_or(0);
                held >= level.max(1)
            }
            None => false,
        }
    };
    // RedstoneMode: emit at or above the level, or below it.
    let on = active && if nbt_i64(&fields.nbt, "below") != 0 { !reached } else { reached };
    set_signal(dim, pos, if on { 15 } else { 0 });
    crate::net::with_net(dim, |n| n.prop_sets.push((pos, "powered", on.to_string())));
}

fn set_signal(dim: &'static str, pos: Pos, level: i32) {
    with_state(|s| {
        if s.signals.get(&(dim, pos)).copied() != Some(level) {
            s.signals.insert((dim, pos), level);
            s.updates.push((dim, pos));
        }
    });
}

/// `block_signal`: the level emitters' and redstone tunnel outputs' redstone.
pub(crate) fn signal(dim: &'static str, pos: Pos) -> Option<i32> {
    with_state(|s| s.signals.get(&(dim, pos)).copied())
}

pub(crate) fn is_signal_block(block: &str) -> bool {
    matches!(block, "ae2:level_emitter" | "ae2:energy_level_emitter" | "ae2:redstone_p2p_tunnel")
}

// ---- P2P tunnels

/// A tunnel's tick: its settings into the level's table (a memory card's waiting first), a redstone input's signal,
/// a light input's light; outputs take theirs from their input.
fn p2p_tick(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str, pos: Pos, block: &str, fields: &mut Fields) {
    if let Some((freq, output)) = with_state(|s| s.p2p_pending.remove(&(dim, pos))) {
        set_nbt_i64(&mut fields.nbt, "freq", freq);
        set_nbt_i64(&mut fields.nbt, "output", i64::from(output));
    }
    let freq = nbt_i64(&fields.nbt, "freq");
    let output = nbt_i64(&fields.nbt, "output") != 0;
    crate::net::report_p2p(dim, pos, block, freq, output);
    let peers = crate::net::p2p_peers(dim, pos);
    match block {
        "ae2:redstone_p2p_tunnel" => {
            if output {
                let level = peers.first().and_then(|i| with_state(|s| s.signal_in.get(&(dim, *i)).copied())).unwrap_or(0);
                set_signal(dim, pos, level);
            } else {
                // RedstoneP2PTunnelPart.setNetworkReady: the input's strongest neighbour, outputs not counted.
                let level_in = SIDES.iter().filter_map(|d| {
                    let q = add(pos, *d);
                    if crate::net::block_id(&*host, level, q) == "ae2:redstone_p2p_tunnel" {
                        return None;
                    }
                    level.signal(&*host, q, dir_of(*d))
                }).max().unwrap_or(0);
                with_state(|s| s.signal_in.insert((dim, pos), level_in));
                set_signal(dim, pos, 0);
            }
        }
        "ae2:light_p2p_tunnel" => {
            if net_tick(dim) % 10 != 0 {
                return;
            }
            let lit = if output {
                peers.first().and_then(|i| with_state(|s| s.light_in.get(&(dim, *i)).copied())).unwrap_or(0)
            } else {
                // LightP2PTunnelPart: the light of the block in front of the input.
                let front = add(pos, facing(&*host, level, pos));
                let block_light = level.brightness(&*host, LightLayer::Block, front).unwrap_or(0);
                let sky = level.brightness(&*host, LightLayer::Sky, front).unwrap_or(0);
                with_state(|s| s.light_in.insert((dim, pos), block_light.max(sky)));
                0
            };
            crate::net::with_net(dim, |n| n.prop_sets.push((pos, "light", lit.clamp(0, 15).to_string())));
        }
        _ => {}
    }
}

/// The tunnel kind an item attunes a tunnel to (AE2's `ae2:p2p_attunements/*` tags, and InitP2PAttunements' items
/// with an energy or fluid storage).
fn attunement(host: &dyn Host, stack: &ItemStack) -> Option<&'static str> {
    let item = sid(stack);
    for kind in P2P_KINDS {
        let name = kind.trim_start_matches("ae2:");
        let tags = host.tags();
        if let Ok(id) = Identifier::parse(&format!("ae2:p2p_attunements/{name}"))
            && let Some(tag) = tags.key(&id)
            && tags.is_in_tag(stack.item(), tag) == Some(true)
        {
            return Some(kind);
        }
    }
    match item {
        "ae2:entropy_manipulator" | "ae2:color_applicator" | "ae2:matter_cannon" | "ae2:charged_staff" | "ae2:wireless_terminal" | "ae2:wireless_crafting_terminal"
        | "ae2:energy_cell" | "ae2:dense_energy_cell" | "ae2:creative_energy_cell" => Some("ae2:fe_p2p_tunnel"),
        i if i.starts_with("ae2:portable_item_cell_") || i.starts_with("ae2:portable_fluid_cell_") => Some("ae2:fe_p2p_tunnel"),
        "minecraft:bucket" | "minecraft:water_bucket" | "minecraft:lava_bucket" => Some("ae2:fluid_p2p_tunnel"),
        _ => None,
    }
}

/// A tunnel turned into another kind, its facing kept, its settings at its next tick.
fn convert_tunnel(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, to: &str, freq: i64, output: bool) {
    if is_hosted(pos) {
        let dim = host.dimension();
        with_state(|s| s.p2p_pending.insert((dim, pos), (freq, output)));
        crate::bus::retype(host, level, pos, to);
        crate::net::mark_dirty(dim);
        return;
    }
    let f = level.block_property(&*host, pos, "facing").unwrap_or_else(|| "north".to_owned());
    let Some(state) = host.block_state_id(&format!("{to}[facing={f}]")) else { return };
    let dim = host.dimension();
    with_state(|s| s.p2p_pending.insert((dim, pos), (freq, output)));
    let _ = level.set_block(host, pos, state, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS);
    crate::net::mark_dirty(dim);
}

/// The stack in a player's hand written back (the card's new frequency).
fn replace_held(host: &mut dyn ServerHost, player: u128, old: &ItemStack, new: ItemStack) {
    let Some(slot) = host.player(player).and_then(|p| p.selected_slot(&*host)) else { return };
    let place = ItemPlace::PlayerSlot { player, slot: slot as i32 };
    let _ = host.transaction(|host, tx| {
        if place.extract(host, tx, &ItemResource::of(old), old.count()).unwrap_or(0) != old.count() {
            return Err(());
        }
        if place.insert(host, tx, &ItemResource::of(&new), new.count()).unwrap_or(0) != new.count() {
            return Err(());
        }
        Ok(())
    });
}

/// A use of a tunnel with an item (P2PTunnelPart.onUseItemOn): a memory card (sneaking: this becomes an input and
/// the card takes its frequency; else an output of the card's frequency and kind), or an attunement item. Some when
/// it took the click.
/// The settings a memory card carries between parts (MemoryCardItem / IConfigurableObject, the config inventory and
/// priority): the parts' own keys in their nbt.
const CARD_KEYS: [&str; 12] = ["ghosts", "redstone", "schedule", "craft_only", "craft_redstone", "fuzzy", "access", "no_extract_filter", "report_all", "priority", "level", "below"];

/// MemoryCardItem on a part that isn't a tunnel: sneak-use saves its settings on the card (its kind's name with them);
/// use pastes them onto a part of the same menu kind (an import bus's onto an export bus, as AE2's io buses share).
pub(crate) fn memory_card_use(host: &mut dyn ServerHost, held: &ItemStack, click: &BlockClick<'_>, block: &str, pos: Pos) -> Option<InteractionResult> {
    let stack = held.clone();
    debug(&format!("memory card: on {block} at {pos:?}"));
    if sid(&stack) != "ae2:memory_card" || is_p2p(block) {
        return None;
    }
    let (menu, _) = menu_of(block)?;
    let level = here(&*host)?;
    let player = click.player;
    let e = host.player(player)?.entity();
    let sneaking = e.is_shift_key_down(&*host).unwrap_or(false) || e.is_crouching(&*host).unwrap_or(false);
    let say = |host: &mut dyn ServerHost, t: &str| {
        if let Some(p) = host.player(player) {
            let _ = p.action_bar(host, Text::literal(t));
        }
    };
    let own = crate::bus::nbt_at(&*host, level, pos).unwrap_or_default();
    if sneaking {
        let mut saved = Compound::new();
        for k in CARD_KEYS {
            if let Some(v) = own.get(&java(k)) {
                saved.insert(java(k), v.clone());
            }
        }
        let mut card = stack.clone();
        edit_custom_data(&mut card, |d| {
            d.insert(java("ae2:settings"), Tag::Compound(saved));
            d.insert(java("ae2:settings_from"), Tag::String(java(menu)));
            d.remove(&java("ae2:p2p_freq"));
        });
        replace_held(host, player, &stack, card);
        say(host, "Settings saved to memory card.");
        debug(&format!("memory card: saved {block} at {pos:?}"));
        return Some(InteractionResult::SuccessServer);
    }
    let data = custom_data(&stack)?;
    let (Some(Tag::Compound(saved)), Some(Tag::String(from))) = (data.get(&java("ae2:settings")).cloned(), data.get(&java("ae2:settings_from")).cloned()) else {
        return None;
    };
    if from.to_string_lossy() != menu {
        say(host, "Invalid memory card settings for this machine.");
        return Some(InteractionResult::SuccessServer);
    }
    let mut out = own;
    for k in CARD_KEYS {
        match saved.get(&java(k)) {
            Some(v) => {
                out.insert(java(k), v.clone());
            }
            None => {
                out.remove(&java(k));
            }
        }
    }
    crate::bus::set_nbt_at(host, level, pos, out);
    say(host, "Settings loaded from memory card.");
    debug(&format!("memory card: loaded onto {block} at {pos:?}"));
    Some(InteractionResult::SuccessServer)
}

pub(crate) fn p2p_use(host: &mut dyn ServerHost, click: &BlockClick<'_>, block: &str, pos: Pos) -> Option<InteractionResult> {
    let stack = click.stack?.clone();
    let level = here(&*host)?;
    let dim = host.dimension();
    let player = click.player;
    let (freq, output) = crate::net::p2p_info(dim, pos).map_or((0, false), |i| (i.freq, i.output));
    let say = |host: &mut dyn ServerHost, t: String| {
        if let Some(p) = host.player(player) {
            let _ = p.action_bar(host, Text::literal(&t));
        }
    };
    if sid(&stack) == "ae2:memory_card" {
        let e = host.player(player)?.entity();
        let sneaking = e.is_shift_key_down(&*host).unwrap_or(false) || e.is_crouching(&*host).unwrap_or(false);
        if sneaking {
            let freq = if freq != 0 && !output {
                freq
            } else {
                (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as i64) & 0x7FFF_FFFF_FFFF) | 1
            };
            with_state(|s| s.p2p_pending.insert((dim, pos), (freq, false)));
            let mut card = stack.clone();
            edit_custom_data(&mut card, |d| {
                d.insert(java("ae2:p2p_freq"), Tag::Long(freq));
                d.insert(java("ae2:p2p_kind"), Tag::String(java(block)));
            });
            replace_held(host, player, &stack, card);
            say(host, format!("P2P input, frequency {:04X}", freq & 0xFFFF));
            return Some(InteractionResult::SuccessServer);
        }
        let data = custom_data(&stack)?;
        let Some(Tag::Long(f)) = data.get(&java("ae2:p2p_freq")).cloned() else { return None };
        let kind = match data.get(&java("ae2:p2p_kind")) {
            Some(Tag::String(j)) => j.to_string_lossy(),
            _ => block.to_owned(),
        };
        if kind != block && is_p2p(&kind) {
            convert_tunnel(host, level, pos, &kind, f, true);
        } else {
            with_state(|s| s.p2p_pending.insert((dim, pos), (f, true)));
        }
        say(host, format!("P2P output, frequency {:04X}", f & 0xFFFF));
        return Some(InteractionResult::SuccessServer);
    }
    match attunement(&*host, &stack) {
        Some(kind) if kind != block => {
            convert_tunnel(host, level, pos, kind, freq, output);
            say(host, format!("Attuned: {}", kind.trim_start_matches("ae2:").replace('_', " ")));
            Some(InteractionResult::SuccessServer)
        }
        _ => None,
    }
}

/// Re-entry guard: a tunnel whose output feeds its own input stops there.
static ROUTING: Mutex<u32> = Mutex::new(0);

fn routing<R: Default>(f: impl FnOnce() -> R) -> R {
    {
        let mut depth = ROUTING.lock().unwrap_or_else(|e| e.into_inner());
        if *depth > 0 {
            return R::default();
        }
        *depth += 1;
    }
    let out = f();
    *ROUTING.lock().unwrap_or_else(|e| e.into_inner()) = 0;
    out
}

/// What is in front of the tunnel at `pos`, and the face it is reached through.
pub(crate) fn front_of(host: &dyn Host, level: LevelRef, pos: Pos) -> (Pos, Direction) {
    let f = facing(host, level, pos);
    (add(pos, f), dir_of(neg(f)))
}

/// Whether `at` asks through the tunnel's front (only that face of a tunnel reaches anything).
fn through_front(host: &dyn Host, at: &BlockSide) -> bool {
    let f = facing(host, at.level, at.pos);
    at.side == Some(dir_of(f))
}

/// A tunnel's end of a resource capability (CapabilityP2PTunnelPart): the input hands what comes in to its outputs'
/// fronts, an even share each first (P2PTunnelPart's InputStorage), the rest to whoever takes it; an output reaches
/// what is in front of its input.
pub(crate) struct TunnelEnd<R: 'static> {
    level: LevelRef,
    dim: &'static str,
    pos: Pos,
    output: bool,
    key: &'static CapabilityKey<dyn ResourceHandler<R>>,
}

impl<R: 'static> TunnelEnd<R> {
    /// The handlers in front of the other ends.
    fn ends(&self, host: &mut dyn ServerHost) -> Vec<Capability<dyn ResourceHandler<R>>> {
        crate::net::p2p_peers(self.dim, self.pos)
            .into_iter()
            .filter_map(|p| {
                let (target, face) = front_of(&*host, self.level, p);
                self.level.capability(host, self.key, target, Some(face))
            })
            .collect()
    }
    /// An output's: the input's front.
    fn source(&self, host: &mut dyn ServerHost) -> Option<Capability<dyn ResourceHandler<R>>> {
        if !self.output {
            return None;
        }
        self.ends(host).into_iter().next()
    }
}

impl<R: 'static> ResourceHandler<R> for TunnelEnd<R> {
    fn size(&self, host: &mut dyn ServerHost) -> usize {
        if !self.output {
            return 1;
        }
        routing(|| self.source(host).map_or(0, |s| s.size(host)))
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<R> {
        routing(|| self.source(host).and_then(|s| s.resource(host, index)))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        routing(|| self.source(host).map_or(0, |s| s.amount(host, index)))
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, resource: Option<&R>) -> u32 {
        if !self.output {
            return u32::MAX;
        }
        routing(|| self.source(host).map_or(0, |s| s.capacity(host, index, resource)))
    }
    fn is_valid(&self, _host: &mut dyn ServerHost, _index: usize, _resource: &R) -> bool {
        true
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &R, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        if self.output {
            return Ok(routing(|| self.source(host).map_or(0, |s| s.insert_at(host, tx, index, resource, amount).unwrap_or(0).min(amount))));
        }
        Ok(routing(|| {
            let ends = self.ends(host);
            if ends.is_empty() {
                return 0;
            }
            let share = amount / ends.len() as u32;
            let mut went = 0u32;
            for round in [share, amount] {
                for e in &ends {
                    if went >= amount || round == 0 {
                        break;
                    }
                    let give = round.min(amount - went);
                    went += e.insert(host, tx, resource, give).unwrap_or(0).min(give);
                }
            }
            went
        }))
    }
    fn insert(&self, host: &mut dyn ServerHost, tx: &TransactionContext, resource: &R, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        self.insert_at(host, tx, 0, resource, amount)
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &R, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        Ok(routing(|| self.source(host).map_or(0, |s| s.extract_at(host, tx, index, resource, amount).unwrap_or(0).min(amount))))
    }
}

fn tunnel_end<R: 'static>(host: &mut dyn ServerHost, at: BlockSide, kind: &str, key: &'static CapabilityKey<dyn ResourceHandler<R>>) -> Option<TunnelEnd<R>> {
    if !through_front(&*host, &at) {
        return None;
    }
    let dim = host.dimension();
    let info = crate::net::p2p_info(dim, at.pos)?;
    (info.kind == kind).then_some(TunnelEnd { level: at.level, dim, pos: at.pos, output: info.output, key })
}

/// Whether a tunnel block's providers answer: only through its front (`through_front`, read from the state). Which
/// kind it is now (`net::p2p_info`) is the server's and is asked as the handler is built: another kind's answers empty.
pub(crate) fn p2p_present(level: &dyn LevelHost, at: &BlockSide, state: BlockState, _: BlockEntityRef) -> bool {
    let f = facing_dir(crate::capdefer::state_facing(level, state).as_deref());
    at.side == Some(dir_of(f))
}

pub(crate) fn p2p_items(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    Some(Box::new(tunnel_end(host, at, "ae2:item_p2p_tunnel", &ITEMS)?))
}

pub(crate) fn p2p_fluids(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<FluidResource>>> {
    Some(Box::new(tunnel_end(host, at, "ae2:fluid_p2p_tunnel", &FLUIDS)?))
}

/// FEP2PTunnelPart: the input takes FE for its outputs' fronts (an even share each, then whoever takes the rest); an
/// output gives what its input's front gives.
pub(crate) struct TunnelEnergy {
    level: LevelRef,
    dim: &'static str,
    pos: Pos,
    output: bool,
}

impl TunnelEnergy {
    fn ends(&self, host: &mut dyn ServerHost) -> Vec<Capability<dyn EnergyHandler>> {
        crate::net::p2p_peers(self.dim, self.pos)
            .into_iter()
            .filter_map(|p| {
                let (target, face) = front_of(&*host, self.level, p);
                self.level.capability(host, &ENERGY, target, Some(face))
            })
            .collect()
    }
}

impl EnergyHandler for TunnelEnergy {
    fn amount(&self, host: &mut dyn ServerHost) -> u32 {
        if !self.output {
            return 0;
        }
        routing(|| self.ends(host).first().map_or(0, |e| e.amount(host)))
    }
    fn capacity(&self, host: &mut dyn ServerHost) -> u32 {
        routing(|| self.ends(host).iter().map(|e| e.capacity(host)).fold(0u32, u32::saturating_add))
    }
    fn insert(&self, host: &mut dyn ServerHost, tx: &TransactionContext, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        if self.output {
            return Ok(0);
        }
        Ok(routing(|| {
            let ends = self.ends(host);
            if ends.is_empty() {
                return 0;
            }
            let share = amount / ends.len() as u32;
            let mut went = 0u32;
            for round in [share, amount] {
                for e in &ends {
                    if went >= amount || round == 0 {
                        break;
                    }
                    let give = round.min(amount - went);
                    went += e.insert(host, tx, give).unwrap_or(0).min(give);
                }
            }
            went
        }))
    }
    fn extract(&self, host: &mut dyn ServerHost, tx: &TransactionContext, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        if !self.output {
            return Ok(0);
        }
        Ok(routing(|| self.ends(host).first().map_or(0, |e| e.extract(host, tx, amount).unwrap_or(0).min(amount))))
    }
}

pub(crate) fn p2p_energy(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn EnergyHandler>> {
    if !through_front(&*host, &at) {
        return None;
    }
    let dim = host.dimension();
    let info = crate::net::p2p_info(dim, at.pos)?;
    (info.kind == "ae2:fe_p2p_tunnel").then(|| Box::new(TunnelEnergy { level: at.level, dim, pos: at.pos, output: info.output }) as Box<dyn EnergyHandler>)
}

// ---- The cell workbench (a cell's partition and cards)

const WB_UPGRADES: usize = 4;

fn is_cell_card(id: &str) -> bool {
    matches!(id, "ae2:energy_card" | "ae2:speed_card" | "ae2:fuzzy_card" | "ae2:inverter_card" | "ae2:equal_distribution_card" | "ae2:void_card")
}

/// [tools] The cards the workbench's item takes, each no more than `Upgrades.add` allows it (`tool_cells::cards_for`).
fn wb_cards_for(cell: &ItemStack, fields: &Fields, room: usize) -> Vec<String> {
    crate::tool_cells::cards_for(sid(cell), &wb_cards(fields), room)
}

/// The upgrade cards a cell takes (BasicStorageCell.getUpgrades: four on an item cell, three on a fluid cell; two
/// on a view cell).
fn cell_upgrade_slots(stack: &ItemStack) -> usize {
    let id = sid(stack);
    if id == "ae2:view_cell" {
        return 2;
    }
    // [tools] Portable cells, the matter cannon, the colour applicator.
    if let Some(n) = crate::tool_cells::upgrade_slots(id) {
        return n;
    }
    match cell_type(id) {
        Some(t) if t.key_type == KeyType::Fluids => 3,
        Some(_) => 4,
        None => 0,
    }
}

fn workbench_item(stack: &ItemStack) -> bool {
    is_cell_item(sid(stack)) || sid(stack) == "ae2:view_cell" || crate::tool_cells::workbench_takes(sid(stack))
}

/// The cell's partition, 63 cells ("" empty).
fn cell_partition(stack: &ItemStack) -> Vec<String> {
    let mut cells = vec![String::new(); 63];
    if let Some(Tag::List(keys)) = custom_data(stack).and_then(|d| d.get(&java("ae2:config"))) {
        for (i, k) in keys.iter().enumerate().take(63) {
            if let Tag::String(s) = k {
                cells[i] = s.to_string_lossy();
            }
        }
    }
    cells
}

/// Writes the partition onto the cell; its id goes, so a drive mounts it again with the new partition.
fn set_cell_partition(stack: &mut ItemStack, cells: &[String]) {
    let mut last = cells.len();
    while last > 0 && cells[last - 1].is_empty() {
        last -= 1;
    }
    edit_custom_data(stack, |d| {
        if last == 0 {
            d.remove(&java("ae2:config"));
        } else {
            d.insert(java("ae2:config"), Tag::List(cells[..last].iter().map(|s| Tag::String(java(s))).collect()));
        }
        d.remove(&java("ae2:cell_id"));
        d.remove(&java("ae2:cell_ver"));
    });
}

fn set_cell_cards(stack: &mut ItemStack, cards: &[String]) {
    edit_custom_data(stack, |d| {
        if cards.is_empty() {
            d.remove(&java("ae2:upgrades"));
        } else {
            d.insert(java("ae2:upgrades"), Tag::List(cards.iter().map(|c| Tag::String(java(c))).collect()));
        }
        d.remove(&java("ae2:cell_id"));
        d.remove(&java("ae2:cell_ver"));
    });
    // [tools] An Energy Card taken off a powered item takes its charge above the new capacity with it.
    if let Some(most) = crate::powered::capacity(stack)
        && crate::world::item_power(stack) > most
    {
        crate::world::set_item_power(stack, most);
    }
}

/// The workbench's own config pane (CellWorkbenchBlockEntity.config).
fn wb_config(nbt: &Compound) -> Vec<String> {
    let mut cells = vec![String::new(); 63];
    if let Some(Tag::List(keys)) = nbt.get(&java("config")) {
        for (i, k) in keys.iter().enumerate().take(63) {
            if let Tag::String(s) = k {
                cells[i] = s.to_string_lossy();
            }
        }
    }
    cells
}

fn set_wb_config(nbt: &mut Compound, cells: &[String]) {
    nbt.insert(java("config"), Tag::List(cells.iter().map(|s| Tag::String(java(s))).collect()));
}

fn wb_cards(fields: &Fields) -> Vec<String> {
    (1..=WB_UPGRADES).map(|i| fields.items.get(i).cloned().flatten().map_or(String::new(), |s| sid(&s).to_owned())).collect()
}

fn wb_on(fields: &Fields, room: usize) -> Vec<String> {
    wb_cards(fields).into_iter().filter(|c| !c.is_empty()).take(room).filter(|c| is_cell_card(c)).collect()
}

fn wb_clear_on(fields: &mut Fields) {
    let mut left = nbt_i64(&fields.nbt, "room");
    for i in 1..=WB_UPGRADES {
        if left <= 0 {
            break;
        }
        if fields.items[i].as_ref().is_some_and(|s| is_cell_card(sid(s))) {
            fields.items[i] = None;
            left -= 1;
        }
    }
}

/// CellWorkbenchBlockEntity.onChangeInventory and its menu's upgrade slots: a cell put in brings its partition (or
/// takes the pane's, when it has none) and its cards; one taken out takes its cards and, in "clear on remove" copy
/// mode, clears the pane; cards put in or taken out while it's there go onto the cell.
fn workbench_tick(host: &dyn Host, fields: &mut Fields) {
    if fields.items.len() < 1 + WB_UPGRADES {
        fields.items.resize(1 + WB_UPGRADES, None);
    }
    let had = nbt_i64(&fields.nbt, "had") != 0;
    let saved = match fields.nbt.get(&java("cards")) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let cell = fields.items[0].clone().filter(workbench_item);
    let now = wb_cards(fields).join(",");
    match (had, cell) {
        (false, Some(mut cell)) => {
            let own = cell_partition(&cell);
            if own.iter().any(|k| !k.is_empty()) {
                set_wb_config(&mut fields.nbt, &own);
            } else {
                let pane = wb_config(&fields.nbt);
                if pane.iter().any(|k| !k.is_empty()) {
                    set_cell_partition(&mut cell, &pane);
                }
            }
            let mut cards = cell_cards(&cell).into_iter();
            for i in 1..=WB_UPGRADES {
                if fields.items[i].is_none()
                    && let Some(c) = cards.next()
                {
                    fields.items[i] = new_stack(host, &c, 1);
                }
            }
            let room = cell_upgrade_slots(&cell);
            set_nbt_i64(&mut fields.nbt, "room", room as i64);
            let on = wb_cards_for(&cell, fields, room);
            if on != cell_cards(&cell) {
                set_cell_cards(&mut cell, &on);
            }
            fields.items[0] = Some(cell);
            set_nbt_i64(&mut fields.nbt, "had", 1);
            let cards = wb_cards(fields).join(",");
            fields.nbt.insert(java("cards"), Tag::String(java(&cards)));
        }
        (true, None) => {
            if now == saved {
                wb_clear_on(fields);
            }
            if nbt_i64(&fields.nbt, "copy") == 0 {
                set_wb_config(&mut fields.nbt, &vec![String::new(); 63]);
            }
            set_nbt_i64(&mut fields.nbt, "had", 0);
            fields.nbt.insert(java("cards"), Tag::String(java("")));
        }
        (true, Some(mut cell)) if now != saved => {
            let room = cell_upgrade_slots(&cell);
            set_nbt_i64(&mut fields.nbt, "room", room as i64);
            let on = wb_cards_for(&cell, fields, room);
            set_cell_cards(&mut cell, &on);
            fields.items[0] = Some(cell);
            fields.nbt.insert(java("cards"), Tag::String(java(&now)));
        }
        _ => {}
    }
    let cell = fields.items[0].clone();
    let data = [
        nbt_i64(&fields.nbt, "copy") as i32,
        match &cell {
            Some(c) if cell_upgrades(c).fuzzy => FuzzyMode::VALUES.iter().position(|m| *m == cell_fuzzy(c)).unwrap_or(0) as i32,
            _ => -1,
        },
        cell.as_ref().map_or(0, cell_upgrade_slots) as i32,
    ];
    if fields.data.len() != 3 || fields.data[..] != data[..] {
        fields.data = data.to_vec();
    }
}

fn workbench_button(host: &dyn Host, fields: &mut Fields, id: i32, carried: &Option<ItemStack>) {
    if id & !BACK == BTN_WB_COPY {
        let m = nbt_i64(&fields.nbt, "copy");
        set_nbt_i64(&mut fields.nbt, "copy", 1 - m);
        return;
    }
    let cell = fields.items.first().cloned().flatten().filter(workbench_item);
    if id & !BACK == BTN_FUZZY {
        if let Some(mut c) = cell {
            let at = FuzzyMode::VALUES.iter().position(|m| *m == cell_fuzzy(&c)).unwrap_or(0);
            let next = FuzzyMode::VALUES[if id & BACK != 0 { (at + 4) % 5 } else { (at + 1) % 5 }];
            edit_custom_data(&mut c, |d| {
                d.insert(java("ae2:fuzzy"), Tag::String(java(next.java_name())));
                d.remove(&java("ae2:cell_id"));
                d.remove(&java("ae2:cell_ver"));
            });
            fields.items[0] = Some(c);
        }
        return;
    }
    // The pane works without a cell only in "keep on remove" mode (CellWorkbenchMenu.isPartitionSlotEnabled).
    if cell.is_none() && nbt_i64(&fields.nbt, "copy") == 0 {
        return;
    }
    let mut cells = wb_config(&fields.nbt);
    if id >= GHOST {
        let i = ((id - GHOST) >> 3) as usize;
        if i >= 63 {
            return;
        }
        cells[i] = carried.as_ref().map_or(String::new(), |st| key_to_string(&ghost_key(host, st)));
    } else if id & !BACK == BTN_PARTITION {
        // "Partition storage": the config becomes what the cell holds.
        let Some(c) = cell.as_ref() else { return };
        cells = read_cell(c).keys.iter().map(key_to_string).take(63).collect();
        cells.resize(63, String::new());
    } else if id & !BACK == BTN_CLEAR {
        cells = vec![String::new(); 63];
    } else {
        return;
    }
    set_wb_config(&mut fields.nbt, &cells);
    if let Some(mut c) = cell {
        set_cell_partition(&mut c, &cells);
        fields.items[0] = Some(c);
    }
}

/// A ghost cell's key from the cursor's stack: a filled bucket is its fluid (AE2's fake slots read the held
/// fluid container), anything else one of the item.
pub(crate) fn ghost_key(host: &dyn Host, stack: &ItemStack) -> AeKey {
    match bucket_fluid(sid(stack)) {
        Some(fluid) => AeKey::fluid(&fluid, ""),
        None => key_of(host, stack),
    }
}

// ---- View cells (the terminals' filters, set in the workbench)

/// ViewCellItem.createItemFilter: what the view cells with a partition let through (all of it when none has one).
pub(crate) fn view_filter(cells: &[ItemStack], items: Vec<(AeKey, i64)>) -> Vec<(AeKey, i64)> {
    let filters: Vec<BusFilter> = cells
        .iter()
        .filter(|c| sid(c) == "ae2:view_cell")
        .map(|c| {
            let up = cell_upgrades(c);
            BusFilter { keys: cell_config(c), fuzzy: up.fuzzy, inverted: up.inverter, mode: cell_fuzzy(c), ..BusFilter::default() }
        })
        .filter(|f| !f.keys.is_empty())
        .collect();
    if filters.is_empty() {
        return items;
    }
    items.into_iter().filter(|(k, _)| filters.iter().any(|f| f.allows(k))).collect()
}

// ---- Menu buttons

/// A part screen's button (`menu_button`): its toolbar settings and ghost cells.
pub(crate) fn menu_button(host: &mut dyn ServerHost, player: u128, pos: Pos, block: &str, id: i32, fields: &mut Fields, carried: &Option<ItemStack>) {
    if id < 0 || !storage::rate_ok(player) {
        return;
    }
    let Some((at, viewed, _)) = storage::viewer_for_button(&*host, player) else { return };
    if at != pos || viewed != block {
        return;
    }
    let Some(k) = kind(block) else { return };
    if k == Kind::Workbench {
        workbench_button(&*host, fields, id, carried);
        workbench_tick(&*host, fields);
        return;
    }
    if id >= GHOST {
        let cell = ((id - GHOST) >> 3) as usize;
        let action = (id - GHOST) & 7;
        let (n, open) = config_cells(block, fields);
        if cell >= open.min(n) {
            return;
        }
        let mut cells = ghosts(&fields.nbt, n);
        let interface = k == Kind::Interface;
        match (action, carried) {
            // Wheel over an interface's config: one more or less.
            (2 | 3, _) if interface => {
                if let Some((_, a)) = cells[cell].as_mut() {
                    *a = if action == 2 { *a + 1 } else { (*a - 1).max(1) };
                }
            }
            (2 | 3, _) => return,
            (0 | 1, Some(st)) => {
                let key = ghost_key(&*host, st);
                let amount = if !interface {
                    1
                } else if key.key_type() == KeyType::Fluids {
                    1000
                } else if action == 1 {
                    // A right-click adds one of it (or starts at one).
                    match &cells[cell] {
                        Some((k, a)) if *k == key => a + 1,
                        _ => 1,
                    }
                } else {
                    i64::from(st.count())
                };
                cells[cell] = Some((key, amount));
            }
            (1, None) if interface => {
                // A right-click with nothing: one less, cleared at none.
                if let Some((_, a)) = cells[cell].as_mut() {
                    *a -= 1;
                }
                if cells[cell].as_ref().is_some_and(|(_, a)| *a <= 0) {
                    cells[cell] = None;
                }
            }
            _ => cells[cell] = None,
        }
        set_ghosts(&mut fields.nbt, &cells);
        return;
    }
    let back = id & BACK != 0;
    let cycle = |fields: &mut Fields, key: &str, n: i64| {
        let v = nbt_i64(&fields.nbt, key);
        set_nbt_i64(&mut fields.nbt, key, if back { (v + n - 1) % n } else { (v + 1) % n });
    };
    match id & !BACK {
        BTN_REDSTONE => cycle(fields, "redstone", 3),
        BTN_SCHEDULE => cycle(fields, "schedule", 3),
        BTN_CRAFT_ONLY => cycle(fields, "craft_only", 2),
        BTN_FUZZY => cycle(fields, "fuzzy", 5),
        BTN_REPORT => cycle(fields, "report_all", 2),
        BTN_EXTRACT_FILTER => cycle(fields, "no_extract_filter", 2),
        BTN_ACCESS => cycle(fields, "access", 3),
        BTN_PLACE => cycle(fields, "drop_items", 2),
        BTN_EMIT_MODE => cycle(fields, "below", 2),
        BTN_CRAFT_RS if k == Kind::LevelEmitter && cards(block, fields, "ae2:crafting_card") > 0 => cycle(fields, "craft_redstone", 2),
        BTN_CLEAR => {
            // StorageBusMenu.clear: the config and the settings.
            let (n, _) = config_cells(block, fields);
            set_ghosts(&mut fields.nbt, &vec![None; n]);
            for k in ["access", "no_extract_filter", "report_all", "fuzzy"] {
                fields.nbt.remove(&java(k));
            }
        }
        BTN_PARTITION if k == Kind::StorageBus => {
            // StorageBusMenu.partition: the config becomes what the container holds now.
            let Some(level) = here(&*host) else { return };
            let f = facing(&*host, level, pos);
            let (target, face) = (add(pos, f), dir_of(neg(f)));
            let mut keys: Vec<AeKey> = Vec::new();
            if let Some(h) = level.capability(host, &ITEMS, target, Some(face)) {
                for i in 0..h.size(host) {
                    if let Some(r) = h.resource(host, i).filter(|_| h.amount(host, i) > 0) {
                        let key = key_of(&*host, r.as_stack());
                        if !keys.contains(&key) {
                            keys.push(key);
                        }
                    }
                }
            }
            if let Some(h) = level.capability(host, &FLUIDS, target, Some(face)) {
                for i in 0..h.size(host) {
                    if let Some(id) = h.resource(host, i).filter(|_| h.amount(host, i) > 0).and_then(|r| fluid_id(&*host, &r)) {
                        let key = AeKey::fluid(&id, "");
                        if !keys.contains(&key) {
                            keys.push(key);
                        }
                    }
                }
            }
            let (n, open) = config_cells(block, fields);
            let mut cells: Vec<Option<(AeKey, i64)>> = keys.into_iter().take(open).map(|k| Some((k, 1))).collect();
            cells.resize(n, None);
            set_ghosts(&mut fields.nbt, &cells);
        }
        _ => {}
    }
}

/// A message from a client on `ae2:part`: the level typed into an emitter's screen (`level <n>`), for the part the
/// player has open, in reach.
pub(crate) fn from_client(host: &mut dyn ServerHost, player: u128, data: &[u8]) {
    if data.len() > 64 || !storage::rate_ok(player) {
        return;
    }
    let text = String::from_utf8_lossy(data);
    let Some(n) = text.strip_prefix("level ").and_then(|n| n.trim().parse::<i64>().ok()) else { return };
    let Some((pos, block, _)) = storage::viewer_for_button(&*host, player) else { return };
    if !matches!(kind(&block), Some(Kind::LevelEmitter | Kind::EnergyEmitter)) {
        return;
    }
    let dim = host.dimension();
    with_state(|s| s.levels.insert((dim, pos), n.clamp(0, i64::MAX / 2)));
}

// ---- The level's tick end

pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    // What storage buses' containers wouldn't take after all, back into the network.
    let back: Vec<(&'static str, Pos, AeKey, i64)> = with_state(|s| {
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut s.leftovers).into_iter().partition(|l| l.0 == dim);
        s.leftovers = rest;
        here
    });
    for (_, bus, key, n) in back {
        let stored = storage::insert(host, Scope::Grid(bus), None, &key, n);
        if stored < n {
            // Nowhere for it: at the bus.
            if let Some(s) = stack_of(&*host, &key, (n - stored).min(i64::from(u32::MAX)) as u32) {
                let bus = real(bus);
                let _ = level.spawn_item(host, f64::from(bus[0]) + 0.5, f64::from(bus[1]) + 0.5, f64::from(bus[2]) + 0.5, s, 10);
            }
        }
    }
    // Redstone changed: the neighbours read it again.
    let updates: Vec<Pos> = with_state(|s| {
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut s.updates).into_iter().partition(|u| u.0 == dim);
        s.updates = rest;
        here.into_iter().map(|u| u.1).collect()
    });
    for p in updates {
        let p = real(p);
        if let Some(state) = level.block_state(&*host, p) {
            let _ = level.update_neighbors(host, p, state, NeighbourUpdate::Neighbours);
        }
    }
    stream(host, level);
}

/// Each part screen's viewer gets its config when it changed (`ae2:part`): `#part`, then `g <cell> <amount> <key>`
/// lines, `level <n>`, `target <block>` (what a storage bus is on).
fn stream(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    // Sent again every second: a config that reached the client before its screen did (or as the menu's screen
    // reopened, which clears the client's copy) would otherwise never show until it changed.
    if net_tick(dim) % 20 == 0 {
        with_state(|s| s.sent.clear());
    }
    let viewers: Vec<(u128, Pos, String)> =
        storage::with_store(|s| s.viewers.iter().filter(|(_, v)| v.dim == dim && has_screen(&v.block)).map(|(u, v)| (*u, v.pos, v.block.clone())).collect());
    for (player, pos, block) in viewers {
        let Some(nbt) = crate::bus::nbt_at(&*host, level, pos) else {
            debug(&format!("part stream: no nbt for {block} at {pos:?}"));
            continue;
        };
        let mut text = format!("#part {block}\n");
        if block == "ae2:cell_workbench" {
            for (i, k) in wb_config(&nbt).iter().enumerate() {
                if !k.is_empty() {
                    text.push_str(&format!("g {i} 1 {k}\n"));
                }
            }
        } else {
            for (i, c) in ghosts(&nbt, 63).iter().enumerate() {
                if let Some((k, n)) = c {
                    text.push_str(&format!("g {i} {n} {}\n", key_to_string(k)));
                }
            }
        }
        text.push_str(&format!("level {}\n", nbt_i64(&nbt, "level")));
        if block == "ae2:storage_bus" {
            let target = add(pos, facing(&*host, level, pos));
            text.push_str(&format!("target {}\n", crate::net::block_id(&*host, level, target)));
        }
        if kind(&block) == Some(Kind::Interface) {
            text.push_str(&crate::iface_fluids::stream_lines(&*host, level, pos));
        }
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut h);
            h.finish()
        };
        if with_state(|s| s.sent.get(&player) == Some(&hash)) {
            continue;
        }
        let sent = crate::util::send_to_player(host, player, NET_PART, text.as_bytes());
        if sent.is_err() {
            debug(&format!("part stream: send to {player:x} failed: {sent:?}"));
        }
        if sent.is_ok() {
            debug(&format!("part stream: {block} at {pos:?}, {} ghost lines", text.lines().filter(|l| l.starts_with("g ")).count()));
            with_state(|s| s.sent.insert(player, hash));
        }
    }
}

/// A part screen opened: its config goes again.
pub(crate) fn opened(player: u128) {
    with_state(|s| {
        s.sent.remove(&player);
    });
}
