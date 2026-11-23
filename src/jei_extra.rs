//! The parts of AE2's JEI plugin (appeng.integration.modules.jei.JEIPlugin) the JEI fork's API can carry: the P2P
//! tunnel attunement category (AttunementCategory), the crafting catalysts (the crafting terminals), the item
//! descriptions (presses, certus quartz, budding quartz, the crank), and the inscriber screen's clickable arrow
//! (getGuiClickableAreas). Compiled only with JEI installed.
//!
//! jeix (JEI 0.5.1): the facades in the ingredient list (`register_extra_stacks`, `jei_facades`), "+" on the wireless
//! crafting terminal (`register_screen_transfer`, `wct_transfer`) and R/U over the item screens' stacks.
//!
//! Not here, for want of a JEI API: ghost ingredient drags into config slots (IGhostIngredientHandler), the "+"
//! button's per-slot state and tooltips (IRecipeTransferError with missing/craftable slots), and opening one category
//! (only R/U on an item exist).

use mod_api::*;

pub(crate) const ATTUNEMENT: &str = "ae2:attunement";

/// The tunnels, as InitP2PAttunements and the `ae2:p2p_attunements/*` tags give them.
const TUNNELS: [&str; 6] = [
    "me_p2p_tunnel",
    "item_p2p_tunnel",
    "fluid_p2p_tunnel",
    "redstone_p2p_tunnel",
    "light_p2p_tunnel",
    "fe_p2p_tunnel",
];

/// The items with an energy (or fluid) storage that attune a tunnel by their API (InitP2PAttunements.ENERGY / FLUID),
/// as `parts::attunement` knows them.
fn api_items(kind: &str) -> Vec<String> {
    match kind {
        "fe_p2p_tunnel" => {
            let mut v: Vec<String> = [
                "entropy_manipulator",
                "color_applicator",
                "matter_cannon",
                "charged_staff",
                "wireless_terminal",
                "wireless_crafting_terminal",
                "energy_cell",
                "dense_energy_cell",
                "creative_energy_cell",
            ]
            .iter()
            .map(|n| format!("ae2:{n}"))
            .collect();
            for size in ["1k", "4k", "16k", "64k", "256k"] {
                v.push(format!("ae2:portable_item_cell_{size}"));
                v.push(format!("ae2:portable_fluid_cell_{size}"));
            }
            v
        }
        "fluid_p2p_tunnel" => vec![
            "minecraft:bucket".into(),
            "minecraft:water_bucket".into(),
            "minecraft:lava_bucket".into(),
        ],
        _ => Vec::new(),
    }
}

fn tag_items(host: &dyn Host, kind: &str) -> Vec<String> {
    crate::cells::ids(host);
    let tags = host.tags();
    let Ok(id) = Identifier::parse(&format!("ae2:p2p_attunements/{kind}")) else {
        return Vec::new();
    };
    let Some(tag) = tags.key(&id) else {
        return Vec::new();
    };
    // A missing or not-yet-ready tag lists nothing, as the old empty read did.
    let TagMembers::Members(keys) = tags.items(tag) else {
        return Vec::new();
    };
    keys.into_iter().map(|k| crate::cells::key_id(k).to_owned()).collect()
}

/// JEIPlugin.registerP2PAttunement: the API kinds first, then each tag's items; a list with nothing in it goes.
fn recipes(host: &dyn Host) -> Vec<(String, jei::api::RecipeDef)> {
    let mut out = Vec::new();
    let card = |id: String, items: &[String], tunnel: &str, key: &str, args: &[&str]| {
        let items: Vec<(&str, u32)> = items.iter().map(|i| (i.as_str(), 1)).collect();
        jei::api::RecipeDef::new(&id, [140.0, 44.0])
            .input(1.0, 1.0, &items)
            .arrow(26.0, 1.0)
            .output(60.0, 1.0, &[(tunnel, 1)])
            .text_tr(1.0, 24.0, key, args)
    };
    for (kind, desc) in [
        ("fe_p2p_tunnel", "gui.ae2.P2PAttunementEnergy"),
        ("fluid_p2p_tunnel", "gui.ae2.P2PAttunementFluid"),
    ] {
        let items = api_items(kind);
        if !items.is_empty() {
            let id = format!("ae2:attunement/api/{kind}");
            let tunnel = format!("ae2:{kind}");
            let arg = jei::api::tr_arg(desc);
            out.push((
                ATTUNEMENT.to_owned(),
                card(
                    id,
                    &items,
                    &tunnel,
                    "ae2.rei_jei_integration.p2p_api_attunement",
                    &[&arg],
                ),
            ));
        }
    }
    for kind in TUNNELS {
        let items = tag_items(host, kind);
        if !items.is_empty() {
            let id = format!("ae2:attunement/tag/{kind}");
            let tunnel = format!("ae2:{kind}");
            out.push((
                ATTUNEMENT.to_owned(),
                card(
                    id,
                    &items,
                    &tunnel,
                    "ae2.rei_jei_integration.p2p_tag_attunement",
                    &[],
                ),
            ));
        }
    }
    out
}

/// Everything above, once per world join (registering again replaces).
pub(crate) fn register() {
    // The upgrade and view cell columns and the toolbox belt beside AE2's screens: JEI's list keeps clear of them.
    jei::api::register_global_extra_areas("ae2", crate::screens::extra_areas);
    jei::api::register_category(jei::api::CategoryDef::new(
        ATTUNEMENT,
        "ae2.rei_jei_integration.p2p_tunnel_attunement",
        "ae2:me_p2p_tunnel",
    ));
    jei::api::register_recipes("ae2_attunement", recipes);
    // registerRecipeCatalysts: the crafting terminals craft vanilla's recipes.
    for item in ["ae2:crafting_terminal", "ae2:wireless_crafting_terminal"] {
        jei::api::add_catalyst("minecraft:crafting", item);
    }
    // registerDescriptions.
    jei::api::add_info(
        &[
            "ae2:silicon_press",
            "ae2:logic_processor_press",
            "ae2:calculation_processor_press",
            "ae2:engineering_processor_press",
        ],
        &["gui.ae2.inWorldCraftingPresses"],
    );
    jei::api::add_info(
        &["ae2:certus_quartz_crystal"],
        &["gui.ae2.CertusQuartzObtain"],
    );
    jei::api::add_info(
        &["ae2:flawless_budding_quartz"],
        &["ae2.rei_jei_integration.flawless_budding_quartz_description"],
    );
    jei::api::add_info(
        &[
            "ae2:flawed_budding_quartz",
            "ae2:chipped_budding_quartz",
            "ae2:damaged_budding_quartz",
        ],
        &[
            "ae2.rei_jei_integration.imperfect_budding_quartz_decays",
            "ae2.rei_jei_integration.budding_quartz_creation_and_worldgen",
        ],
    );
    jei::api::add_info(
        &["ae2:crank"],
        &["ae2.rei_jei_integration.crank_description"],
    );
    // jeix: one facade per block after the plain facade (registerIngredients), told apart by their block: R on one
    // shows its own recipe.
    jei::api::register_subtype(crate::jei_facades::FACADE, crate::jei_facades::SUBTYPE);
    jei::api::register_extra_stacks("ae2_facades", crate::jei_facades::FACADE, facades);
    // AE2's UseCraftingRecipeTransfer / EncodePatternTransferHandler dry runs: "+" marks the cells the network and
    // the inventory can't fill (red) and those it can craft (blue), and says so, before the click.
    jei::api::register_transfer_state("ae2:crafting_terminal", crafting_plus);
    jei::api::register_transfer_state("ae2:pattern_encoding_terminal", encoding_plus);
    // jeix: "+" on the wireless crafting terminal's item screen (WirelessCraftingTermMenu's transfer handler).
    jei::api::register_screen_transfer(
        crate::wireless::SCREEN,
        "minecraft:crafting",
        crate::wct_transfer::client_open,
        crate::wct_transfer::client_transfer,
    );
}

/// `TransferHelper`'s slot colours: a missing ingredient, one the network can craft.
const RED_SLOT: u32 = 0x66FF_0000;
const BLUE_SLOT: u32 = 0x4000_00FF;
/// UseCraftingRecipeTransfer's tint of "+": something is missing, or everything missing can be crafted.
const RED_BUTTON: u32 = 0x80FF_0000;
const BLUE_BUTTON: u32 = 0x8045_FFFF;

/// `CraftingTermMenu.findMissingIngredients` on the client: each cell in order takes one of its items from the
/// terminal's network, else from the inventory (what one cell takes, the next can't); a cell neither fills is
/// craftable when the network lists one of its items as craftable, else missing. (missing, craftable) cells.
pub(crate) fn missing_cells(host: &dyn ClientHost, cells: &[jei::api::TransferCell]) -> (Vec<u32>, Vec<u32>) {
    let mut network = crate::screens::repo_items();
    let mut inventory: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    if let Some(p) = host.local_player() {
        // The main inventory (slots 0 to 35), as `inventory` read it.
        p.with_inventory(host, |view| {
            for (_, stack) in view.iter().take_while(|(slot, _)| *slot < 36) {
                if let Some(id) = stack.item().id(host) {
                    *inventory.entry(id.to_owned()).or_default() += i64::from(stack.count());
                }
            }
        });
    }
    let (mut missing, mut craftable) = (Vec::new(), Vec::new());
    for cell in cells {
        let ids: Vec<String> = cell.items.iter().filter_map(|k| k.id(host).map(str::to_owned)).collect();
        if let Some(id) = ids.iter().find(|id| network.get(*id).is_some_and(|(n, _)| *n > 0)) {
            if let Some((n, _)) = network.get_mut(id) {
                *n -= 1;
            }
        } else if let Some(id) = ids.iter().find(|id| inventory.get(*id).is_some_and(|n| *n > 0)) {
            if let Some(n) = inventory.get_mut(id) {
                *n -= 1;
            }
        } else if ids.iter().any(|id| network.get(id).is_some_and(|(_, c)| *c)) {
            craftable.push(cell.cell);
        } else {
            missing.push(cell.cell);
        }
    }
    (missing, craftable)
}

/// UseCraftingRecipeTransfer.transferRecipe's dry run on the crafting terminal: no item at all for any cell blocks
/// "+" ("Found no compatible items"); otherwise missing and craftable cells are marked, "+" tinted, and the tooltip
/// says what the click will do (`TransferHelper.createCraftingTooltip`).
fn crafting_plus(host: &dyn ClientHost, _menu: &str, category: &str, cells: &[jei::api::TransferCell]) -> Option<jei::api::TransferState> {
    if category != "minecraft:crafting" || cells.is_empty() {
        return None;
    }
    let tr = |key: &str, fallback: &str| crate::screens::tr(host, key, fallback);
    let (missing, craftable) = missing_cells(host, cells);
    let marks = missing.iter().map(|c| (*c, RED_SLOT)).chain(craftable.iter().map(|c| (*c, BLUE_SLOT))).collect();
    if missing.len() == cells.len() {
        return Some(jei::api::TransferState {
            blocked: true,
            marks,
            lines: vec![format!("§c{}", tr("ae2.rei_jei_integration.no_items", "Found no compatible items"))],
            tint: None,
        });
    }
    if missing.is_empty() && craftable.is_empty() {
        return None;
    }
    let mut lines = vec![tr("ae2.rei_jei_integration.move_items", "Move items")];
    if !craftable.is_empty() {
        lines.push(if host.modifiers().control() {
            format!("§9{}", tr("ae2.rei_jei_integration.will_craft", "Will craft unavailable items"))
        } else {
            format!("§9{}", tr("ae2.rei_jei_integration.ctrl_click_to_craft", "CTRL + click to craft unavailable items"))
        });
    }
    if !missing.is_empty() {
        lines.push(format!("§c{}", tr("ae2.rei_jei_integration.missing_items", "Missing items will be skipped")));
    }
    Some(jei::api::TransferState { blocked: false, marks, lines, tint: Some(if missing.is_empty() { BLUE_BUTTON } else { RED_BUTTON }) })
}

/// EncodePatternTransferHandler's dry run: "+" always encodes; the cells the network can already craft are marked
/// blue, and the tooltip says so (`TransferHelper.createEncodingTooltip`).
fn encoding_plus(host: &dyn ClientHost, _menu: &str, _category: &str, cells: &[jei::api::TransferCell]) -> Option<jei::api::TransferState> {
    let network = crate::screens::repo_items();
    let craftable: Vec<u32> =
        cells.iter().filter(|c| c.items.iter().any(|k| k.id(host).and_then(|id| network.get(id)).is_some_and(|(_, c)| *c))).map(|c| c.cell).collect();
    let tr = |key: &str, fallback: &str| crate::screens::tr(host, key, fallback);
    let mut lines = vec![tr("ae2.rei_jei_integration.encode_pattern", "Encode pattern")];
    if !craftable.is_empty() {
        lines.push(format!("§9{}", tr("ae2.rei_jei_integration.has_encoded_ingredients", "Highlighted elements are already craftable")));
    }
    Some(jei::api::TransferState { blocked: false, marks: craftable.into_iter().map(|c| (c, BLUE_SLOT)).collect(), lines, tint: None })
}

/// The facades JEI lists (`jei_facades::list`).
fn facades(host: &dyn ClientHost) -> Vec<jei::api::ExtraStack> {
    crate::jei_facades::list(host)
        .into_iter()
        .map(|(components, name)| jei::api::ExtraStack::new(crate::jei_facades::FACADE, &components).name(&name))
        .collect()
}

/// JEI's R or U over an item screen's stack: the recipe window on it. True when it opened (the key is taken).
pub(crate) fn lookup(host: &dyn ClientHost, view: &ScreenView<'_>, key: &str) -> bool {
    let Some(uses) = jei::api::lookup_key(host, key) else { return false };
    let Some((item, components)) = crate::wireless::hovered_stack(host, view) else { return false };
    jei::api::show_stack(&item, &components, uses);
    true
}

/// JEIPlugin.getGuiClickableAreas: the inscriber screen's arrow, from the window's top left.
pub(crate) const INSCRIBER_AREA: [f32; 4] = [82.0, 39.0, 26.0, 16.0];

pub(crate) fn on_area(rect: [f32; 4], mouse: [f32; 2]) -> bool {
    let [x, y, w, h] = INSCRIBER_AREA;
    let (ax, ay) = (rect[0] + x, rect[1] + y);
    mouse[0] >= ax && mouse[0] < ax + w && mouse[1] >= ay && mouse[1] < ay + h
}

fn on_inscriber_area(screen: &ScreenView<'_>) -> bool {
    screen.menu == Some(crate::machines::INSCRIBER) && on_area(screen.rect, screen.mouse)
}

/// A left click on the arrow: the inscriber's recipes (its uses as their catalyst).
pub(crate) fn pressed(screen: &ScreenView<'_>, button: i32) -> bool {
    if button == 0 && on_inscriber_area(screen) {
        jei::api::show(crate::machines::INSCRIBER, true);
        return true;
    }
    false
}

/// The area's tooltip (IGuiClickableArea's "Show Recipes").
pub(crate) fn render_post(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    screen: &ScreenView<'_>,
) {
    if on_inscriber_area(screen) {
        let label = host
            .translate("jei.tooltip.show.recipes")
            .unwrap_or_else(|| "Show Recipes".to_owned());
        let _ = draw.gui_tooltip(&[Text::literal(label)], screen.mouse);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inscriber_arrow_area() {
        let rect = [100.0, 50.0, 176.0, 176.0];
        assert!(on_area(rect, [182.0, 89.0]));
        assert!(on_area(rect, [207.9, 104.9]));
        assert!(!on_area(rect, [208.0, 89.0]));
        assert!(!on_area(rect, [181.0, 89.0]));
    }

    #[test]
    fn energy_items_attune_fe_tunnels() {
        assert!(api_items("fe_p2p_tunnel").contains(&"ae2:portable_item_cell_256k".to_owned()));
        assert_eq!(api_items("fluid_p2p_tunnel").len(), 3);
        assert!(api_items("me_p2p_tunnel").is_empty());
    }
}
