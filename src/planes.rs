//! The annihilation plane (AnnihilationPlanePart): items lying in front of it go into the network, and the block in
//! front is broken into it (a fluid source picked up as a bucket's worth), only when the network has room for all of
//! it. Its enchantments (Fortune, Silk Touch, Efficiency, Unbreaking...) are kept from its item and used as Java does,
//! on a diamond pickaxe it breaks with (`createBreakingTool`), and in the power it takes. Upstream's `annihilate`
//! (`../ae2-horizon/mixins/ae2/me.rs`) on the fork's `destroy_block` / `block_drops` with a tool and `entities_in`.

use crate::core::{Actionable, KeyCounter};
use crate::grid::MeStorage;
use crate::storage::Scope;
use crate::util::*;
use mod_api::*;

/// Where a plane keeps its item's components (its enchantments).
const ENCH: &str = "hz_item_components";

/// A plane placed by a player: its item's components onto its block entity (or onto its hosted part).
pub(crate) fn placed(host: &mut dyn ServerHost, event: &EntityPlaceEvent) {
    let Some(placer) = event.placer else { return };
    let Some(level) = here(&*host) else { return };
    let pos = [event.pos.x, event.pos.y, event.pos.z];
    let block = crate::net::block_id(&*host, level, pos);
    if block != "ae2:annihilation_plane" {
        return;
    }
    let living = LivingRef::new(placer);
    let held = [Hand::Main, Hand::Off].into_iter().filter_map(|h| living.hand_item_owned(&*host, h)).find(|s| crate::cells::sid(s) == "ae2:annihilation_plane");
    let Some(components) = held.as_ref().and_then(|s| components_of(&*host, s)) else { return };
    let Some(be) = host.block_entity(level, pos) else { return };
    let mut nbt = be.data(&*host).unwrap_or_default();
    nbt.insert(java(ENCH), Tag::Compound(components));
    let _ = be.replace_data(host, nbt);
}

/// A plane's item's component patch (`minecraft:enchantments` among them).
pub(crate) fn components_of(host: &dyn Host, stack: &ItemStack) -> Option<Compound> {
    match stack.to_nbt(host)?.get(&java("components")) {
        Some(Tag::Compound(c)) if !c.is_empty() => Some(c.clone()),
        _ => None,
    }
}

/// The diamond pickaxe a plane breaks with, enchanted as the plane.
fn breaking_tool(host: &dyn Host, fields: &Fields) -> Option<ItemStack> {
    let mut nbt = Compound::new();
    nbt.insert(java("id"), Tag::String(java("minecraft:diamond_pickaxe")));
    nbt.insert(java("count"), Tag::Int(1));
    if let Some(Tag::Compound(c)) = fields.nbt.get(&java(ENCH)) {
        if let Some(e) = c.get(&java("minecraft:enchantments")) {
            let mut only = Compound::new();
            only.insert(java("minecraft:enchantments"), e.clone());
            nbt.insert(java("components"), Tag::Compound(only));
        }
    }
    ItemStack::from_nbt(host, &nbt)
}

/// The plane's enchantments: (id, level).
fn enchantments(fields: &Fields) -> Vec<(String, i32)> {
    enchantments_of(&fields.nbt)
}

/// polish: the same from a plane's (or hosted plane part's) nbt, for Jade.
pub(crate) fn enchantments_of(nbt: &Compound) -> Vec<(String, i32)> {
    let Some(Tag::Compound(c)) = nbt.get(&java(ENCH)) else { return Vec::new() };
    let Some(Tag::Compound(e)) = c.get(&java("minecraft:enchantments")) else { return Vec::new() };
    // Either `{levels: {id: n}}` or `{id: n}`.
    let map = match e.get(&java("levels")) {
        Some(Tag::Compound(l)) => l,
        _ => e,
    };
    map.iter()
        .filter_map(|(k, v)| {
            let n = match v {
                Tag::Int(n) => *n,
                Tag::Byte(n) => i32::from(*n),
                Tag::Short(n) => i32::from(*n),
                _ => return None,
            };
            Some((k.to_string_lossy(), n))
        })
        .collect()
}

/// Blocks a plane never breaks (unbreakable ones, and `ae2:blacklisted/annihilation_plane`).
fn unbreakable(host: &dyn Host, block: &str) -> bool {
    if matches!(
        block,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:bedrock" | "minecraft:barrier" | "minecraft:end_portal" | "minecraft:end_portal_frame"
            | "minecraft:end_gateway" | "minecraft:nether_portal" | "minecraft:command_block" | "minecraft:chain_command_block" | "minecraft:repeating_command_block"
            | "minecraft:structure_block" | "minecraft:structure_void" | "minecraft:jigsaw" | "minecraft:light" | "minecraft:reinforced_deepslate" | "minecraft:moving_piston"
            | "minecraft:fire" | "minecraft:soul_fire"
    ) {
        return true;
    }
    // (`ae2:blacklisted/annihilation_plane` is empty in AE2; the fork's Host has no block tag test by name.)
    let _ = host;
    false
}

/// AnnihilationPlanePart's tick: TickRates.AnnihilationPlane (2 to 120 ticks: back to 2 after work, slower without).
pub(crate) fn annihilation_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let dim = host.dimension();
    if !crate::net::device_state(dim, pos).2 {
        return;
    }
    let wait = match fields.nbt.get(&java("hz_wait")) {
        Some(Tag::Int(n)) => *n,
        _ => 0,
    };
    if wait > 0 {
        fields.nbt.insert(java("hz_wait"), Tag::Int(wait - 1));
        return;
    }
    let (fastest, slowest) = crate::config::config().tick_rate("AnnihilationPlane");
    let rate = match fields.nbt.get(&java("hz_rate")) {
        Some(Tag::Int(n)) => *n,
        _ => fastest,
    };
    let worked = annihilate(host, pos, fields);
    let next = if worked { fastest } else { (rate + 1).min(slowest) };
    fields.nbt.insert(java("hz_rate"), Tag::Int(next));
    fields.nbt.insert(java("hz_wait"), Tag::Int(next - 1));
}

fn annihilate(host: &mut dyn ServerHost, pos: Pos, fields: &Fields) -> bool {
    let Some(level) = here(&*host) else { return false };
    let f = facing_dir(crate::bus::prop(&*host, level, pos, "facing").as_deref());
    let t = add(real(pos), f);
    let scope = Scope::Grid(pos);
    let mut worked = false;
    // Items lying in its block space (onEntityCollision): as much of each as fits.
    let lo = [f64::from(t[0]), f64::from(t[1]), f64::from(t[2])];
    for e in level.entities_in_owned(&*host, lo, [lo[0] + 1.0, lo[1] + 1.0, lo[2] + 1.0]).unwrap_or_default() {
        if !e.kind(&*host).is_some_and(crate::util::is_item_entity) {
            continue;
        }
        let ie = ItemEntityRef::new(e);
        let Some(stack) = ie.stack_owned(&*host) else { continue };
        let key = crate::cells::key_of(&*host, &stack);
        let stored = crate::storage::insert(host, scope, None, &key, i64::from(stack.count()));
        if stored > 0 {
            worked = true;
            match crate::cells::with_count(&stack, stack.count() - stored as u32) {
                Some(left) => {
                    let _ = ie.set_stack(host, left);
                }
                None => {
                    let _ = e.discard(host);
                }
            }
        }
    }
    let name = crate::net::block_id(&*host, level, t);
    // FluidPickupStrategy: a source, a bucket's worth, when the network has room for it.
    if matches!(name.as_str(), "minecraft:water" | "minecraft:lava") {
        if level.block_property(&*host, t, "level").as_deref() == Some("0") {
            let key = crate::core::AeKey::fluid(&name, "");
            if crate::storage::simulate(host, scope, None, &key, 1000, true) == 1000
                && let Some(air) = host.block_state_id("minecraft:air")
                && matches!(level.set_block(host, t, air, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS), Ok(true))
            {
                crate::storage::insert(host, scope, None, &key, 1000);
                return true;
            }
        }
        return worked;
    }
    if unbreakable(&*host, &name) || crate::bus::is_cell_block(&name) && is_hosted(pos) && real(pos) == t {
        return worked;
    }
    let tool = breaking_tool(&*host, fields);
    // ItemPickupStrategy: everything it would drop must fit, else nothing is broken.
    let Ok(Some(drops)) = level.block_drops(host, t, tool.as_ref(), None) else { return worked };
    let keys: Vec<(crate::core::AeKey, i64)> = drops.iter().map(|d| (crate::cells::key_of(&*host, d), i64::from(d.count()))).collect();
    if keys.iter().any(|(k, n)| crate::storage::simulate(host, scope, None, k, *n, true) < *n) {
        return worked;
    }
    // calculateEnergyUsage: 1 + the block's hardness + the items, times 8 a level of the other enchantments and
    // 0.85 a level of Efficiency; Unbreaking makes it free but one time in (level + 1).
    let enchants = enchantments(fields);
    let lvl = |id: &str| enchants.iter().find(|(e, _)| e == id).map_or(0, |(_, l)| *l);
    let hardness = level.block_state(&*host, t).and_then(|st| host.block_states().destroy_speed(st)).unwrap_or(0.0).max(0.0);
    let dropped: i64 = keys.iter().map(|(_, n)| *n).sum();
    let mut power = 1.0 + f64::from(hardness) + dropped as f64;
    let mut use_power = true;
    if !enchants.is_empty() {
        let efficiency = lvl("minecraft:efficiency");
        let unbreaking = lvl("minecraft:unbreaking");
        if unbreaking > 0 {
            use_power = (crate::world::rand() * f64::from(unbreaking + 1)) < 1.0;
        }
        let others: i32 = enchants.iter().map(|(_, l)| *l).sum::<i32>() - efficiency - unbreaking;
        power *= f64::from(if others > 0 { 8 * others } else { 1 }) * 0.85f64.powi(efficiency);
    }
    let dim = host.dimension();
    if use_power {
        let stored = crate::net::grid_power(dim, pos).map_or(0.0, |(e, _)| e);
        if stored < power {
            return worked;
        }
        crate::net::extract_power(dim, pos, power);
    }
    // Broken with the tool, its drops (rolled with that tool) into the network, the rest at the block.
    if !matches!(level.destroy_block(host, t, false, tool.as_ref(), false), Ok(_)) {
        return worked;
    }
    for d in drops {
        let key = crate::cells::key_of(&*host, &d);
        let n = i64::from(d.count());
        let stored = crate::storage::insert(host, scope, None, &key, n);
        if stored < n
            && let Some(rest) = crate::cells::with_count(&d, (n - stored) as u32)
        {
            let _ = level.spawn_item(host, lo[0] + 0.5, lo[1] + 0.5, lo[2] + 0.5, rest, 10);
        }
    }
    debug(&format!("plane: broke {name} at {t:?} ({} enchantments, drops {dropped}, power {power:.2}, hardness {hardness})", enchants.len()));
    true
}

// ---- The formation plane (FormationPlanePart): a storage the network puts into, which places blocks and fluids in
// front of it and throws other items out, by its config (a storage bus's: filter, fuzzy and inverter cards).

/// Whether an item id is also a block (an item that places as a block shares its id), noted per key before a mount.
static IS_BLOCK: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> = std::sync::LazyLock::new(Default::default);

/// Notes whether `key` names a block (asked before each insert, with the host the storage itself doesn't get).
pub(crate) fn note_key(host: &dyn Host, key: &crate::core::AeKey) {
    let mut m = IS_BLOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !m.contains_key(key.id()) {
        let block = !key.id().ends_with(":air") && host.block_state_id(key.id()).is_some();
        m.insert(key.id().to_owned(), block);
    }
}

fn is_block(id: &str) -> bool {
    IS_BLOCK.lock().unwrap_or_else(|e| e.into_inner()).get(id).copied().unwrap_or(false)
}

/// A formation plane mounted for one storage call: what it would place or throw, done when it's closed.
pub(crate) struct Formation {
    pos: Pos,
    target: Pos,
    /// The block in front can be replaced (air, plants, snow layers...).
    open: bool,
    /// A bucket's worth can go there: open, or a flowing fluid.
    fluid_open: bool,
    filter: crate::parts::BusFilter,
    /// Fewer than `formationPlaneEntityLimit` items lie in front.
    can_throw: bool,
    /// Settings.PLACE_BLOCK off: blocks are dropped as items.
    drop_items: bool,
    placed: Option<crate::core::AeKey>,
    thrown: Vec<(crate::core::AeKey, i64)>,
}

impl MeStorage for Formation {
    fn insert(&mut self, what: &crate::core::AeKey, amount: i64, mode: Actionable) -> i64 {
        if !self.filter.allows(what) || amount < 1 {
            return 0;
        }
        if what.key_type() == crate::core::KeyType::Fluids {
            // FluidPlacementStrategy: a whole bucket a block, never thrown.
            if !self.fluid_open || self.drop_items || amount < 1000 || self.placed.is_some() || !is_block(what.id()) {
                return 0;
            }
            if mode == Actionable::Modulate {
                self.placed = Some(what.clone());
            }
            return 1000;
        }
        if !self.open {
            return 0;
        }
        if self.drop_items || !is_block(what.id()) {
            // ItemPlacementStrategy throws what isn't a block (and every item with Block Placement off): one stack a call, while few items lie there.
            if !self.can_throw || !self.thrown.is_empty() {
                return 0;
            }
            let n = amount.min(64);
            if mode == Actionable::Modulate {
                self.thrown.push((what.clone(), n));
            }
            return n;
        }
        if self.placed.is_some() {
            return 0;
        }
        if mode == Actionable::Modulate {
            self.placed = Some(what.clone());
        }
        1
    }
    fn extract(&mut self, _: &crate::core::AeKey, _: i64, _: Actionable) -> i64 {
        0
    }
    fn available_stacks(&self, _: &mut KeyCounter) {}
    fn is_preferred_for(&self, what: &crate::core::AeKey) -> bool {
        !self.filter.inverted && !self.filter.keys.is_empty() && self.filter.allows(what)
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// The grid's active formation planes, as storages (with their priority).
pub(crate) fn open_formations(host: &mut dyn ServerHost, dim: &'static str, devices: &[(Pos, String)]) -> Vec<(i32, Formation)> {
    let Some(level) = here(&*host) else { return Vec::new() };
    let mut out = Vec::new();
    for (pos, block) in devices {
        if block != "ae2:formation_plane" || !crate::net::device_state(dim, *pos).2 {
            continue;
        }
        let (filter, priority) = crate::parts::filter_of(dim, *pos).unwrap_or_default();
        let f = facing_dir(crate::bus::prop(&*host, level, *pos, "facing").as_deref());
        let target = add(real(*pos), f);
        let name = crate::net::block_id(&*host, level, target);
        let open = replaceable(&name);
        let fluid = matches!(name.as_str(), "minecraft:water" | "minecraft:lava");
        let flowing = fluid && level.block_property(&*host, target, "level").as_deref() != Some("0");
        let lo = [f64::from(target[0]), f64::from(target[1]), f64::from(target[2])];
        let lying = level
            .entities_in_owned(&*host, lo, [lo[0] + 1.0, lo[1] + 1.0, lo[2] + 1.0])
            .unwrap_or_default()
            .iter()
            .filter(|e| e.kind(&*host).is_some_and(crate::util::is_item_entity))
            .count();
        let can_throw = (lying as i64) < i64::from(crate::config::config().formation_plane_entity_limit);
        let fluid_open = (open && !fluid) || flowing;
        let drop_items = crate::bus::nbt_at(&*host, level, *pos).is_some_and(|n| nbt_i64(&n, "drop_items") != 0);
        out.push((priority, Formation { pos: *pos, target, open, fluid_open, filter, can_throw, drop_items, placed: None, thrown: Vec::new() }));
    }
    out
}

/// What `canBeReplaced` answers true for, by name.
fn replaceable(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:water" | "minecraft:lava" | "minecraft:short_grass"
            | "minecraft:grass" | "minecraft:tall_grass" | "minecraft:fern" | "minecraft:large_fern" | "minecraft:dead_bush" | "minecraft:snow"
            | "minecraft:vine" | "minecraft:glow_lichen" | "minecraft:seagrass" | "minecraft:tall_seagrass" | "minecraft:fire" | "minecraft:soul_fire"
            | "minecraft:structure_void" | "minecraft:light" | "minecraft:crimson_roots" | "minecraft:warped_roots" | "minecraft:nether_sprouts"
            | "minecraft:hanging_roots"
    )
}

/// A closed formation plane: the block (or fluid) it took goes in front, the items it took are thrown out.
pub(crate) fn close_formation(host: &mut dyn ServerHost, f: Formation) {
    let Some(level) = here(&*host) else { return };
    // polish: water boils away in the Nether, with a fizz (FluidPlacement), and is used up.
    if let Some(key) = &f.placed
        && key.id() == "minecraft:water"
        && crate::effects::boils_water(host.dimension())
    {
        crate::effects::fizz(host, f.target);
    } else if let Some(key) = &f.placed
        && let Some(state) = host.block_state_id(key.id())
    {
        let _ = level.set_block(host, f.target, state, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS);
    }
    // ItemPlacementStrategy.spawnItemEntity: from the plane's own block, just past its face along its facing (0.525 and
    // half the item's width), anywhere across it the item fits (RANDOM_OFFSET), on the floor of a sideways face (an up
    // face's item at the next block's floor, a down face's under the plane), moving 0.1 a tick outward; no pickup delay.
    let at = real(f.pos);
    let step = [f.target[0] - at[0], f.target[1] - at[1], f.target[2] - at[2]];
    // EntityType.ITEM's size: 0.25 by 0.25.
    let (width, height) = (0.25f64, 0.25f64);
    let across = (1.0 - width).max(0.0);
    let up = (1.0 - height).max(0.0);
    for (key, n) in f.thrown {
        let Some(stack) = crate::cells::stack_of(&*host, &key, n as u32) else { continue };
        let along = |s: i32| f64::from(s) * (0.525 + width / 2.0);
        let ox = if step[0] == 0 { crate::world::rand() * across - across / 2.0 } else { along(step[0]) };
        let oz = if step[2] == 0 { crate::world::rand() * across - across / 2.0 } else { along(step[2]) };
        let oy = if step[1] == 0 { crate::world::rand() * up } else { f64::from(step[1]) + if step[1] == -1 { 1.0 - height } else { 0.0 } };
        let x = f64::from(at[0]) + 0.5 + ox;
        let y = f64::from(at[1]) + oy;
        let z = f64::from(at[2]) + 0.5 + oz;
        let motion = [f64::from(step[0]) * 0.1, f64::from(step[1]) * 0.1, f64::from(step[2]) * 0.1];
        let _ = level.spawn_item_with_motion(host, x, y, z, stack, motion, 0);
    }
}
