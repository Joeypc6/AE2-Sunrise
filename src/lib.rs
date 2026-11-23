//! Applied Energistics 2 15.4.12 (TeamAppliedEnergistics, LGPL-3.0) for the arismoko/horizon fork's mod API.
//! Hooks and Parts only, no mixins and no `[[inject]]`, so it builds against the fork's prebuilt SDK.
//!
//! The logic is the upstream port's (`../ae2-horizon/mixins/ae2`), moved onto the fork's APIs:
//! - `core`, `crafting`, `execution`, `grid`: AE2's storage keys and cells, the crafting calculator, the crafting
//!   CPU and the grid (channels, power, network storage). Pure Rust, unchanged from upstream (`cargo test`);
//! - `devices`: AE2's blocks with a block entity, their declared block entity types and Parts;
//! - `net`: the ME network (cables, channels, power); `cells`, `storage`, `screens`: drives, cells, the ME chest and
//!   the terminals (stage 3).
//!
//! Content (blocks, items, recipes, models, the W guide's pages) is in `assets/` and `data/`, written by
//! `tools/make_fork_content.py` from the upstream content mod.

#![allow(dead_code)]

mod commands_menu;
mod autocraft;
// ETA (agent eta): AE2's API for other mods (`register_machine`) and `/ae2debug eta`.
pub mod api;
mod ae2debug;
mod blocks_gen;
mod bus;
mod capdefer;
// fixB: plain smart cables' channel lights per side.
mod cable_lights;
mod cells;
mod config;
mod craft_screens;
mod core;
mod crafting;
mod devices;
mod execution;
mod grid;
mod mathexpr;
mod net;
mod part_screens;
mod parts;
mod screens;
mod storage;
mod term_ui;
mod util;
mod spatial;
mod world;
mod tool;
mod transfer;
mod planes;
mod meteor;
mod generators;
mod priority;
mod sky_chest;
mod me_chest;
mod comparator;
mod machines;
mod inscriber;
mod io_port;
mod condenser;
mod craft_cards;
mod iface_fluids;
mod wrench;
mod cell_tools;
mod powered;
mod wireless;
mod portable;
mod toolbox;
// enc: the encoder's panel, the pattern access terminal, JEI's "+" on the crafting terminal.
mod encoder;
mod access_term;
mod ct_transfer;
// jeix: JEI's "+" on the wireless crafting terminal, and the facades JEI lists.
mod wct_transfer;
mod jei_facades;
// [tools] The matter cannon, colour applicator, paint, charged staff, entropy manipulator, compass, knife.
mod applicator;
mod cannon;
mod handtools;
mod knife;
mod paint;
mod tool_cells;
// [blocks] Gap-list item 8.
mod monitors;
mod links;
mod misc_blocks;
mod qnb;
mod ext_cache; // fixA: the storage bus's cached container
mod xlink; // fixA: quantum bridges across levels
mod anchor;
mod tiny_tnt;
// polish: visuals and sounds, commands, advancements, JEI extras.
mod effects;
mod commands;
mod advancements;
mod rules;
mod cell_icons;
#[cfg(feature = "jei")]
mod jei_extra;
#[cfg(feature = "jade")]
mod jade;

use mod_api::*;

#[derive(Default)]
pub(crate) struct Mod;

/// The item tint source of the storage cells' status light (BasicStorageCell's item colour, index 1).
const CELL_TINT: &str = "ae2:cell_status";

/// A serverbound channel: the sender is the connection's player, never one named in the bytes.
struct FromClient(&'static str);
impl IPayloadHandler for FromClient {
    fn handle(&self, payload: PayloadBytes, cx: &mut dyn IPayloadContext) -> Result<(), PayloadHandleError> {
        let Ok(player) = cx.player() else { return Ok(()) };
        let player = player.uuid();
        let Ok(host) = cx.server_host() else { return Ok(()) };
        let data = &payload.0;
        if self.0 == bus::NET_BUS {
            // A level typed into an emitter's screen, and the bus's other requests.
            bus::from_client(host, player, data);
        } else if self.0 == parts::NET_PART {
            parts::from_client(host, player, data);
        } else if self.0 == autocraft::NET_CRAFT {
            autocraft::from_client(host, player, data);
        } else if self.0 == wireless::NET_WIRELESS {
            wireless::from_client(host, player, data);
        } else if self.0 == handtools::NET_TOOLS {
            handtools::from_client(host, player, data);
        }
        Ok(())
    }
}

/// A clientbound channel, dispatched as the fork's `net_from_server` was.
struct FromServer(&'static str);
impl IPayloadHandler for FromServer {
    fn handle(&self, payload: PayloadBytes, cx: &mut dyn IPayloadContext) -> Result<(), PayloadHandleError> {
        let Ok(host) = cx.client_host() else { return Ok(()) };
        let (channel, data) = (self.0, &payload.0[..]);
        if priority::from_server(channel, data) || bus::from_server(channel, data) || tool::from_server(host, channel, data) || wireless::from_server(host, channel, data) || handtools::from_server(host, channel, data) || craft_screens::from_server(channel, data) {
            return Ok(());
        }
        if !part_screens::from_server(channel, data) {
            screens::from_server(&*host, channel, data);
        }
        Ok(())
    }
}

/// The block entities' storage, declared once per block at registration (the fork asked hooks for these; each answer
/// depends on the block alone).
impl Mod {
    fn declare_storage(r: &mut dyn Registrar) {
        for blocks in devices::kinds().into_values() {
            for block in blocks {
                let (energy, container, fluids) = (Self::energy_of(&block), Self::container_of(&block), Self::fluids_of(&block));
                if energy.is_none() && container.is_none() && fluids.is_none() {
                    continue;
                }
                let key = r.block_key(&block);
                if let Some(e) = energy {
                    r.block_energy(&key, e);
                }
                if let Some(c) = container {
                    r.block_container(&key, c);
                }
                if let Some(f) = fluids {
                    r.block_fluids(&key, f);
                }
            }
        }
    }

    /// AEBasePoweredBlockEntity's energy storage on the acceptor and the controller: an insert-only FE buffer, which
    /// the fork's ENERGY capability answers for every mod; each tick it goes into the network as AE.
    fn energy_of(block: &str) -> Option<ModEnergyBuffer> {
        if let Some(e) = machines::energy_of(block) {
            return Some(e);
        }
        // A cable bus's buffer is its acceptors' (the other sides answer nothing: `bus::energy`).
        (net::takes_fe(block) || bus::is_cell_block(block)).then(|| ModEnergyBuffer {
            capacity: net::FE_BUFFER,
            max_insert: net::FE_BUFFER,
            max_extract: 0,
            insert_faces: [true; 6],
            extract_faces: [false; 6],
        })
    }

    /// The menus of stage 3: the drive's ten cells, the ME chest's one, a terminal's insert slot (never shown) and
    /// five view cells, the crafting terminal's grid (slot 10, its result, is no slot: the server crafts on a click).
    /// Hoppers reach none of them.
    fn container_of(block: &str) -> Option<ModContainer> {
        // The inscriber, IO port and condenser (`machines`).
        if let Some(c) = machines::container_of(block) {
            return Some(c);
        }
        // The wireless access point's boosters and link slots.
        if let Some(c) = wireless::container_of(block) {
            return Some(c);
        }
        // [blocks] The spatial anchor's menu (its statistics; no slot shown).
        if let Some(c) = anchor::container_of(block) {
            return Some(c);
        }
        // [blocks] The quantum link chamber's singularity.
        if let Some(c) = qnb::container_of(block) {
            return Some(c);
        }
        // A cable bus: each side's part's slots (its own menu's, at `side * SLOTS`); the open part's are shown.
        if bus::is_cell_block(block) {
            // No face of its own reaches them: each side's part answers ITEMS (`bus::items`), over these.
            return Some(ModContainer {
                slots: 6 * bus::SLOTS,
                faces: std::array::from_fn(|_| Vec::new()),
                inputs: (0..6 * bus::SLOTS).collect(),
                outputs: (0..6 * bus::SLOTS).collect(),
                insert_rules: Vec::new(),
                menu: Some(bus::MENU.to_owned()),
            });
        }
        // Stage 4's parts and the cell workbench: their cards (and an interface's nine stored stacks, which hoppers and
        // pipes reach: its ITEMS provider in `parts` keeps each to its config).
        // The charger's one slot (no menu: a use puts in and takes out); hoppers reach it from every side.
        if block == "ae2:charger" {
            return Some(ModContainer { slots: 1, faces: std::array::from_fn(|_| vec![0]), inputs: vec![0], outputs: vec![0], insert_rules: Vec::new(), menu: None });
        }
        // The vibration chamber's fuel and three cards (hoppers reach the fuel only: `generators::vibration_items`);
        // the sky stone chests' 36 slots.
        if generators::is_vibration(block) {
            return Some(ModContainer {
                slots: generators::VIB_SLOTS,
                faces: std::array::from_fn(|_| vec![0]),
                inputs: vec![0],
                outputs: vec![0],
                insert_rules: Vec::new(),
                menu: Some(generators::VIBRATION.to_owned()),
            });
        }
        if let Some(c) = sky_chest::container(block) {
            return Some(c);
        }
        if let Some((menu, slots)) = parts::menu_of(block) {
            let reach: Vec<usize> = if parts::kind(block) == Some(parts::Kind::Interface) { (0..9).collect() } else { Vec::new() };
            return Some(ModContainer {
                slots,
                faces: std::array::from_fn(|_| reach.clone()),
                inputs: reach.clone(),
                outputs: reach,
                insert_rules: Vec::new(),
                menu: Some(menu.to_owned()),
            });
        }
        // Stage 5: a provider's nine patterns and nine return slots (machines and hoppers put into those from any
        // side); a crafting CPU's status (no slots shown); the pattern encoding terminal's insert slot, blank and
        // encoded pattern, view cells.
        if autocraft::is_provider(block) {
            let ret: Vec<usize> = (9..18).collect();
            return Some(ModContainer {
                slots: 18,
                faces: std::array::from_fn(|_| ret.clone()),
                inputs: ret,
                outputs: Vec::new(),
                insert_rules: Vec::new(),
                menu: Some("ae2:pattern_provider".to_owned()),
            });
        }
        if block == "ae2:molecular_assembler" {
            // The pattern slot, five acceleration cards, the grid and the output (shown only: crafts are worked out).
            return Some(ModContainer {
                slots: 16,
                faces: std::array::from_fn(|_| Vec::new()),
                // Main's Container.canPlaceItem / canTakeItem read these: the menu's clicks need every slot; with no
                // faces, hoppers and pipes still reach none.
                inputs: (0..16).collect(),
                outputs: (0..16).collect(),
                insert_rules: Vec::new(),
                menu: Some("ae2:molecular_assembler".to_owned()),
            });
        }
        if block == "ae2:pattern_access_terminal" {
            return Some(ModContainer {
                slots: 1,
                faces: std::array::from_fn(|_| Vec::new()),
                // Main's Container.canPlaceItem / canTakeItem read these: the menu's clicks need every slot; with no
                // faces, hoppers and pipes still reach none.
                inputs: (0..1).collect(),
                outputs: (0..1).collect(),
                insert_rules: Vec::new(),
                menu: Some("ae2:pattern_access_terminal".to_owned()),
            });
        }
        if net::is_crafting(block) {
            return Some(ModContainer {
                slots: 1,
                faces: std::array::from_fn(|_| Vec::new()),
                // Main's Container.canPlaceItem / canTakeItem read these: the menu's clicks need every slot; with no
                // faces, hoppers and pipes still reach none.
                inputs: (0..1).collect(),
                outputs: (0..1).collect(),
                insert_rules: Vec::new(),
                menu: Some("ae2:crafting_cpu".to_owned()),
            });
        }
        if block == "ae2:spatial_io_port" {
            // Cell in at 0, used cell out at 1: hoppers and pipes put in and take out as Java's sided inventory.
            return Some(ModContainer {
                slots: 2,
                faces: std::array::from_fn(|_| vec![0, 1]),
                inputs: vec![0],
                outputs: vec![1],
                insert_rules: Vec::new(),
                menu: Some("ae2:spatial_io_port".to_owned()),
            });
        }
        let slots = match block {
            "ae2:drive" => 10,
            "ae2:chest" => 1,
            "ae2:terminal" => 1 + storage::VIEW_CELLS,
            "ae2:crafting_terminal" => 11 + storage::VIEW_CELLS,
            "ae2:pattern_encoding_terminal" => 3 + storage::VIEW_CELLS,
            _ => return None,
        };
        Some(ModContainer {
            slots,
            // The drive's cells face every side, as Java's AEBaseInvBlockEntity.getExposedInventoryForSide gives its
            // whole internal inventory (DriveBlockEntity's CellValidInventoryFilter: cells only, `can_place_item`).
            // The ME chest's sides are its own ITEMS provider (`me_chest`); terminals show no inventory.
            faces: std::array::from_fn(|_| if block == "ae2:drive" { (0..slots).collect() } else { Vec::new() }),
            // Main's Container.canPlaceItem / canTakeItem read these: the menu's clicks need every slot; with no
            // faces, hoppers and pipes still reach none.
            inputs: (0..slots).collect(),
            outputs: (0..slots).collect(),
            insert_rules: Vec::new(),
            menu: Some(block.to_owned()),
        })
    }

    /// A pattern provider's return tanks (PatternProviderReturnInventory's fluid slots): machines and pipes fill them
    /// from any side; its tick moves them into the network.
    fn fluids_of(block: &str) -> Option<ModFluidTanks> {
        if let Some(t) = machines::fluids_of(block) {
            return Some(t);
        }
        // A cable bus keeps each side's provider's nine return tanks; its sides answer through `bus::fluids`.
        if bus::is_cell_block(block) {
            let mut t = ModFluidTanks::default();
            t.tanks = vec![16_000; 6 * bus::TANKS];
            t.faces = std::array::from_fn(|_| Vec::new());
            t.inputs = (0..6 * bus::TANKS).collect();
            return Some(t);
        }
        if let Some(t) = sky_chest::fluids(block) {
            return Some(t);
        }
        // The interface's fluid stock (`iface_fluids`): nine tanks of four buckets.
        if matches!(block, "ae2:interface" | "ae2:cable_interface") {
            let mut t = ModFluidTanks::default();
            t.tanks = vec![iface_fluids::TANK_MB as i32; iface_fluids::TANKS];
            t.faces = std::array::from_fn(|_| (0..iface_fluids::TANKS).collect());
            t.inputs = (0..iface_fluids::TANKS).collect();
            t.outputs = (0..iface_fluids::TANKS).collect();
            return Some(t);
        }
        autocraft::is_provider(block).then(|| {
            let mut t = ModFluidTanks::default();
            t.tanks = vec![16_000; 9];
            t.faces = std::array::from_fn(|_| (0..9).collect());
            t.inputs = (0..9).collect();
            t
        })
    }

}

#[mod_api::sunrise_mod]
impl Hooks for Mod {
    /// The cables' pieces (`hz_cable/*`), which a cable bus cell draws one by one (bus.rs, CableBusBakedModel).
    fn standalone_models(&self, _cx: &RegistrationContext, event: &mut StandaloneModelsEvent<'_>) {
        // fixE: the cell's quartz fiber rods (bus::part_model_ids); fixB: the plain smart cables' lit overlays (cable_lights).
        for id in bus::cable_model_ids().into_iter().chain(bus::part_model_ids()).chain(cable_lights::model_ids()) {
            event.register_model(id);
        }
    }

    fn model_baking_completed(&self, host: &mut dyn ClientHost, _event: &mut ModelBakingCompletedEvent) {
        bus::resolve_cable_models(host);
        cable_lights::resolve_models(host); // fixB
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        // `/<ns>` commands menu: the client's side of its channel.
        commands_menu::register_payload(r);
        // Item entities in water (`entity_tick_post`): told after each one's own tick.
        r.entity_tick_each("minecraft:item");
        // AE2's machine recipes, read with `recipes_of_type`: Sunrise refuses a recipe file whose serializer no mod
        // registered, so each type and its serializer (the files' `type`, the same id) are declared here.
        // [tools] `ae2:matter_cannon`: the matter cannon's ammo (a data pack's; AE2's own are in `tool_cells`).
        for kind in ["ae2:charger", "ae2:entropy", "ae2:inscriber", "ae2:transform", "ae2:matter_cannon"] {
            r.register_recipe_type(kind);
            r.register_raw_recipe_serializer(kind, kind);
        }
        devices::register(r);
        cable_lights::register(r); // fixB
        Self::declare_storage(r);
        let _ = util::ITEM_ENTITY.set(r.entity_type_key("minecraft:item"));
        // The fork's raw channels, as Sunrise payloads with the same bytes.
        {
            use std::sync::Arc;
            let mut payloads = r.payload_handlers();
            let mut reg = payloads.registrar("1");
            for ch in [bus::NET_BUS, parts::NET_PART, wireless::NET_WIRELESS, handtools::NET_TOOLS] {
                reg.play_bidirectional(PayloadId(ch.into()), Arc::new(FromClient(ch)), Some(Arc::new(FromServer(ch))));
            }
            reg.play_to_server(PayloadId(autocraft::NET_CRAFT.into()), Arc::new(FromClient(autocraft::NET_CRAFT)));
            for ch in [
                autocraft::NET_PLAN,
                autocraft::NET_CPU,
                autocraft::NET_ENC,
                autocraft::NET_PAT,
                autocraft::NET_ASM,
                storage::NET_ITEMS,
                storage::NET_EXTRAS,
                tool::NET_STATUS,
                priority::NET_PRIORITY,
            ] {
                reg.play_to_client(PayloadId(ch.into()), Some(Arc::new(FromServer(ch))));
            }
        }
        spatial::register(r);
        world::register(r);
        powered::register(r);
        wireless::register(r);
        meteor::register(r);
        handtools::register(r);
        anchor::register(r);
        tiny_tnt::register(r);
        effects::register(r); // polish
        rules::register(r); // polish
        cell_icons::register(r); // polish
    }

    fn register_villager_professions(&self, _context: &RegistrationContext, event: &mut RegisterVillagerProfessionsEvent<'_>) {
        world::professions(event);
    }

    /// fixF: the cells other levels wrote for this level's drives and chests go onto them before anything here uses
    /// them (`storage::flush_held`).
    fn level_tick_pre(&self, host: HostSide, event: &mut LevelTickPreEvent) {
        if let HostSide::Server(host) = host {
            storage::flush_held(host, event.level);
        }
    }

    fn level_tick_post(&self, host: HostSide, event: &mut LevelTickPostEvent) {
        if let HostSide::Server(host) = host {
            // polish: `/ae2 tickmonitor` times AE2's level tick.
            let timer = commands::monitoring().then(std::time::Instant::now);
            screens::server_thread_here();
            cells::ids(&*host);
            net::level_tick(host, event.level);
            bus::level_tick(host, event.level);
            storage::level_tick(host, event.level);
            let dim = host.dimension();
            let tick = net::with_net(dim, |n| n.tick);
            world::transform_scan(host, event.level, dim, tick);
            parts::level_tick(host, event.level);
            spatial::level_tick(host, event.level);
            autocraft::level_tick(host, event.level);
            meteor::level_tick(host, event.level);
            comparator::level_tick(host, event.level);
            inscriber::export_pending(host, event.level);
            craft_cards::level_tick(host, event.level);
            paint::level_tick(host, event.level);
            handtools::level_tick(host, tick);
            misc_blocks::level_tick(host, event.level);
            anchor::level_tick(host, event.level);
            tiny_tnt::level_tick(host, event.level);
            advancements::level_tick(host, event.level); // polish
            if let Some(t) = timer {
                commands::tick_timed(t.elapsed().as_nanos() as u64);
            }
        }
    }

    /// polish: sky stone breaks faster with a good tool (SkyStoneBreakSpeed).
    fn player_break_speed(&self, host: HostSide, event: &mut PlayerBreakSpeedEvent) -> Flow {
        let h: &dyn Host = match &host {
            HostSide::Server(h) => &**h,
            HostSide::Client(h) => &**h,
        };
        rules::break_speed(h, event);
        Flow::Continue
    }

    // ---- polish: AE2's commands (`/ae2`, `/ae2debug`) and the chunk logger.

    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        commands::register(event);
        event.commands.push(commands_menu::command());
        // ETA (agent eta): `/ae2debug eta`.
        ae2debug::register(event);
    }

    fn chunk_load(&self, host: &mut dyn ServerHost, event: &mut ChunkLoadEvent<'_>) {
        commands::chunk_logged(event.dimension, event.chunk, true);
        meteor::chunk_loaded(&*host, event.dimension, event.chunk); // fixA: older meteorites for the compass
    }

    fn chunk_unload(&self, _host: &mut dyn ServerHost, event: &mut ChunkUnloadEvent<'_>) {
        commands::chunk_logged(event.dimension, event.chunk, false);
    }

    /// A network block placed, broken or changed: its level's grids are worked out again at the end of the tick.
    fn block_changed(&self, level: &dyn LevelHost, event: &mut BlockChangedEvent) {
        let ae2 = |s: BlockState| level.state_block(s).is_some_and(|b| b.starts_with("ae2:"));
        if ae2(event.old) || ae2(event.new) {
            let dim = event.dimension.key().id();
            net::mark_dirty(dim);
            let (old, new) = (level.state_block(event.old).map(|b| b.to_string()).unwrap_or_default(), level.state_block(event.new).map(|b| b.to_string()).unwrap_or_default());
            bus::note_changed(dim, [event.pos.x, event.pos.y, event.pos.z], &old, &new);
            // [tools] The meteorite compass's mysterious cubes.
            handtools::block_changed(dim, [event.pos.x, event.pos.y, event.pos.z], &old, &new);
            anchor::note_changed(dim, [event.pos.x, event.pos.y, event.pos.z], &old, &new);
            if bus::is_cell_block(&old) && !bus::is_cell_block(&new) {
                misc_blocks::note_cell([event.pos.x, event.pos.y, event.pos.z], &Default::default());
            }
        }
    }

    /// CableBusBlock.onDestroyedByPlayer: a cell loses what the crosshair is on, a part or a facade; the cable takes
    /// the rest with it.
    fn break_block(&self, host: &mut dyn ServerHost, event: &mut BreakBlockEvent<'_>) -> Flow {
        let cell = host.level(host.dimension()).and_then(|l| l.block(&*host, event.pos)).is_some_and(|b| bus::is_cell_block(b.id(&*host).unwrap_or_default()));
        if cell && bus::break_cell(host, event.pos) {
            return Flow::Cancel;
        }
        Flow::Continue
    }

    /// [blocks] A punch on a conversion monitor takes a stack out (BlockAttackHook); the monitor isn't dug.
    fn left_click_block(&self, host: &mut dyn ServerHost, event: &mut LeftClickBlockEvent) -> Flow {
        let pos = event.pos;
        let cell = host.level(host.dimension()).and_then(|l| l.block(&*host, pos)).is_some_and(|b| bus::is_cell_block(b.id(&*host).unwrap_or_default()));
        if cell && monitors::punch(host, pos, event.face) {
            event.cancelled = true;
            return Flow::Cancel;
        }
        Flow::Continue
    }

    /// [blocks] The spatial anchors' saved chunk tickets.
    fn validate_tickets(&self, host: &mut dyn ServerHost, event: &mut ValidateTicketsEvent<'_>) {
        anchor::validate(host, event);
    }

    /// [blocks] A blast's items: AE2's explosion transforms (entangled singularities).
    fn explosion_detonate(&self, host: &mut dyn ServerHost, event: &mut ExplosionDetonateEvent<'_>) {
        world::explosion_transform(host, event.entities);
        tiny_tnt::blasted(host, event.blocks);
    }

    /// [blocks] A primed tiny TNT blows with AE2's own small blast.
    fn explosion_start(&self, host: &mut dyn ServerHost, event: &mut ExplosionStartEvent<'_>) -> Flow {
        if tiny_tnt::explosion_start(host, event.explosion) {
            event.cancelled = true;
            return Flow::Cancel;
        }
        Flow::Continue
    }

    /// No game outline on a cable bus: the aimed part's own is drawn (`render_level_stage`).
    fn block_highlight(&self, _host: &dyn ClientHost, event: &mut BlockHighlightEvent) -> Flow {
        if bus::is_drawn_cell(event.pos) { Flow::Cancel } else { Flow::Continue }
    }

    fn render_level_stage(&self, host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
        bus::draw_outline(host, event);
        // [blocks] The spatial anchors' overlay.
        anchor::draw_overlay(host, event);
    }

    fn menu_closed(&self, host: &mut dyn ServerHost, event: &mut MenuClosedEvent) {
        // AE2's clearGridOnClose (a terminal setting the player's client told the server): the crafting grid back
        // into the network.
        if let Some((dim, pos, block)) = storage::menu_closed(event.uuid, event.window)
            && block == "ae2:crafting_terminal"
            && autocraft::clears_grid(event.uuid)
        {
            storage::clear_grid_on_close(host, dim, pos);
        }
    }

    /// PlayerContainerEvent.Close: main tells `menu_closed` only of menus a mod opened with no block, so a block's
    /// menu (a terminal, a drive, a hosted part's) ends its record here (AE2's menus' `removed`).
    fn player_container_close(&self, host: &mut dyn ServerHost, event: &mut PlayerContainerCloseEvent<'_>) {
        let Some(pos) = event.pos else { return };
        if let Some((dim, pos, block)) = storage::container_closed(event.player, event.dimension, pos)
            && block == "ae2:crafting_terminal"
            && autocraft::clears_grid(event.player)
        {
            storage::clear_grid_on_close(host, dim, pos);
        }
    }

    fn player_inventory_changed(&self, host: &mut dyn ServerHost, event: &mut PlayerInventoryChangedEvent<'_>) {
        if event.stacks.iter().flatten().any(|s| cells::sid(s) == storage::MOVED) {
            storage::sweep_moved(host, event.player, event.stacks);
        }
    }

    fn player_tick_post(&self, host: HostSide, event: &mut PlayerTickPostEvent) {
        if let HostSide::Server(host) = host
            && let Some(player) = event.player.entity().uuid(host)
        {
            storage::player_tick(host, player);
            wireless::player_tick(host, player);
            access_term::player_tick(host, player);
            ct_transfer::player_tick(host, player);
            let tick = net::with_net(host.dimension(), |n| n.tick);
            handtools::player_tick(host, player, tick);
        }
    }

    fn player_logged_out(&self, _host: &mut dyn ServerHost, event: &mut PlayerLoggedOutEvent<'_>) {
        wireless::logged_out(event.player);
        handtools::logged_out(event.player);
    }

    fn server_starting(&self, host: &mut dyn ServerHost, _event: &mut ServerStartingEvent) {
        config::load(&*host);
        cells::ids(&*host);
        mod_api::if_installed!(host, jade, {
            crate::jade::register();
        });
    }

    /// client.json (and common.json) for the client's own options (config files aren't readable during Setup).
    fn client_player_logging_in(&self, host: &mut dyn ClientHost, _event: &mut ClientPlayerLoggingInEvent) {
        config::load(&*host);
        // AE2's EncodePatternTransferHandler: "+" on these categories while a pattern encoding terminal's screen is
        // open writes the recipe into its cells (the server's `menu_transfer`, `transfer.rs`). A terminal hosted on
        // a cable bus shares the bus's menu type with every other part, so "+" is registered for the block only.
        mod_api::if_installed!(host, jei, {
            for category in transfer::JEI_CATEGORIES {
                jei::api::register_transfer("ae2:pattern_encoding_terminal", category);
            }
            // polish: P2P attunement, catalysts, descriptions.
            crate::jei_extra::register();
        });
    }

    fn server_stopped(&self, _host: &mut dyn ServerHost, _event: &mut ServerStoppedEvent) {
        net::forget_all();
        bus::forget_all();
        storage::forget_all();
        parts::forget_all();
        autocraft::forget_all();
        world::forget_all();
        generators::forget_all();
        sky_chest::forget_all();
        comparator::forget_all();
        machines::forget_all();
        craft_cards::forget_all();
        wrench::forget_all();
        wireless::forget_all();
        access_term::forget_all();
        ct_transfer::forget_all();
        handtools::forget_all();
        links::forget_all();
        misc_blocks::forget_all();
        anchor::forget_all();
        tiny_tnt::forget_all();
        advancements::forget_all(); // polish
    }

    /// An annihilation plane placed: its item's enchantments onto it.
    fn entity_place(&self, host: &mut dyn ServerHost, event: &mut EntityPlaceEvent) -> Flow {
        planes::placed(host, event);
        wrench::placed(host, event);
        Flow::Continue
    }

    /// Items lying in water (AE2's in-world transformation).
    /// Told after each item entity's own tick (`entity_tick_each("minecraft:item")` in `register`).
    fn entity_tick_post(&self, host: HostSide, event: &mut EntityTickPostEvent<'_>) {
        let HostSide::Server(host) = host else { return };
        world::POST_TICKS.store(true, std::sync::atomic::Ordering::Relaxed);
        for (&entity, &kind) in event.entities.iter().zip(event.kinds) {
            if kind == "minecraft:item" || kind == "item" {
                world::item_entity_tick(host, event.dimension, entity, 1);
            }
        }
    }

    // ---- Client

    fn client_tick_post(&self, host: &mut dyn ClientHost, _event: &mut ClientTickPostEvent) {
        cells::ids(&*host);
        wireless::client_tick(host);
        screens::client_tick(host);
        part_screens::client_tick(host);
        bus::client_tick(host);
        craft_screens::client_tick(host);
        ct_transfer::client_tick(host);
        priority::client_tick(host);
        handtools::client_tick(host);
        // JEI's search text, for the terminal's "Use JEI" and "Sync with JEI search" settings.
        mod_api::if_installed!(host, jei, {
            screens::external_search(jei::api::search_text());
        });
    }

    fn gui_render_post(&self, host: &dyn ClientHost, event: &mut GuiRenderPostEvent<'_>) {
        screens::gui_size(event.batch.size());
        // FinishedJobToast.
        if !event.gui_hidden {
            term_ui::hud(host, event.batch);
        }
    }

    fn right_click_block(&self, host: HostSide, event: &mut RightClickBlockEvent<'_>) -> Flow {
        if let HostSide::Client(host) = host {
            screens::clicked_block(&*host, event.click.pos, event.click.hit.face);
        }
        Flow::Continue
    }

    /// The charger's use, claimed at the first phase of the click (its beacon template would answer the block's own
    /// use before the charger's Part): ChargerBlock.useItemOn / useWithoutItem, on the server.
    fn use_item_on_block(&self, host: HostSide, event: &mut UseItemOnBlockEvent<'_>) -> Flow {
        let HostSide::Server(host) = host else { return Flow::Continue };
        let Some(level) = util::here(&*host) else { return Flow::Continue };
        // [tools] A block placed into a face covered by paint takes the paint's place.
        paint::clear_for_place(host, event.click);
        // The sky stone tank: a bucket fills from it or empties into it (the block's phase: not while sneaking).
        if event.phase == UsePhase::Block
            && let Some(answer) = sky_chest::tank_use(host, event.click)
        {
            *event.cancellation_result = answer;
            return Flow::Cancel;
        }
        // The wireless access point's screen (a beacon template too).
        if event.phase == UsePhase::Block
            && let Some(answer) = wireless::wap_use(host, event.click)
        {
            *event.cancellation_result = answer;
            return Flow::Cancel;
        }
        // [blocks] The quantum link chamber's screen (a beacon template too).
        if event.phase == UsePhase::Block
            && let Some(answer) = qnb::link_use(host, event.click)
        {
            *event.cancellation_result = answer;
            return Flow::Cancel;
        }
        // [blocks] Flint and steel on tiny TNT (a beacon template too).
        if event.phase == UsePhase::Block
            && let Some(answer) = tiny_tnt::block_use(host, event.click)
        {
            *event.cancellation_result = answer;
            return Flow::Cancel;
        }
        // [blocks] The crank (a beacon template too).
        if let Some(answer) = misc_blocks::crank_use(host, event.click) {
            *event.cancellation_result = answer;
            return Flow::Cancel;
        }
        // polish: the controller's network status.
        if event.phase == UsePhase::Block
            && let Some(answer) = tool::controller_use(host, event.click)
        {
            *event.cancellation_result = answer;
            return Flow::Cancel;
        }
        if net::block_id(&*host, level, event.click.pos) != "ae2:charger" {
            return Flow::Continue;
        }
        util::debug(&format!("charger: use_item_on_block {:?}", event.phase));
        match world::charger_use(host, event.click) {
            Some(answer) => {
                *event.cancellation_result = answer;
                Flow::Cancel
            }
            None => Flow::Continue,
        }
    }

    /// The crafting overlays (amount, plan) take the clicks while open; a click on a craftable entry opens them.
    fn screen_mouse_button_pressed_pre(&self, host: &dyn ClientHost, event: &mut ScreenMouseButtonPressedPreEvent<'_>) -> Flow {
        if knife::is_open(host) {
            knife::pressed(host, event.screen, event.button);
            event.cancelled = true;
            return Flow::Cancel;
        }
        if tool::is_open(host) {
            if tool::pressed(host, event.screen, event.button) {
                event.cancelled = true;
                return Flow::Cancel;
            }
            return Flow::Continue;
        }
        // An item's screen (wireless terminals, portable cells, the toolbox): the terminal's overlays first.
        if let Some(view) = wireless::view(host, event.screen) {
            let _ = term_ui::pressed(host, &view, event.button)
                || wireless::portable_middle(host, event.button)
                || craft_screens::pressed(host, &view, event.button)
                || wireless::pressed(host, &view, event.button);
            event.cancelled = true;
            return Flow::Cancel;
        }
        // polish: the inscriber screen's arrow opens its JEI recipes.
        mod_api::if_installed!(host, jei, {
            if crate::jei_extra::pressed(event.screen, event.button) {
                event.cancelled = true;
                return Flow::Cancel;
            }
        });
        // The upgrade toolbelt beside a machine's screen.
        if let Some(msg) = toolbox::belt_pressed(host, event.screen, event.button) {
            if !msg.is_empty() {
                wireless::queue(msg);
            }
            event.cancelled = true;
            return Flow::Cancel;
        }
        if priority::pressed(event.screen) || term_ui::pressed(host, event.screen, event.button) || craft_screens::pressed(host, event.screen, event.button) {
            event.cancelled = true;
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        if knife::is_open(host) {
            knife::draw(host, event.draw, event.screen);
            return;
        }
        if tool::is_open(host) {
            tool::draw(host, event.draw, event.screen);
            return;
        }
        if let Some(view) = wireless::view(host, event.screen) {
            wireless::draw(host, event.draw, &view);
            craft_screens::render_post(host, event.draw, &view);
            term_ui::render_post(host, event.draw, &view);
            screens::panels_frame_end();
            return;
        }
        craft_screens::render_post(host, event.draw, event.screen);
        term_ui::render_post(host, event.draw, event.screen);
        priority::render_post(host, event.draw, event.screen);
        toolbox::belt_draw(host, event.draw, event.screen);
        mod_api::if_installed!(host, jei, {
            crate::jei_extra::render_post(host, event.draw, event.screen); // polish
        });
        screens::panels_frame_end();
    }

    /// The upgrade and view cell panels beside AE2's screens, kept clear by recipe viewers' item panels.
    fn screen_exclusion_areas(&self, host: &dyn ClientHost, event: &mut ScreenExclusionAreasEvent<'_>) {
        event.areas.extend(screens::extra_areas(host, event.screen));
    }

    fn screen_mouse_scrolled_pre(&self, host: &dyn ClientHost, event: &mut ScreenMouseScrolledPreEvent<'_>) -> Flow {
        if tool::is_open(host) {
            tool::scrolled(event.scroll[1]);
            return Flow::Cancel;
        }
        if let Some(view) = wireless::view(host, event.screen) {
            let _ = term_ui::takes_input(&view) || craft_screens::scrolled(&view, event.scroll[1]) || screens::scrolled(host, &view, event.scroll[1]);
            event.cancelled = true;
            return Flow::Cancel;
        }
        if term_ui::takes_input(event.screen) || access_term::scrolled(host, event.screen, event.scroll[1]) || craft_screens::scrolled(event.screen, event.scroll[1]) || part_screens::scrolled(event.screen, event.scroll[1]) || screens::scrolled(host, event.screen, event.scroll[1]) {
            event.cancelled = true;
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn screen_mouse_dragged_pre(&self, host: &dyn ClientHost, event: &mut ScreenMouseDraggedPreEvent<'_>) -> Flow {
        if let Some(view) = wireless::view(host, event.screen) {
            return if screens::dragged(&view) { Flow::Cancel } else { Flow::Continue };
        }
        if access_term::dragged(host, event.screen) || screens::dragged(event.screen) { Flow::Cancel } else { Flow::Continue }
    }

    fn screen_mouse_button_released_pre(&self, _host: &dyn ClientHost, _event: &mut ScreenMouseButtonReleasedPreEvent<'_>) -> Flow {
        screens::released();
        access_term::released();
        encoder::released();
        Flow::Continue
    }

    fn screen_key_pressed_pre(&self, host: &dyn ClientHost, event: &mut ScreenKeyPressedPreEvent<'_>) -> Flow {
        if knife::is_open(host) && knife::key(event.key) {
            event.cancelled = true;
            return Flow::Cancel;
        }
        if let Some(view) = wireless::view(host, event.screen) {
            if term_ui::key(&view, event.key) || craft_screens::key(&view, event.key) || screens::key(&view, event.key) {
                event.cancelled = true;
                return Flow::Cancel;
            }
            // jeix: JEI's R and U over the item screen's stacks (it has no item panel to press them on).
            #[allow(unused_mut)]
            let mut shown = false;
            mod_api::if_installed!(host, jei, {
                shown = crate::jei_extra::lookup(host, &view, event.key);
            });
            if shown {
                event.cancelled = true;
                return Flow::Cancel;
            }
            return Flow::Continue;
        }
        if priority::key(event.screen, event.key) || term_ui::key(event.screen, event.key) || access_term::key(event.screen, event.key) || craft_screens::key(event.screen, event.key) || part_screens::key(event.screen, event.key) || screens::key(event.screen, event.key) {
            event.cancelled = true;
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn screen_char_typed_pre(&self, host: &dyn ClientHost, event: &mut ScreenCharTypedPreEvent<'_>) -> Flow {
        if knife::is_open(host) && knife::typed(event.c) {
            event.cancelled = true;
            return Flow::Cancel;
        }
        if let Some(view) = wireless::view(host, event.screen) {
            if term_ui::takes_input(&view) || craft_screens::typed(&view, event.c) || screens::typed(&view, event.c) {
                event.cancelled = true;
                return Flow::Cancel;
            }
            return Flow::Continue;
        }
        if term_ui::takes_input(event.screen) || access_term::typed(event.screen, event.c) || craft_screens::typed(event.screen, event.c) || part_screens::typed(event.screen, event.c) || screens::typed(event.screen, event.c) {
            event.cancelled = true;
            return Flow::Cancel;
        }
        Flow::Continue
    }

    fn screen_closing(&self, _host: &dyn ClientHost, _event: &mut ScreenClosingEvent<'_>) {
        screens::closing();
        craft_screens::closing();
        encoder::closing();
        access_term::closing();
        term_ui::closing();
        part_screens::closing();
        screens::panels_clear();
        priority::closing();
    }

    /// CellState.getStateColor on the cell's light (StorageCellItem's colour handler).
    fn register_item_tint_sources(&self, _context: &RegistrationContext, event: &mut RegisterItemTintSourcesEvent<'_>) {
        let Ok(id) = Identifier::parse(CELL_TINT) else { return };
        event.register(id, |_config| {
            Ok(|host: &dyn ClientHost, input: &ItemTintInput<'_>| {
                cells::ids(host);
                let status = cells::cell_status(input.stack).clamp(0, 4) as usize;
                0xFF00_0000 | cells::STATUS_COLORS[status]
            })
        });
    }

    fn item_tooltip(&self, host: &dyn ClientHost, event: &mut ItemTooltipEvent<'_>) {
        cells::ids(host);
        let Some(stack) = event.stack.project() else { return };
        let stack = &stack;
        for line in autocraft::pattern_tooltip(host, stack) {
            let _ = event.lines.push(Text::literal(line), 0xFFAA_AAAA);
        }
        for line in cells::cell_tooltip(stack) {
            let _ = event.lines.push(Text::literal(line.trim_start_matches("§7")), 0xFFAA_AAAA);
        }
        // AE2's powered items: "Stored Energy" (Tooltips.energyStorageComponent).
        if let Some(line) = powered::tooltip(host, stack) {
            let _ = event.lines.push(Text::literal(line), 0xFFAA_AAAA);
        }
        // [tools] Energy cells' stored energy, the name press's name, the applicator's colour.
        for line in handtools::tooltip(host, stack).into_iter().chain(applicator::tooltip(host, stack)) {
            let _ = event.lines.push(Text::literal(line), 0xFFAA_AAAA);
        }
    }

    // ---- [tools] The hand tools' hooks

    /// The colour applicator's wheel while sneaking (MouseWheelScrolled).
    fn mouse_scrolling(&self, host: &mut dyn ClientHost, event: &mut MouseScrollingEvent) -> Flow {
        if applicator::client_wheel(host, event.scroll[1]) {
            event.cancelled = true;
            return Flow::Cancel;
        }
        Flow::Continue
    }

    /// Picking a coloured block with the colour applicator selects its colour (ColorApplicatorPickColorHook).
    fn player_pick_item(&self, host: &mut dyn ServerHost, event: &mut PlayerPickItemEvent<'_>) -> Flow {
        if !event.from_entity && applicator::pick(host, event.player, event.pos) {
            event.cancelled = true;
            return Flow::Cancel;
        }
        Flow::Continue
    }

    /// An energy cell drops with its charge (EnergyCellBlockEntity's DISMANTLE_ITEM settings).
    fn block_drops(&self, level: &dyn LevelHost, event: &mut BlockDropsEvent<'_>) -> Flow {
        if let Some(block) = level.state_block(event.state) {
            handtools::block_drops(event.dimension.key().id(), [event.pos.x, event.pos.y, event.pos.z], block, &mut event.drops);
        }
        Flow::Continue
    }

    /// The meteorite compass's needle (MeteoriteCompassBakedModel).
    fn register_range_select_item_model_property(&self, _context: &RegistrationContext, event: &mut RegisterRangeSelectItemModelPropertyEvent<'_>) {
        let Ok(id) = Identifier::parse(handtools::COMPASS_PROPERTY) else { return };
        event.register(id, |_config| {
            Ok(|host: &dyn ClientHost, _input: &ItemModelPropertyInput<'_>| handtools::compass_property(host))
        });
    }
}
