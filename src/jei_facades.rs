//! jeix: AE2's facades in JEI's item list (JEIPlugin.registerIngredients with FacadeItemDefinitions: one facade per
//! block that may be a facade), unless the client option `enableFacadesInJEI` is off. The blocks are the ones AE2
//! makes facades of: the server's facade crafting recipes (`data/ae2/recipe/hz_facade/*`, four cable anchors round
//! the block), whose results are the facade stacks themselves (`minecraft:custom_data.item` names the block, as
//! `bus::place` reads it, and `minecraft:item_model` draws it). Listed after the plain facade, in the block items'
//! registry order, each searchable as "Cable Facade - <block>". `jei_extra` registers them; this part is plain code.

use mod_api::serde_json::Value;
use mod_api::*;

pub(crate) const FACADE: &str = "ae2:facade";
/// The component JEI tells facades apart by (`register_subtype`).
pub(crate) const SUBTYPE: &str = "minecraft:custom_data";

/// A facade recipe's block and its result's components as compact JSON (pure): None for any other recipe.
pub(crate) fn facade_of_recipe(json: &Value) -> Option<(String, String)> {
    let result = json.get("result")?;
    if result
        .get("id")
        .or_else(|| result.get("item"))
        .and_then(Value::as_str)
        != Some(FACADE)
    {
        return None;
    }
    let components = result.get("components")?;
    let block = components.get(SUBTYPE)?.get("item")?.as_str()?;
    if !block.contains(':') || block.len() > 128 {
        return None;
    }
    Some((block.to_owned(), components.to_string()))
}

/// The facades in list order (pure): one per block (the first recipe's stack), ordered by `rank` (the block item's
/// place in the registry; unknown ones last, by id).
pub(crate) fn order_facades(
    mut list: Vec<(String, String)>,
    rank: &dyn Fn(&str) -> Option<usize>,
) -> Vec<(String, String)> {
    let mut seen = std::collections::HashSet::new();
    list.retain(|(b, _)| seen.insert(b.clone()));
    list.sort_by(|a, b| {
        (rank(&a.0).unwrap_or(usize::MAX), &a.0).cmp(&(rank(&b.0).unwrap_or(usize::MAX), &b.0))
    });
    list
}

/// The block a facade stack shows (FacadeItem.getTextureItem).
pub(crate) fn facade_block(stack: &ItemStack) -> Option<String> {
    if crate::cells::sid(stack) != FACADE {
        return None;
    }
    match crate::cells::custom_data(stack)?.get(&crate::util::java("item")) {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// A facade's subtype as JEI compares it (pure): its custom data's JSON.
pub(crate) fn facade_json(block: &str) -> String {
    let mut data = mod_api::serde_json::Map::new();
    data.insert("item".to_owned(), Value::String(block.to_owned()));
    let mut c = mod_api::serde_json::Map::new();
    c.insert(SUBTYPE.to_owned(), Value::Object(data));
    Value::Object(c).to_string()
}

/// A block's name, as Jade names a facade's (`block.<ns>.<path>`, else the item's).
fn block_name(host: &dyn ClientHost, id: &str) -> String {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    host.translate(&format!("block.{ns}.{path}"))
        .or_else(|| host.translate(&format!("item.{ns}.{path}")))
        .unwrap_or_else(|| path.replace('_', " "))
}

/// The item model a facade of `block` is drawn with, as the content generator names it (pure):
/// `ae2:hz_facade_<path>` for a vanilla block, `ae2:hz_facade_<namespace>_<path>` for a mod's.
pub(crate) fn facade_model(block: &str) -> String {
    let (ns, path) = block.split_once(':').unwrap_or(("minecraft", block));
    let flat = if ns == "minecraft" {
        path.to_owned()
    } else {
        format!("{ns}_{path}")
    };
    format!("ae2:hz_facade_{}", flat.replace('/', "_"))
}

/// A facade stack's components as the facade recipes write them (pure): its block, and its item model.
pub(crate) fn facade_components(block: &str) -> String {
    let mut data = mod_api::serde_json::Map::new();
    data.insert("item".to_owned(), Value::String(block.to_owned()));
    let mut c = mod_api::serde_json::Map::new();
    c.insert(SUBTYPE.to_owned(), Value::Object(data));
    c.insert(
        "minecraft:item_model".to_owned(),
        Value::String(facade_model(block)),
    );
    Value::Object(c).to_string()
}

/// The facade blocks the client knows, with their stacks' components: from the facade crafting recipes it lists
/// (`recipes_of_type`), else from the facade item models its resources hold (`ae2:items/hz_facade_*.json`, one per
/// block AE2 makes facades of), so the list never depends on the recipes reaching the client. Client side.
fn found(host: &dyn ClientHost) -> (Vec<(String, String)>, &'static str, usize) {
    let recipes = host.recipes_of_type("minecraft:crafting");
    let from_recipes: Vec<(String, String)> = recipes
        .iter()
        .filter_map(|r| facade_of_recipe(&r.json))
        .collect();
    if !from_recipes.is_empty() {
        return (from_recipes, "recipes", recipes.len());
    }
    let from_models: Vec<(String, String)> = host
        .item_keys()
        .iter()
        .filter_map(|k| k.id(host).map(str::to_owned))
        .filter(|id| id.contains(':') && id != FACADE && !id.starts_with("ae2:hz_"))
        .filter(|id| {
            let model = facade_model(id);
            let path = format!("ae2:items/{}.json", model.trim_start_matches("ae2:"));
            Identifier::parse(&path)
                .ok()
                .is_some_and(|p| host.resource(PackType::ClientResources, &p).is_some())
        })
        .map(|id| {
            let c = facade_components(&id);
            (id, c)
        })
        .collect();
    (from_models, "item models", recipes.len())
}

/// The facades JEI lists: (components JSON, search name), in order. Client side.
pub(crate) fn list(host: &dyn ClientHost) -> Vec<(String, String)> {
    if !crate::config::config().enable_facades_in_jei {
        crate::util::debug("jei facades: off (enableFacadesInJEI)");
        return Vec::new();
    }
    let (found, source, recipes) = found(host);
    crate::util::debug(&format!(
        "jei facades: {} from {source} ({recipes} crafting recipes listed)",
        found.len()
    ));
    LAST.store(found.len(), std::sync::atomic::Ordering::Relaxed);
    if found.is_empty() {
        return Vec::new();
    }
    let ranks: std::collections::HashMap<String, usize> = host
        .item_keys()
        .iter()
        .enumerate()
        .filter_map(|(i, k)| Some((k.id(host)?.to_owned(), i)))
        .collect();
    let facade = host
        .translate("item.ae2.facade")
        .unwrap_or_else(|| "Cable Facade".to_owned());
    order_facades(found, &|b| ranks.get(b).copied())
        .into_iter()
        .map(|(block, components)| {
            let name = format!("{facade} - {}", block_name(host, &block));
            (components, name)
        })
        .collect()
}

/// How many facades the last list had (`/ae2debug`-style checks; JEI's `/jei debug` counts what it listed).
pub(crate) static LAST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &str) -> Value {
        mod_api::serde_json::from_str(s).unwrap()
    }

    #[test]
    fn facade_recipes_give_their_stacks() {
        let r = json(
            r#"{"type":"minecraft:crafting_shaped","result":{"id":"ae2:facade","count":4,"components":{"minecraft:custom_data":{"item":"minecraft:acacia_log"},"minecraft:item_model":"ae2:hz_facade_acacia_log"}}}"#,
        );
        let (block, components) = facade_of_recipe(&r).unwrap();
        assert_eq!(block, "minecraft:acacia_log");
        assert!(components.contains(r#""minecraft:item_model":"ae2:hz_facade_acacia_log""#));
        // The stack's subtype is what `facade_json` writes for the same block (JEI compares the custom data).
        let v = json(&components);
        assert_eq!(
            v[SUBTYPE],
            json(&facade_json("minecraft:acacia_log"))[SUBTYPE]
        );
        assert_eq!(
            facade_json("minecraft:stone"),
            r#"{"minecraft:custom_data":{"item":"minecraft:stone"}}"#
        );
        // The stack the item models give is the recipe's own.
        assert_eq!(json(&facade_components("minecraft:acacia_log")), v);
        assert_eq!(
            facade_model("ae2:chiseled_quartz_block"),
            "ae2:hz_facade_ae2_chiseled_quartz_block"
        );
        // Every generated recipe's model follows the same naming.
        for entry in std::fs::read_dir("data/ae2/recipe/hz_facade")
            .into_iter()
            .flatten()
            .flatten()
        {
            let r = json(&std::fs::read_to_string(entry.path()).unwrap());
            let (block, components) = facade_of_recipe(&r).unwrap();
            assert_eq!(
                json(&components),
                json(&facade_components(&block)),
                "{block}"
            );
        }
        // Other recipes, and a facade of nothing, are not facades.
        assert_eq!(
            facade_of_recipe(&json(r#"{"result":{"id":"minecraft:stick","count":4}}"#)),
            None
        );
        assert_eq!(
            facade_of_recipe(&json(r#"{"result":{"id":"ae2:facade"}}"#)),
            None
        );
        assert_eq!(
            facade_of_recipe(&json(
                r#"{"result":{"id":"ae2:facade","components":{"minecraft:custom_data":{"item":"stone"}}}}"#
            )),
            None
        );
    }

    #[test]
    fn facades_in_registry_order_once_each() {
        let list = vec![
            ("b:z".to_owned(), "1".to_owned()),
            ("a:x".to_owned(), "2".to_owned()),
            ("b:z".to_owned(), "3".to_owned()),
            ("c:unknown".to_owned(), "4".to_owned()),
            ("a:y".to_owned(), "5".to_owned()),
        ];
        let rank = |b: &str| match b {
            "a:x" => Some(5),
            "a:y" => Some(1),
            "b:z" => Some(3),
            _ => None,
        };
        let out = order_facades(list, &rank);
        let blocks: Vec<&str> = out.iter().map(|(b, _)| b.as_str()).collect();
        assert_eq!(blocks, ["a:y", "b:z", "a:x", "c:unknown"]);
        // The first recipe's stack is kept.
        assert_eq!(out[1].1, "1");
    }
}
