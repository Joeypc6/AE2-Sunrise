//! JEI's "+" on the pattern encoding terminal (Sunrise's `menu_transfer`, server side): AE2's
//! EncodePatternTransferHandler and EncodingHelper. Nothing moves: the recipe is written into the encoder's cells as
//! ghosts, in the mode it needs. A crafting recipe (3x3 at most) fills the crafting grid as JEI lays it out, a
//! smithing recipe the smithing cells, a stonecutting recipe the stonecutter's cell; any other recipe is encoded as
//! processing (its inputs into the nine input cells, its results into the three output cells, with their amounts).
//! Each cell takes the ingredient's first item the network stores (EncodingHelper's priority), else its first item.
//! The recipe is looked up by its id on the server; the client sends nothing else.

use crate::cells::*;
use crate::core::{AeKey, KeyType};
use crate::storage::{self, Scope};
use crate::util::*;
use mod_api::serde_json;
use mod_api::*;

/// The encoder's cells: 9 crafting (or processing inputs), then 3 processing outputs.
const CELLS: usize = crate::autocraft::ENC_CELLS;

/// The JEI categories whose "+" writes into the encoder: the ones `encoding_of` reads. Read only by the JEI
/// registration, which builds with the `jei` feature alone.
#[allow(dead_code)]
pub(crate) const JEI_CATEGORIES: [&str; 9] = [
    "minecraft:crafting",
    "minecraft:smelting",
    "minecraft:blasting",
    "minecraft:smoking",
    "minecraft:campfire_cooking",
    "minecraft:stonecutting",
    "minecraft:smithing",
    "ae2:inscriber",
    "ae2:charger",
];

/// What a recipe encodes as: the mode (0 crafting, 1 processing, 2 smithing, 3 stonecutting) and each cell's
/// choices (item ids in the ingredient's order) with their amount.
#[derive(Debug, PartialEq)]
pub(crate) struct Encoding {
    pub mode: i64,
    pub cells: Vec<Option<(Vec<String>, i64)>>,
}

/// JEI's CraftingGridHelper.getCraftingIndex: where the `i`th input of a `width` x `height` recipe sits in the 3x3
/// grid (narrow and short recipes are centred).
pub(crate) fn crafting_index(i: usize, width: usize, height: usize) -> usize {
    if width == 1 {
        if height >= 2 { i * 3 + 1 } else { 4 }
    } else if height == 1 {
        i + 3
    } else if width == 2 {
        let mut index = i;
        if i > 1 {
            index += 1;
            if i > 3 {
                index += 1;
            }
        }
        index
    } else if height == 2 {
        i + 3
    } else {
        i
    }
}

/// JEI's shapeless layout: up to one input in a 1x1 grid, up to four in 2x2, else 3x3.
pub(crate) fn shapeless_size(n: usize) -> usize {
    if n > 4 {
        3
    } else if n > 1 {
        2
    } else {
        1
    }
}

/// A shaped recipe's file: its pattern's cells in the 3x3 grid, each with its key's ingredient JSON. None when the
/// pattern is larger than 3x3 ("recipe too large").
pub(crate) fn shaped_cells(json: &serde_json::Value) -> Option<Vec<(usize, String)>> {
    let rows: Vec<&str> = json.get("pattern")?.as_array()?.iter().filter_map(|r| r.as_str()).collect();
    let height = rows.len();
    let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    if height == 0 || height > 3 || width > 3 {
        return None;
    }
    let key = json.get("key")?.as_object()?;
    let mut out = Vec::new();
    for (y, row) in rows.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let ingredient = key.get(&ch.to_string())?;
            out.push((crafting_index(y * width + x, width, height), ingredient.to_string()));
        }
    }
    Some(out)
}

/// The recipe `id` as the encoder takes it, looked up among the types a viewer shows for this terminal.
pub(crate) fn encoding_of(host: &dyn Host, id: &str) -> Option<Encoding> {
    let one = |i: &Ingredient| Some((i.order.clone(), 1));
    let mut cells: Vec<Option<(Vec<String>, i64)>> = vec![None; CELLS];
    for kind in ["minecraft:crafting", "minecraft:crafting_shaped", "minecraft:crafting_shapeless"] {
        let recipes = host.recipes_of_type(kind);
        let Some(r) = recipes.iter().find(|r| r.id == id) else { continue };
        if r.rule.is_some() || r.ingredients.is_empty() {
            // A special recipe (dyes, banners...): nothing to lay out.
            return None;
        }
        if r.json.get("pattern").is_some() {
            for (at, json) in shaped_cells(&r.json)? {
                let ingredient = host.recipe_ingredient(&json).ok().flatten()?;
                cells[at] = one(&ingredient);
            }
        } else {
            if r.ingredients.len() > 9 {
                return None;
            }
            let size = shapeless_size(r.ingredients.len());
            for (i, ingredient) in r.ingredients.iter().enumerate() {
                cells[crafting_index(i, size, size)] = one(ingredient);
            }
        }
        return Some(Encoding { mode: 0, cells });
    }
    for kind in ["minecraft:stonecutting", "minecraft:smithing"] {
        let recipes = host.recipes_of_type(kind);
        let Some(r) = recipes.iter().find(|r| r.id == id) else { continue };
        match r.rule.as_ref()? {
            RecipeRule::Stonecutting { input, .. } => {
                cells[0] = one(input);
                return Some(Encoding { mode: 3, cells });
            }
            RecipeRule::SmithingTransform { template, base, addition, .. } => {
                cells[0] = template.as_ref().and_then(one);
                cells[1] = one(base);
                cells[2] = addition.as_ref().and_then(one);
                return Some(Encoding { mode: 2, cells });
            }
            RecipeRule::SmithingTrim { template, base, addition, .. } => {
                cells[0] = one(template);
                cells[1] = one(base);
                cells[2] = one(addition);
                return Some(Encoding { mode: 2, cells });
            }
            _ => return None,
        }
    }
    // Processing: the cooking types' typed parts, and AE2's own inscriber and charger files.
    for kind in ["minecraft:smelting", "minecraft:blasting", "minecraft:smoking", "minecraft:campfire_cooking"] {
        let recipes = host.recipes_of_type(kind);
        let Some(r) = recipes.iter().find(|r| r.id == id) else { continue };
        let input = r.ingredients.first()?;
        let output = r.results.first()?;
        cells[0] = one(input);
        cells[9] = Some((vec![crate::cells::sid(output).to_owned()], i64::from(output.count())));
        return Some(Encoding { mode: 1, cells });
    }
    for kind in ["ae2:inscriber", "ae2:charger"] {
        let recipes = host.recipes_of_type(kind);
        let Some(r) = recipes.iter().find(|r| r.id == id) else { continue };
        let j = &r.json;
        let inputs: Vec<&serde_json::Value> = match kind {
            "ae2:inscriber" => {
                let ing = j.get("ingredients")?;
                ["top", "middle", "bottom"].iter().filter_map(|k| ing.get(*k)).collect()
            }
            _ => vec![j.get("ingredient")?],
        };
        for (i, v) in inputs.into_iter().enumerate() {
            let ingredient = host.recipe_ingredient(&v.to_string()).ok().flatten()?;
            cells[i] = one(&ingredient);
        }
        let result = host.recipe_result(&j.get("result")?.to_string()).ok().flatten()?;
        cells[9] = Some((vec![crate::cells::sid(&result).to_owned()], i64::from(result.count())));
        return Some(Encoding { mode: 1, cells });
    }
    None
}

/// `menu_transfer` on a pattern encoding terminal at `pos` (a block, or a part on a cable bus at its hosted
/// position).
pub(crate) fn encoder(host: &mut dyn ServerHost, pos: Pos, recipe: &str, dry_run: bool) -> RecipeTransferResult {
    let Some(encoding) = encoding_of(&*host, recipe) else { return RecipeTransferResult::NotIntoMenu };
    if dry_run {
        return RecipeTransferResult::Moves;
    }
    let Some(level) = here(&*host) else { return RecipeTransferResult::NotIntoMenu };
    // EncodingHelper's priority: an item the network stores, else the ingredient's first.
    let stored: std::collections::HashSet<String> =
        storage::contents(host, Scope::Grid(pos), None).into_iter().filter(|(k, _)| k.key_type() == KeyType::Items).map(|(k, _)| k.id().to_owned()).collect();
    let cells: Vec<Option<(AeKey, i64)>> = encoding
        .cells
        .iter()
        .map(|c| {
            let (choices, amount) = c.as_ref()?;
            let id = choices.iter().find(|i| stored.contains(*i)).or_else(|| choices.first())?;
            let stack = ItemStack::new(item_key(&*host, id)?, 1)?;
            Some((crate::parts::ghost_key(&*host, &stack), *amount))
        })
        .collect();
    let Some(mut nbt) = crate::bus::nbt_at(&*host, level, pos) else { return RecipeTransferResult::NotIntoMenu };
    set_nbt_i64(&mut nbt, "mode", encoding.mode);
    crate::parts::set_ghosts(&mut nbt, &cells);
    // enc: a stonecutting recipe is the stonecutter's pick too (EncodePatternTransferHandler).
    if encoding.mode == 3 {
        nbt.insert(java("stonecut"), Tag::String(java(recipe)));
    }
    if crate::bus::set_nbt_at(host, level, pos, nbt) { RecipeTransferResult::Moves } else { RecipeTransferResult::NotIntoMenu }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jei_centres_narrow_recipes() {
        // A stick (1x2) sits in the middle column, a 2x2 in the top left, a slab (3x1) in the middle row.
        assert_eq!([crafting_index(0, 1, 2), crafting_index(1, 1, 2)], [1, 4]);
        assert_eq!((0..4).map(|i| crafting_index(i, 2, 2)).collect::<Vec<_>>(), vec![0, 1, 3, 4]);
        assert_eq!((0..3).map(|i| crafting_index(i, 3, 1)).collect::<Vec<_>>(), vec![3, 4, 5]);
        assert_eq!(crafting_index(0, 1, 1), 4);
        assert_eq!(crafting_index(5, 3, 2), 8);
        assert_eq!(crafting_index(8, 3, 3), 8);
    }

    #[test]
    fn shapeless_sizes() {
        assert_eq!([shapeless_size(1), shapeless_size(2), shapeless_size(4), shapeless_size(5), shapeless_size(9)], [1, 2, 2, 3, 3]);
    }

    #[test]
    fn shaped_pattern_cells() {
        let json: serde_json::Value = serde_json::json!({
            "pattern": ["#", "#"],
            "key": {"#": "minecraft:oak_planks"}
        });
        let cells = shaped_cells(&json).unwrap();
        assert_eq!(cells.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![1, 4]);
        assert_eq!(cells[0].1, "\"minecraft:oak_planks\"");
        let gaps: serde_json::Value = serde_json::json!({"pattern": ["a a", " a "], "key": {"a": "minecraft:stone"}});
        assert_eq!(shaped_cells(&gaps).unwrap().iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![3, 5, 7]);
        let big: serde_json::Value = serde_json::json!({"pattern": ["aaaa"], "key": {"a": "minecraft:stone"}});
        assert_eq!(shaped_cells(&big), None);
    }
}
