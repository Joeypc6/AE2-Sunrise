//! Comparators beside AE2's blocks (AEBaseEntityBlock.hasAnalogOutputSignal / getAnalogOutputSignal): an inventory's
//! fullness (InternalInventory.getRedstoneSignal, over the slots AE2 counts: not the upgrade cards), an energy cell's
//! fill (EnergyCellBlock.getAnalogOutputSignal).
//!
//! Sunrise asks `analog_output` from inside the level's update with a level reader only, so each block's reading is
//! worked out in its block entity's tick and kept here; a change tells the comparators beside it at the level's tick end
//! (`NeighbourUpdate::OutputSignal`).

use crate::cells::max_stack;
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct Signals {
    values: HashMap<(&'static str, Pos), i32>,
    updates: Vec<(&'static str, Pos)>,
}

static SIGNALS: Mutex<Option<Signals>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Signals) -> R) -> R {
    let mut g = SIGNALS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Signals::default))
}

pub(crate) fn forget_all() {
    *SIGNALS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The slots of a block's container that its comparator reads, and the inventory's size (upstream `comparator_slots`;
/// the ME chest's input slot, never holding anything, counts towards its size).
pub(crate) fn slots_of(block: &str) -> Option<(Vec<usize>, usize)> {
    Some(match block {
        "ae2:drive" => ((0..10).collect(), 10),
        "ae2:chest" => (vec![0], 2),
        "ae2:charger" | "ae2:vibration_chamber" => (vec![0], 1),
        "ae2:spatial_io_port" => (vec![0, 1], 2),
        // gridInv and patternInv, not the cards between them.
        "ae2:molecular_assembler" => (std::iter::once(0).chain(6..16).collect(), 11),
        "ae2:sky_stone_chest" | "ae2:smooth_sky_stone_chest" => ((0..36).collect(), 36),
        // InternalInventory.empty().
        "ae2:controller" | "ae2:energy_acceptor" => (Vec::new(), 1),
        _ => return None,
    })
}

fn is_energy_cell(block: &str) -> bool {
    matches!(block, "ae2:energy_cell" | "ae2:dense_energy_cell")
}

/// `has_analog_output`: the blocks comparators read.
pub(crate) fn has_output(block: &str) -> bool {
    slots_of(block).is_some() || is_energy_cell(block)
}

/// InternalInventory.getRedstoneSignal: each stack's share of its stack limit, over the inventory's size, as 0 to 14,
/// plus one when anything is there.
pub(crate) fn signal(fills: &[f32], size: usize) -> i32 {
    let any = !fills.is_empty();
    let total = fills.iter().sum::<f32>() / size.max(1) as f32;
    (total.min(1.0) * 14.0).floor() as i32 + i32::from(any)
}

/// The reading of `stacks` (an inventory of `size`).
pub(crate) fn of_slots(host: &dyn Host, stacks: &[Option<ItemStack>], size: usize) -> i32 {
    let fills: Vec<f32> = stacks.iter().flatten().map(|s| s.count() as f32 / max_stack(host, s).clamp(1, 64) as f32).collect();
    signal(&fills, size)
}

/// EnergyCellBlock.getAnalogOutputSignal: the cell's fill (a cell's share is the grid store's fill here).
pub(crate) fn energy_signal(current: f64, most: f64) -> i32 {
    if most <= 0.0 {
        return 0;
    }
    ((current / most).clamp(0.0, 1.0) * 14.0).floor() as i32 + i32::from(current > 0.0)
}

/// A block's reading now: its neighbours' comparators are told at the tick's end when it changed.
pub(crate) fn set(dim: &'static str, pos: Pos, value: i32) {
    let value = value.clamp(0, 15);
    with(|s| {
        if s.values.insert((dim, pos), value).is_none_or(|was| was != value) {
            s.updates.push((dim, pos));
        }
    });
}

/// `analog_output`: the block's last reading.
pub(crate) fn get(dim: &'static str, pos: Pos) -> i32 {
    with(|s| s.values.get(&(dim, pos)).copied().unwrap_or(0))
}

/// A device's tick (`devices::run_device`): its reading from its slots, or a cell's from its grid's store.
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, block: &str, fields: &Fields) {
    if is_hosted(pos) {
        return;
    }
    let dim = host.dimension();
    if is_energy_cell(block) {
        let (current, most) = crate::net::grid_power(dim, pos).unwrap_or((0.0, 1.0));
        set(dim, pos, energy_signal(current, most));
        return;
    }
    // The sky chests' own tick reads theirs.
    if crate::sky_chest::is_sky_chest(block) {
        return;
    }
    let Some((slots, size)) = slots_of(block) else { return };
    let stacks: Vec<Option<ItemStack>> = slots.iter().map(|i| fields.items.get(*i).cloned().flatten()).collect();
    set(dim, pos, of_slots(&*host, &stacks, size));
}

/// The level's tick end: comparators beside a block whose reading changed read it again.
pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let updates: Vec<Pos> = with(|s| {
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut s.updates).into_iter().partition(|u| u.0 == dim);
        s.updates = rest;
        mine.into_iter().map(|u| u.1).collect()
    });
    for p in updates {
        match level.block_state(&*host, p) {
            Some(state) if has_output(&crate::net::block_id(&*host, level, p)) => {
                let _ = level.update_neighbors(host, p, state, NeighbourUpdate::OutputSignal);
            }
            // The block went: its reading with it.
            _ => with(|s| {
                s.values.remove(&(dim, p));
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_signal() {
        assert_eq!(signal(&[], 36), 0);
        // One item in a big chest still gives 1.
        assert_eq!(signal(&[1.0 / 64.0], 36), 1);
        assert_eq!(signal(&[1.0; 36], 36), 15);
        assert_eq!(signal(&[1.0], 1), 15);
        assert_eq!(signal(&[0.5], 1), 8);
        // A cell in the ME chest (its size two): half full.
        assert_eq!(signal(&[1.0], 2), 8);
    }

    #[test]
    fn cell_signal() {
        assert_eq!(energy_signal(0.0, 200_000.0), 0);
        assert_eq!(energy_signal(1.0, 200_000.0), 1);
        assert_eq!(energy_signal(200_000.0, 200_000.0), 15);
        assert_eq!(energy_signal(5.0, 0.0), 0);
    }
}
