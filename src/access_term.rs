//! The pattern access terminal (AE2's PatternAccessTermMenu and PatternAccessTermScreen), server and client.
//!
//! - **Server** (`view`, sent on `ae2:pat` when it changes): the grid's pattern providers the terminal's display
//!   mode shows (ShowPatternProviders: the visible ones, the visible ones with an empty slot, all of them; a provider
//!   is visible unless its own "show in pattern access terminal" is off), each with its group (PatternContainerGroup:
//!   the machine it faces, by its item; facing several, the provider itself and the machines in its tooltip; facing
//!   none, the provider), its sort value and its nine slots. In the "not full" mode a provider shown once stays
//!   while the terminal is open (pinnedHosts), so it doesn't vanish as its last slot fills.
//! - **Clicks** (PatternAccessTermMenu.doAction), all checked on the server: the provider is one the player was
//!   sent, still a provider on the terminal's online grid; only an encoded pattern goes in, one to a slot; Shift
//!   moves a slot's pattern into the inventory (in the player's tick, only when it fits); the creative duplicate
//!   needs creative mode and an empty cursor.
//! - **Client**: the groups sorted by name, a header row each (icon, name, the count) and a row of slots per
//!   provider, the terminal style's rows, the scroll bar, the search box (a pattern's outputs' names or the group's
//!   name), the toolbar's terminal style and display mode buttons, tooltips (the group's machines, the patterns).

use crate::autocraft::{ENC, NET_PAT, PAT_SLOT, is_pattern, is_provider};
use crate::cells::*;
use crate::core::AeKey;
use crate::util::*;
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// The display mode button (Settings.TERMINAL_SHOW_PATTERN_PROVIDERS), on and back.
pub(crate) const PAT_SHOW: i32 = ENC | 35;
pub(crate) const PAT_SHOW_BACK: i32 = ENC | 36;
/// `PAT_SLOT | provider << 6 | slot << 2 | action`.
const ACT_PICKUP: i32 = 0;
const ACT_SPLIT: i32 = 1;
const ACT_SHIFT: i32 = 2;
const ACT_DUPLICATE: i32 = 3;
/// The most providers sent (and clicked) at once.
const MAX_PROVIDERS: usize = 1023;
const SLOTS: usize = 9;

// ---- Pure rules

/// ShowPatternProviders: whether a provider is listed. `mode` 0 visible, 1 visible with an empty slot (or pinned),
/// 2 all.
pub(crate) fn shown(mode: i64, hidden: bool, full: bool, pinned: bool) -> bool {
    match mode {
        0 => !hidden,
        1 => !hidden && (pinned || !full),
        _ => true,
    }
}

/// PatternProviderLogic.getSortValue.
pub(crate) fn sort_value(p: Pos) -> i64 {
    (i64::from(p[2]) << 24) ^ (i64::from(p[0]) << 8) ^ i64::from(p[1])
}

/// PatternContainerGroup of a provider: the one machine it faces, or (several) the provider with the machines for its
/// tooltip, or (none) the provider. As one word: `icon` or `icon|tip,tip`.
pub(crate) fn group_word(own: &str, machines: &[String]) -> String {
    let mut distinct: Vec<&String> = Vec::new();
    for m in machines {
        if !distinct.contains(&m) {
            distinct.push(m);
        }
    }
    match distinct.len() {
        0 => own.to_owned(),
        1 => distinct[0].clone(),
        _ => format!("{own}|{}", distinct.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")),
    }
}

/// What a slot click does (doAction): the slot's new content and the cursor's, or None for no change.
/// `put_one`: the cursor's stack with one taken off (None when it was one).
pub(crate) fn slot_action<T: Clone>(
    action: i32,
    slot: Option<T>,
    carried: Option<(T, u32)>,
    carried_is_pattern: bool,
    creative: bool,
) -> Option<(Option<T>, Option<(T, u32)>)> {
    match (action, slot, carried) {
        (ACT_PICKUP, None, Some((c, n))) if carried_is_pattern => Some((Some(c.clone()), (n > 1).then(|| (c, n - 1)))),
        (ACT_PICKUP, Some(s), Some((c, 1))) if carried_is_pattern => Some((Some(c), Some((s, 1)))),
        (ACT_PICKUP | ACT_SPLIT, Some(s), None) => Some((None, Some((s, 1)))),
        (ACT_SPLIT, None, Some((c, n))) if carried_is_pattern => Some((Some(c.clone()), (n > 1).then(|| (c, n - 1)))),
        (ACT_DUPLICATE, Some(s), None) if creative => Some((Some(s.clone()), Some((s, 1)))),
        _ => None,
    }
}

// ---- Server

#[derive(Default)]
struct Server {
    /// The providers each viewer was last sent, in order.
    sent: HashMap<u128, Vec<Pos>>,
    /// ShowPatternProviders.NOT_FULL's pinned providers, by viewer.
    pinned: HashMap<u128, HashSet<Pos>>,
    /// Shift-clicks waiting for the player's tick: (player, provider, slot).
    shifts: Vec<(u128, Pos, usize)>,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);

fn with_server<R>(f: impl FnOnce(&mut Server) -> R) -> R {
    let mut g = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Server::default))
}

pub(crate) fn forget_all() {
    *SERVER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn online_grid(dim: &'static str, pos: Pos) -> Option<usize> {
    let (grid, online, _) = crate::net::device_state(dim, pos);
    grid.filter(|_| online)
}

/// The machines a provider faces, by the item that names them (PatternContainerGroup.fromMachine: a molecular
/// assembler, anything that takes items, a part on a cable bus facing back).
fn machines(host: &mut dyn ServerHost, level: LevelRef, at: Pos, block: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (q, f) in crate::autocraft::targets(&*host, level, at, block) {
        let raw = level.block(&*host, q).and_then(|b| b.id(&*host).map(str::to_owned)).unwrap_or_default();
        if crate::bus::is_cell_block(&raw) {
            // A cable bus: the part on the side facing the provider.
            let back = [real(at)[0] - q[0], real(at)[1] - q[1], real(at)[2] - q[2]];
            let Some(side) = SIDES.iter().position(|d| *d == back) else { continue };
            if let Some(part) = crate::bus::cell(&*host, level, q).and_then(|c| c.parts[side].clone())
                && !part.is_empty()
                && level.capability(host, &ITEMS, q, Some(f)).is_some()
            {
                out.push(part);
            }
            continue;
        }
        if raw == "ae2:molecular_assembler" || (!raw.is_empty() && level.capability(host, &ITEMS, q, Some(f)).is_some()) {
            out.push(raw);
        }
    }
    out
}

/// PatternAccessTermMenu's update for a viewer: `#s <display mode>`, then for each provider shown `#p <group> <sort>`
/// and its nine slots (a pattern's key, or `-`); `#off` without an online grid.
pub(crate) fn view(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str, player: u128, pos: Pos) -> String {
    let mode = crate::bus::nbt_at(&*host, level, pos).map_or(0, |n| nbt_i64(&n, "show").clamp(0, 2));
    let Some(grid) = online_grid(dim, pos) else {
        with_server(|s| {
            s.sent.insert(player, Vec::new());
            s.pinned.remove(&player);
        });
        return format!("#s {mode}\n#off\n");
    };
    let mut list: Vec<(Pos, String)> = crate::net::grid_devices(dim, pos, true).unwrap_or_default().into_iter().filter(|(_, b)| is_provider(b)).collect();
    list.retain(|(p, _)| online_grid(dim, *p) == Some(grid));
    list.sort_by_key(|(p, _)| (sort_value(*p), *p));
    let pinned = with_server(|s| {
        if mode != 1 {
            s.pinned.remove(&player);
        }
        s.pinned.get(&player).cloned().unwrap_or_default()
    });
    let mut out = format!("#s {mode}\n");
    let mut sent = Vec::new();
    let mut pin = HashSet::new();
    for (p, b) in &list {
        let hidden = crate::bus::nbt_at(&*host, level, *p).is_some_and(|n| nbt_i64(&n, "pat_hide") != 0);
        let slots: Vec<Option<ItemStack>> = (0..SLOTS).map(|i| crate::bus::item_slot(&*host, level, *p, i)).collect();
        let full = slots.iter().all(Option::is_some);
        if !shown(mode, hidden, full, pinned.contains(p)) {
            continue;
        }
        if mode == 1 {
            pin.insert(*p);
        }
        if sent.len() >= MAX_PROVIDERS || out.len() > 28_000 {
            break;
        }
        let own = if b == "ae2:cable_pattern_provider" { "ae2:cable_pattern_provider" } else { "ae2:pattern_provider" };
        let group = group_word(own, &machines(host, level, *p, b));
        out.push_str(&format!("#p {group} {}\n", sort_value(*p)));
        for st in slots {
            match st {
                Some(st) => out.push_str(&format!("{}\n", key_to_string(&key_of(&*host, &st)))),
                None => out.push_str("-\n"),
            }
        }
        sent.push(*p);
    }
    with_server(|s| {
        s.sent.insert(player, sent);
        if mode == 1 {
            s.pinned.entry(player).or_default().extend(pin);
        }
    });
    out
}

/// A button on the access terminal's menu (the caller checked the player's terminal and the rate).
pub(crate) fn button(host: &mut dyn ServerHost, player: u128, id: i32, fields: &mut Fields, carried: &mut Option<ItemStack>) {
    if id == PAT_SHOW || id == PAT_SHOW_BACK {
        let step = if id == PAT_SHOW { 1 } else { 2 };
        let next = (nbt_i64(&fields.nbt, "show").clamp(0, 2) + step) % 3;
        set_nbt_i64(&mut fields.nbt, "show", next);
        resend(player);
        return;
    }
    if id < PAT_SLOT {
        return;
    }
    let n = id - PAT_SLOT;
    let (index, slot, action) = ((n >> 6) as usize, ((n >> 2) & 15) as usize, n & 3);
    if slot >= SLOTS {
        return;
    }
    let Some(at) = with_server(|s| s.sent.get(&player).and_then(|l| l.get(index).copied())) else { return };
    let Some(level) = here(&*host) else { return };
    let dim = host.dimension();
    let Some((term, _, _)) = crate::storage::viewer_for_button(&*host, player) else { return };
    if !is_provider(&crate::net::block_id(&*host, level, at)) || online_grid(dim, at).is_none() || online_grid(dim, at) != online_grid(dim, term) {
        return;
    }
    if action == ACT_SHIFT {
        // Into the inventory in the player's tick (Inventory.add), only if it fits there.
        with_server(|s| {
            if s.shifts.len() < 256 {
                s.shifts.push((player, at, slot));
            }
        });
        return;
    }
    let held = crate::bus::item_slot(&*host, level, at, slot);
    let creative = host.player(player).and_then(|p| p.is_creative(&*host)).unwrap_or(false);
    let cursor = carried.clone().map(|c| {
        let n = c.count();
        (c, n)
    });
    let pattern = carried.as_ref().is_some_and(|c| is_pattern(sid(c)));
    let Some((into, back)) = slot_action(action, held.clone(), cursor, pattern, creative) else { return };
    let into = into.and_then(|s| with_count(&s, 1));
    let back = back.and_then(|(s, n)| with_count(&s, n));
    if action == ACT_DUPLICATE {
        *carried = back;
    } else if crate::bus::set_item_slot(host, level, at, slot, into) {
        *carried = back;
    }
    resend(player);
}

fn resend(player: u128) {
    crate::autocraft::resend(player, NET_PAT);
}

/// The player's tick: their shift-clicks on provider slots, each pattern into the inventory if it fits there.
pub(crate) fn player_tick(host: &mut dyn ServerHost, player: u128) {
    let mine: Vec<(Pos, usize)> = with_server(|s| {
        if !s.shifts.iter().any(|(p, _, _)| *p == player) {
            return Vec::new();
        }
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut s.shifts).into_iter().partition(|(p, _, _)| *p == player);
        s.shifts = rest;
        mine.into_iter().map(|(_, at, slot)| (at, slot)).collect()
    });
    let Some(level) = here(&*host) else { return };
    for (at, slot) in mine {
        let Some(stack) = crate::bus::item_slot(&*host, level, at, slot) else { continue };
        let res = ItemResource::of(&stack);
        let count = stack.count();
        // A slot with room for it all (the same item first, then an empty one).
        let most = max_stack(&*host, &stack);
        let mut target = None;
        for pass in 0..2 {
            for s in 0..36 {
                let now = ItemPlace::PlayerSlot { player, slot: s }.stack_owned(host);
                let fits = match &now {
                    Some(n) => pass == 0 && n.is_stackable_with(&stack) && n.count() + count <= most,
                    None => pass == 1,
                };
                if fits {
                    target = Some(s);
                    break;
                }
            }
            if target.is_some() {
                break;
            }
        }
        let Some(s) = target else { continue };
        let moved =
            host.transaction(|host, tx| if (ItemPlace::PlayerSlot { player, slot: s }).insert(host, tx, &res, count).unwrap_or(0) == count { Ok(()) } else { Err(()) });
        if moved.is_ok() && !crate::bus::set_item_slot(host, level, at, slot, None) {
            // The slot wouldn't change: take the copy back out of the inventory.
            let _ =
                host.transaction(|host, tx| if (ItemPlace::PlayerSlot { player, slot: s }).extract(host, tx, &res, count).unwrap_or(0) == count { Ok(()) } else { Err(()) });
        }
        resend(player);
    }
}

// ---- Client

const TEXT: u32 = 0xFF40_4040;
/// pattern_access_terminal.json: the search box and the scroll bar.
const SEARCH_BOX: [f32; 4] = [104.0, 4.0, 65.0, 12.0];
const SCROLL_X: f32 = 175.0;
/// The left toolbar: the terminal style, then the display mode.
const TOOLBAR: [[f32; 2]; 2] = [[-20.0, 8.0], [-20.0, 28.0]];
/// PatternAccessTermScreen's height without its rows.
pub(crate) const BOTTOM: f32 = 97.0;
const TEX: &str = "ae2:gui/patternaccessterminal";

#[derive(Clone)]
struct Record {
    group: String,
    sort: i64,
    slots: Vec<Option<AeKey>>,
}

#[derive(Clone, Debug, PartialEq)]
enum Row {
    /// A group's header: its word and how many providers it has.
    Header(String, usize),
    /// A provider's slots (its index in the server's list).
    Slots(usize),
}

#[derive(Default)]
struct Client {
    mode: i64,
    online: bool,
    records: Vec<Record>,
    search: String,
    focus: bool,
    scroll: usize,
    drag: bool,
    rows: usize,
}

static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Client) -> R) -> R {
    let mut g = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Client::default))
}

fn inside(at: [f32; 2], r: [f32; 4]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// The group word's icon (its first id) and tooltip ids.
fn group_parts(word: &str) -> (&str, Vec<&str>) {
    match word.split_once('|') {
        Some((icon, tips)) => (icon, tips.split(',').filter(|t| !t.is_empty()).collect()),
        None => (word, Vec::new()),
    }
}

fn item_name(host: &dyn ClientHost, id: &str) -> String {
    match new_stack(host, id, 1) {
        Some(st) => crate::screens::key_name(host, &key_of(host, &st)),
        None => id.to_owned(),
    }
}

fn group_name(host: &dyn ClientHost, word: &str) -> String {
    item_name(host, group_parts(word).0)
}

/// The rows the search keeps (refreshList): records matching by a pattern's outputs' names or the group's name;
/// groups by name (lower case), providers in a group by their sort value.
fn rows_of(records: &[Record], search: &str, name_of: &dyn Fn(&str) -> String, outputs_of: &dyn Fn(&AeKey) -> Vec<String>) -> Vec<Row> {
    let search = search.to_lowercase();
    let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
    for (i, r) in records.iter().enumerate() {
        let name = name_of(&r.group).to_lowercase();
        let hit =
            search.is_empty() || name.contains(&search) || r.slots.iter().flatten().any(|k| outputs_of(k).iter().any(|o| o.to_lowercase().contains(&search)));
        if !hit {
            continue;
        }
        match groups.iter_mut().find(|g| g.0 == r.group) {
            Some(g) => g.2.push(i),
            None => groups.push((r.group.clone(), name, vec![i])),
        }
    }
    groups.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let mut rows = Vec::new();
    for (word, _, mut members) in groups {
        members.sort_by_key(|i| (records[*i].sort, *i));
        rows.push(Row::Header(word, members.len()));
        rows.extend(members.into_iter().map(Row::Slots));
    }
    rows
}

fn current_rows(host: &dyn ClientHost) -> Vec<Row> {
    let (records, search) = with(|c| (c.records.clone(), c.search.clone()));
    let outputs = |k: &AeKey| -> Vec<String> {
        let Some(st) = stack_of(host, k, 1) else { return Vec::new() };
        crate::autocraft::pattern_lists(&st).map(|(_, out, _)| out.iter().map(|(o, _)| crate::screens::key_name(host, o)).collect()).unwrap_or_default()
    };
    rows_of(&records, &search, &|w| group_name(host, w), &outputs)
}

/// The visible rows for the terminal style (laid out as the screen opens).
pub(crate) fn layout_rows() -> usize {
    let rows = crate::screens::style_rows(BOTTOM);
    with(|c| c.rows = rows);
    rows
}

fn visible_rows() -> usize {
    with(|c| c.rows.max(3))
}

pub(crate) fn draw(host: &dyn ClientHost, d: &mut dyn DrawContext, o: [f32; 2], mouse: [f32; 2]) {
    let rows = visible_rows();
    let spr = |d: &mut dyn DrawContext, at: [f32; 2], src: [f32; 4]| crate::screens::spr(d, TEX, o, at, src);
    spr(d, [0.0, 0.0], [0.0, 0.0, 195.0, 17.0]);
    let foot = 17.0 + 18.0 * rows as f32;
    spr(d, [0.0, foot], [0.0, 125.0, 195.0, 97.0]);
    d.text(
        &crate::screens::fit_text(d, &crate::screens::tr(host, "gui.ae2.PatternAccessTerminalShort", "Pattern A. Terminal"), 94.0),
        [o[0] + 8.0, o[1] + 6.0],
        TEXT,
        false,
    );
    d.text(&crate::screens::tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + foot + 4.0], TEXT, false);
    let all = current_rows(host);
    let (scroll, records, online, mode, search, focus) = with(|c| {
        c.scroll = c.scroll.min(all.len().saturating_sub(rows));
        (c.scroll, c.records.clone(), c.online, c.mode, c.search.clone(), c.focus)
    });
    for i in 0..rows {
        let y = 17.0 + 18.0 * i as f32;
        let (first, last) = (i == 0, i + 1 == rows);
        let text_src = if first {
            17.0
        } else if last {
            89.0
        } else {
            53.0
        };
        spr(d, [0.0, y], [0.0, text_src, 195.0, 18.0]);
        match all.get(scroll + i) {
            Some(Row::Slots(r)) => {
                let inv_src = if first {
                    35.0
                } else if last {
                    107.0
                } else {
                    71.0
                };
                spr(d, [0.0, y], [0.0, inv_src, 8.0 + 18.0 * SLOTS as f32 - 1.0, 18.0]);
                for (n, k) in records[*r].slots.iter().enumerate().take(SLOTS) {
                    let Some(k) = k else { continue };
                    let at = [o[0] + 8.0 + 18.0 * n as f32, o[1] + y + 1.0];
                    let valid =
                        stack_of(host, k, 1).is_some_and(|st| crate::autocraft::pattern_lists(&st).is_some_and(|(i, out, _)| !i.is_empty() && !out.is_empty()));
                    if !valid {
                        d.fill([at[0], at[1], 16.0, 16.0], 0x7FFF_0000);
                    }
                    crate::craft_screens::draw_key(host, d, k, at, None);
                }
            }
            Some(Row::Header(word, count)) => {
                let (icon, _) = group_parts(word);
                if let Some(st) = new_stack(host, icon, 1) {
                    d.item_stack(&st, [o[0] + 10.0, o[1] + y + 6.0], 8.0);
                }
                let mut name = group_name(host, word);
                if *count > 1 {
                    name.push_str(&format!(" ({count})"));
                }
                d.text(&crate::screens::fit_text(d, &name, 145.0), [o[0] + 20.0, o[1] + y + 6.0], TEXT, false);
            }
            None => {}
        }
    }
    if !online {
        let text = crate::screens::tr(host, "gui.ae2.NoNetwork", "Network offline");
        let w = d.text_width(&text);
        d.text(&text, [o[0] + 89.0 - w / 2.0, o[1] + 17.0 + 9.0 * rows as f32 - 4.0], 0xFFFF_6060, true);
    }
    // The search box (AETextField).
    let [sx, sy, sw, _] = SEARCH_BOX;
    let v = if focus { 24.0 } else { 0.0 };
    d.sprite("ae2:gui/text_field", [0.0, v, 1.0, 12.0], [o[0] + sx, o[1] + sy, 1.0, 12.0], None);
    d.sprite("ae2:gui/text_field", [1.0, v, sw - 2.0, 12.0], [o[0] + sx + 1.0, o[1] + sy, sw - 2.0, 12.0], None);
    d.sprite("ae2:gui/text_field", [127.0, v, 1.0, 12.0], [o[0] + sx + sw - 1.0, o[1] + sy, 1.0, 12.0], None);
    if search.is_empty() && !focus {
        d.text(&crate::screens::tr(host, "gui.ae2.SearchPlaceholder", "Search..."), [o[0] + sx + 2.0, o[1] + sy + 2.0], 0xFFAE_AEAE, false);
    } else {
        let shown = if focus { format!("{search}_") } else { search };
        let shown = crate::screens::fit_text(d, &shown, sw - 4.0);
        d.text(&shown, [o[0] + sx + 2.0, o[1] + sy + 2.0], 0xFFFF_FFFF, true);
    }
    // The toolbar: the terminal style, the display mode (Icon.PATTERN_TERMINAL_VISIBLE / NOT_FULL / ALL).
    crate::screens::state_button(d, o, TOOLBAR[0], [[0.0, 208.0], [16.0, 208.0], [32.0, 208.0], [48.0, 208.0]][usize::from(crate::screens::style().min(3))]);
    crate::screens::state_button(d, o, TOOLBAR[1], [[96.0, 80.0], [128.0, 80.0], [112.0, 80.0]][mode.clamp(0, 2) as usize]);
    // The scroll bar.
    let most = all.len().saturating_sub(rows);
    let track = 18.0 * rows as f32 - 2.0;
    let (sprite, dy) = if most == 0 {
        ("container/creative_inventory/scroller_disabled", 0.0)
    } else {
        ("container/creative_inventory/scroller", ((track - 15.0) * scroll as f32 / most as f32).floor())
    };
    d.sprite(sprite, [0.0, 0.0, 12.0, 15.0], [o[0] + SCROLL_X, o[1] + 18.0 + dy, 12.0, 15.0], None);
    let _ = mouse;
}

fn scroll_to(y: f32, total: usize) {
    let rows = visible_rows();
    let most = total.saturating_sub(rows);
    let travel = (18.0 * rows as f32 - 2.0 - 15.0).max(1.0);
    with(|c| c.scroll = (((y - 18.0 - 7.5) / travel).clamp(0.0, 1.0) * most as f32).round() as usize);
}

/// A press on the screen (`at` from its corner): a menu button, `Some(None)` for one the client keeps.
pub(crate) fn click(host: &dyn ClientHost, at: [f32; 2], right: bool) -> Option<Option<i32>> {
    let in_search = inside(at, SEARCH_BOX);
    with(|c| {
        c.focus = in_search;
        if in_search && right {
            c.search.clear();
            c.scroll = 0;
        }
    });
    if in_search {
        return Some(None);
    }
    if inside(at, [TOOLBAR[0][0], TOOLBAR[0][1], 16.0, 16.0]) {
        crate::screens::cycle_style(right);
        return Some(None);
    }
    if inside(at, [TOOLBAR[1][0], TOOLBAR[1][1], 16.0, 16.0]) {
        return Some(Some(if right { PAT_SHOW_BACK } else { PAT_SHOW }));
    }
    let rows = visible_rows();
    let all = current_rows(host);
    if inside(at, [SCROLL_X, 18.0, 12.0, 18.0 * rows as f32 - 2.0]) {
        with(|c| c.drag = true);
        scroll_to(at[1], all.len());
        return Some(None);
    }
    if at[0] >= 8.0 && at[0] < 8.0 + 18.0 * SLOTS as f32 && at[1] >= 18.0 && at[1] < 18.0 + 18.0 * rows as f32 {
        let scroll = with(|c| c.scroll);
        let row = scroll + ((at[1] - 18.0) / 18.0) as usize;
        if let Some(Row::Slots(r)) = all.get(row) {
            let slot = ((at[0] - 8.0) / 18.0) as i32;
            let shift = host.is_key_down("key.keyboard.left.shift") || host.is_key_down("key.keyboard.right.shift");
            let action = match (shift, right) {
                (true, false) => ACT_SHIFT,
                (true, true) => return Some(None),
                (false, true) => ACT_SPLIT,
                (false, false) => ACT_PICKUP,
            };
            return Some(Some(PAT_SLOT | ((*r as i32) << 6) | (slot << 2) | action));
        }
        return Some(None);
    }
    None
}

/// The middle button on a slot (creative: a copy of its pattern on the cursor).
pub(crate) fn middle_click(host: &dyn ClientHost, at: [f32; 2]) -> Option<i32> {
    let rows = visible_rows();
    if !(at[0] >= 8.0 && at[0] < 8.0 + 18.0 * SLOTS as f32 && at[1] >= 18.0 && at[1] < 18.0 + 18.0 * rows as f32) {
        return None;
    }
    let all = current_rows(host);
    let row = with(|c| c.scroll) + ((at[1] - 18.0) / 18.0) as usize;
    let Some(Row::Slots(r)) = all.get(row) else { return None };
    let slot = ((at[0] - 8.0) / 18.0) as i32;
    Some(PAT_SLOT | ((*r as i32) << 6) | (slot << 2) | ACT_DUPLICATE)
}

pub(crate) fn tooltip(host: &dyn ClientHost, at: [f32; 2], lines: &mut Vec<String>) -> bool {
    let tr = |k: &str, f: &str| crate::screens::tr(host, k, f);
    if inside(at, [TOOLBAR[0][0], TOOLBAR[0][1], 16.0, 16.0]) {
        lines.push(tr("gui.tooltips.ae2.TerminalStyle", "Terminal Style"));
        let (k, f) = [
            ("gui.tooltips.ae2.TerminalStyle_Small", "Small Centered Terminal"),
            ("gui.tooltips.ae2.TerminalStyle_Medium", "Medium Centered Terminal"),
            ("gui.tooltips.ae2.TerminalStyle_Tall", "Tall Centered Terminal"),
            ("gui.tooltips.ae2.TerminalStyle_Full", "Full-Height Terminal"),
        ][usize::from(crate::screens::style().min(3))];
        lines.push(format!("§7{}", tr(k, f)));
        return true;
    }
    if inside(at, [TOOLBAR[1][0], TOOLBAR[1][1], 16.0, 16.0]) {
        lines.push(tr("gui.tooltips.ae2.InterfaceTerminalDisplayMode", "Display Mode"));
        let (k, f) = [
            ("gui.tooltips.ae2.ShowVisibleProviders", "Show visible Pattern Providers"),
            ("gui.tooltips.ae2.ShowNonFullProviders", "Show visible Pattern Providers with empty slots"),
            ("gui.tooltips.ae2.ShowAllProviders", "Show all Pattern Providers"),
        ][with(|c| c.mode.clamp(0, 2)) as usize];
        lines.push(format!("§7{}", tr(k, f)));
        return true;
    }
    let rows = visible_rows();
    if at[0] < 8.0 || at[0] >= 170.0 || at[1] < 18.0 || at[1] >= 18.0 + 18.0 * rows as f32 {
        return inside(at, [0.0, 0.0, 195.0, 17.0 + 18.0 * rows as f32]);
    }
    let all = current_rows(host);
    let row = with(|c| c.scroll) + ((at[1] - 18.0) / 18.0) as usize;
    match all.get(row) {
        Some(Row::Header(word, _)) => {
            let (_, tips) = group_parts(word);
            if !tips.is_empty() {
                lines.push(tr("gui.ae2.AdjacentToDifferentMachines", "Adjacent to Different Machines"));
                lines.extend(tips.iter().map(|t| format!("§7{}", item_name(host, t))));
            }
        }
        Some(Row::Slots(r)) => {
            let n = ((at[0] - 8.0) / 18.0) as usize;
            if n < SLOTS
                && let Some(Some(k)) = with(|c| c.records.get(*r).and_then(|rec| rec.slots.get(n).cloned()))
            {
                lines.push(crate::screens::key_name(host, &k));
                if let Some(st) = stack_of(host, &k, 1) {
                    lines.extend(crate::autocraft::pattern_tooltip(host, &st).into_iter().map(|l| format!("§7{l}")));
                }
            }
        }
        None => {}
    }
    true
}

/// Whether the open screen is the access terminal.
pub(crate) fn is_open(screen: &ScreenView<'_>) -> bool {
    crate::bus::menu(screen.menu) == Some("ae2:pattern_access_terminal")
}

pub(crate) fn scrolled(host: &dyn ClientHost, screen: &ScreenView<'_>, dy: f64) -> bool {
    if !is_open(screen) || dy == 0.0 {
        return false;
    }
    let total = current_rows(host).len();
    let rows = visible_rows();
    with(|c| c.scroll = if dy > 0.0 { c.scroll.saturating_sub(1) } else { (c.scroll + 1).min(total.saturating_sub(rows)) });
    true
}

pub(crate) fn dragged(host: &dyn ClientHost, screen: &ScreenView<'_>) -> bool {
    if !is_open(screen) || !with(|c| c.drag) {
        return false;
    }
    let total = current_rows(host).len();
    scroll_to(screen.mouse[1] - screen.rect[1], total);
    true
}

pub(crate) fn released() {
    with(|c| c.drag = false);
}

/// Keys while the search box has them (Escape still closes; `e` types).
pub(crate) fn key(screen: &ScreenView<'_>, key: &str) -> bool {
    if !is_open(screen) || !with(|c| c.focus) {
        return false;
    }
    match key {
        "key.keyboard.escape" => return false,
        "key.keyboard.backspace" => with(|c| {
            c.search.pop();
            c.scroll = 0;
        }),
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => with(|c| c.focus = false),
        _ => {}
    }
    true
}

pub(crate) fn typed(screen: &ScreenView<'_>, ch: char) -> bool {
    if !is_open(screen) || ch.is_control() {
        return false;
    }
    with(|c| {
        if !c.focus {
            // A space with an empty search is swallowed (PatternAccessTermScreen.charTyped).
            return ch == ' ' && c.search.is_empty();
        }
        if c.search.chars().count() < 64 {
            c.search.push(ch);
            c.scroll = 0;
        }
        true
    })
}

pub(crate) fn closing() {
    with(|c| {
        c.focus = false;
        c.drag = false;
    });
}

/// The server's `ae2:pat` (see `view`).
pub(crate) fn from_server(text: &str) {
    let mut records: Vec<Record> = Vec::new();
    let (mut mode, mut online) = (0, true);
    for l in text.lines() {
        if let Some(m) = l.strip_prefix("#s ") {
            mode = m.trim().parse::<i64>().unwrap_or(0).clamp(0, 2);
        } else if l == "#off" {
            online = false;
        } else if let Some(rest) = l.strip_prefix("#p ") {
            let mut w = rest.split(' ');
            let group = w.next().unwrap_or("ae2:pattern_provider").to_owned();
            let sort = w.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            records.push(Record { group, sort, slots: Vec::new() });
        } else if let Some(r) = records.last_mut()
            && r.slots.len() < SLOTS
        {
            r.slots.push(if l == "-" { None } else { key_from_string(l) });
        }
    }
    with(|c| {
        c.mode = mode;
        c.online = online;
        c.records = records;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_modes() {
        // Visible: not hidden. Not full: not hidden, and an empty slot or pinned. All: everything.
        assert!(shown(0, false, true, false) && !shown(0, true, false, false));
        assert!(shown(1, false, false, false) && !shown(1, false, true, false));
        assert!(shown(1, false, true, true) && !shown(1, true, false, true));
        assert!(shown(2, true, true, false));
    }

    #[test]
    fn groups() {
        let own = "ae2:pattern_provider";
        assert_eq!(group_word(own, &[]), own);
        assert_eq!(group_word(own, &["minecraft:furnace".into()]), "minecraft:furnace");
        assert_eq!(group_word(own, &["minecraft:furnace".into(), "minecraft:furnace".into()]), "minecraft:furnace");
        assert_eq!(group_word(own, &["minecraft:furnace".into(), "minecraft:chest".into()]), "ae2:pattern_provider|minecraft:furnace,minecraft:chest");
        assert_eq!(
            group_parts("ae2:pattern_provider|minecraft:furnace,minecraft:chest"),
            ("ae2:pattern_provider", vec!["minecraft:furnace", "minecraft:chest"])
        );
    }

    #[test]
    fn slot_clicks() {
        // A pattern into an empty slot: one of it; the rest stays on the cursor.
        assert_eq!(slot_action(ACT_PICKUP, None, Some(("p", 1)), true, false), Some((Some("p"), None)));
        assert_eq!(slot_action(ACT_PICKUP, None, Some(("p", 3)), true, false), Some((Some("p"), Some(("p", 2)))));
        // A swap with one on the cursor; several on the cursor can't swap.
        assert_eq!(slot_action(ACT_PICKUP, Some("q"), Some(("p", 1)), true, false), Some((Some("p"), Some(("q", 1)))));
        assert_eq!(slot_action(ACT_PICKUP, Some("q"), Some(("p", 2)), true, false), None);
        // Anything but an encoded pattern stays out.
        assert_eq!(slot_action(ACT_PICKUP, None, Some(("dirt", 1)), false, false), None);
        // An empty cursor takes the pattern (left or right).
        assert_eq!(slot_action::<&str>(ACT_PICKUP, Some("q"), None, false, false), Some((None, Some(("q", 1)))));
        assert_eq!(slot_action::<&str>(ACT_SPLIT, Some("q"), None, false, false), Some((None, Some(("q", 1)))));
        // The creative copy needs creative mode and an empty cursor.
        assert_eq!(slot_action::<&str>(ACT_DUPLICATE, Some("q"), None, false, false), None);
        assert_eq!(slot_action::<&str>(ACT_DUPLICATE, Some("q"), None, false, true), Some((Some("q"), Some(("q", 1)))));
        assert_eq!(slot_action(ACT_DUPLICATE, Some("q"), Some(("p", 1)), true, true), None);
    }

    #[test]
    fn rows_by_group_and_search() {
        let k = |id: &str| AeKey::item(id, "", 0, 0);
        let records = vec![
            Record { group: "minecraft:smoker".into(), sort: 5, slots: vec![Some(k("p:a"))] },
            Record { group: "minecraft:furnace".into(), sort: 9, slots: vec![Some(k("p:b"))] },
            Record { group: "minecraft:furnace".into(), sort: 2, slots: vec![None] },
        ];
        let name = |w: &str| group_parts(w).0.trim_start_matches("minecraft:").to_owned();
        let outputs = |key: &AeKey| if key.id() == "p:b" { vec!["Iron Ingot".to_owned()] } else { vec!["Cooked Beef".to_owned()] };
        let rows = rows_of(&records, "", &name, &outputs);
        assert_eq!(
            rows,
            vec![Row::Header("minecraft:furnace".into(), 2), Row::Slots(2), Row::Slots(1), Row::Header("minecraft:smoker".into(), 1), Row::Slots(0)]
        );
        // By an output's name: only that provider, under its group.
        assert_eq!(rows_of(&records, "iron", &name, &outputs), vec![Row::Header("minecraft:furnace".into(), 1), Row::Slots(1)]);
        // By the group's name.
        assert_eq!(rows_of(&records, "SMOK", &name, &outputs), vec![Row::Header("minecraft:smoker".into(), 1), Row::Slots(0)]);
    }

    #[test]
    fn sort_values() {
        assert!(sort_value([0, 64, 1]) > sort_value([100, 64, 0]));
        assert_eq!(sort_value([1, 2, 3]), (3 << 24) ^ (1 << 8) ^ 2);
    }
}
