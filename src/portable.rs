//! The portable item and fluid cells (PortableCellItem): a storage cell with its own battery, opened from the hand or a
//! hotkey on a three-row terminal of its own contents (`wireless`'s item screen). Upstream's `portable_type`,
//! `portable_tick`, `cell_store` and `portable_power` (`../ae2-horizon/mixins/ae2/me.rs`).
//!
//! The contents are the cell's own (`cells::read_cell`/`write_cell` on the item, as a storage cell's), listed to the
//! client on the terminal's channel (`storage::NET_ITEMS`, the same lines a terminal gets) and changed by the
//! terminal's grid clicks (the same button ids). Moving items costs the cell's charge as the network's would
//! (StorageHelper.poweredInsert: an AE an item, a bucket's worth of fluid an AE), and an open cell draws half an AE a
//! tick (`wireless::player_tick`). The item is checked in its slot before each operation, and written back in one
//! edit before anything is handed out.

use crate::cells::*;
use crate::core::{Actionable, AeKey, CellType, KeyType, StorageTier};
use crate::storage::{CREATIVE_DUPLICATE, MOVE_REGION, RIGHT, ROLL_DOWN, ROLL_UP, SHIFT};
use crate::util::*;
use crate::wireless::{Session, edit_held, give_back};
use crate::world::{item_power, set_item_power};
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

const MESSAGE_BYTES: usize = 29_000;

/// PortableCellItem's storage: the storage cell of its size (item or fluid).
pub(crate) fn portable_type(item: &str) -> Option<CellType> {
    let name = item.strip_prefix("ae2:")?;
    let (size, fluid) = match name.strip_prefix("portable_item_cell_") {
        Some(s) => (s, false),
        None => (name.strip_prefix("portable_fluid_cell_")?, true),
    };
    let tier = StorageTier::ALL.into_iter().find(|t| t.name_prefix == size)?;
    Some(if fluid { CellType::fluid_cell(tier) } else { CellType::item_cell(tier) })
}

/// How much of a key one AE moves (AEKeyType.transferFactor): fluids by the bucket.
fn factor(key: &AeKey) -> f64 {
    if key.key_type() == KeyType::Fluids { 1000.0 } else { 1.0 }
}

/// The charge `n` of `key` costs (pure: unit tested).
pub(crate) fn cost(key: &AeKey, n: i64, usage: f64) -> f64 {
    n as f64 / factor(key) * usage
}

/// How much of `key` a charge of `power` moves (`availablePower * factor + 0.9` when it falls short).
pub(crate) fn affordable(key: &AeKey, want: i64, power: f64, usage: f64) -> i64 {
    if want <= 0 {
        return 0;
    }
    if cost(key, want, usage) <= power {
        return want;
    }
    want.min((power / usage.max(1e-9) * factor(key) + 0.9).floor() as i64)
}

/// A portable cell holding something: no cell goes in it (StorageCells.isCellHandled).
fn is_filled_portable(stack: &ItemStack) -> bool {
    portable_type(sid(stack)).is_some() && custom_data(stack).is_some_and(|d| d.contains_key(&java(CELL_KEY)))
}

// ---- What each viewer was sent

#[derive(Default)]
struct Sent {
    keys: Vec<(AeKey, i64)>,
    index: HashMap<AeKey, usize>,
    free: Vec<usize>,
    started: bool,
    last: u64,
}

static SENT: Mutex<Option<HashMap<u128, Sent>>> = Mutex::new(None);

fn with_sent<R>(f: impl FnOnce(&mut HashMap<u128, Sent>) -> R) -> R {
    let mut g = SENT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

pub(crate) fn forget_all() {
    *SENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(crate) fn opened(player: u128) {
    with_sent(|s| {
        s.insert(player, Sent::default());
    });
}

pub(crate) fn closed(player: u128) {
    with_sent(|s| {
        s.remove(&player);
    });
}

/// The key the player's list has at `index`.
fn key_at(player: u128, index: usize) -> Option<(AeKey, i64)> {
    with_sent(|s| s.get(&player).and_then(|v| v.keys.get(index).cloned()).filter(|(_, n)| *n >= 0))
}

/// The cell's list to the player when it changed: `#reset` first, then the terminal's lines (`index amount key` for a
/// new entry, `index amount` for a new amount, `index -1` for one gone).
pub(crate) fn stream(host: &mut dyn ServerHost, player: u128, cell: &ItemStack) {
    let contents = read_cell(cell);
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (k, a) in contents.keys.iter().zip(&contents.amounts) {
            key_to_string(k).hash(&mut h);
            a.hash(&mut h);
        }
        h.finish()
    };
    let lines = with_sent(|s| {
        let v = s.get_mut(&player)?;
        if v.started && v.last == hash {
            return None;
        }
        let mut lines = Vec::new();
        if !v.started {
            lines.push("#reset".to_owned());
        }
        lines.push("#on 1".to_owned());
        let mut now = std::collections::HashSet::new();
        for (key, amount) in contents.keys.iter().zip(&contents.amounts).filter(|(_, a)| **a > 0) {
            match v.index.get(key) {
                Some(&i) => {
                    now.insert(i);
                    if v.keys[i].1 != *amount {
                        v.keys[i].1 = *amount;
                        lines.push(format!("{i} {amount}"));
                    }
                }
                None => {
                    let i = v.free.pop().unwrap_or_else(|| {
                        v.keys.push((key.clone(), -1));
                        v.keys.len() - 1
                    });
                    lines.push(format!("{i} {amount} {}", key_to_string(key)));
                    v.keys[i] = (key.clone(), *amount);
                    v.index.insert(key.clone(), i);
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
        v.started = true;
        v.last = hash;
        Some(lines)
    });
    let Some(lines) = lines else { return };
    let mut body = String::new();
    for line in lines {
        if body.len() + line.len() + 1 > MESSAGE_BYTES {
            let _ = send_to_player(host, player, crate::storage::NET_ITEMS, std::mem::take(&mut body).as_bytes());
        }
        body.push_str(&line);
        body.push('\n');
    }
    if !body.is_empty() {
        let _ = send_to_player(host, player, crate::storage::NET_ITEMS, body.as_bytes());
    }
}

/// The cell as an inventory with its charge, for one operation.
struct Open {
    cell: ItemStack,
    inv: crate::core::BasicCellInventory,
    power: f64,
    creative: bool,
    usage: f64,
    fluid: bool,
}

impl Open {
    fn new(host: &dyn Host, player: u128, cell: &ItemStack) -> Option<Open> {
        let ty = portable_type(sid(cell))?;
        let creative = host.player(player).and_then(|p| p.is_creative(host)).unwrap_or(false);
        Some(Open {
            cell: cell.clone(),
            inv: cell_inventory(cell, ty),
            power: item_power(cell),
            creative,
            usage: crate::config::config().power_usage_multiplier,
            fluid: ty.key_type == KeyType::Fluids,
        })
    }

    fn insert(&mut self, key: &AeKey, n: i64) -> i64 {
        let fits = self.inv.insert(key, n, Actionable::Simulate);
        let want = if self.creative { fits } else { affordable(key, fits, self.power, self.usage) };
        let done = if want > 0 { self.inv.insert(key, want, Actionable::Modulate) } else { 0 };
        if !self.creative {
            self.power = (self.power - cost(key, done, self.usage)).max(0.0);
        }
        done
    }

    fn extract(&mut self, key: &AeKey, n: i64) -> i64 {
        let has = self.inv.extract(key, n, Actionable::Simulate);
        let want = if self.creative { has } else { affordable(key, has, self.power, self.usage) };
        let done = if want > 0 { self.inv.extract(key, want, Actionable::Modulate) } else { 0 };
        if !self.creative {
            self.power = (self.power - cost(key, done, self.usage)).max(0.0);
        }
        done
    }

    /// The cell with what it holds and its charge now.
    fn finish(mut self) -> ItemStack {
        if let Some(contents) = self.inv.persist() {
            write_cell(&mut self.cell, &contents);
        }
        set_item_power(&mut self.cell, self.power);
        self.cell
    }
}

/// `tryInsertFromPlayerOwnedItem` for a shift-click from the inventory: as many of `st` as the cell takes and its charge
/// pays for. How many went in.
pub(crate) fn store(host: &mut dyn ServerHost, player: u128, s: &Session, cell: &ItemStack, st: &ItemStack) -> u32 {
    if is_filled_portable(st) {
        return 0;
    }
    let Some(mut open) = Open::new(&*host, player, cell) else { return 0 };
    let key = key_of(&*host, st);
    let n = open.insert(&key, i64::from(st.count())).clamp(0, i64::from(st.count())) as u32;
    if n == 0 {
        return 0;
    }
    let new = open.finish();
    if edit_held(host, player, s, |c| *c = new) { n } else { 0 }
}

/// A grid click on the cell's terminal (MEStorageMenu.handleNetworkInteraction on the cell): the cursor's stack in
/// (all, or one with the right button), a stack (half with the right button) onto the cursor or with shift into the
/// inventory, buckets of a fluid in and out, the wheel's one in or out.
pub(crate) fn button(host: &mut dyn ServerHost, player: u128, s: &Session, cell: ItemStack, id: i32, carried: &mut Option<ItemStack>) {
    if !(0..1 << 26).contains(&id) {
        return;
    }
    let Some(mut open) = Open::new(&*host, player, &cell) else { return };
    let (index, action) = ((id >> 3) as usize, id & 7);
    let entry = key_at(player, index);
    let mut cursor = carried.clone();
    let mut gives: Vec<ItemStack> = Vec::new();
    let held = cursor.clone();
    match (held.as_ref(), action) {
        // A bucket of a fluid in; an empty bucket filled from a fluid entry.
        (Some(st), _) if open.fluid && bucket_fluid(sid(st)).is_some() => {
            let fluid = bucket_fluid(sid(st)).unwrap_or_default();
            let key = AeKey::fluid(&fluid, "");
            if open.inv.insert(&key, 1000, Actionable::Simulate) == 1000 && open.insert(&key, 1000) == 1000 {
                swap_bucket(&*host, &mut cursor, &mut gives, st, "minecraft:bucket");
            }
        }
        (Some(st), _) if sid(st) == "minecraft:bucket" && entry.as_ref().is_some_and(|(k, _)| k.key_type() == KeyType::Fluids) => {
            let Some((key, _)) = entry.clone() else { return };
            let bucket = fluid_bucket(key.id());
            if item_key(&*host, &bucket).is_some() && open.inv.extract(&key, 1000, Actionable::Simulate) == 1000 && open.extract(&key, 1000) == 1000 {
                swap_bucket(&*host, &mut cursor, &mut gives, st, &bucket);
            }
        }
        (Some(st), ROLL_DOWN) => {
            let key = key_of(&*host, st);
            let n = open.insert(&key, 1) as u32;
            cursor = with_count(st, st.count() - n);
        }
        (_, ROLL_UP | 3) => {
            let Some((key, _)) = entry.filter(|(k, n)| k.key_type() == KeyType::Items && *n > 0) else { return };
            if let Some(c) = held.as_ref()
                && (c.count() >= max_stack(&*host, c) || key_of(&*host, c) != key)
            {
                return;
            }
            if open.extract(&key, 1) == 1 {
                cursor = match held {
                    Some(c) => with_count(&c, c.count() + 1),
                    None => stack_of(&*host, &key, 1),
                };
            }
        }
        (None, CREATIVE_DUPLICATE) => {
            if open.creative
                && let Some((key, _)) = entry
                && let Some(mut one) = stack_of(&*host, &key, 1)
            {
                let most = max_stack(&*host, &one);
                let _ = one.set_count(most);
                *carried = Some(one);
            }
            return;
        }
        (Some(st), _) => {
            if is_filled_portable(st) {
                return;
            }
            let key = key_of(&*host, st);
            let n = open.insert(&key, if action & RIGHT != 0 { 1 } else { i64::from(st.count()) }) as u32;
            cursor = with_count(st, st.count() - n);
        }
        (None, _) => {
            let Some((key, _)) = entry.filter(|(k, n)| k.key_type() == KeyType::Items && *n > 0) else { return };
            let Some(one) = stack_of(&*host, &key, 1) else { return };
            let most = i64::from(max_stack(&*host, &one));
            let (right, shift) = (action & RIGHT != 0, action & SHIFT != 0 || action == MOVE_REGION);
            let have = open.inv.extract(&key, most, Actionable::Simulate);
            let want = if right && !shift { (have + 1) / 2 } else { have };
            let got = open.extract(&key, want);
            if got <= 0 {
                return;
            }
            let Some(out) = stack_of(&*host, &key, got as u32) else { return };
            if shift {
                gives.push(out);
            } else {
                cursor = Some(out);
            }
        }
    }
    if cursor == *carried && gives.is_empty() {
        return;
    }
    // The cell first: nothing is handed out unless it was written.
    let new = open.finish();
    if !edit_held(host, player, s, |c| *c = new) {
        return;
    }
    *carried = cursor;
    for g in gives {
        give_back(host, player, g);
    }
}

/// The cursor's bucket swapped for `to` (one of a stack: the rest stays, the new one goes to the inventory).
fn swap_bucket(host: &dyn Host, cursor: &mut Option<ItemStack>, gives: &mut Vec<ItemStack>, st: &ItemStack, to: &str) {
    let Some(new) = new_stack(host, to, 1) else { return };
    if st.count() > 1 {
        *cursor = with_count(st, st.count() - 1);
        gives.push(new);
    } else {
        *cursor = Some(new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_types() {
        let t = portable_type("ae2:portable_item_cell_4k").expect("a 4k cell");
        assert_eq!(t.key_type, KeyType::Items);
        assert_eq!(portable_type("ae2:portable_fluid_cell_256k").map(|t| t.key_type), Some(KeyType::Fluids));
        assert!(portable_type("ae2:item_storage_cell_1k").is_none());
        assert!(portable_type("ae2:portable_item_cell_2k").is_none());
    }

    #[test]
    fn charge_pays_for_moves() {
        let dirt = AeKey::item("minecraft:dirt", "", 0, 0);
        let water = AeKey::fluid("minecraft:water", "");
        assert_eq!(cost(&dirt, 64, 1.0), 64.0);
        assert_eq!(cost(&water, 1000, 1.0), 1.0);
        assert_eq!(affordable(&dirt, 64, 100.0, 1.0), 64);
        assert_eq!(affordable(&dirt, 64, 10.0, 1.0), 10);
        assert_eq!(affordable(&dirt, 64, 0.0, 1.0), 0);
        assert_eq!(affordable(&water, 5000, 2.0, 1.0), 2000);
        assert_eq!(affordable(&dirt, 0, 10.0, 1.0), 0);
    }
}
