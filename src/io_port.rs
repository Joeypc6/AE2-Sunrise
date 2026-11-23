//! AE2's ME IO port (IOPortBlockEntity, IOPortMenu, IOPortScreen).
//!
//! Slots: 0..6 cells in, 6..12 cells out, 12..15 cards (up to 3 acceleration cards, 1 redstone card). On an active
//! node it empties each input cell into the network (or, filling, fills it from the network), 256 operations a tick
//! (doubled by each acceleration card), paying the network's power per item as StorageHelper does; a cell whose work
//! is done goes to the first free output slot when the fullness mode says so (empty, work done, or full). With a
//! redstone card it works by its redstone mode. Top and bottom reach the inputs (insert only, cells only); the sides
//! reach the outputs (extract only).

use crate::cells::*;
use crate::core::{Actionable, AeKey, CellState};
use crate::machines::{IO_PORT, TEXT, bar_place, base_screen, card_slot, cards, inside, slot_icon};
use crate::screens::{side_panel, spr, state_button, tr};
use crate::storage::{Scope, pay_moved, power_allows, with_storage};
use crate::util::*;
use mod_api::*;

pub(crate) const SLOTS: usize = 15;
const INPUTS: std::ops::Range<usize> = 0..6;
const OUTPUTS: std::ops::Range<usize> = 6..12;
const CARDS_FROM: usize = 12;
const CARD_SLOTS: usize = 3;
/// Upgrades.add(SPEED_CARD, IO_PORT, 3) and (REDSTONE_CARD, IO_PORT, 1).
const CARD_LIMITS: [(&str, usize); 2] = [("ae2:speed_card", 3), ("ae2:redstone_card", 1)];

/// The menu's buttons: the transfer direction (Settings.OPERATION_MODE), the fullness mode and the redstone mode.
pub(crate) const BTN_DIRECTION: i32 = 1;
pub(crate) const BTN_FULLNESS: i32 = 2;
pub(crate) const BTN_REDSTONE: i32 = 3;
const DIRECTION_AT: [f32; 2] = [80.0, 17.0];

/// The operations a tick: 256, twice that for each acceleration card (up to three).
pub(crate) fn budget(speed_cards: usize) -> i64 {
    256 << speed_cards.min(3)
}

/// FullnessMode (0 empty, 1 half: when its work is done, 2 full): whether a cell is ready to move out.
pub(crate) fn ready(fullness: i64, state: CellState) -> bool {
    match fullness {
        1 => true,
        2 => state == CellState::Full,
        _ => state == CellState::Empty,
    }
}

/// RedstoneMode (0 ignore, 1 low signal, 2 high signal, 3 pulse): whether it works this tick.
pub(crate) fn redstone_enabled(mode: i64, on: bool, was: bool) -> bool {
    match mode {
        1 => !on,
        2 => on,
        3 => on && !was,
        _ => true,
    }
}

/// Whether an item goes in the inputs (StorageCells.isCellHandled).
fn is_cell(item: &str) -> bool {
    is_cell_item(item)
}

/// IOPortBlockEntity.tickingRequest.
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    if fields.items.len() < SLOTS {
        fields.items.resize(SLOTS, None);
    }
    if fields.data.len() < 4 {
        fields.data.resize(4, 0);
    }
    let dim = host.dimension();
    let Some(level) = here(&*host) else { return };
    let (_, _, active) = crate::net::device_state(dim, pos);
    let rs_card = cards(&fields.items, CARDS_FROM, CARD_SLOTS, "ae2:redstone_card") > 0;
    let n = |k: &str| nbt_i64(&fields.nbt, k);
    let data = [i32::from(active), n("mode") as i32, n("full") as i32, if rs_card { n("redstone") as i32 } else { -1 }];
    if fields.data[..4] != data {
        fields.data[..4].copy_from_slice(&data);
    }
    // The redstone state, kept each tick so a pulse is a rising edge.
    let on = level.has_neighbor_signal(&*host, pos);
    let was = nbt_i64(&fields.nbt, "rs") != 0;
    if on != was {
        set_nbt_i64(&mut fields.nbt, "rs", i64::from(on));
    }
    if !active || rs_card && !redstone_enabled(nbt_i64(&fields.nbt, "redstone"), on, was) {
        return;
    }
    let fill = nbt_i64(&fields.nbt, "mode") == 1;
    let fullness = nbt_i64(&fields.nbt, "full");
    let speed = cards(&fields.items, CARDS_FROM, CARD_SLOTS, "ae2:speed_card").min(3);
    let mut left = budget(speed);
    for slot in INPUTS {
        let Some(mut stack) = fields.items[slot].clone() else { continue };
        let Some(cell) = cell_type(sid(&stack)) else {
            // No cell inventory (a creative cell): straight out.
            move_out(fields, slot);
            continue;
        };
        let mut inv = cell_inventory(&stack, cell);
        if left > 0 {
            let before = left;
            left = with_storage(host, Scope::Grid(pos), None, |st| {
                let mut left = before;
                // transferContents: the source's keys as they stand, each moved as far as the other side, the
                // budget and the power allow.
                let source: Vec<(AeKey, i64)> = if fill {
                    st.contents().iter().filter(|(_, n)| *n > 0).map(|(k, n)| (k.clone(), n)).collect()
                } else {
                    let mut held = crate::core::KeyCounter::new();
                    inv.available_stacks(&mut held);
                    held.iter().filter(|(_, n)| *n > 0).map(|(k, n)| (k.clone(), n)).collect()
                };
                for (key, total) in source {
                    if left <= 0 {
                        break;
                    }
                    let per = key.key_type().amount_per_operation();
                    let fits = if fill { inv.insert(&key, total, Actionable::Simulate) } else { st.insert(&key, total, Actionable::Simulate) };
                    let want = power_allows(dim, pos, &key, fits.min(left.saturating_mul(per)));
                    if want <= 0 {
                        continue;
                    }
                    let got = if fill { st.extract(&key, want, Actionable::Modulate) } else { inv.extract(&key, want, Actionable::Modulate) };
                    if got <= 0 {
                        continue;
                    }
                    let put = if fill { inv.insert(&key, got, Actionable::Modulate) } else { st.insert(&key, got, Actionable::Modulate) };
                    if put < got {
                        // What didn't go goes back where it came from.
                        if fill {
                            st.insert(&key, got - put, Actionable::Modulate);
                        } else {
                            inv.insert(&key, got - put, Actionable::Modulate);
                        }
                    }
                    if put > 0 {
                        pay_moved(dim, pos, &key, put);
                        left -= (put / per).max(1);
                    }
                }
                left
            })
            .unwrap_or(before);
            if let Some(contents) = inv.persist() {
                write_cell(&mut stack, &contents);
                rewritten(&mut stack, status_number(inv.status()));
                fields.items[slot] = Some(stack);
            }
        }
        if left > 0 && ready(fullness, inv.status()) {
            move_out(fields, slot);
        }
    }
}

/// A cell the port changed: no network's id or version on it any more (a drive mounts it afresh, never a copy it
/// kept of what it held before), and its new status for its light.
fn rewritten(stack: &mut ItemStack, status: i64) {
    edit_custom_data(stack, |d| {
        d.remove(&java("ae2:cell_id"));
        d.remove(&java("ae2:cell_ver"));
        d.insert(java("ae2:cell_status"), Tag::Long(status));
    });
}

/// IOPortBlockEntity.moveSlot: the input to the first free output slot (none free: it stays).
fn move_out(fields: &mut Fields, slot: usize) {
    if let Some(out) = OUTPUTS.clone().find(|&i| fields.items[i].is_none()) {
        fields.items[out] = fields.items[slot].take();
    }
}

// ---- Automation and the menu's rules

pub(crate) fn insert_limit(slot: usize, stack: &ItemStack) -> u32 {
    if INPUTS.contains(&slot) && is_cell(sid(stack)) { 1 } else { 0 }
}

pub(crate) fn may_extract(slot: usize) -> bool {
    OUTPUTS.contains(&slot)
}

/// ITEMS for the IO port (getExposedInventoryForSide: its top and bottom the input cells, its sides the outputs).
pub(crate) fn items(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    let slots: Vec<usize> = match at.side {
        Some(Direction::Up | Direction::Down) => INPUTS.collect(),
        Some(_) => OUTPUTS.collect(),
        None => INPUTS.chain(OUTPUTS).collect(),
    };
    crate::machines::handler(host, &at, IO_PORT, slots)
}

/// IOPortMenu's slots: cells into the inputs (one a slot), nothing into the outputs, the cards it takes.
pub(crate) fn slot_rules(context: &MenuSlotContext, slot: usize, stack: &ItemStack, may_place: &mut bool, max_stack: &mut u32) {
    let id = sid(stack);
    if INPUTS.contains(&slot) {
        *may_place = is_cell(id);
        *max_stack = 1;
    } else if OUTPUTS.contains(&slot) {
        *may_place = false;
    } else {
        let owned: Vec<Option<ItemStack>> = (CARDS_FROM..CARDS_FROM + CARD_SLOTS).map(|i| context.slot(MenuContainer::Main, i)).collect();
        let shown: Vec<Option<&ItemStack>> = owned.iter().map(Option::as_ref).collect();
        *may_place = crate::machines::card_fits(&CARD_LIMITS, &shown, slot - CARDS_FROM, id);
        *max_stack = 1;
    }
}

pub(crate) fn button(fields: &mut Fields, id: i32, back: bool) {
    match id {
        BTN_DIRECTION => crate::machines::cycle(&mut fields.nbt, "mode", 2, back),
        BTN_FULLNESS => crate::machines::cycle(&mut fields.nbt, "full", 3, back),
        // Only with a redstone card (the button shows only then).
        BTN_REDSTONE if cards(&fields.items, CARDS_FROM, CARD_SLOTS, "ae2:redstone_card") > 0 => crate::machines::cycle(&mut fields.nbt, "redstone", 4, back),
        _ => {}
    }
}

// ---- The screen (io_port.json over io_port.png)

pub(crate) fn screen() -> MenuScreenLayout {
    let mut slots: Vec<[f32; 2]> = INPUTS.map(|i| [19.0 + 18.0 * (i % 2) as f32, 17.0 + 18.0 * (i / 2) as f32]).collect();
    slots.extend((0..6).map(|i| [122.0 + 18.0 * (i % 2) as f32, 17.0 + 18.0 * (i / 2) as f32]));
    slots.extend((0..CARD_SLOTS).map(card_slot));
    base_screen(176.0, 166.0, slots, 84.0)
}

const FULLNESS_ICONS: [[f32; 2]; 3] = [[0.0, 80.0], [16.0, 80.0], [32.0, 80.0]];
const REDSTONE_ICONS: [[f32; 2]; 4] = [[48.0, 0.0], [0.0, 0.0], [16.0, 0.0], [32.0, 0.0]];

pub(crate) fn draw(host: &dyn ClientHost, data: &[i32], d: &mut dyn DrawContext, o: [f32; 2]) {
    let v = |i: usize| data.get(i).copied().unwrap_or(0);
    spr(d, "ae2:gui/io_port", o, [0.0, 0.0], [0.0, 0.0, 176.0, 166.0]);
    side_panel(d, o, 176.0, CARD_SLOTS);
    d.text(&tr(host, "gui.ae2.IOPort", "ME IO Port"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
    d.text(&tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 73.0], TEXT, false);
    for i in INPUTS {
        slot_icon(d, o, [19.0 + 18.0 * (i % 2) as f32, 17.0 + 18.0 * (i / 2) as f32], [240.0, 0.0]);
    }
    for i in 0..CARD_SLOTS {
        slot_icon(d, o, card_slot(i), crate::machines::UPGRADE_ICON);
    }
    // A cell and a drive either side of the direction button.
    for (id, x) in [("ae2:item_storage_cell_1k", 58.0), ("ae2:drive", 102.0)] {
        if let Some(stack) = new_stack(host, id, 1) {
            d.gui_draw_item_with(&stack, [o[0] + x, o[1] + 17.0], None);
        }
    }
    state_button(d, o, DIRECTION_AT, if v(1) == 1 { [48.0, 48.0] } else { [32.0, 48.0] });
    state_button(d, o, bar_place(0), FULLNESS_ICONS[v(2).clamp(0, 2) as usize]);
    if v(3) >= 0 {
        state_button(d, o, bar_place(1), REDSTONE_ICONS[v(3).clamp(0, 3) as usize]);
    }
}

pub(crate) fn click(data: &[i32], at: [f32; 2]) -> Option<i32> {
    let on = |p: [f32; 2]| inside(at, [p[0], p[1], 16.0, 16.0]);
    if on(DIRECTION_AT) {
        Some(BTN_DIRECTION)
    } else if on(bar_place(0)) {
        Some(BTN_FULLNESS)
    } else if on(bar_place(1)) && data.get(3).copied().unwrap_or(-1) >= 0 {
        Some(BTN_REDSTONE)
    } else {
        None
    }
}

pub(crate) fn tooltip(host: &dyn ClientHost, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) {
    let v = |i: usize| data.get(i).copied().unwrap_or(0);
    let t = |k: &str, f: &str| tr(host, k, f);
    let (title, value) = match click(data, at) {
        Some(BTN_DIRECTION) => (
            t("gui.tooltips.ae2.TransferDirection", "Transfer Direction"),
            if v(1) == 1 { t("gui.tooltips.ae2.TransferToStorageCell", "Transfer data to Storage Cell") } else { t("gui.tooltips.ae2.TransferToNetwork", "Transfer data to Network") },
        ),
        Some(BTN_FULLNESS) => (
            "Fullness Mode".to_owned(),
            match v(2) {
                1 => t("gui.tooltips.ae2.MoveWhenWorkIsDone", "Move to output when work is done."),
                2 => t("gui.tooltips.ae2.MoveWhenFull", "Move to output when full."),
                _ => t("gui.tooltips.ae2.MoveWhenEmpty", "Move to output when empty."),
            },
        ),
        Some(BTN_REDSTONE) => (
            t("gui.tooltips.ae2.RedstoneMode", "Redstone Mode"),
            match v(3) {
                1 => t("gui.tooltips.ae2.ActiveWithoutSignal", "Active without signal"),
                2 => t("gui.tooltips.ae2.ActiveWithSignal", "Active with signal"),
                3 => t("gui.tooltips.ae2.ActiveOnPulse", "Activate once per pulse"),
                _ => t("gui.tooltips.ae2.AlwaysActive", "Always active"),
            },
        ),
        _ => return,
    };
    lines.push(title);
    lines.push(format!("§7{value}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullness_modes() {
        assert!(ready(0, CellState::Empty));
        assert!(!ready(0, CellState::NotEmpty));
        assert!(ready(1, CellState::NotEmpty));
        assert!(ready(1, CellState::Full));
        assert!(ready(2, CellState::Full));
        assert!(!ready(2, CellState::TypesFull));
        assert!(!ready(2, CellState::Empty));
    }

    #[test]
    fn redstone_modes() {
        assert!(redstone_enabled(0, false, false));
        assert!(redstone_enabled(0, true, true));
        assert!(redstone_enabled(1, false, true) && !redstone_enabled(1, true, true));
        assert!(redstone_enabled(2, true, true) && !redstone_enabled(2, false, false));
        // A pulse works once, on the rising edge.
        assert!(redstone_enabled(3, true, false));
        assert!(!redstone_enabled(3, true, true));
        assert!(!redstone_enabled(3, false, true));
    }

    #[test]
    fn budget_by_cards() {
        assert_eq!(budget(0), 256);
        assert_eq!(budget(1), 512);
        assert_eq!(budget(3), 2048);
        assert_eq!(budget(7), 2048);
    }

    #[test]
    fn faces_and_slots() {
        assert!(may_extract(6) && may_extract(11));
        assert!(!may_extract(0) && !may_extract(5) && !may_extract(12));
    }
}
