//! AE2's blocks with a block entity: one declared block entity type per device kind (`ae2:<kind>`), so a block on a
//! template with a vanilla block entity (beacon, barrel, shulker box, skull) gets AE2's, never the template's.

use crate::blocks_gen::BE_BLOCKS;
use crate::util::*;
use mod_api::*;
use std::collections::BTreeMap;

/// The block entity type (device kind) of an AE2 block: the block's own name, except the helper blocks, which share
/// their device's type (the cable bus's facade twins `hz_fcb*`/`hz_fcl*`, paint's lit twins, the lit panels).
pub(crate) fn kind_of(block: &str) -> &str {
    if block.starts_with("hz_fcb") || block.starts_with("hz_fcl") {
        return "cable_bus";
    }
    if block.starts_with("hz_paint_lit") {
        return "paint";
    }
    match block {
        "hz_monitor_lit" => "monitor",
        "hz_dark_monitor_lit" => "dark_monitor",
        "hz_semi_dark_monitor_lit" => "semi_dark_monitor",
        other => other,
    }
}

/// Every device kind and its blocks (`ae2:` ids).
pub(crate) fn kinds() -> BTreeMap<&'static str, Vec<String>> {
    let mut out: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for (block, _) in BE_BLOCKS {
        out.entry(kind_of(block)).or_default().push(format!("ae2:{block}"));
    }
    out
}

/// AE2's devices' Part.
pub(crate) struct Device;

/// The memory card (P2P frequencies).
pub(crate) struct MemoryCard;

#[mod_api::sunrise_part]
impl ItemPart for MemoryCard {
    fn on_item_use_first(&self, host: &mut dyn ServerHost, event: &mut OnItemUseFirstEvent<'_>) -> Option<InteractionResult> {
        debug(&format!("memory card: used at {:?}", event.click.pos));
        let level = here(&*host)?;
        // On a cable bus: the tunnel under the hit point.
        let pos = crate::bus::aimed_node(&*host, level, event.click).unwrap_or(event.click.pos);
        let block = crate::net::block_id(&*host, level, pos);
        if crate::autocraft::is_provider(&block) {
            return crate::cell_tools::provider_use(host, event.stack, event.click, &block, pos);
        }
        if !crate::parts::is_p2p(&block) {
            return crate::parts::memory_card_use(host, event.stack, event.click, &block, pos);
        }
        crate::parts::p2p_use(host, event.click, &block, pos)
    }

    /// MemoryCardItem's colour code (`cell_tools`).
    fn item_decorations(&self, _host: &dyn ClientHost, event: &mut ItemDecorationsEvent<'_>) {
        crate::cell_tools::memory_card_decorations(event);
    }
}

/// An upgrade card used on a machine while sneaking (`wrench::card_use`).
pub(crate) struct CardUse;

#[mod_api::sunrise_part]
impl ItemPart for CardUse {
    fn on_item_use_first(&self, host: &mut dyn ServerHost, event: &mut OnItemUseFirstEvent<'_>) -> Option<InteractionResult> {
        crate::wrench::card_use(host, event.stack, event.click)
    }
}

/// Cells and crafting blocks taken apart in the hand (`cell_tools::use_item`).
pub(crate) struct TakeApart;

#[mod_api::sunrise_part]
impl ItemPart for TakeApart {
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else { return None };
        crate::cell_tools::use_item(host, event.stack, event.hand, event.player)
    }
}

/// The menus' slot rules (cells in drives and chests, view cells in terminals).
pub(crate) struct Menus;

/// Blocks with a menu of stage 3.
pub(crate) const MENU_BLOCKS: [&str; 5] = ["ae2:drive", "ae2:chest", "ae2:terminal", "ae2:crafting_terminal", "ae2:pattern_encoding_terminal"];

pub(crate) fn register(r: &mut dyn Registrar) {
    use crate::capdefer;
    for (kind, blocks) in kinds() {
        let names: Vec<&str> = blocks.iter().map(String::as_str).collect();
        let ty = r.block_entity_type(kind).blocks(&names).build();
        r.block_entity_part(&ty, Device).expect("AE2's block entity types are native");
        // Stage 4: what interfaces and tunnels answer other mods (asked before the game's own).
        match kind {
            "interface" | "cable_interface" => {
                r.block_entity_capability_before_game(&ITEMS, &ty, capdefer::items(capdefer::sided_with_data, crate::parts::interface_items));
                r.block_entity_capability_before_game(&FLUIDS, &ty, capdefer::items(capdefer::sided_with_data, crate::iface_fluids::interface_fluids));
            }
            // ChargerInvHandler: hoppers and pipes put in only what it charges, and take out only what is done.
            "charger" => r.block_entity_capability_before_game(&ITEMS, &ty, capdefer::items(capdefer::sided, crate::world::charger_items)),
            // The vibration chamber's fuel slot; the ME chest's cell slot (front) and cell (other sides).
            "vibration_chamber" => r.block_entity_capability_before_game(&ITEMS, &ty, capdefer::items(capdefer::sided, crate::generators::vibration_items)),
            "chest" => r.block_entity_capability_before_game(&ITEMS, &ty, capdefer::items(capdefer::sided, crate::me_chest::chest_items)),
            "inscriber" => r.block_entity_capability_before_game(&ITEMS, &ty, capdefer::items(capdefer::always, crate::inscriber::items)),
            "io_port" => r.block_entity_capability_before_game(&ITEMS, &ty, capdefer::items(capdefer::always, crate::io_port::items)),
            "condenser" => r.block_entity_capability_before_game(&ITEMS, &ty, capdefer::items(capdefer::always, crate::condenser::items)),
            "cable_bus" => {
                r.block_entity_capability_before_game(&ITEMS, &ty, capdefer::items(crate::bus::items_present, crate::bus::items));
                r.block_entity_capability_before_game(&FLUIDS, &ty, capdefer::items(crate::bus::fluids_present, crate::bus::fluids));
                r.block_entity_capability_before_game(&ENERGY, &ty, capdefer::energy(crate::bus::energy_present, crate::bus::energy));
            }
            k if crate::parts::is_p2p(&format!("ae2:{k}")) => {
                // A tunnel turns into another kind in place, so every kind's type answers all three (each checks).
                r.block_entity_capability_before_game(&ITEMS, &ty, capdefer::items(crate::parts::p2p_present, crate::parts::p2p_items));
                r.block_entity_capability_before_game(&FLUIDS, &ty, capdefer::items(crate::parts::p2p_present, crate::parts::p2p_fluids));
                r.block_entity_capability_before_game(&ENERGY, &ty, capdefer::energy(crate::parts::p2p_present, crate::parts::p2p_energy));
            }
            _ => {}
        }
    }
    // The memory card on a tunnel (MemoryCardItem.onItemUseFirst: before the block, sneaking or not).
    let card = r.item_key("ae2:memory_card");
    r.item_part(card, MemoryCard);
    // enc: an encoded pattern shows what it makes with shift held.
    crate::encoder::register_parts(r);
    // Parts and facades onto a cable's side (the cable bus).
    for id in HOSTED_ITEMS.iter().chain(["ae2:facade"].iter()) {
        let k = r.item_key(id);
        r.item_part(k, BusPlace);
    }
    r.menu_part(crate::bus::MENU, Menus);
    for w in ["ae2:certus_quartz_wrench", "ae2:nether_quartz_wrench"] {
        let k = r.item_key(w);
        r.item_part(k, BusWrench);
    }
    for c in crate::wrench::CARDS {
        let k = r.item_key(c);
        r.item_part(k, CardUse);
    }
    // Portable cells are taken apart by their screen's part (`wireless::ItemScreen`), which asks first.
    for id in crate::cell_tools::TAKEN_APART.iter().filter(|i| !i.contains("portable_")) {
        let k = r.item_key(id);
        r.item_part(k, TakeApart);
    }
    for menu in ae2_menus() {
        r.menu_part(menu, Menus);
    }
    // The screens: AE2's menus, a cable bus's (its open part's), and the sky stone chest's and spatial IO port's,
    // which have no menu Part of their own but open their block's screen too. Each opening builds its own.
    for menu in ae2_menus().chain([crate::bus::MENU, crate::sky_chest::CHEST_MENU, "ae2:spatial_io_port"]) {
        if let Err(e) = r.register_menu_screen(menu, std::sync::Arc::new(menu_screen)) {
            crate::util::debug(&format!("screens: {menu}: {e}"));
        }
    }
}

/// The part menus of stage 4 (content menu types).
pub(crate) const PART_MENUS: [&str; 6] = ["ae2:io_bus", "ae2:storage_bus", "ae2:interface", "ae2:level_emitter", "ae2:energy_level_emitter", "ae2:cell_workbench"];

/// The part items a cable bus hosts (`bus::hostable`).
const HOSTED_ITEMS: &[&str] = &[
    "ae2:terminal", "ae2:crafting_terminal", "ae2:pattern_encoding_terminal", "ae2:pattern_access_terminal",
    "ae2:import_bus", "ae2:export_bus", "ae2:storage_bus", "ae2:level_emitter", "ae2:energy_level_emitter",
    "ae2:cable_interface", "ae2:cable_energy_acceptor", "ae2:storage_monitor", "ae2:conversion_monitor",
    "ae2:monitor", "ae2:semi_dark_monitor", "ae2:dark_monitor", "ae2:quartz_fiber", "ae2:toggle_bus",
    "ae2:inverted_toggle_bus", "ae2:annihilation_plane", "ae2:formation_plane", "ae2:cable_anchor",
    "ae2:me_p2p_tunnel", "ae2:item_p2p_tunnel", "ae2:fluid_p2p_tunnel", "ae2:redstone_p2p_tunnel",
    "ae2:light_p2p_tunnel", "ae2:fe_p2p_tunnel", "ae2:cable_pattern_provider",
];

/// A wrench sneak-used on a cable bus: the aimed part or facade into the inventory.
pub(crate) struct BusWrench;

#[mod_api::sunrise_part]
impl ItemPart for BusWrench {
    fn on_item_use_first(&self, host: &mut dyn ServerHost, event: &mut OnItemUseFirstEvent<'_>) -> Option<InteractionResult> {
        crate::bus::wrench(host, event.click).or_else(|| crate::wrench::use_on(host, event.click))
    }
}

/// A part or a facade used on a cable (PartItem.useOn, FacadeItem.onItemUseFirst).
pub(crate) struct BusPlace;

#[mod_api::sunrise_part]
impl ItemPart for BusPlace {
    fn on_item_use_first(&self, host: &mut dyn ServerHost, event: &mut OnItemUseFirstEvent<'_>) -> Option<InteractionResult> {
        crate::bus::place(host, event.stack, event.click)
    }
}

#[mod_api::sunrise_part]
impl MenuPart for Menus {
    /// Every button id reaches `menu_button`, which checks it against the open menu (and refuses what isn't its).
    fn menu_button_valid(&self, _host: &dyn Host, _event: &mut MenuButtonValidEvent<'_>) -> Flow {
        Flow::Cancel
    }

    fn menu_button(&self, event: &mut MenuButtonEvent<'_>) {
        let (player, id) = (event.context.player, event.context.id);
        // AE2's buttons are all on a block's menu (its fields): worked on as they are, written back where they changed.
        let Some(pos) = event.context.pos else { return };
        let Some(before) = event.subject.with_view(|v| tick_fields(&v)) else { return };
        let mut fields = before.clone();
        block_menu_button(event.subject.host(), player, id, pos, &mut fields, &mut *event.carried);
        if fields != before {
            event.subject.with_view(|mut v| write_tick_fields(&mut v, &before, fields));
        }
    }

    fn menu_slot_rules(&self, host: &dyn Host, event: &mut MenuSlotRulesEvent<'_>) {
        // AE2's own slots only: the player's inventory slots are asked too (`MenuContainer::Player`).
        if event.container != MenuContainer::Main {
            return;
        }
        // The rules below name items by id (`cells::sid`), which reads "" until the id map is made. Since main this
        // hook has a host, so make the map here: a click can come before anything else asked for it.
        let _ = crate::cells::ids(host);
        if event.menu == crate::bus::MENU {
            if !crate::bus::on_open_side(event.context.player, event.context.pos, event.slot) {
                event.may_place = false;
                event.may_pickup = false;
                return;
            }
            let (stack, mut may, mut max) = (event.stack, event.may_place, event.max_stack);
            let mut hidden = false;
            let player = event.context.player;
            let ok = crate::bus::slot_rules(host, event.context.pos, event.slot, |menu, slot| {
                hidden = hidden_slot(menu, slot, player);
                crate::storage::slot_rules(menu, slot, stack, &mut may, &mut max);
                crate::parts::slot_rules(menu, slot, stack, &mut may, &mut max);
                crate::autocraft::slot_rules(menu, slot, stack, &mut may, &mut max);
            });
            // A slot no part shows (`ok` false) or its part's screen hides is no slot of the screen's at all.
            event.may_place = ok && may && !hidden;
            event.may_pickup &= ok && !hidden;
            event.max_stack = max;
            return;
        }
        if hidden_slot(event.menu, event.slot, event.context.player) {
            event.may_place = false;
            event.may_pickup = false;
            return;
        }
        crate::storage::slot_rules(event.menu, event.slot, event.stack, &mut event.may_place, &mut event.max_stack);
        crate::parts::slot_rules(event.menu, event.slot, event.stack, &mut event.may_place, &mut event.max_stack);
        crate::autocraft::slot_rules(event.menu, event.slot, event.stack, &mut event.may_place, &mut event.max_stack);
        crate::generators::slot_rules(host, event.menu, event.slot, event.stack, &mut event.may_place, &mut event.max_stack);
        crate::wireless::slot_rules(event.menu, event.slot, event.stack, &mut event.may_place, &mut event.max_stack);
        crate::qnb::slot_rules(event.menu, event.stack, &mut event.may_place, &mut event.max_stack);
        crate::machines::slot_rules(host, event.menu, event.slot, event.stack, event.context, &mut event.may_place, &mut event.max_stack);
    }

    /// JEI's "+" on a pattern encoding terminal (a block, or a part on a cable bus): the recipe into its cells.
    fn menu_transfer(&self, host: &mut dyn ServerHost, event: &mut MenuTransferEvent<'_>) -> Option<RecipeTransferResult> {
        let pos = event.pos?;
        let (at, block) = if event.menu == crate::bus::MENU {
            let (hosted, id, _) = crate::bus::opened_part(&*host, event.player, pos)?;
            (hosted, id)
        } else {
            (pos, event.menu.to_owned())
        };
        match block.as_str() {
            "ae2:pattern_encoding_terminal" => Some(crate::transfer::encoder(host, at, event.recipe, event.dry_run)),
            // enc: AE2's "+" on the crafting terminal (`ct_transfer`).
            "ae2:crafting_terminal" => Some(crate::ct_transfer::transfer(host, event.player, at, event.recipe, event.dry_run)),
            _ => None,
        }
    }
}

/// A use of a drive, chest or terminal opens its screen (DriveBlock, ChestBlock, AbstractTerminalPart): the ME chest's
/// top opens its cell's own terminal, with AE2's message when it can't read one.
fn use_block(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let level = here(&*host)?;
    let pos = click.pos;
    if level.block(&*host, pos).is_some_and(|b| crate::bus::is_cell_block(b.id(&*host).unwrap_or_default())) {
        return crate::bus::use_cell(host, click);
    }
    let block = crate::net::block_id(&*host, level, pos);
    if block == "ae2:charger" {
        debug(&format!("charger: use_block, stack {:?}", click.stack.map(crate::cells::sid)));
        return crate::world::charger_use(host, click);
    }
    if !MENU_BLOCKS.contains(&block.as_str())
        && !crate::parts::has_screen(&block)
        && !crate::autocraft::has_screen(&block)
        && !crate::generators::has_screen(&block)
        && !crate::sky_chest::has_screen(&block)
        && !crate::machines::has_screen(&block)
        && !crate::wireless::has_screen(&block)
        && !crate::qnb::has_screen(&block)
        && !crate::anchor::has_screen(&block)
    {
        return None;
    }
    // A crafting block opens its CPU's status only when formed (AbstractCraftingUnitBlock.useWithoutItem).
    if crate::net::is_crafting(&block) && level.block_property(&*host, pos, "structure").as_deref() != Some("true") {
        return None;
    }
    crate::cells::ids(&*host);
    let player = host.player(click.player)?;
    let e = player.entity();
    if e.is_shift_key_down(&*host).unwrap_or(false) || e.is_crouching(&*host).unwrap_or(false) {
        return None;
    }
    let dim = host.dimension();
    let mut chest_term = false;
    if block == "ae2:chest" {
        let facing = facing_dir(level.block_property(&*host, pos, "facing").as_deref());
        // Its top as its front and spin put it (FacingWithSpinStrategy).
        let spin = level.block_property(&*host, pos, "spin").and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
        let top = crate::wrench::spin_up(facing, spin);
        if click.hit.face == top {
            let cell = host.block_entity(level, pos).and_then(|b| b.item_slot(&*host, 0).ok().flatten());
            let readable = cell.is_some_and(|c| crate::cells::is_cell_item(crate::cells::sid(&c)));
            let (_, online, _) = crate::net::device_state(dim, pos);
            if !readable || !online {
                let _ = player.action_bar(host, Text::literal("ME Chest cannot read storage cell."));
                return Some(InteractionResult::Success);
            }
            chest_term = true;
        }
    }
    let entity = host.block_entity(level, pos)?;
    pad_slots(host, entity, &block);
    crate::screens::set_server_chest_term(chest_term);
    let opened = matches!(player.open_menu(host, entity), Ok(true));
    crate::screens::set_server_chest_term(false);
    if opened {
        let window = player.container_menu(&*host).and_then(|m| m.container_id(&*host));
        crate::storage::opened(click.player, window, dim, pos, &block, chest_term);
        crate::parts::opened(click.player);
        if crate::sky_chest::is_sky_chest(&block) {
            crate::sky_chest::opened(click.player, dim, pos, window);
        }
    }
    Some(InteractionResult::SuccessServer)
}

#[mod_api::sunrise_part]
impl BlockPart for Device {
    fn use_without_item(&self, host: &mut dyn ServerHost, event: &mut UseWithoutItemEvent<'_>) -> Option<InteractionResult> {
        use_block(host, event.click)
    }

    fn use_item_on(&self, host: &mut dyn ServerHost, event: &mut UseItemOnEvent<'_>) -> Option<InteractionResult> {
        // A tunnel used with a memory card or an attunement item.
        if let Some(level) = here(&*host) {
            let pos = crate::bus::aimed_node(&*host, level, event.click).unwrap_or(event.click.pos);
            let block = crate::net::block_id(&*host, level, pos);
            if crate::parts::is_p2p(&block)
                && let Some(r) = crate::parts::p2p_use(host, event.click, &block, pos)
            {
                return Some(r);
            }
        }
        use_block(host, event.click)
    }

    /// The level emitters' and redstone tunnel outputs' redstone (all sides, as AE2's part on its own block).
    fn block_signal(&self, level: &dyn LevelHost, event: &mut BlockSignalEvent) {
        if level.state_block(event.state).is_some_and(|b| crate::bus::is_cell_block(&b)) {
            // The receiver is on the side opposite the face it asks through: only that side's part's signal.
            let d = match event.face {
                Direction::Down => [0, -1, 0],
                Direction::Up => [0, 1, 0],
                Direction::North => [0, 0, -1],
                Direction::South => [0, 0, 1],
                Direction::West => [-1, 0, 0],
                _ => [1, 0, 0],
            };
            if let Some(side) = SIDES.iter().position(|s| *s == [-d[0], -d[1], -d[2]])
                && let Some(s) = crate::parts::signal(event.dimension.key().id(), hosted([event.pos.x, event.pos.y, event.pos.z], side))
            {
                event.signal = s;
            }
            return;
        }
        if level.state_block(event.state).is_some_and(|b| crate::parts::is_signal_block(&b))
            && let Some(s) = crate::parts::signal(event.dimension.key().id(), [event.pos.x, event.pos.y, event.pos.z])
        {
            event.signal = s;
        }
        // [blocks] The light detector.
        if level.state_block(event.state) == Some(crate::misc_blocks::LIGHT_DETECTOR)
            && let Some(s) = crate::misc_blocks::signal(event.dimension.key().id(), [event.pos.x, event.pos.y, event.pos.z])
        {
            event.signal = s;
        }
    }

    /// Comparators read AE2's inventories and energy cells (`comparator`).
    fn has_analog_output(&self, level: &dyn LevelHost, event: &mut HasAnalogOutputEvent) {
        if level.state_block(event.state).is_some_and(crate::comparator::has_output) {
            event.output = true;
        }
    }

    fn analog_output(&self, level: &mut dyn LevelRandomHost, event: &mut AnalogOutputEvent) {
        if level.state_block(event.state).is_some_and(crate::comparator::has_output) {
            event.output = crate::comparator::get(event.dimension.key().id(), [event.pos.x, event.pos.y, event.pos.z]);
        }
    }

    /// The orientation strategies' placement (`wrench::placement_state`).
    /// Server only, as before version 2: a client's prediction keeps the template's rule and the server decides.
    fn state_for_placement(&self, level: LevelSide<'_>, event: &mut StateForPlacementEvent<'_>) -> Option<Option<BlockState>> {
        let LevelSide::Server(level) = level else { return None };
        // [blocks] The crank: on a crankable machine only.
        crate::misc_blocks::placement(level, event)
            .or_else(|| crate::bus::part_placement(level, event)) // fixC: a part's own block faces the block it is put on
            .or_else(|| crate::wrench::placement_state(level, event))
    }

    /// [blocks] A cell with a cable anchor on a side climbs as a ladder (CableBusBlock.isLadder).
    fn is_ladder(&self, _context: &dyn LoopContext, event: &mut IsLadderEvent) {
        if crate::misc_blocks::is_ladder([event.pos.x, event.pos.y, event.pos.z]) {
            event.ladder = true;
        }
    }

    fn is_signal_source(&self, level: &dyn LevelHost, event: &mut IsSignalSourceEvent) {
        if level.state_block(event.state).is_some_and(|b| crate::parts::is_signal_block(&b) || crate::bus::is_cell_block(&b) || b == crate::misc_blocks::LIGHT_DETECTOR) {
            event.source = true;
        }
    }
}

/// A device's logic for a tick on its fields: a block's own, or a hosted part's (`bus::tick`, at its node).
pub(crate) fn run_device(host: &mut dyn ServerHost, pos: Pos, block: &str, fields: &mut Fields) {
    let block = block.to_owned();
    crate::wrench::apply_pending(&*host, pos, &block, fields);
    crate::net::device_tick(host, pos, &block, fields);

        if crate::storage::is_holder(&block) {
            crate::storage::holder_tick(host.dimension(), pos, &block, fields);
        }
        if crate::storage::is_terminal(&block) {
            crate::storage::flush_insert_slot(host, pos, fields);
        }
        // enc: the encoder's amount dialog values and an encoded pattern put in its slot.
        if block == "ae2:pattern_encoding_terminal" {
            crate::encoder::tick(host, pos, fields);
        }
        if crate::parts::kind(&block).is_some() {
            crate::parts::tick(host, pos, &block, fields);
        }
        if block == "ae2:spatial_io_port" {
            crate::spatial::port_tick(host, pos, fields);
        }
        if crate::autocraft::is_provider(&block) {
            crate::autocraft::provider_tick(host, pos, fields);
        }
        if crate::net::is_crafting(&block) {
            crate::autocraft::crafting_tick(host, pos, &block, fields);
        }
        if block == "ae2:molecular_assembler" {
            crate::autocraft::assembler_tick(host, pos, fields);
        }
        if block == "ae2:charger" {
            crate::world::charger_tick(host, pos, fields);
        }
        // polish: what the charger's and assembler's renderers draw, the accelerator's lit look.
        crate::effects::device_looks(host, pos, &block, fields);
        if block == "ae2:annihilation_plane" {
            crate::planes::annihilation_tick(host, pos, fields);
        }
        if block == "ae2:growth_accelerator" {
            crate::world::accelerator_tick(host, pos, fields);
        }
        if block == crate::wireless::WAP {
            crate::wireless::wap_tick(host, pos, fields);
        }
        if block == crate::generators::VIBRATION {
            crate::generators::vibration_tick(host, pos, fields);
        }
        if block == crate::generators::RESONANCE {
            crate::generators::resonance_tick(host, pos, fields);
        }
        if block == "ae2:chest" {
            crate::me_chest::chest_tick(host, pos, fields);
        }
        crate::comparator::tick(host, pos, &block, fields);
        // [blocks] Gap-list item 8: monitors.
        if crate::monitors::is_monitor(&block) {
            crate::monitors::tick(host, pos, fields);
        }
        if crate::links::is_toggle(&block) {
            crate::links::toggle_tick(host, pos, &block);
        }
        if crate::qnb::is_qnb(&block) {
            crate::qnb::tick(host, pos, &block, fields);
        }
        if block == crate::anchor::ANCHOR {
            crate::anchor::tick(host, pos, fields);
        }
        if crate::machines::is_machine(&block) {
            crate::machines::tick(host, pos, &block, fields);
        }
}

/// CraftingMonitorRenderer: the job's final output on the monitor's front.
fn monitor_submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let Some(Tag::String(shown)) = event.entity.nbt.get(&java("shown")) else { return };
    let Some(key) = crate::cells::key_from_string(&shown.to_string_lossy()) else { return };
    let Some(stack) = crate::cells::stack_of(host, &key, 1) else { return };
    let Some(level) = host.level(host.dimension()) else { return };
    let facing = level.block_property(host, event.pos, "facing").unwrap_or_else(|| "north".to_owned());
    let draw = &mut *event.draw;
    draw.push();
    let c = [0.5, 0.5, 0.5];
    match facing.as_str() {
        "south" => draw.rotate_about([0.0, 1.0, 0.0], 180.0, c),
        "east" => draw.rotate_about([0.0, 1.0, 0.0], -90.0, c),
        "west" => draw.rotate_about([0.0, 1.0, 0.0], 90.0, c),
        "up" => draw.rotate_about([1.0, 0.0, 0.0], 90.0, c),
        "down" => draw.rotate_about([1.0, 0.0, 0.0], -90.0, c),
        _ => {}
    }
    // Items draw centred on the pose's origin (the block's corner): onto the front's middle, a little out.
    draw.translate([0.5, 0.5, -0.03]);
    draw.scale([0.6, 0.6, 0.6]);
    draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
    draw.pop();
}

/// The drive's cells (DriveModel's cell parts): each in a hole of the drive's front, with its status light.
fn drive_submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let Some(Tag::List(cells)) = event.entity.nbt.get(&java("cells")) else { return };
    let Some(level) = host.level(host.dimension()) else { return };
    let facing = level.block_property(host, event.pos, "facing").unwrap_or_else(|| "north".to_owned());
    let spin: f32 = level.block_property(host, event.pos, "spin").and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let draw = &mut *event.draw;
    draw.push();
    let c = [0.5, 0.5, 0.5];
    match facing.as_str() {
        "south" => draw.rotate_about([0.0, 1.0, 0.0], 180.0, c),
        "east" => draw.rotate_about([0.0, 1.0, 0.0], -90.0, c),
        "west" => draw.rotate_about([0.0, 1.0, 0.0], 90.0, c),
        "up" => draw.rotate_about([1.0, 0.0, 0.0], 90.0, c),
        "down" => draw.rotate_about([1.0, 0.0, 0.0], -90.0, c),
        _ => {}
    }
    draw.rotate_about([0.0, 0.0, 1.0], -90.0 * spin, c);
    for (slot, look) in cells.iter().enumerate().take(10) {
        let Tag::String(look) = look else { continue };
        let look = look.to_string_lossy();
        let Some((model, state)) = look.split_once(':') else { continue };
        // Two columns of five from the top; x is mirrored seen from the north. A plate a fifth of a texel in front
        // of the face (geometry inside an opaque block is culled whole).
        let (col, row) = (slot % 2, slot / 2);
        let x0 = if col == 0 { 9.0 } else { 2.0 };
        let y1 = 16.0 - (1.0 + 3.0 * row as f32);
        let sprite = format!("ae2:block/drive/cells/{model}");
        let face = |uv: [f32; 4]| Some(CubeFace::new(sprite.as_str()).with_uv(uv));
        draw.cube(
            [x0, y1 - 2.0, -0.2],
            [x0 + 5.0, y1, 0.0],
            [face([0.0, 4.0, 5.0, 6.0]), face([0.0, 0.0, 5.0, 2.0]), face([0.0, 2.0, 5.0, 4.0]), None, None, None],
            0xFFFF_FFFF,
        );
        let light = crate::cells::STATUS_COLORS[state.parse::<usize>().unwrap_or(0).min(4)];
        if light != 0 {
            let lx = if col == 0 { x0 } else { x0 + 4.0 };
            draw.cube([lx, y1 - 1.5, -0.3], [lx + 1.0, y1 - 0.5, -0.2], [None, None, face([5.0, 2.0, 6.0, 3.0]), None, None, None], 0xFF00_0000 | light);
        }
    }
    draw.pop();
}

/// A ticking entity's saved compound, item slots and menu data, read from its view.
fn tick_fields(v: &BlockEntityView<'_>) -> Fields {
    Fields {
        nbt: v.nbt().clone(),
        items: (0..v.item_slots() as u32).map(|i| v.item_slot(i).and_then(|s| s.to_owned())).collect(),
        data: v.data_slots().to_vec(),
    }
}

/// Writes `after` into the view where it differs from `before`: each changed saved field, each changed slot, the data.
fn write_tick_fields(v: &mut BlockEntityView<'_>, before: &Fields, after: Fields) {
    if after.nbt != before.nbt {
        for (k, t) in after.nbt.iter() {
            if before.nbt.get(k) != Some(t) {
                v.set_nbt(k.clone(), Some(t.clone()));
            }
        }
        for k in before.nbt.keys() {
            if !after.nbt.contains_key(k) {
                v.set_nbt(k.clone(), None);
            }
        }
    }
    for (i, stack) in after.items.into_iter().enumerate() {
        if before.items.get(i) != Some(&stack) {
            let _ = v.set_item_slot(i as u32, stack);
        }
    }
    if after.data != before.data {
        v.set_data_slots(&after.data);
    }
}

#[mod_api::sunrise_part]
impl BlockEntityPart for Device {
    fn block_entity_ticks(&self, _host: &dyn Host, event: &mut BlockEntityTicksEvent<'_>) {
        event.ticks = crate::net::is_device(event.block) || crate::bus::is_cell_block(event.block) || crate::sky_chest::ticks(event.block) || crate::misc_blocks::ticks(event.block);
    }

    // polish: client ticks for the ambient particles and the assembler's sparks.
    fn block_entity_client_ticks(&self, _host: &dyn ClientHost, event: &mut BlockEntityClientTicksEvent<'_>) {
        if crate::effects::client_ticks(event.block) {
            event.ticks = true;
        }
    }

    fn block_entity_client_tick(&self, event: &mut BlockEntityClientTickEvent<'_>) {
        crate::effects::client_tick(event);
    }

    fn block_entity_tick(&self, event: &mut BlockEntityTickEvent<'_>) {
        // The entity's fields as they are, worked on as before and written back where they changed.
        let Some((pos, handle, before)) = event.subject.with_view(|v| (v.position(), v.handle(), tick_fields(&v))) else { return };
        let mut fields = before.clone();
        {
            let host = event.subject.host();
            let Some(level) = here(host) else { return };
            if level.block(&*host, pos).is_some_and(|b| crate::bus::is_cell_block(b.id(&*host).unwrap_or_default())) {
                crate::bus::tick(host, pos, &mut fields);
            } else {
                let block = crate::net::block_id(host, level, pos);
                if crate::sky_chest::ticks(&block) {
                    crate::sky_chest::chest_tick(host, pos, &mut fields);
                } else if crate::misc_blocks::ticks(&block) {
                    // [blocks] The light detector and the crank.
                    crate::misc_blocks::tick(host, pos, &block, &mut fields);
                } else if !crate::net::is_device(&block) {
                    return;
                } else {
                    run_device(host, pos, &block, &mut fields);
                }
            }
        }
        // Written back only where something changed (each write is a new revision clients may redraw for).
        if fields == before {
            return;
        }
        let saved = fields.nbt != before.nbt || fields.items != before.items;
        event.subject.with_view(|mut v| write_tick_fields(&mut v, &before, fields));
        // The view leaves the chunk as it was: what is saved changed (BlockEntity.setChanged, as the tick's reply did).
        if saved {
            let _ = handle.set_changed(event.subject.host());
        }
    }

    fn block_entity_update_tag(&self, event: &mut BlockEntityUpdateTagEvent<'_>) -> Option<Compound> {
        if event.entity.nbt.contains_key(&java("cable")) {
            return Some(crate::bus::update_tag(&event.entity.nbt));
        }
        // [tools] A paint block's splotches.
        if crate::paint::is_paint(event.entity.id()) {
            return crate::paint::update_tag(&event.entity.nbt);
        }
        // A sky stone chest's lid, the sky stone tank's fluid.
        let id = event.entity.id();
        // [blocks] The crank's turns; the spatial anchor's overlay.
        if id == crate::misc_blocks::CRANK {
            return Some(crate::misc_blocks::update_tag(&event.entity.nbt));
        }
        if id == crate::anchor::ANCHOR {
            return Some(crate::anchor::update_tag(&event.entity.nbt));
        }
        if crate::sky_chest::is_sky_chest(id) || id == crate::sky_chest::TANK {
            return crate::sky_chest::update_tag(&event.entity.nbt);
        }
        // polish: the charger's item, the assembler's craft (their renderers).
        if let Some(tag) = crate::effects::update_tag(id, &event.entity.nbt) {
            return Some(tag);
        }
        // The drive's cells' looks (DriveBlockEntity.writeToStream): what its renderer draws.
        // The inscriber's window (InscriberBlockEntity.writeToStream): its items and the press.
        if let Some(look) = event.entity.nbt.get(&java("look")) {
            let mut out = Compound::new();
            out.insert(java("look"), look.clone());
            return Some(out);
        }
        // The crafting monitor's item (CraftingMonitorBlockEntity.writeToStream).
        if let Some(shown) = event.entity.nbt.get(&java("shown")) {
            let mut out = Compound::new();
            out.insert(java("shown"), shown.clone());
            return Some(out);
        }
        let cells = event.entity.nbt.get(&java("cells"))?;
        let mut out = Compound::new();
        out.insert(java("cells"), cells.clone());
        Some(out)
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        if crate::bus::is_cell_block(event.block) {
            crate::bus::submit(host, event);
            return;
        }
        // polish: the assembler's light and craft, the charger's item.
        if crate::effects::submit(host, event) {
            return;
        }
        if event.block == "ae2:drive" {
            drive_submit(host, event);
        }
        // [tools] Paint splotches.
        if crate::paint::is_paint(event.block) {
            crate::paint::submit(host, event);
            return;
        }
        if event.block == "ae2:crafting_monitor" {
            monitor_submit(host, event);
        }
        if event.block == crate::anchor::ANCHOR {
            crate::anchor::submit(event);
            return;
        }
        // [blocks] The crank's handle.
        if event.block == crate::misc_blocks::CRANK {
            crate::misc_blocks::crank_submit(host, event);
            return;
        }
        if event.block == "ae2:chest" {
            crate::me_chest::chest_submit(host, event);
        } else if crate::sky_chest::is_sky_chest(event.block) {
            crate::sky_chest::chest_submit(host, event);
        } else if event.block == crate::sky_chest::TANK {
            crate::sky_chest::tank_submit(host, event);
        } else if event.block == crate::machines::INSCRIBER {
            crate::inscriber::submit(host, event);
        }
    }

    /// DriveBlockEntity's CellValidInventoryFilter: hoppers, pipes and shift-clicks put only storage cells into a
    /// drive's slots.
    fn can_place_item(&self, event: &mut CanPlaceItemEvent<'_>) -> Option<bool> {
        (event.entity.id() == "ae2:drive").then(|| crate::cells::is_cell_item(crate::cells::sid(event.stack)))
    }

    fn menu_quick_move(&self, event: &mut MenuQuickMoveEvent<'_>) -> Flow {
        // The entity's fields as they are, worked on as before and written back where they changed.
        let Some((pos, before)) = event.subject.with_view(|v| (v.position(), tick_fields(&v))) else { return Flow::Continue };
        let mut fields = before.clone();
        let (container_slots, slot) = (event.container_slots, event.slot);
        let flow = block_quick_move(event.subject.host(), pos, container_slots, slot, &mut fields, &mut *event.stack);
        if fields != before {
            event.subject.with_view(|mut v| write_tick_fields(&mut v, &before, fields));
        }
        flow
    }
}

/// `menu_button` on a block's menu: `entity` is its fields, `carried` the player's cursor stack.
fn block_menu_button(host: &mut dyn ServerHost, player: u128, id: i32, pos: Pos, entity: &mut Fields, carried: &mut Option<ItemStack>) {
    // A cable bus's menu: the button is its open part's, on that part's own fields.
    if here(&*host).is_some_and(|l| l.block(&*host, pos).is_some_and(|b| crate::bus::is_cell_block(b.id(&*host).unwrap_or_default()))) {
        let Some((node, block, side)) = crate::bus::opened_part(&*host, player, pos) else { return };
        let e: &mut Fields = entity;
        let mut f = crate::bus::side_fields(e, side);
        if crate::iface_fluids::menu_button(host, player, node, &block, id, &mut f, carried) || crate::priority::menu_button(host, player, &block, id, &mut f) {
            crate::bus::put_side(e, side, f);
            return;
        }
        if !crate::autocraft::menu_button(host, player, node, &block, id, &mut f, carried) {
            if crate::parts::has_screen(&block) {
                let carried = carried.clone();
                crate::parts::menu_button(host, player, node, &block, id, &mut f, &carried);
            } else {
                crate::storage::menu_button(host, player, node, id, &mut f, carried);
            }
        }
        crate::bus::put_side(e, side, f);
        return;
    }
    let block = here(&*host).map(|l| crate::net::block_id(&*host, l, pos)).unwrap_or_default();
    let e: &mut Fields = entity;
    if crate::iface_fluids::menu_button(host, player, pos, &block, id, e, carried) || crate::priority::menu_button(host, player, &block, id, e) {
        return;
    }
    if crate::machines::menu_button(host, player, pos, &block, id, e) || crate::anchor::menu_button(&block, id, e) {
        return;
    }
    if crate::autocraft::menu_button(host, player, pos, &block, id, e, carried) {
        return;
    }
    if crate::parts::has_screen(&block) {
        let carried = carried.clone();
        crate::parts::menu_button(host, player, pos, &block, id, e, &carried);
        return;
    }
    crate::storage::menu_button(host, player, pos, id, e, carried);
}

/// `menu_quick_move` on a block's menu at `pos`: `entity` is its fields, `source` the clicked slot's stack.
fn block_quick_move(host: &mut dyn ServerHost, pos: Pos, container_slots: usize, slot: usize, entity: &mut Fields, source: &mut Option<ItemStack>) -> Flow {
    if container_slots == 6 * crate::bus::SLOTS {
        // A cable bus's menu: within the open part's slots only (the others are hidden: nothing goes there).
        let Some((node, side, own)) = crate::bus::quick_move_part(&*host, pos, entity) else {
            crate::util::debug(&format!("bus: quick move at {:?}: no open part", pos));
            return Flow::Cancel;
        };
        if slot < container_slots {
            return if slot / crate::bus::SLOTS == side { Flow::Continue } else { Flow::Cancel };
        }
        let e: &mut Fields = entity;
        let mut f = crate::bus::side_fields(e, side);
        if source.is_none() {
            return Flow::Continue;
        }
        let done = crate::storage::quick_move(host, node, slot - container_slots + own, own, &mut f, source);
        crate::bus::put_side(e, side, f);
        // Cards into a bus and the like: the game's move, which the slot rules keep to this part's slots.
        return if done { Flow::Cancel } else { Flow::Continue };
    }
    // An empty slot: nothing to move.
    let Some(stack) = source.clone() else { return Flow::Continue };
    if crate::storage::quick_move(host, pos, slot, container_slots, entity, source) {
        return Flow::Cancel;
    }
    // A drive's or an ME chest's cell slots, from the inventory (AEBaseMenu.quickMoveStack: the stack goes to the
    // first slot that takes it), when this hook is asked (a LAN guest's click on main), one cell per slot.
    if slot >= container_slots && crate::cells::is_cell_item(crate::cells::sid(&stack)) {
        let block = here(&*host).map(|l| crate::net::block_id(&*host, l, pos)).unwrap_or_default();
        let cell_slots = match block.as_str() {
            "ae2:drive" => 10,
            "ae2:chest" if !entity.items.is_empty() => 1,
            _ => 0,
        };
        let e: &mut Fields = entity;
        if let Some(k) = (0..cell_slots.min(e.items.len())).find(|&k| e.items[k].is_none()) {
            e.items[k] = crate::cells::with_count(&stack, 1);
            *source = crate::cells::with_count(&stack, stack.count() - 1);
            return Flow::Cancel;
        }
    }
    Flow::Continue
}

// ---- The screens of AE2's menus (client): one factory per menu type, for the AE2 block the menu was opened for

/// The layout of the screen `block`'s menu opens (a cable bus's: its open part's).
fn screen_layout(block: &str) -> Option<MenuScreenLayout> {
    let own = |b: &str| {
        crate::screens::screen(b)
            .or_else(|| crate::part_screens::screen(b))
            .or_else(|| crate::craft_screens::screen(b))
            .or_else(|| crate::generators::screen(b))
            .or_else(|| crate::sky_chest::screen(b))
            .or_else(|| crate::machines::screen(b))
            .or_else(|| crate::wireless::screen(b))
            .or_else(|| crate::qnb::screen(b))
            .or_else(|| crate::anchor::screen(b))
    };
    if crate::bus::is_cell_block(block) {
        return crate::bus::screen(own);
    }
    own(block)
}

/// `MenuScreenFactory` for AE2's menus: only a menu opened for an AE2 block has one of these screens.
pub(crate) fn menu_screen(_host: &dyn ClientHost, open: MenuScreenOpen<'_>) -> Option<MenuScreen> {
    let opened = open.block?.to_owned();
    let layout = screen_layout(&opened)?;
    let (draw_block, click_block, tip_block) = (opened.clone(), opened.clone(), opened);
    Some(
        MenuScreen::new(layout)
            .with_draw(move |host, frame, draw| screen_draw(host, &draw_block, frame.data, draw, frame.origin))
            .with_click(move |host, frame, right| screen_click(host, &click_block, frame.data, frame.at, right))
            .with_tooltip(move |host, frame, _slot, lines| {
                // Also asked over a filled slot, its lines starting as the item's tooltip:
                // AE2's screens only add lines over no item, so a filled slot keeps the item's own.
                if !lines.is_empty() {
                    return;
                }
                let mut own: Vec<String> = Vec::new();
                screen_tooltip_lines(host, &tip_block, frame.data, frame.at, &mut own);
                // Plain lines with legacy colour codes, drawn as before: a literal in the default colour.
                lines.extend(own.into_iter().map(|l| (Text::literal(l), u32::MAX)));
            }),
    )
}

fn screen_draw(host: &dyn ClientHost, opened: &str, data: &[i32], draw: &mut dyn DrawContext, origin: [f32; 2]) {
    // A cable bus's menu draws as its open part's screen.
    let hosted = crate::bus::client_part(opened, data);
    let (block, data) = match &hosted {
        Some((b, d)) => (b.as_str(), d.as_slice()),
        None => (opened, data),
    };
    if crate::generators::screen_draw(host, block, data, draw, origin)
        || crate::sky_chest::screen_draw(host, block, draw, origin)
        || crate::wireless::screen_draw(host, block, data, draw, origin)
        || crate::qnb::screen_draw(host, block, draw, origin)
        || crate::anchor::screen_draw(host, block, data, draw, origin)
    {
        crate::priority::draw_tab(block, draw, origin);
        return;
    }
    if crate::craft_screens::screen_draw(host, block, data, draw, origin) || crate::machines::screen_draw(host, block, data, draw, origin) {
        crate::priority::draw_tab(block, draw, origin);
        return;
    }
    if !crate::part_screens::screen_draw(host, block, data, draw, origin) {
        crate::screens::screen_draw(host, block, draw, origin);
    }
    // The priority tab (drive, ME chest, storage bus, formation plane, interface).
    crate::priority::draw_tab(block, draw, origin);
}

fn screen_click(host: &dyn ClientHost, opened: &str, data: &[i32], at: [f32; 2], right: bool) -> Option<i32> {
    let hosted = crate::bus::client_part(opened, data);
    let (block, data) = match &hosted {
        Some((b, d)) => (b.as_str(), d.as_slice()),
        None => (opened, data),
    };
    if let Some(id) = crate::priority::click_tab(block, at).or_else(|| crate::generators::screen_click(block, at)).or_else(|| crate::anchor::screen_click(block, at).map(Some)) {
        return id;
    }
    if let Some(id) = crate::craft_screens::screen_click(host, block, data, at, right) {
        return id;
    }
    if let Some(id) = crate::machines::screen_click(block, data, at, right) {
        return id;
    }
    match crate::part_screens::screen_click(host, block, data, at, right) {
        Some(id) => id,
        None => crate::screens::screen_click(host, block, at, right),
    }
}

/// Slots a screen hides as it opens (`None` in its layout). The server's menu had no such slot while the layout hid
/// them on both sides; Sunrise main hides them on the client only, so the server refuses them here: the level
/// emitters' unused slot, the crafting terminal's result (the server crafts on a click), the ME chest's cell while
/// it shows its terminal.
fn hidden_slot(menu: &str, slot: usize, player: Option<u128>) -> bool {
    match (menu, slot) {
        ("ae2:level_emitter" | "ae2:energy_level_emitter", 0) | ("ae2:crafting_terminal", 10) => true,
        ("ae2:chest", 0) => player.is_some_and(crate::storage::chest_term_open),
        _ => false,
    }
}

/// Which menus AE2's screens and menu Part answer.
fn ae2_menus() -> impl Iterator<Item = &'static str> {
    MENU_BLOCKS
        .iter()
        .copied()
        .chain(PART_MENUS)
        .chain(["ae2:pattern_provider", "ae2:crafting_cpu", "ae2:molecular_assembler", "ae2:pattern_access_terminal", crate::generators::VIBRATION, crate::wireless::WAP])
        .chain(crate::machines::MENUS)
        .chain([crate::qnb::LINK, crate::anchor::ANCHOR])
}

/// AE2's tooltip lines at `at` on block `block`'s screen over no item (`screen_tooltip`).
fn screen_tooltip_lines(host: &dyn ClientHost, block: &str, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) {
    let hosted = crate::bus::client_part(block, data);
    let (block, data) = match &hosted {
        Some((b, d)) => (b.as_str(), d.as_slice()),
        None => (block, data),
    };
    if crate::priority::tooltip(host, block, at, lines)
        || crate::generators::screen_tooltip(block, data, at, lines)
        || crate::wireless::screen_tooltip(host, block, at, lines)
        || crate::anchor::screen_tooltip(host, block, data, at, lines)
    {
        return;
    }
    if crate::craft_screens::screen_tooltip(host, block, at, lines) || crate::machines::screen_tooltip(host, block, data, at, lines) {
        return;
    }
    if !crate::part_screens::screen_tooltip(host, block, data, at, lines) {
        crate::screens::screen_tooltip(host, block, at, lines);
    }
}

/// Workaround (Sunrise #4463, fixed by #4505 after Dev 2026-11-22): a new mod block entity holds no item slots until one is written, and the menu it opens
/// is sized by the slots it holds (`lan::menu::shown`), not by its container's. A shift-click on such a menu is then
/// mapped to the wrong inventory slot: the clicked slot keeps its stack while it moves (a duplicate). Writing the last
/// slot once makes the block entity hold all of them; clearing it leaves the contents as they were.
pub(crate) fn pad_slots(host: &mut dyn ServerHost, entity: BlockEntityRef, block: &str) {
    let Some(c) = crate::Mod::container_of(block) else { return };
    let Some(last) = c.slots.checked_sub(1) else { return };
    if !matches!(entity.item_slot(&*host, last as u32), Ok(None)) {
        return;
    }
    let Some(filler) = crate::cells::new_stack(&*host, "minecraft:stone", 1) else { return };
    let r = host.transaction(|host, tx| {
        entity.set_item_slot(host, tx, last as u32, Some(filler)).map_err(|_| ())?;
        entity.set_item_slot(host, tx, last as u32, None).map_err(|_| ())?;
        Ok::<(), ()>(())
    });
    if r.is_err() {
        crate::util::debug(&format!("pad_slots {block}: the slots couldn't be written"));
    }
}
