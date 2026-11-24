//! Stage 7, AE2 in the world: certus quartz growth (budding blocks and the growth accelerator), the charger (charged
//! certus and powered tools), in-world transformation (items thrown into water: fluix crystals, charged certus regrowing
//! budding quartz...) and the entropy manipulator. Upstream's logic (`../ae2-horizon/mixins/ae2/me.rs`: `budding_tick`,
//! `charger_work`, `transform_tick`, `entropy_recipe`) on the fork's APIs: a BlockPart's `random_tick`, the level's
//! `block_random_tick`, `entity_tick_post` with `ItemEntityRef`, `recipes_of_type`.

use crate::cells::{custom_data, edit_custom_data, new_stack, sid, with_count};
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const BUDDING: [&str; 4] = ["ae2:flawless_budding_quartz", "ae2:flawed_budding_quartz", "ae2:chipped_budding_quartz", "ae2:damaged_budding_quartz"];
const BUDS: [&str; 4] = ["ae2:small_quartz_bud", "ae2:medium_quartz_bud", "ae2:large_quartz_bud", "ae2:quartz_cluster"];
/// BuddingCertusQuartzBlock: one try in five a random tick.
const BUD_CHANCE: f64 = 1.0 / 5.0;
/// What an imperfect budding block wears down to as it grows a bud, one chance in twelve.
const DECAY: [(&str, &str); 3] = [
    ("ae2:flawed_budding_quartz", "ae2:chipped_budding_quartz"),
    ("ae2:chipped_budding_quartz", "ae2:damaged_budding_quartz"),
    ("ae2:damaged_budding_quartz", "ae2:quartz_block"),
];
const DECAY_CHANCE: f64 = 1.0 / 12.0;
/// ChargerBlockEntity: its buffer, and a crystal's charge.
const CHARGER_MAX: f64 = 1600.0;
/// EntropyManipulatorItem.ENERGY_PER_USE.
const ENTROPY_AE: f64 = 1600.0;

static SEED: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

/// The server's own random numbers for these chances (xorshift; nothing a client sees depends on the sequence).
pub(crate) fn rand() -> f64 {
    let mut x = SEED.load(Ordering::Relaxed);
    if x == 0x9E37_79B9_7F4A_7C15 {
        x ^= std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    SEED.store(x, Ordering::Relaxed);
    (x >> 11) as f64 / (1u64 << 53) as f64
}

fn chance(p: f64) -> bool {
    rand() < p
}

/// `name[a=b,c=d]` into its name and properties.
fn parse_state(s: &str) -> (String, HashMap<String, String>) {
    let (name, rest) = s.split_once('[').unwrap_or((s, ""));
    let props = rest.trim_end_matches(']').split(',').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
    (name.to_owned(), props)
}

// ---- Budding quartz

/// The budding blocks' Part: their random tick is AE2's (the template's amethyst growth is cancelled).
pub(crate) struct Budding;

#[mod_api::sunrise_part]
impl BlockPart for Budding {
    fn random_tick(&self, level: &mut dyn LevelWriteHost, event: &mut RandomTickEvent) -> Flow {
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        if let Some(block) = level.state_block(event.state).map(str::to_owned) {
            budding_tick(level, pos, &block);
        }
        Flow::Cancel
    }
}

/// BuddingCertusQuartzBlock.randomTick: now and then a bud grows on a random side (into air or a water source), or the
/// bud there facing away grows a stage; an imperfect block may wear down a stage as it does.
fn budding_tick(level: &mut dyn LevelWriteHost, pos: Pos, block: &str) {
    if !chance(BUD_CHANCE) {
        return;
    }
    let side = ((rand() * 6.0) as usize).min(5);
    let target = add(pos, SIDES[side]);
    let facing = SIDE_NAMES[side];
    let Some(state) = level.block_state(target) else { return };
    let (here, props) = parse_state(&level.state_string(state).unwrap_or_default());
    let water_source = here == "minecraft:water" && props.get("level").map(String::as_str) == Some("0");
    let next = if matches!(here.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air") || water_source {
        Some(BUDS[0])
    } else {
        BUDS.iter()
            .position(|b| *b == here)
            .filter(|i| *i + 1 < BUDS.len())
            .filter(|_| props.get("facing").map(String::as_str) == Some(facing))
            .map(|i| BUDS[i + 1])
    };
    let Some(next) = next else { return };
    let waterlogged = water_source || props.get("waterlogged").map(String::as_str) == Some("true");
    let flags = BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS;
    if let Some(s) = level.block_state_id(&format!("{next}[facing={facing},waterlogged={waterlogged}]")) {
        level.set_block(target, s, flags);
    }
    if let Some((_, worn)) = DECAY.iter().find(|(from, _)| *from == block)
        && chance(DECAY_CHANCE)
        && let Some(s) = level.block_state_id(worn)
    {
        level.set_block(pos, s, flags);
    }
}

/// `ae2:growth_acceleratable` (AETags.GROWTH_ACCELERATABLE): AE2's budding quartz and vanilla's growing things.
fn accelerated(block: &str) -> bool {
    BUDDING.contains(&block)
        || block == "minecraft:budding_amethyst"
        || matches!(
            block.strip_prefix("minecraft:").unwrap_or(""),
            "bamboo_sapling" | "bamboo" | "sugar_cane" | "vine" | "twisting_vines" | "twisting_vines_plant" | "weeping_vines" | "weeping_vines_plant" | "cave_vines"
                | "cave_vines_plant" | "sweet_berry_bush" | "nether_wart" | "kelp" | "kelp_plant" | "cocoa" | "wheat" | "carrots" | "potatoes" | "beetroots"
                | "melon_stem" | "pumpkin_stem" | "torchflower_crop" | "pitcher_crop" | "mangrove_propagule"
        )
        || (block.starts_with("minecraft:") && block.ends_with("_sapling"))
}

/// GrowthAcceleratorBlockEntity: every `growthAccelerator` ticks (config), while its network is powered, each
/// acceleratable neighbour gets a random tick.
pub(crate) fn accelerator_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let n = match fields.nbt.get(&java("hz_wait")) {
        Some(Tag::Int(n)) => *n + 1,
        _ => 1,
    };
    let every = crate::config::config().growth_accelerator_speed.max(1);
    if n < every {
        fields.nbt.insert(java("hz_wait"), Tag::Int(n));
        return;
    }
    fields.nbt.insert(java("hz_wait"), Tag::Int(0));
    let dim = host.dimension();
    // [blocks] Power from a crank first (GrowthAcceleratorBlockEntity.storedPower, 8 AE a tick), else the network's.
    let crank = nbt_i64(&fields.nbt, crate::misc_blocks::ACCEL_POWER);
    if crank > 0 {
        set_nbt_i64(&mut fields.nbt, crate::misc_blocks::ACCEL_POWER, (crank - 8 * i64::from(every)).max(0));
    } else if !crate::net::device_state(dim, pos).1 {
        return;
    }
    let Some(level) = here(&*host) else { return };
    for d in SIDES {
        let p = add(pos, d);
        let b = crate::net::block_id(&*host, level, p);
        if accelerated(&b) {
            level.block_random_tick(host, p);
        }
    }
}

// ---- The charger

/// The items a recipe file's ingredient names: a bare id, `{"item"}`, a list of them; a `#tag` (or `{"tag"}`) through
/// the game's `recipe_ingredient`.
pub(crate) fn ingredient_ids(host: &dyn Host, v: &mod_api::serde_json::Value) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    if let Some(s) = v.as_str().filter(|s| !s.starts_with('#')) {
        out.insert(if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") });
    } else if let Some(i) = v.get("item").and_then(|i| i.as_str()) {
        out.insert(i.to_owned());
    } else if let Some(list) = v.as_array() {
        for e in list {
            out.extend(ingredient_ids(host, e));
        }
    } else if let Ok(Some(i)) = host.recipe_ingredient(&v.to_string()) {
        out.extend(i.items.iter().cloned());
        out.extend(i.order.iter().cloned());
    }
    out
}

/// ChargerRecipes: what the charger makes of an item (data `ae2:charger` recipes, upstream's three as a fallback).
fn charger_result(host: &dyn Host, item: &str) -> Option<String> {
    for r in host.recipes_of_type("ae2:charger").iter() {
        let Some(ing) = r.json.get("ingredient") else { continue };
        let fits = ingredient_ids(host, ing).contains(item);
        if fits && let Some(id) = r.json.get("result").and_then(|v| v.get("id")).and_then(|v| v.as_str()) {
            return Some(id.to_owned());
        }
    }
    None
}

/// AE2's powered items: (capacity in AE, charge rate a charger tick).
pub(crate) fn item_battery(item: &str) -> Option<(f64, f64)> {
    let c = crate::config::config();
    if item.starts_with("ae2:portable_item_cell_") || item.starts_with("ae2:portable_fluid_cell_") {
        return Some((f64::from(c.portable_cell_battery), 80.0));
    }
    match item.strip_prefix("ae2:")? {
        "wireless_terminal" | "wireless_crafting_terminal" => Some((f64::from(c.wireless_terminal_battery), 800.0)),
        "entropy_manipulator" => Some((f64::from(c.entropy_manipulator_battery), 800.0)),
        "matter_cannon" => Some((f64::from(c.matter_cannon_battery), 800.0)),
        "charged_staff" => Some((f64::from(c.charged_staff_battery), 32.0)),
        "color_applicator" => Some((f64::from(c.color_applicator_battery), 80.0)),
        _ => None,
    }
}

/// A powered item's AE (`internalCurrentPower` in its custom data, AE2's own key).
pub(crate) fn item_power(stack: &ItemStack) -> f64 {
    match custom_data(stack).and_then(|c| c.get(&java("internalCurrentPower"))) {
        Some(Tag::Double(b)) => f64::from_bits(*b),
        _ => 0.0,
    }
}

pub(crate) fn set_item_power(stack: &mut ItemStack, ae: f64) {
    edit_custom_data(stack, |c| {
        c.insert(java("internalCurrentPower"), Tag::Double(ae.to_bits()));
    });
}

/// Whether the charger takes this item (ChargerBlockEntity's filter: a recipe's ingredient or a powered item).
fn chargeable(host: &dyn Host, item: &str) -> bool {
    item_battery(item).is_some() || charger_result(host, item).is_some()
}

/// ITEMS for the charger, asked before the game's container (ChargerInvHandler): an insert takes only what the charger
/// charges (`chargeable`); an extract takes only what is done: a powered item at full charge, or anything with no
/// charger recipe (the result).
pub(crate) fn charger_items(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    // Asked with no side, the game's own container answers (the slot itself, which this reads).
    at.side?;
    let inner = at.level.capability(host, &ITEMS, at.pos, None)?;
    Some(Box::new(ChargerItems { inner: Box::new(crate::bus::Slice { inner, base: 0, len: usize::MAX }) }))
}

pub(crate) struct ChargerItems {
    inner: Box<dyn ResourceHandler<ItemResource>>,
}

pub(crate) fn charger_done(host: &dyn Host, stack: &ItemStack) -> bool {
    let item = sid(stack);
    match item_battery(item) {
        Some(_) => item_power(stack) >= crate::powered::capacity(stack).unwrap_or(0.0),
        None => charger_result(host, item).is_none(),
    }
}

impl ResourceHandler<ItemResource> for ChargerItems {
    fn size(&self, host: &mut dyn ServerHost) -> usize {
        self.inner.size(host).min(1)
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        self.inner.resource(host, index)
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        self.inner.amount(host, index)
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, resource: Option<&ItemResource>) -> u32 {
        self.inner.capacity(host, index, resource).min(1)
    }
    fn is_valid(&self, host: &mut dyn ServerHost, index: usize, resource: &ItemResource) -> bool {
        index == 0 && chargeable(&*host, sid(resource.as_stack()))
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        if !self.is_valid(host, index, resource) || self.inner.amount(host, index) > 0 {
            return Ok(0);
        }
        self.inner.insert_at(host, tx, index, resource, amount.min(1)).map(|n| n.min(1))
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        let done = self.inner.resource(host, index).is_some_and(|r| charger_done(&*host, r.as_stack()));
        if !done {
            return Ok(0);
        }
        self.inner.extract_at(host, tx, index, resource, amount).map(|n| n.min(amount))
    }
}

/// ChargerBlock.useItemOn / useWithoutItem: what's in it comes out to the player; else one of the held item goes in.
pub(crate) fn charger_use(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let level = here(&*host)?;
    let be = host.block_entity(level, click.pos)?;
    let inside = be.item_slot(&*host, 0).ok().flatten();
    let player = host.player(click.player)?;
    let e = player.entity();
    if let Some(stack) = inside {
        let _ = host.transaction(|host, tx| be.set_item_slot(host, tx, 0, None).map_err(|_| ()));
        let at = e.position(&*host).unwrap_or([f64::from(click.pos[0]) + 0.5, f64::from(click.pos[1]) + 1.0, f64::from(click.pos[2]) + 0.5]);
        if click.stack.is_none() {
            let _ = LivingRef::new(e).set_hand_item(host, click.hand, Some(stack));
        } else {
            let _ = level.spawn_item(host, at[0], at[1], at[2], stack, 0);
        }
        return Some(InteractionResult::SuccessServer);
    }
    let held = click.stack?.clone();
    debug(&format!("charger: use with {} (chargeable {})", sid(&held), chargeable(&*host, sid(&held))));
    if !chargeable(&*host, sid(&held)) {
        return None;
    }
    let one = with_count(&held, 1);
    let _ = host.transaction(|host, tx| be.set_item_slot(host, tx, 0, one.clone()).map_err(|_| ()));
    if !player.is_creative(&*host).unwrap_or(false) {
        let _ = LivingRef::new(e).set_hand_item(host, click.hand, with_count(&held, held.count() - 1));
    }
    Some(InteractionResult::SuccessServer)
}

/// ChargerBlockEntity.doWork, every 10 ticks (TickRates.Charger): a powered item charges from the buffer, then the
/// network; a crystal needs a full buffer, then has one chance in five, which spends it. A short buffer takes up to
/// 800 AE from the network.
pub(crate) fn charger_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let n = match fields.nbt.get(&java("hz_wait")) {
        Some(Tag::Int(n)) => *n + 1,
        _ => 1,
    };
    let (every, _) = crate::config::config().tick_rate("Charger");
    if n < every.max(1) {
        fields.nbt.insert(java("hz_wait"), Tag::Int(n));
        return;
    }
    fields.nbt.insert(java("hz_wait"), Tag::Int(0));
    let cfg = crate::config::config();
    let dim = host.dimension();
    let on_grid = crate::net::device_state(dim, pos).1;
    let mut stored = crate::net::internal_power(&fields.nbt);
    if let Some(Some(mut stack)) = fields.items.first().cloned() {
        let item = sid(&stack).to_owned();
        if let Some((_, rate)) = item_battery(&item) {
            let most = crate::powered::capacity(&stack).unwrap_or(0.0);
            let have = item_power(&stack);
            if have < most {
                let charge_rate = rate * 10.0 * cfg.charger_charge_rate;
                let from_buffer = (charge_rate * cfg.power_usage_multiplier).min(stored);
                stored -= from_buffer;
                let mut got = from_buffer / cfg.power_usage_multiplier;
                if on_grid {
                    got += crate::net::extract_power(dim, pos, (charge_rate - got).min(most - have).max(0.0));
                }
                if got > 0.0 {
                    stored += (have + got - most).max(0.0);
                    set_item_power(&mut stack, (have + got).min(most));
                    fields.items[0] = Some(stack);
                }
            }
        } else if stored > CHARGER_MAX - 1.0
            && stack.count() == 1
            && let Some(charged) = charger_result(&*host, &item)
            && chance(0.2)
        {
            stored = (stored - CHARGER_MAX * cfg.power_usage_multiplier).max(0.0);
            fields.items[0] = new_stack(&*host, &charged, 1);
            debug(&format!("charger: {item} became {charged} at {pos:?}"));
        }
    }
    if stored < CHARGER_MAX - 1.0 && on_grid {
        stored += crate::net::extract_power(dim, pos, 800.0_f64.min(CHARGER_MAX - stored));
    }
    if stored != crate::net::internal_power(&fields.nbt) {
        crate::net::set_internal_power(&mut fields.nbt, stored);
    }
}

// ---- In-world transformation

pub(crate) struct Transform {
    /// The fluid tag it happens in, or None for an explosion.
    fluid: Option<String>,
    ingredients: Vec<std::collections::HashSet<String>>,
    result: (String, u32),
}

static TRANSFORMS: Mutex<Option<Vec<Transform>>> = Mutex::new(None);
/// How long each item entity that could start a recipe has lain in its fluid (`ae2_transformTime`), by (level, id).
static TIMES: Mutex<Option<HashMap<(String, u32), i32>>> = Mutex::new(None);

pub(crate) fn forget_all() {
    *TRANSFORMS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *TIMES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn with_transforms<R>(host: &dyn Host, f: impl FnOnce(&[Transform]) -> R) -> R {
    let mut t = TRANSFORMS.lock().unwrap_or_else(|e| e.into_inner());
    let all = t.get_or_insert_with(|| {
        host.recipes_of_type("ae2:transform")
            .iter()
            .filter_map(|r| {
                let c = r.json.get("circumstance")?;
                let fluid = (c.get("type")?.as_str()? == "fluid").then(|| c.get("tag").and_then(|t| t.as_str()).unwrap_or("minecraft:water").to_owned());
                let ingredients = r
                    .json
                    .get("ingredients")?
                    .as_array()?
                    .iter()
                    .map(|v| ingredient_ids(host, v))
                    .collect();
                let res = r.json.get("result")?;
                let id = res.get("id")?.as_str()?.to_owned();
                let count = res.get("count").and_then(|c| c.as_u64()).unwrap_or(1) as u32;
                Some(Transform { fluid, ingredients, result: (id, count) })
            })
            .collect()
    });
    f(all)
}

/// The fluid tag the block at `p` counts as (water, or a waterlogged block; lava).
pub(crate) fn fluid_at(host: &dyn Host, level: LevelRef, p: Pos) -> Option<&'static str> {
    let b = crate::net::block_id(host, level, p);
    if b == "minecraft:water" || b == "minecraft:bubble_column" || level.block_property(host, p, "waterlogged").as_deref() == Some("true") {
        Some("minecraft:water")
    } else if b == "minecraft:lava" {
        Some("minecraft:lava")
    } else {
        None
    }
}

/// AE2's handleEntityTransform (an item entity's tick): one that can start a recipe and has lain in its fluid more than
/// 60 ticks tries it with what lies within a block of it; the others are used up one each, the result appears.
/// `ticks`: how many ticks this call stands for (1 from the entity's own tick, 5 from the scan).
pub(crate) fn item_entity_tick(host: &mut dyn ServerHost, dimension: &str, id: u32, ticks: u32) {
    if id % 40 == 0 {
        debug(&format!("transform: item tick {id} found {}", host.entity(id).is_some()));
    }
    let Some(entity) = host.entity(id) else { return };
    let item = ItemEntityRef::new(entity);
    let Some(stack) = item.stack_owned(&*host) else { return };
    let name = sid(&stack).to_owned();
    let starts = with_transforms(&*host, |all| all.iter().any(|t| t.fluid.is_some() && t.ingredients.first().is_some_and(|i| i.contains(&name))));
    if !starts {
        return;
    }
    if id % 20 == 0 {
        debug(&format!("transform: candidate {name}"));
    }
    let Some(level) = here(&*host) else { return };
    let Some(at) = entity.position(&*host) else { return };
    let p = [at[0].floor() as i32, (at[1] + 0.125).floor() as i32, at[2].floor() as i32];
    let fluid = fluid_at(&*host, level, p);
    let valid = fluid.is_some_and(|f| with_transforms(&*host, |all| all.iter().any(|t| t.fluid.as_deref() == Some(f) && t.ingredients.first().is_some_and(|i| i.contains(&name)))));
    let key = (dimension.to_owned(), id);
    let time = {
        let mut times = TIMES.lock().unwrap_or_else(|e| e.into_inner());
        let times = times.get_or_insert_with(HashMap::new);
        if times.len() > 4096 {
            times.clear();
        }
        let n = times.entry(key.clone()).or_insert(0);
        *n = if valid { *n + ticks as i32 } else { 0 };
        *n
    };
    if time <= 60 {
        return;
    }
    if !try_transform(host, level, Some(fluid.unwrap_or("")), &name, at) {
        if let Some(t) = TIMES.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            t.insert(key, 0);
        }
    }
}

/// [blocks] An explosion's item entities (TransformLogic in ItemEntityMixin.handleExplosion): one that is an ingredient of
/// an explosion recipe tries it with what lies within a block of it; those are spared by the blast. A pair of entangled
/// singularities so made shares a new frequency (QuantumBridgeBlockEntity.assignFrequency).
pub(crate) fn explosion_transform(host: &mut dyn ServerHost, entities: &mut Vec<EntityRef>) {
    let Some(level) = here(&*host) else { return };
    let ingredient = |host: &dyn Host, item: &str| with_transforms(host, |all| all.iter().any(|t| t.fluid.is_none() && t.ingredients.iter().any(|i| i.contains(item))));
    let candidates: Vec<EntityRef> = entities
        .iter()
        .copied()
        .filter(|e| e.kind(&*host).is_some_and(crate::util::is_item_entity))
        .filter(|e| ItemEntityRef::new(*e).stack_owned(&*host).is_some_and(|s| ingredient(&*host, sid(&s))))
        .collect();
    if candidates.is_empty() {
        return;
    }
    for e in &candidates {
        let Some(stack) = ItemEntityRef::new(*e).stack_owned(&*host) else { continue };
        let Some(at) = e.position(&*host) else { continue };
        try_transform(host, level, None, sid(&stack), at);
    }
    entities.retain(|e| !candidates.contains(e));
}

fn try_transform(host: &mut dyn ServerHost, level: LevelRef, fluid: Option<&str>, item: &str, at: [f64; 3]) -> bool {
    let near = level.entities_in_owned(&*host, [at[0] - 1.0, at[1] - 1.0, at[2] - 1.0], [at[0] + 1.0, at[1] + 1.0, at[2] + 1.0]).unwrap_or_default();
    let stacks: Vec<(EntityRef, ItemStack)> = near
        .into_iter()
        .filter(|e| e.kind(&*host).is_some_and(crate::util::is_item_entity))
        .filter_map(|e| ItemEntityRef::new(e).stack_owned(&*host).map(|s| (e, s)))
        .collect();
    let plan = with_transforms(&*host, |all| {
        for t in all.iter().filter(|t| t.fluid.as_deref() == fluid) {
            // In a fluid the first ingredient starts it; in a blast any of them.
            let starts = if fluid.is_some() { t.ingredients.first().is_some_and(|i| i.contains(item)) } else { t.ingredients.iter().any(|i| i.contains(item)) };
            if !starts {
                continue;
            }
            let mut missing: Vec<&std::collections::HashSet<String>> = t.ingredients.iter().collect();
            let mut used = Vec::new();
            for (i, (_, s)) in stacks.iter().enumerate() {
                if let Some(m) = missing.iter().position(|ing| ing.contains(sid(s))) {
                    missing.remove(m);
                    used.push(i);
                }
            }
            if missing.is_empty() {
                return Some((used, t.result.clone()));
            }
        }
        None
    });
    let Some((used, (result, count))) = plan else { return false };
    for i in used {
        let (e, s) = &stacks[i];
        if s.count() <= 1 {
            let _ = e.discard(host);
        } else if let Some(less) = with_count(s, s.count() - 1) {
            let _ = ItemEntityRef::new(*e).set_stack(host, less);
        }
    }
    if let Some(out) = new_stack(&*host, &result, count).map(crate::qnb::with_frequency) {
        let _ = level.spawn_item_with_motion(host, at[0].floor() + 0.5, at[1].floor() + 0.5, at[2].floor() + 0.5, out, [0.0, 0.05, 0.0], 10);
    }
    debug(&format!("transform: {item} in {fluid:?} made {count} {result}"));
    true
}

// ---- The entropy manipulator

pub(crate) fn entropy_recipe(host: &dyn Host, block: &str, cool: bool) -> Option<(String, Vec<String>)> {
    let mode = if cool { "cool" } else { "heat" };
    for r in host.recipes_of_type("ae2:entropy").iter() {
        let j = &r.json;
        if j.get("mode").and_then(|m| m.as_str()) != Some(mode) {
            continue;
        }
        let input = j.get("input");
        let id = input.and_then(|i| i.get("block").or_else(|| i.get("fluid"))).and_then(|b| b.get("id")).and_then(|v| v.as_str());
        if id != Some(block) {
            continue;
        }
        let out = j.get("output");
        let to = out
            .and_then(|o| o.get("block").or_else(|| o.get("fluid")))
            .and_then(|b| b.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("minecraft:air")
            .to_owned();
        let drops = out
            .and_then(|o| o.get("drops"))
            .and_then(|d| d.as_array())
            .map(|d| d.iter().filter_map(|x| x.get("item").and_then(|i| i.as_str()).map(str::to_owned)).collect())
            .unwrap_or_default();
        return Some((to, drops));
    }
    None
}

/// [blocks] A dispenser firing an entropy manipulator (BlockToolDispenseItemBehavior): it heats the block in front, as
/// a use on its face toward the dispenser would, for 1600 AE of its charge. The stack to leave in the slot.
pub(crate) fn entropy_dispense(host: &mut dyn ServerHost, pos: Pos, facing: Pos, stack: &ItemStack) -> Option<ItemStack> {
    let level = here(&*host)?;
    if item_power(stack) < ENTROPY_AE {
        return None;
    }
    let target = add(pos, facing);
    if !entropy_apply(host, level, target, [-facing[0], -facing[1], -facing[2]], false) {
        return None;
    }
    let mut left = stack.clone();
    set_item_power(&mut left, item_power(stack) - ENTROPY_AE);
    Some(left)
}

/// EntropyManipulatorItem's effect on the block at `click_pos` used on its face `face`: heat (or cool) by its recipe;
/// heat with none lights a fire on the face. Whether it did anything.
pub(crate) fn entropy_apply(host: &mut dyn ServerHost, level: LevelRef, click_pos: Pos, face: Pos, cool: bool) -> bool {
    {
        // A fluid in front of the face (the manipulator aims at fluids too), else the block itself.
        let front = add(click_pos, face);
        let pos = if fluid_at(&*host, level, front).is_some() && crate::net::block_id(&*host, level, front).ends_with("water") || crate::net::block_id(&*host, level, front) == "minecraft:lava" {
            front
        } else {
            click_pos
        };
        let here_id = crate::net::block_id(&*host, level, pos);
        let mut name = here_id.clone();
        if name == "minecraft:water" || name == "minecraft:lava" {
            // A source is the fluid's id; anything else its flowing form.
            if level.block_property(&*host, pos, "level").as_deref() != Some("0") {
                name = name.replace("minecraft:", "minecraft:flowing_");
            }
        }
        let found = entropy_recipe(&*host, &name, cool).or_else(|| (name != here_id).then(|| entropy_recipe(&*host, &here_id, cool)).flatten());
        // polish: which sound it makes (a fizz, or flint and steel's for a fire).
        let mut lit_fire = false;
        let applied = if let Some((out, drops)) = found {
            if let Some(state) = host.block_state_id(&out) {
                let _ = level.set_block(host, pos, state, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS);
            }
            for d in drops {
                if let Some(s) = new_stack(&*host, &d, 1) {
                    let _ = level.spawn_item(host, f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5, s, 10);
                }
            }
            true
        } else if !cool && crate::net::block_id(&*host, level, front) == "minecraft:air" {
            // No recipe: heat lights a fire on the face, as flint and steel would.
            lit_fire = true;
            host.block_state_id("minecraft:fire").is_some_and(|s| matches!(level.set_block(host, front, s, BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS), Ok(true)))
        } else {
            false
        };
        if applied {
            crate::effects::entropy_used(host, if lit_fire { front } else { pos }, lit_fire);
            debug(&format!("entropy: {} {name} at {pos:?}", if cool { "cooled" } else { "heated" }));
        }
        applied
    }
}

/// The charger's own Part: a use puts in or takes out (its beacon template would answer otherwise).
pub(crate) struct ChargerBlock;

#[mod_api::sunrise_part]
impl BlockPart for ChargerBlock {
    fn use_item_on(&self, host: &mut dyn ServerHost, event: &mut UseItemOnEvent<'_>) -> Option<InteractionResult> {
        // An item the charger doesn't take goes on to its own use (ChargerBlock.useItemOn's PASS): a crank or any block
        // is placed against the charger, as in AE2. Without an answer the beacon template's default would end the click.
        Some(charger_use(host, event.click).unwrap_or(InteractionResult::Pass))
    }

    fn use_without_item(&self, host: &mut dyn ServerHost, event: &mut UseWithoutItemEvent<'_>) -> Option<InteractionResult> {
        charger_use(host, event.click)
    }
}

/// Set once `entity_tick_post` reaches the mod: each item entity is then counted on its own tick, one at a time.
pub(crate) static POST_TICKS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// In-world transformation where `entity_tick_post` isn't dispatched (as at 514ecf9): every 5 ticks, the level's item
/// entities, each counted 5 ticks. Off once the hook arrives, so no item is counted twice.
pub(crate) fn transform_scan(host: &mut dyn ServerHost, level: LevelRef, dimension: &str, tick: u64) {
    if tick % 5 != 0 || POST_TICKS.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let items: Vec<u32> = level.all_entities_owned(&*host).unwrap_or_default().into_iter().filter(|e| e.kind(&*host).is_some_and(crate::util::is_item_entity)).map(|e| e.id()).collect();
    for id in items {
        item_entity_tick(host, dimension, id, 5);
    }
}

pub(crate) fn register(r: &mut dyn Registrar) {
    let k = r.block_key("ae2:charger");
    r.block_part(k, ChargerBlock);
    for b in BUDDING {
        let k = r.block_key(b);
        r.block_part(k, Budding);
    }
    let k = r.item_key("ae2:network_tool");
    r.item_part(k, crate::tool::NetworkTool);
}

/// InitVillager: the Fluix Researcher, whose work site is the charger (PointOfInterestHelper.register(ID, 1, 1,
/// AEBlocks.CHARGER)), with the librarian's work sound and AE2's trades (`data/ae2/trade_set/fluix_researcher/`).
pub(crate) fn professions(event: &mut RegisterVillagerProfessionsEvent<'_>) {
    let id = |s: &str| Identifier::parse(s).expect("a valid id");
    let poi = PoiKey(id("ae2:fluix_researcher"));
    event.professions.register(ProfessionDefinition {
        key: ProfessionKey(id("ae2:fluix_researcher")),
        workstations: vec![ProfessionWorkstation { poi: poi.clone(), block: id("ae2:charger"), properties: Vec::new() }],
        held_poi: vec![poi.clone()],
        acquirable_poi: vec![poi],
        requested_items: Vec::new(),
        secondary_blocks: Vec::new(),
        work_sound: Some(id("minecraft:entity.villager.work_librarian")),
        trades: (1..=5u8).map(|n| (n, TradeSetKey(id(&format!("ae2:fluix_researcher/level_{n}"))))).collect(),
    });
}
