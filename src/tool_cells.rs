//! The tools that are storage cells (IBasicCellItem): the matter cannon (MatterCannonItem: one type of ammo, 512 bytes,
//! 8 a type) and the colour applicator (ColorApplicatorItem: 27 types of paint, 2048 bytes, 32 a type). They go in a
//! drive or an ME chest to be filled (and their tooltip shows bytes and types as a cell's), each keeping out what it
//! can't use (`isBlackListed`); their cards go on in the cell workbench (`Upgrades.add` for each, InitUpgrades).
//!
//! The matter cannon's ammo and weights are AE2's matter-cannon ammo data (`data/ae2/recipes/matter_cannon/`,
//! MatterCannonAmmoProvider); a data pack's `ae2:matter_cannon` recipes are read too. Which items a cell takes is
//! decided from item tags, so the answer is kept per item id, worked out on the server's tick (`refresh`) and read
//! where a cell is mounted (`cells::cell_inventory`, which has no host).

use crate::core::{CellType, KeyType};
use mod_api::*;
use std::collections::HashMap;
use std::sync::RwLock;

pub(crate) const CANNON: &str = "ae2:matter_cannon";
pub(crate) const APPLICATOR: &str = "ae2:color_applicator";

/// MatterCannonAmmoProvider: the matter ball and every `c:<metal>_nuggets` tag, by atomic weight.
pub(crate) const CANNON_AMMO: &[(&str, f32)] = &[
    ("ae2:matter_ball", 32.0),
    ("#c:aluminum_nuggets", 26.9815),
    ("#c:antimony_nuggets", 121.76),
    ("#c:barium_nuggets", 137.327),
    ("#c:beef_nuggets", 32.0),
    ("#c:beryllium_nuggets", 9.0122),
    ("#c:bismuth_nuggets", 208.9804),
    ("#c:boron_nuggets", 10.811),
    ("#c:bromine_nuggets", 79.904),
    ("#c:cadmium_nuggets", 112.411),
    ("#c:calcium_nuggets", 40.078),
    ("#c:carbon_nuggets", 12.0107),
    ("#c:cerium_nuggets", 140.116),
    ("#c:cesium_nuggets", 132.9055),
    ("#c:charcoal_nuggets", 12.0107),
    ("#c:chicken_nuggets", 32.0),
    ("#c:coal_nuggets", 12.0107),
    ("#c:cobalt_nuggets", 58.9332),
    ("#c:copper_nuggets", 63.546),
    ("#c:electrum_nuggets", 152.41737),
    ("#c:fish_nuggets", 32.0),
    ("#c:gallium_nuggets", 69.723),
    ("#c:germanium_nuggets", 72.64),
    ("#c:gold_nuggets", 196.96655),
    ("#c:indium_nuggets", 114.818),
    ("#c:invar_nuggets", 56.794468),
    ("#c:iodine_nuggets", 126.9045),
    ("#c:iridium_nuggets", 192.217),
    ("#c:iron_nuggets", 55.845),
    ("#c:krypton_nuggets", 83.8),
    ("#c:lanthanum_nuggets", 138.9055),
    ("#c:lead_nuggets", 207.2),
    ("#c:lithium_nuggets", 6.941),
    ("#c:magnesium_nuggets", 24.305),
    ("#c:manganese_nuggets", 54.938),
    ("#c:meat_nuggets", 32.0),
    ("#c:meatcooked_nuggets", 32.0),
    ("#c:meatraw_nuggets", 32.0),
    ("#c:nickel_nuggets", 58.6934),
    ("#c:niobium_nuggets", 92.9064),
    ("#c:osmium_nuggets", 190.23),
    ("#c:palladium_nuggets", 106.42),
    ("#c:phosphorus_nuggets", 30.9738),
    ("#c:platinum_nuggets", 195.078),
    ("#c:plutonium_nuggets", 244.0),
    ("#c:potassium_nuggets", 39.0983),
    ("#c:rhodium_nuggets", 102.9055),
    ("#c:rubidium_nuggets", 85.4678),
    ("#c:ruthenium_nuggets", 101.07),
    ("#c:scandium_nuggets", 44.9559),
    ("#c:sheep_nuggets", 32.0),
    ("#c:silicon_nuggets", 28.0855),
    ("#c:silver_nuggets", 107.8682),
    ("#c:sodium_nuggets", 22.9897),
    ("#c:strontium_nuggets", 87.62),
    ("#c:sulfur_nuggets", 32.065),
    ("#c:tantalum_nuggets", 180.9479),
    ("#c:technetium_nuggets", 98.0),
    ("#c:tellurium_nuggets", 127.6),
    ("#c:tin_nuggets", 118.71),
    ("#c:titanium_nuggets", 47.867),
    ("#c:tungsten_nuggets", 183.84),
    ("#c:uranium_nuggets", 238.0289),
    ("#c:vanadium_nuggets", 50.9415),
    ("#c:xenon_nuggets", 131.293),
    ("#c:yttrium_nuggets", 88.9059),
    ("#c:zinc_nuggets", 65.39),
    ("#c:zirconium_nuggets", 91.224),
];

/// The cell each tool is (IBasicCellItem's bytes, bytes a type, types, idle drain 0.5).
pub(crate) fn cell_type(item: &str) -> Option<CellType> {
    let (total_bytes, bytes_per_type, total_types) = match item {
        CANNON => (512, 8, 1),
        // StorageTier.SIZE_4K.bytes() / 2 and / 128.
        APPLICATOR => (2048, 32, 27),
        _ => return None,
    };
    Some(CellType {
        key_type: KeyType::Items,
        total_bytes,
        bytes_per_type,
        total_types,
        idle_drain: 0.5,
        storable_in_storage_cell: true,
    })
}

// ---- What each takes (kept per item id)

#[derive(Default)]
struct Known {
    /// An item's AE colour for the applicator (`ColorApplicatorItem.getColorFromItem`), from the `c:dyes/<colour>` tags.
    dye_tags: HashMap<String, usize>,
    /// The data's `ae2:matter_cannon` recipes and the tags the table names, resolved (item id -> weight).
    ammo: HashMap<String, f32>,
}

static KNOWN: RwLock<Option<Known>> = RwLock::new(None);

/// Works out the ammo and dye tables with the server's tags and recipes (each second; tags change on `/reload`).
pub(crate) fn refresh(host: &dyn Host) {
    crate::cells::ids(host);
    let tags = host.tags();
    let mut ammo: HashMap<String, f32> = HashMap::new();
    let add =
        |entry: &str, weight: f32, ammo: &mut HashMap<String, f32>| match entry.strip_prefix('#') {
            Some(tag) => {
                if let Ok(id) = Identifier::parse(tag)
                    && let Some(key) = tags.key(&id)
                {
                    for item in tag_items(tags.items(key)) {
                        ammo.entry(crate::cells::key_id(item).to_owned())
                            .or_insert(weight);
                    }
                }
            }
            None => {
                ammo.entry(entry.to_owned()).or_insert(weight);
            }
        };
    // A data pack's recipes first (MatterCannonAmmo: `ammo` an item or a tag, `weight`).
    for r in host.recipes_of_type(CANNON).iter() {
        let Some(weight) = r.json.get("weight").and_then(|w| w.as_f64()) else {
            continue;
        };
        let Some(a) = r.json.get("ammo") else {
            continue;
        };
        if let Some(i) = a.get("item").and_then(|v| v.as_str()) {
            add(i, weight as f32, &mut ammo);
        } else if let Some(t) = a.get("tag").and_then(|v| v.as_str()) {
            add(&format!("#{t}"), weight as f32, &mut ammo);
        }
    }
    for (entry, weight) in CANNON_AMMO {
        add(entry, *weight, &mut ammo);
    }
    let mut dye_tags = HashMap::new();
    for (c, name) in crate::paint::COLORS.iter().enumerate().take(16) {
        if let Ok(id) = Identifier::parse(&format!("c:dyes/{name}"))
            && let Some(key) = tags.key(&id)
        {
            for item in tag_items(tags.items(key)) {
                dye_tags
                    .entry(crate::cells::key_id(item).to_owned())
                    .or_insert(c);
            }
        }
    }
    let mut g = KNOWN.write().unwrap_or_else(|e| e.into_inner());
    *g = Some(Known { dye_tags, ammo });
}

pub(crate) fn forget_all() {
    *KNOWN.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// MatterCannonItem.getPenetration: the ammo's weight (0 for paint balls and anything else).
pub(crate) fn ammo_weight(item: &str) -> f32 {
    let g = KNOWN.read().unwrap_or_else(|e| e.into_inner());
    let Some(k) = g.as_ref() else {
        return if item == "ae2:matter_ball" { 32.0 } else { 0.0 };
    };
    k.ammo.get(item).copied().unwrap_or(0.0)
}

/// ColorApplicatorItem.getColorFromItem: a snowball is TRANSPARENT, a paint ball its colour, a vanilla dye or anything in
/// a `c:dyes/<colour>` tag that colour.
pub(crate) fn paint_color(item: &str) -> Option<usize> {
    if item == "minecraft:snowball" {
        return Some(crate::paint::TRANSPARENT);
    }
    if let Some((c, _)) = crate::paint::paint_ball(item) {
        return Some(c);
    }
    if let Some(c) = item
        .strip_prefix("minecraft:")
        .and_then(|n| n.strip_suffix("_dye"))
        .and_then(crate::paint::color_index)
        .filter(|c| *c < 16)
    {
        return Some(c);
    }
    let g = KNOWN.read().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|k| k.dye_tags.get(item).copied())
}

/// `isBlackListed`'s other side: what each tool's cell takes. The cannon: ammo (a weight above 0) or a paint ball; the
/// applicator: anything with a colour.
pub(crate) fn accepts(tool: &str, item: &str) -> bool {
    match tool {
        CANNON => ammo_weight(item) > 0.0 || crate::paint::paint_ball(item).is_some(),
        APPLICATOR => paint_color(item).is_some(),
        _ => true,
    }
}

// ---- Cards (the cell workbench and the item screens)

/// The items the cell workbench takes beside the storage cells and view cells (ICellWorkbenchItem: the portable cells,
/// the matter cannon, the colour applicator; a wireless terminal is no ICellWorkbenchItem, its own screen takes its cards).
pub(crate) fn workbench_takes(item: &str) -> bool {
    item == CANNON || item == APPLICATOR || crate::portable::portable_type(item).is_some()
}

/// The upgrade slots of what the cell workbench holds (each item's `getUpgrades`).
pub(crate) fn upgrade_slots(item: &str) -> Option<usize> {
    match item {
        CANNON => Some(4),
        APPLICATOR => Some(2),
        i if i.starts_with("ae2:portable_item_cell_") => Some(4),
        i if i.starts_with("ae2:portable_fluid_cell_") => Some(3),
        "ae2:wireless_terminal" | "ae2:wireless_crafting_terminal" => Some(2),
        _ => None,
    }
}

/// InitUpgrades: how many of `card` each item takes (0: none). Storage cells: fuzzy (items only), inverter, equal
/// distribution and void, one each.
pub(crate) fn card_limit(item: &str, card: &str) -> usize {
    let one_of = |cards: &[&str]| usize::from(cards.contains(&card));
    match item {
        CANNON => match card {
            "ae2:energy_card" => 2,
            "ae2:speed_card" => 4,
            _ => one_of(&["ae2:fuzzy_card", "ae2:inverter_card", "ae2:void_card"]),
        },
        APPLICATOR => match card {
            "ae2:energy_card" => 2,
            _ => one_of(&["ae2:equal_distribution_card", "ae2:void_card"]),
        },
        "ae2:wireless_terminal" | "ae2:wireless_crafting_terminal" => {
            if card == "ae2:energy_card" {
                2
            } else {
                0
            }
        }
        i if i.starts_with("ae2:portable_item_cell_") => match card {
            "ae2:energy_card" => 2,
            _ => one_of(&[
                "ae2:fuzzy_card",
                "ae2:inverter_card",
                "ae2:equal_distribution_card",
                "ae2:void_card",
            ]),
        },
        i if i.starts_with("ae2:portable_fluid_cell_") => match card {
            "ae2:energy_card" => 2,
            _ => one_of(&[
                "ae2:inverter_card",
                "ae2:equal_distribution_card",
                "ae2:void_card",
            ]),
        },
        "ae2:view_cell" => one_of(&["ae2:fuzzy_card", "ae2:inverter_card"]),
        i => match crate::cells::cell_type(i) {
            Some(t) if t.key_type == KeyType::Fluids => one_of(&[
                "ae2:inverter_card",
                "ae2:equal_distribution_card",
                "ae2:void_card",
            ]),
            Some(_) => one_of(&[
                "ae2:fuzzy_card",
                "ae2:inverter_card",
                "ae2:equal_distribution_card",
                "ae2:void_card",
            ]),
            None => 0,
        },
    }
}

/// The cards from the workbench's slots that go on `item`: in slot order, as many as it has slots, each card no more
/// than the item takes of it.
pub(crate) fn cards_for(item: &str, offered: &[String], slots: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in offered.iter().filter(|c| !c.is_empty()) {
        if out.len() >= slots {
            break;
        }
        let have = out.iter().filter(|x| *x == c).count();
        if have < card_limit(item, c) {
            out.push(c.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_cells() {
        let c = cell_type(CANNON).expect("a cell");
        assert_eq!(
            (c.total_bytes, c.bytes_per_type, c.total_types),
            (512, 8, 1)
        );
        let a = cell_type(APPLICATOR).expect("a cell");
        assert_eq!(
            (a.total_bytes, a.bytes_per_type, a.total_types),
            (2048, 32, 27)
        );
        assert!(cell_type("ae2:charged_staff").is_none());
    }

    #[test]
    fn ammo_and_paint() {
        assert_eq!(ammo_weight("ae2:matter_ball"), 32.0);
        assert!(
            CANNON_AMMO
                .iter()
                .any(|(e, w)| *e == "#c:gold_nuggets" && (*w - 196.96655).abs() < 1e-4)
        );
        assert!(accepts(CANNON, "ae2:matter_ball"));
        assert!(accepts(CANNON, "ae2:blue_lumen_paint_ball"));
        assert!(!accepts(CANNON, "minecraft:dirt"));
        assert_eq!(paint_color("minecraft:snowball"), Some(16));
        assert_eq!(paint_color("minecraft:light_gray_dye"), Some(8));
        assert_eq!(paint_color("ae2:black_paint_ball"), Some(15));
        assert_eq!(paint_color("minecraft:stick"), None);
        assert!(
            accepts(APPLICATOR, "minecraft:red_dye") && !accepts(APPLICATOR, "ae2:matter_ball")
        );
    }

    #[test]
    fn card_limits() {
        assert_eq!(card_limit(CANNON, "ae2:speed_card"), 4);
        assert_eq!(card_limit(CANNON, "ae2:energy_card"), 2);
        assert_eq!(card_limit(CANNON, "ae2:fuzzy_card"), 1);
        assert_eq!(card_limit(CANNON, "ae2:equal_distribution_card"), 0);
        assert_eq!(card_limit(APPLICATOR, "ae2:void_card"), 1);
        assert_eq!(card_limit(APPLICATOR, "ae2:fuzzy_card"), 0);
        assert_eq!(
            card_limit("ae2:portable_item_cell_4k", "ae2:energy_card"),
            2
        );
        assert_eq!(
            card_limit("ae2:portable_fluid_cell_4k", "ae2:fuzzy_card"),
            0
        );
        assert_eq!(card_limit("ae2:item_storage_cell_1k", "ae2:fuzzy_card"), 1);
        assert_eq!(card_limit("ae2:fluid_storage_cell_1k", "ae2:fuzzy_card"), 0);
        assert_eq!(card_limit("ae2:item_storage_cell_1k", "ae2:speed_card"), 0);
        let offered: Vec<String> = [
            "ae2:speed_card",
            "ae2:speed_card",
            "ae2:fuzzy_card",
            "ae2:fuzzy_card",
            "ae2:energy_card",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            cards_for(CANNON, &offered, 4),
            vec![
                "ae2:speed_card",
                "ae2:speed_card",
                "ae2:fuzzy_card",
                "ae2:energy_card"
            ]
        );
        assert_eq!(cards_for(APPLICATOR, &offered, 2), vec!["ae2:energy_card"]);
        assert!(
            workbench_takes(CANNON)
                && workbench_takes("ae2:portable_fluid_cell_1k")
                && !workbench_takes("ae2:wireless_terminal")
        );
    }
}

/// A tag's members, in order: none for a tag that is missing or not ready yet (as the list read gave none).
fn tag_items(members: TagMembers<ItemKey>) -> Vec<ItemKey> {
    match members {
        TagMembers::Members(keys) => keys,
        _ => Vec::new(),
    }
}
