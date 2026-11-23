//! JEI's "+" on the crafting terminal (AE2's UseCraftingRecipeTransfer and FillCraftingGridFromRecipePacket), and
//! Ctrl + "+": craft what's missing (CraftConfirmMenu.openWithCraftingList).
//!
//! JEI asks the open menu's `menu_transfer` part (dry run for "+", then the click). The recipe is looked up by id on
//! the server and laid out as JEI lays out a crafting card (`transfer::encoding_of`); nothing comes from the client
//! but the id. The move itself waits for the player's tick (the inventory can only be read and changed there):
//! the grid goes back into the network (what it doesn't take into the inventory; if that is full too, nothing
//! moves), then each cell takes an item its ingredient accepts from the network, else from the inventory. A cell
//! nothing fills stays empty (AE2: "missing items will be skipped").
//!
//! Ctrl: the fork's JEI passes only Shift with a transfer (`MenuTransferEvent::max`), so the client tells the server
//! when Ctrl goes down or up while a terminal is open (`ctrl 1|0` on `ae2:craft`); a transfer made with it held
//! asks the player to craft the missing ingredients the network can craft, one plan after the other.

use crate::cells::*;
use crate::core::KeyType;
use crate::storage::{self, CT_GRID, Scope};
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

/// Server to client (on `ae2:plan`): `#craftlist` and a line `<amount> <key>` per ingredient to craft.
pub(crate) const CRAFT_LIST: &str = "#craftlist";

#[derive(Default)]
struct Pending {
    /// Transfers for the player's tick: (player, terminal, recipe).
    moves: Vec<(u128, Pos, String)>,
    /// Ctrl held, by player.
    ctrl: HashMap<u128, bool>,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Pending) -> R) -> R {
    let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Pending::default))
}

pub(crate) fn forget_all() {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(crate) fn ctrl_message(player: u128, down: bool) {
    with(|p| {
        if p.ctrl.len() > 4096 {
            p.ctrl.clear();
        }
        p.ctrl.insert(player, down);
    });
}

/// The recipe's nine cells' choices (item ids), if it is a crafting recipe that fits the 3x3 grid.
pub(crate) fn layout(host: &dyn Host, recipe: &str) -> Option<Vec<Option<Vec<String>>>> {
    let e = crate::transfer::encoding_of(host, recipe)?;
    (e.mode == 0).then(|| e.cells.into_iter().take(9).map(|c| c.map(|(choices, _)| choices)).collect())
}

/// `menu_transfer` on a crafting terminal at `pos` (a block, or a part at its hosted position).
pub(crate) fn transfer(host: &mut dyn ServerHost, player: u128, pos: Pos, recipe: &str, dry_run: bool) -> RecipeTransferResult {
    if recipe.len() > 256 || layout(&*host, recipe).is_none() {
        return RecipeTransferResult::NotIntoMenu;
    }
    // AE2's "+" moves what there is; missing items are skipped.
    if dry_run {
        return RecipeTransferResult::Moves;
    }
    if !storage::rate_ok(player) {
        return RecipeTransferResult::NotIntoMenu;
    }
    with(|p| {
        p.moves.retain(|m| m.0 != player);
        p.moves.push((player, pos, recipe.to_owned()));
    });
    RecipeTransferResult::Moves
}

/// The candidate that fills a cell (pure): the first choice the network holds, else the first the inventory holds.
/// `network` and `inventory` say how many of an id there are left; the one taken is counted off.
pub(crate) fn pick_source(choices: &[String], network: &mut HashMap<String, i64>, inventory: &mut HashMap<String, i64>) -> Option<(String, bool)> {
    for c in choices {
        if let Some(n) = network.get_mut(c).filter(|n| **n > 0) {
            *n -= 1;
            return Some((c.clone(), true));
        }
    }
    for c in choices {
        if let Some(n) = inventory.get_mut(c).filter(|n| **n > 0) {
            *n -= 1;
            return Some((c.clone(), false));
        }
    }
    None
}

/// What to craft of the cells nothing filled (FillCraftingGridFromRecipePacket's craftMissing): each cell's first
/// craftable choice, counted by key.
pub(crate) fn to_craft(missing: &[Vec<String>], craftable: &dyn Fn(&str) -> bool) -> Vec<(String, i64)> {
    let mut out: Vec<(String, i64)> = Vec::new();
    for choices in missing {
        let Some(c) = choices.iter().find(|c| craftable(c)) else { continue };
        match out.iter_mut().find(|(id, _)| id == c) {
            Some((_, n)) => *n += 1,
            None => out.push((c.clone(), 1)),
        }
    }
    out
}

/// The player's tick: their transfer, if they still have the terminal open.
pub(crate) fn player_tick(host: &mut dyn ServerHost, player: u128) {
    let Some((pos, recipe)) = with(|p| {
        let i = p.moves.iter().position(|m| m.0 == player)?;
        let (_, pos, recipe) = p.moves.remove(i);
        Some((pos, recipe))
    }) else {
        return;
    };
    let Some((term, block, _)) = storage::viewer_for_button(&*host, player) else { return };
    if term != pos || block != "ae2:crafting_terminal" {
        return;
    }
    let Some(level) = here(&*host) else { return };
    let dim = host.dimension();
    if !crate::net::device_state(dim, pos).2 {
        return;
    }
    let Some(cells) = layout(&*host, &recipe) else { return };
    let scope = Scope::Grid(pos);
    // The grid back into the network, the rest into the inventory.
    for slot in CT_GRID {
        let Some(stack) = crate::bus::item_slot(&*host, level, pos, slot) else { continue };
        let key = key_of(&*host, &stack);
        let count = stack.count();
        let stored = storage::insert(host, scope, None, &key, i64::from(count)).clamp(0, i64::from(count)) as u32;
        let rest = count - stored;
        let given = if rest > 0 { give(host, player, &stack, rest) } else { true };
        if !given {
            // No room anywhere: what went in comes back, and nothing moves.
            let _ = storage::extract(host, scope, None, &key, i64::from(stored));
            return;
        }
        if !crate::bus::set_item_slot(host, level, pos, slot, None) {
            let _ = storage::extract(host, scope, None, &key, i64::from(stored));
            return;
        }
    }
    // What the network and the inventory hold of the choices.
    let wanted: std::collections::HashSet<String> = cells.iter().flatten().flatten().cloned().collect();
    let mut network: HashMap<String, i64> = HashMap::new();
    for (k, n) in storage::contents(host, scope, None) {
        if k.key_type() == KeyType::Items && k.components().is_empty() && wanted.contains(k.id()) {
            *network.entry(k.id().to_owned()).or_default() += n;
        }
    }
    let mut inventory: HashMap<String, i64> = HashMap::new();
    let mut inv_slots: Vec<(i32, ItemStack, u32)> = Vec::new();
    for s in 0..36 {
        if let Some(st) = (ItemPlace::PlayerSlot { player, slot: s }).stack_owned(host)
            && wanted.contains(sid(&st))
        {
            *inventory.entry(sid(&st).to_owned()).or_default() += i64::from(st.count());
            let n = st.count();
            inv_slots.push((s, st, n));
        }
    }
    let mut missing: Vec<Vec<String>> = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        let Some(choices) = cell else { continue };
        let Some((id, from_network)) = pick_source(choices, &mut network, &mut inventory) else {
            missing.push(choices.clone());
            continue;
        };
        let got = if from_network {
            let Some(one) = new_stack(&*host, &id, 1) else { continue };
            let key = key_of(&*host, &one);
            (storage::extract(host, scope, None, &key, 1) == 1).then_some(one)
        } else {
            take_one(host, player, &mut inv_slots, &id)
        };
        let Some(one) = got else {
            missing.push(choices.clone());
            continue;
        };
        if !crate::bus::set_item_slot(host, level, pos, CT_GRID.start + i, Some(one.clone())) {
            // The grid wouldn't take it: back where it came from.
            let key = key_of(&*host, &one);
            if storage::insert(host, scope, None, &key, 1) < 1 {
                let _ = give(host, player, &one, 1);
            }
        }
    }
    if with(|p| p.ctrl.get(&player).copied().unwrap_or(false)) && !missing.is_empty() {
        let craftables: std::collections::HashSet<String> = crate::autocraft::craftables(dim, pos)
            .0
            .into_iter()
            .filter(|k| k.key_type() == KeyType::Items && k.components().is_empty())
            .map(|k| k.id().to_owned())
            .collect();
        let list = to_craft(&missing, &|id| craftables.contains(id));
        if !list.is_empty() {
            let mut text = format!("{CRAFT_LIST}\n");
            for (id, n) in list {
                let Some(st) = new_stack(&*host, &id, 1) else { continue };
                text.push_str(&format!("{n} {}\n", key_to_string(&key_of(&*host, &st))));
            }
            let _ = crate::util::send_to_player(host, player, crate::autocraft::NET_PLAN, text.as_bytes());
        }
    }
}

/// One of `id` out of the inventory (the slots read before, with what is left in each), the stack it was.
pub(crate) fn take_one(host: &mut dyn ServerHost, player: u128, slots: &mut [(i32, ItemStack, u32)], id: &str) -> Option<ItemStack> {
    let (slot, stack, left) = slots.iter_mut().find(|(_, st, left)| sid(st) == id && *left > 0)?;
    let res = ItemResource::of(stack);
    let s = *slot;
    host.transaction(|host, tx| if (ItemPlace::PlayerSlot { player, slot: s }).extract(host, tx, &res, 1) == 1 { Ok(()) } else { Err(()) }).ok()?;
    *left -= 1;
    with_count(stack, 1)
}

/// `count` of `stack` into the inventory (onto the same item, then empty slots), all or nothing.
pub(crate) fn give(host: &mut dyn ServerHost, player: u128, stack: &ItemStack, count: u32) -> bool {
    let res = ItemResource::of(stack);
    let most = max_stack(&*host, stack);
    let mut left = count;
    let mut plan: Vec<(i32, u32)> = Vec::new();
    for pass in 0..2 {
        for s in 0..36 {
            if left == 0 || plan.iter().any(|(p, _)| *p == s) {
                continue;
            }
            let room = match (ItemPlace::PlayerSlot { player, slot: s }).stack_owned(host) {
                Some(n) if pass == 0 && n.is_stackable_with(stack) => most.saturating_sub(n.count()),
                None if pass == 1 => most,
                _ => 0,
            };
            let n = room.min(left);
            if n > 0 {
                plan.push((s, n));
                left -= n;
            }
        }
    }
    if left > 0 {
        return false;
    }
    host.transaction(|host, tx| {
        for (s, n) in &plan {
            if (ItemPlace::PlayerSlot { player, slot: *s }).insert(host, tx, &res, *n) != *n {
                return Err(());
            }
        }
        Ok(())
    })
    .is_ok()
}

// ---- Client

static CTRL: Mutex<bool> = Mutex::new(false);

/// The client's tick: Ctrl's state to the server while a terminal is open (for Ctrl + "+").
pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    let down = crate::screens::terminal_open() && (host.is_key_down("key.keyboard.left.control") || host.is_key_down("key.keyboard.right.control"));
    let mut last = CTRL.lock().unwrap_or_else(|e| e.into_inner());
    if *last != down {
        *last = down;
        crate::craft_screens::send(format!("ctrl {}", u8::from(down)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn network_first_then_inventory() {
        let mut net: HashMap<String, i64> = [("a:oak".to_owned(), 1)].into_iter().collect();
        let mut inv: HashMap<String, i64> = [("a:birch".to_owned(), 2), ("a:oak".to_owned(), 5)].into_iter().collect();
        let choices = ids(&["a:birch", "a:oak"]);
        // The network's oak before the inventory's birch.
        assert_eq!(pick_source(&choices, &mut net, &mut inv), Some(("a:oak".to_owned(), true)));
        // The network has no more: the inventory's first choice.
        assert_eq!(pick_source(&choices, &mut net, &mut inv), Some(("a:birch".to_owned(), false)));
        assert_eq!(pick_source(&choices, &mut net, &mut inv), Some(("a:birch".to_owned(), false)));
        assert_eq!(pick_source(&choices, &mut net, &mut inv), Some(("a:oak".to_owned(), false)));
        assert_eq!(pick_source(&ids(&["a:stone"]), &mut net, &mut inv), None);
    }

    #[test]
    fn craft_missing_list() {
        let missing = vec![ids(&["a:plank", "b:plank"]), ids(&["b:plank"]), ids(&["a:gem"])];
        let list = to_craft(&missing, &|id| id == "b:plank");
        assert_eq!(list, vec![("b:plank".to_owned(), 2)]);
    }
}
