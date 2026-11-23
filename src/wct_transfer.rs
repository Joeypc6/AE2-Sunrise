//! jeix: JEI's "+" on the wireless crafting terminal (AE2's UseCraftingRecipeTransfer for WirelessCraftingTermMenu and
//! FillCraftingGridFromRecipePacket), and Ctrl + "+" (craft what's missing), as `ct_transfer` does for the crafting
//! terminal block. The item's screen is a mod screen with no menu, so JEI's `register_screen_transfer` calls
//! `client_transfer`, which sends `jei <ctrl> <recipe id>` on the wireless channel (`wireless::NET_WIRELESS`).
//!
//! Server rules: the message waits in `wireless`'s INBOX for the player's tick (rate limited there), never handled in
//! the payload handler. Then: the player holds the wireless crafting terminal with its screen open (the session's
//! item in its slot, with its open tag), linked, powered and in range of a working access point (`wireless::checked`,
//! which closes the screen on a failure); the recipe exists and is a crafting recipe that fits the 3x3 grid
//! (`ct_transfer::layout`, by id: nothing but the id comes from the client). The grid's old contents go back first
//! (into the network, the rest into the inventory; if some can't go anywhere they stay and nothing moves), then each
//! cell takes one item its ingredient accepts from the network, else from the inventory. Every move is a storage
//! insert/extract or an inventory transaction, and the item's grid is written in one edit; a failed write puts what
//! was taken back, so nothing is made twice or lost, and no cell holds more than one.

use crate::cells::*;
use crate::core::KeyType;
use crate::storage::{self, Scope};
use crate::wireless::{self, Kind};
use mod_api::*;
use crate::util::PlaceCopy;
use std::collections::HashMap;

/// The message's head on the wireless channel.
pub(crate) const HEAD: &str = "jei";
/// The longest recipe id sent (the wireless channel takes 128 bytes a message).
pub(crate) const MAX_ID: usize = 120;

/// A recipe id the client may send (pure): `namespace:path`, vanilla's id characters, not too long.
pub(crate) fn valid_recipe_id(id: &str) -> bool {
    let Some((ns, path)) = id.split_once(':') else {
        return false;
    };
    !ns.is_empty()
        && !path.is_empty()
        && id.len() <= MAX_ID
        && ns
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.".contains(c))
        && path
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-./".contains(c))
}

/// The message the client sends (pure).
pub(crate) fn message(recipe: &str, ctrl: bool) -> Option<String> {
    valid_recipe_id(recipe).then(|| format!("{HEAD} {} {recipe}", u8::from(ctrl)))
}

/// The message's words after its head (pure): Ctrl, and the recipe id, when both are well formed.
pub(crate) fn parse(words: &[&str]) -> Option<(bool, String)> {
    if words.len() != 3 || words[0] != HEAD {
        return None;
    }
    let ctrl = match words[1] {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    valid_recipe_id(words[2]).then(|| (ctrl, words[2].to_owned()))
}

/// Where each cell's item comes from (pure): `(id, from the network)` per cell (None for an empty cell or one nothing
/// fills), and the choices of the cells nothing filled. The network first, then the inventory (`ct_transfer::
/// pick_source`); `network` and `inventory` are counted down.
pub(crate) fn plan_cells(
    cells: &[Option<Vec<String>>],
    network: &mut HashMap<String, i64>,
    inventory: &mut HashMap<String, i64>,
) -> (Vec<Option<(String, bool)>>, Vec<Vec<String>>) {
    let mut plan = Vec::with_capacity(9);
    let mut missing = Vec::new();
    for cell in cells.iter().take(9) {
        match cell {
            None => plan.push(None),
            Some(choices) => match crate::ct_transfer::pick_source(choices, network, inventory) {
                Some(p) => plan.push(Some(p)),
                None => {
                    missing.push(choices.clone());
                    plan.push(None);
                }
            },
        }
    }
    while plan.len() < 9 {
        plan.push(None);
    }
    (plan, missing)
}

/// The wireless channel's `jei` message, in the player's tick.
pub(crate) fn fill(host: &mut dyn ServerHost, player: u128, ctrl: bool, recipe: &str) {
    if !valid_recipe_id(recipe) {
        return;
    }
    let Some((s, stack)) = wireless::checked(host, player) else {
        return;
    };
    if s.kind != Kind::WirelessCrafting {
        return;
    }
    let Some(wap) = s.wap else { return };
    let Some(cells) = crate::ct_transfer::layout(&*host, recipe) else {
        return;
    };
    let scope = Scope::Grid(wap);

    // The old grid back: off the item in one edit, then into the network, the rest into the inventory.
    let old = wireless::grid_of(&*host, &stack);
    if old.iter().any(Option::is_some) {
        if !wireless::edit_held(host, player, &s, |st| wireless::set_grid(st, None)) {
            return;
        }
        let mut left: Vec<Option<ItemStack>> = vec![None; 9];
        for (i, cell) in old.iter().enumerate() {
            let Some(st) = cell else { continue };
            let key = key_of(&*host, st);
            let count = st.count();
            let stored = storage::insert(host, scope, None, &key, i64::from(count))
                .clamp(0, i64::from(count)) as u32;
            let rest = count - stored;
            if rest > 0 && !crate::ct_transfer::give(host, player, st, rest) {
                left[i] = with_count(st, rest);
            }
        }
        if left.iter().any(Option::is_some) {
            // No room for all of it: what is left goes back into the grid, and nothing moves (as the block's).
            let tag = wireless::grid_tag(&*host, &left);
            if !wireless::edit_held(host, player, &s, |st| wireless::set_grid(st, tag)) {
                for st in left.into_iter().flatten() {
                    wireless::give_back(host, player, st);
                }
            }
            return;
        }
    }

    // What the network and the inventory hold of the choices (not the terminal's own slot).
    let wanted: std::collections::HashSet<String> =
        cells.iter().flatten().flatten().cloned().collect();
    let mut network: HashMap<String, i64> = HashMap::new();
    for (k, n) in storage::contents(host, scope, None) {
        if k.key_type() == KeyType::Items && k.components().is_empty() && wanted.contains(k.id()) {
            *network.entry(k.id().to_owned()).or_default() += n;
        }
    }
    let mut inventory: HashMap<String, i64> = HashMap::new();
    let mut inv_slots: Vec<(i32, ItemStack, u32)> = Vec::new();
    for slot in (0..36).filter(|slot| *slot != s.slot) {
        if let Some(st) = (ItemPlace::PlayerSlot { player, slot }).stack_owned(host)
            && wanted.contains(sid(&st))
        {
            *inventory.entry(sid(&st).to_owned()).or_default() += i64::from(st.count());
            let n = st.count();
            inv_slots.push((slot, st, n));
        }
    }
    let (plan, mut missing) = plan_cells(&cells, &mut network, &mut inventory);
    let mut grid: Vec<Option<ItemStack>> = vec![None; 9];
    for (i, p) in plan.iter().enumerate() {
        let Some((id, from_network)) = p else {
            continue;
        };
        let got = if *from_network {
            new_stack(&*host, id, 1).and_then(|one| {
                let key = key_of(&*host, &one);
                (storage::extract(host, scope, None, &key, 1) == 1).then_some(one)
            })
        } else {
            crate::ct_transfer::take_one(host, player, &mut inv_slots, id)
        };
        match got {
            Some(one) => grid[i] = Some(one),
            None => missing.extend(cells.get(i).cloned().flatten()),
        }
    }
    if grid.iter().any(Option::is_some) {
        let tag = wireless::grid_tag(&*host, &grid);
        if !wireless::edit_held(host, player, &s, |st| wireless::set_grid(st, tag)) {
            // The item went (it was checked this tick: not expected): everything back where it can go.
            for one in grid.into_iter().flatten() {
                let key = key_of(&*host, &one);
                if storage::insert(host, scope, None, &key, 1) < 1 {
                    wireless::give_back(host, player, one);
                }
            }
            return;
        }
    }
    // Ctrl: the missing ingredients the network can craft, one plan after the other (CraftConfirmMenu).
    if ctrl && !missing.is_empty() {
        let dim = host.dimension();
        let craftables: std::collections::HashSet<String> = crate::autocraft::craftables(dim, wap)
            .0
            .into_iter()
            .filter(|k| k.key_type() == KeyType::Items && k.components().is_empty())
            .map(|k| k.id().to_owned())
            .collect();
        let list = crate::ct_transfer::to_craft(&missing, &|id| craftables.contains(id));
        if !list.is_empty() {
            let mut text = format!("{}\n", crate::ct_transfer::CRAFT_LIST);
            for (id, n) in list {
                let Some(st) = new_stack(&*host, &id, 1) else {
                    continue;
                };
                text.push_str(&format!("{n} {}\n", key_to_string(&key_of(&*host, &st))));
            }
            let _ = crate::util::send_to_player(
                host,
                player,
                crate::autocraft::NET_PLAN,
                text.as_bytes(),
            );
        }
    }
}

// ---- Client

/// JEI's question: is the open item screen the wireless crafting terminal's.
pub(crate) fn client_open(host: &dyn ClientHost) -> bool {
    wireless::open_kind_of(host) == Some(Kind::WirelessCrafting)
}

/// JEI's "+" on the wireless crafting terminal: the recipe id (and Ctrl, for craft missing) to the server. Shift
/// (`_max`) moves one set, as AE2's terminal does.
pub(crate) fn client_transfer(host: &mut dyn ClientHost, recipe: &str, _max: bool) {
    if !client_open(&*host) {
        return;
    }
    let ctrl = host.is_key_down("key.keyboard.left.control")
        || host.is_key_down("key.keyboard.right.control");
    if let Some(msg) = message(recipe, ctrl) {
        let _ = crate::util::send_to_server(host, wireless::NET_WIRELESS, msg.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn recipe_ids_from_the_client() {
        assert!(valid_recipe_id("minecraft:oak_planks"));
        assert!(valid_recipe_id("ae2:network/cables/glass_fluix"));
        assert!(!valid_recipe_id("oak_planks"));
        assert!(!valid_recipe_id("minecraft:"));
        assert!(!valid_recipe_id(":x"));
        assert!(!valid_recipe_id("Minecraft:Oak"));
        assert!(!valid_recipe_id("a:b c"));
        assert!(!valid_recipe_id("a:../x\n"));
        assert!(!valid_recipe_id(&format!("a:{}", "x".repeat(MAX_ID))));
        // The longest message fits the wireless channel's 128 bytes.
        let longest = format!("a:{}", "x".repeat(MAX_ID - 2));
        assert!(message(&longest, true).is_some_and(|m| m.len() <= 128));
    }

    #[test]
    fn message_round_trip() {
        let m = message("minecraft:crafting_table", true).unwrap();
        assert_eq!(m, "jei 1 minecraft:crafting_table");
        let w: Vec<&str> = m.split_whitespace().collect();
        assert_eq!(
            parse(&w),
            Some((true, "minecraft:crafting_table".to_owned()))
        );
        assert_eq!(
            parse(&["jei", "0", "minecraft:stick"]),
            Some((false, "minecraft:stick".to_owned()))
        );
        assert_eq!(parse(&["jei", "2", "minecraft:stick"]), None);
        assert_eq!(parse(&["jei", "1"]), None);
        assert_eq!(parse(&["jei", "1", "minecraft:stick", "x"]), None);
        assert_eq!(parse(&["jei", "1", "bad id"]), None);
        assert_eq!(message("bad", false), None);
    }

    /// Each cell takes one item, the network's first; what nothing has is missing; empty cells stay empty.
    #[test]
    fn grid_fill_plan() {
        let cells = vec![
            Some(ids(&["a:plank"])),
            Some(ids(&["a:plank"])),
            None,
            Some(ids(&["a:gem", "a:plank"])),
            Some(ids(&["a:stone"])),
            None,
            None,
            None,
            None,
        ];
        let mut net: HashMap<String, i64> = [("a:plank".to_owned(), 1)].into_iter().collect();
        let mut inv: HashMap<String, i64> = [("a:plank".to_owned(), 2)].into_iter().collect();
        let (plan, missing) = plan_cells(&cells, &mut net, &mut inv);
        assert_eq!(plan.len(), 9);
        assert_eq!(plan[0], Some(("a:plank".to_owned(), true)));
        assert_eq!(plan[1], Some(("a:plank".to_owned(), false)));
        assert_eq!(plan[2], None);
        assert_eq!(plan[3], Some(("a:plank".to_owned(), false)));
        assert_eq!(plan[4], None);
        assert_eq!(missing, vec![ids(&["a:stone"])]);
        // Nothing is counted twice: all three planks used, none left.
        assert_eq!(net["a:plank"] + inv["a:plank"], 0);
        assert!(plan.iter().flatten().count() <= 9);
    }
}
