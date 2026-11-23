//! AE2's hand tools beside the cannon and the applicator: the charged staff (ChargedStaffItem), the entropy manipulator
//! (EntropyManipulatorItem), the meteorite compass (MeteoriteCompassItem, CompassService, MeteoriteCompassBakedModel),
//! the energy cells as items (EnergyCellBlockItem, the cell's DISMANTLE_ITEM settings) and the name press's tooltip
//! (NamePressItem); the tools' channel (`ae2:tools`: the applicator's wheel, the compass's needle, the knife's screen).
//! Upstream's `attack_entity`, `incoming_damage`, `use_item_on`, `compass_tick` and `charged_cell`
//! (`../ae2-horizon/mixins/ae2/me.rs`).
//!
//! Every use is the server's: the item is read from the hand, its charge checked and spent, and written back only if
//! it is still the stack that was read (`replace_hand`). Client messages wait for the player's tick (a payload handler
//! can't reach the inventory) and are rate limited.

use crate::cells::*;
use crate::util::*;
use crate::world::item_power;
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

pub(crate) const NET_TOOLS: &str = "ae2:tools";

/// The particles the tools send (registered keys).
pub(crate) static SPARK: OnceLock<ParticleKey> = OnceLock::new();
pub(crate) static SMOKE: OnceLock<ParticleKey> = OnceLock::new();

/// The hand's stack replaced by `new`, only if it is still `old` (nothing changed under the use). False when it wasn't.
pub(crate) fn replace_hand(
    host: &mut dyn ServerHost,
    player: u128,
    hand: Hand,
    old: &ItemStack,
    new: ItemStack,
) -> bool {
    let place = ItemPlace::PlayerHand { player, hand };
    host.transaction(|host, tx| {
        let mut same = false;
        let edited = crate::cells::edit_stack(&place, host, tx, |s| {
            if s == old {
                same = true;
                *s = new;
            }
        })
        .is_ok();
        if edited && same { Ok(()) } else { Err(()) }
    })
    .is_ok()
}

fn sneaking(host: &dyn Host, player: u128) -> bool {
    host.player(player).is_some_and(|p| {
        let e = p.entity();
        e.is_shift_key_down(host).unwrap_or(false) || e.is_crouching(host).unwrap_or(false)
    })
}

fn rand() -> f64 {
    crate::world::rand()
}

// ---- The charged staff

/// ChargedStaffItem: 300 AE a hit, 6 magic damage.
pub(crate) const STAFF_AE: f64 = 300.0;
pub(crate) const STAFF_DAMAGE: f32 = 6.0;

/// ChargedStaffItem.hurtEnemy: with more than 300 AE, a hit spends 300, sparks twice about the target (LightningPacket)
/// and hurts it 6 magic damage (vanilla's invulnerable time keeps the larger of that and the blow).
pub(crate) struct Staff;

#[mod_api::sunrise_part]
impl ItemPart for Staff {
    fn hurt_enemy(&self, host: &mut dyn ServerHost, event: &mut HurtEnemyEvent<'_>) {
        let Some(p) = host.players().acting_player() else {
            return;
        };
        let Some(player) = p.entity().uuid(host) else {
            return;
        };
        let old = event.stack.clone();
        if item_power(&old) <= STAFF_AE {
            return;
        }
        let mut stack = old.clone();
        crate::powered::use_power(&mut stack, STAFF_AE);
        if !replace_hand(host, player, Hand::Main, &old, stack) {
            return;
        }
        let Some(target) = host.entity(event.entity) else {
            return;
        };
        sparks(host, target);
        let _ = target.hurt(host, "minecraft:magic", STAFF_DAMAGE, None, None);
    }
}

/// LightningPacket twice at random points of the target's box (AEConfig's `enableEffects`).
fn sparks(host: &mut dyn ServerHost, target: EntityRef) {
    if !crate::config::config().enable_effects {
        return;
    }
    let (Some(key), Some(level), Some([lo, hi])) =
        (SPARK.get(), here(&*host), target.bounding_box(&*host))
    else {
        return;
    };
    let options = ParticleOptions::simple(*key);
    for _ in 0..2 {
        let at = [
            lo[0] + rand() * (hi[0] - lo[0]),
            lo[1] + rand() * (hi[1] - lo[1]),
            lo[2] + rand() * (hi[2] - lo[2]),
        ];
        let _ = level.send_particles(host, &options, None, at, [0.05; 3], 0.0, 4);
    }
}

// ---- The entropy manipulator

/// EntropyManipulatorItem.ENERGY_PER_USE.
pub(crate) const ENTROPY_AE: f64 = 1600.0;

/// What a heat use does to a block with no heat recipe, in AE2's order (pure: unit tested): TNT and Tiny TNT are lit,
/// else the block's smelted drops, else fire on the face.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Heat {
    Tnt,
    TinyTnt,
    SmeltOrFire,
}

pub(crate) fn heat_kind(block: &str) -> Heat {
    match block {
        "minecraft:tnt" => Heat::Tnt,
        "ae2:tiny_tnt" => Heat::TinyTnt,
        _ => Heat::SmeltOrFire,
    }
}

fn block_face(face: Pos) -> BlockFace {
    match face {
        [0, -1, 0] => BlockFace::Down,
        [0, 1, 0] => BlockFace::Up,
        [0, 0, -1] => BlockFace::North,
        [0, 0, 1] => BlockFace::South,
        [-1, 0, 0] => BlockFace::West,
        _ => BlockFace::East,
    }
}

fn may_use(host: &dyn Host, player: u128, pos: Pos, face: Pos, stack: &ItemStack) -> bool {
    host.player(player).is_some_and(|p| {
        p.may_build(host).unwrap_or(false)
            && p.may_use_item_at(host, pos, block_face(face), stack)
                .unwrap_or(false)
    })
}

fn play(
    host: &mut dyn ServerHost,
    level: LevelRef,
    sound: &str,
    pos: Pos,
    volume: f32,
    pitch: f32,
) {
    let at = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.5,
        f64::from(pos[2]) + 0.5,
    ];
    let _ = level.play_sound(host, sound, at, SoundSource::Players, volume, pitch, None);
}

/// The block's id as the entropy recipes name it (a fluid's source by the fluid's id, else its flowing form).
fn entropy_name(host: &dyn Host, level: LevelRef, pos: Pos) -> (String, String) {
    let id = crate::net::block_id(host, level, pos);
    let mut name = id.clone();
    if (name == "minecraft:water" || name == "minecraft:lava")
        && level.block_property(host, pos, "level").as_deref() != Some("0")
    {
        name = name.replace("minecraft:", "minecraft:flowing_");
    }
    (id, name)
}

/// `applyRecipe`: the output state (or air), the drops, and for heat the fizz and smoke.
fn apply_recipe(
    host: &mut dyn ServerHost,
    level: LevelRef,
    pos: Pos,
    out: &str,
    drops: &[String],
    heat: bool,
) {
    if let Some(state) = host.block_state_id(out) {
        let _ = level.set_block(
            host,
            pos,
            state,
            BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS,
        );
    }
    for d in drops {
        if let Some(s) = new_stack(&*host, d, 1) {
            let _ = level.spawn_item(
                host,
                f64::from(pos[0]) + 0.5,
                f64::from(pos[1]) + 0.5,
                f64::from(pos[2]) + 0.5,
                s,
                10,
            );
        }
    }
    if heat {
        play(
            host,
            level,
            "minecraft:block.fire.extinguish",
            pos,
            0.5,
            2.6 + ((rand() - rand()) * 0.8) as f32,
        );
        if let Some(key) = SMOKE.get() {
            let options = ParticleOptions::simple(*key);
            let c = [
                f64::from(pos[0]) + 0.5,
                f64::from(pos[1]) + 0.5,
                f64::from(pos[2]) + 0.5,
            ];
            let _ = level.send_particles(host, &options, None, c, [0.3; 3], 0.0, 8);
        }
    }
}

/// `performInWorldSmelting`: every drop of the block (as this tool would break it) has a smelting result; a block result
/// (not the block itself) takes the block's place, the rest drop. False when a drop doesn't smelt.
fn smelt(
    host: &mut dyn ServerHost,
    level: LevelRef,
    pos: Pos,
    player: u128,
    tool: &ItemStack,
) -> bool {
    let block = crate::net::block_id(&*host, level, pos);
    let breaker = host.player(player).map(|p| p.entity());
    let Ok(Some(drops)) = level.block_drops(host, pos, Some(tool), breaker) else {
        return false;
    };
    let Ok(smelting) = Identifier::parse("minecraft:smelting") else {
        return false;
    };
    let mut placed: Option<String> = None;
    let mut out: Vec<ItemStack> = Vec::new();
    for d in drops {
        let Ok(Some(m)) = level.recipe_for(&*host, &smelting, &RecipeInput::Single(d)) else {
            return false;
        };
        let result = sid(&m.output).to_owned();
        if host.block_state_id(&result).is_some() {
            if result == block {
                return false;
            }
            if placed.is_none() && !result.ends_with(":air") {
                placed = Some(result);
                continue;
            }
        }
        out.push(m.output);
    }
    if placed.is_none() && out.is_empty() {
        return false;
    }
    play(
        host,
        level,
        "minecraft:item.flintandsteel.use",
        pos,
        1.0,
        (rand() * 0.4 + 0.8) as f32,
    );
    let state = host.block_state_id(placed.as_deref().unwrap_or("minecraft:air"));
    if let Some(s) = state {
        let _ = level.set_block(
            host,
            pos,
            s,
            BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS,
        );
    }
    for s in out {
        let _ = level.spawn_item(
            host,
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 0.5,
            f64::from(pos[2]) + 0.5,
            s,
            10,
        );
    }
    true
}

/// TntBlock.onCaughtFire / TinyTNTBlock.startFuse: the block primed (the player its owner) and gone.
fn prime(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, kind: &str) -> bool {
    let dim = host.dimension();
    let mut nbt = Compound::new();
    nbt.insert(
        java("fuse"),
        Tag::Short(if kind == "minecraft:tnt" { 80 } else { 40 }),
    );
    let at = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]),
        f64::from(pos[2]) + 0.5,
    ];
    if host.summon(dim, kind, at, nbt).is_none() {
        return false;
    }
    if let Some(air) = host.block_state_id("minecraft:air") {
        let _ = level.set_block(
            host,
            pos,
            air,
            BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS,
        );
    }
    let _ = level.play_sound(
        host,
        "minecraft:entity.tnt.primed",
        at,
        SoundSource::Blocks,
        1.0,
        1.0,
        None,
    );
    true
}

/// EntropyManipulatorItem.tryApplyEffect on `pos` (the face `face` used): cool sneaking (its recipe only), else TNT,
/// Tiny TNT, the heat recipe, smelting, fire. Returns whether it acted and the charge it used.
fn apply(
    host: &mut dyn ServerHost,
    level: LevelRef,
    pos: Pos,
    face: Pos,
    player: u128,
    tool: &ItemStack,
) -> Option<f64> {
    let (id, name) = entropy_name(&*host, level, pos);
    let recipe = |host: &dyn Host, cool: bool| {
        crate::world::entropy_recipe(host, &name, cool).or_else(|| {
            (name != id)
                .then(|| crate::world::entropy_recipe(host, &id, cool))
                .flatten()
        })
    };
    if sneaking(&*host, player) {
        let (out, drops) = recipe(&*host, true)?;
        apply_recipe(host, level, pos, &out, &drops, false);
        return Some(ENTROPY_AE);
    }
    match heat_kind(&id) {
        Heat::Tnt => return prime(host, level, pos, "minecraft:tnt").then_some(0.0),
        // TinyTNTBlock.startFuse: tiny_tnt.rs primes it (there is no `ae2:tiny_tnt_primed` entity kind to summon).
        Heat::TinyTnt => {
            crate::tiny_tnt::queue_prime(host.dimension(), pos, crate::tiny_tnt::FUSE);
            return Some(0.0);
        }
        _ => {}
    }
    if let Some((out, drops)) = recipe(&*host, false) {
        apply_recipe(host, level, pos, &out, &drops, true);
        return Some(ENTROPY_AE);
    }
    if smelt(host, level, pos, player, tool) {
        return Some(ENTROPY_AE);
    }
    // applyFlintAndSteelEffect: fire on the face if it is air (the use is taken either way once allowed there).
    let front = add(pos, face);
    if !may_use(&*host, player, front, face, tool) {
        return None;
    }
    let air = matches!(
        crate::net::block_id(&*host, level, front).as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    );
    if !air {
        return Some(0.0);
    }
    play(
        host,
        level,
        "minecraft:item.flintandsteel.use",
        front,
        1.0,
        (rand() * 0.4 + 0.8) as f32,
    );
    if let Some(s) = host.block_state_id("minecraft:fire") {
        let _ = level.set_block(
            host,
            front,
            s,
            BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS,
        );
    }
    Some(ENTROPY_AE)
}

/// A use of the manipulator on `pos`: more than 1600 AE needed (else the use passes), the player allowed there; the
/// charge it used comes off the item in hand.
fn entropy_use(
    host: &mut dyn ServerHost,
    player: u128,
    hand: Hand,
    held: &ItemStack,
    pos: Pos,
    face: Pos,
) -> InteractionResult {
    let Some(level) = here(&*host) else {
        return InteractionResult::Pass;
    };
    if item_power(held) <= ENTROPY_AE {
        return InteractionResult::Pass;
    }
    if !may_use(&*host, player, pos, face, held) {
        return InteractionResult::Fail;
    }
    let Some(spent) = apply(host, level, pos, face, player, held) else {
        return InteractionResult::Fail;
    };
    if spent > 0.0 {
        let mut stack = held.clone();
        crate::powered::use_power(&mut stack, spent);
        replace_hand(host, player, hand, held, stack);
    }
    InteractionResult::SuccessServer
}

/// The first water or lava source the player looks at within reach, before any block (`getPlayerPOVHitResult` with
/// SOURCE_ONLY fluids: Sunrise's ray passes fluids, so the ray is walked here).
fn aimed_source(host: &dyn Host, level: LevelRef, player: u128) -> Option<(Pos, Pos)> {
    let e = host.player(player)?.entity();
    let eye = e.eye_position(host)?;
    let look = e.view_vector(host)?;
    let reach = 5.0;
    let to = [
        eye[0] + look[0] * reach,
        eye[1] + look[1] * reach,
        eye[2] + look[2] * reach,
    ];
    let stop = level
        .clip(host, eye, to, ClipBlock::Outline, ClipFluid::None)
        .filter(|c| !c.miss)
        .map_or(reach, |c| {
            ((c.location[0] - eye[0]).powi(2)
                + (c.location[1] - eye[1]).powi(2)
                + (c.location[2] - eye[2]).powi(2))
            .sqrt()
        });
    let mut t = 0.0;
    let mut last = [i32::MIN; 3];
    while t <= stop {
        let p = [
            (eye[0] + look[0] * t).floor() as i32,
            (eye[1] + look[1] * t).floor() as i32,
            (eye[2] + look[2] * t).floor() as i32,
        ];
        if p != last {
            let face = if last[0] == i32::MIN {
                [0, 1, 0]
            } else {
                [last[0] - p[0], last[1] - p[1], last[2] - p[2]]
            };
            last = p;
            let b = crate::net::block_id(host, level, p);
            if (b == "minecraft:water" || b == "minecraft:lava")
                && level.block_property(host, p, "level").as_deref() == Some("0")
            {
                let face = if SIDES.contains(&face) {
                    face
                } else {
                    [0, 1, 0]
                };
                return Some((p, face));
            }
        }
        t += 0.05;
    }
    None
}

/// The entropy manipulator's Part.
pub(crate) struct Entropy;

#[mod_api::sunrise_part]
impl ItemPart for Entropy {
    /// useOn (once the block let the click through): the block used on, or a fluid source in front of the face.
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return None;
        };
        let level = here(&*host)?;
        let front = add(event.pos, event.face);
        let fb = crate::net::block_id(&*host, level, front);
        let source = (fb == "minecraft:water" || fb == "minecraft:lava")
            && level.block_property(&*host, front, "level").as_deref() == Some("0");
        let pos = if source { front } else { event.pos };
        let held = event.stack.clone();
        Some(entropy_use(
            host,
            event.player,
            event.hand,
            &held,
            pos,
            event.face,
        ))
    }

    /// use: a fluid source aimed at in the air.
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return None;
        };
        let level = here(&*host)?;
        let (pos, face) = aimed_source(&*host, level, event.player)?;
        let held = event.stack.clone();
        Some(entropy_use(
            host,
            event.player,
            event.hand,
            &held,
            pos,
            face,
        ))
    }

    /// EntropyManipulatorItem.hurtEnemy: with more than 1600 AE, a hit spends it and sets the mob alight for 8 seconds.
    fn hurt_enemy(&self, host: &mut dyn ServerHost, event: &mut HurtEnemyEvent<'_>) {
        let Some(p) = host.players().acting_player() else {
            return;
        };
        let Some(player) = p.entity().uuid(host) else {
            return;
        };
        let old = event.stack.clone();
        if item_power(&old) <= ENTROPY_AE {
            return;
        }
        let mut stack = old.clone();
        crate::powered::use_power(&mut stack, ENTROPY_AE);
        if !replace_hand(host, player, Hand::Main, &old, stack) {
            return;
        }
        if let Some(target) = host.entity(event.entity) {
            let now = target.fire_ticks(&*host).unwrap_or(0);
            if now < 160 {
                let _ = target.set_remaining_fire_ticks(host, 160);
            }
        }
    }
}

// ---- The meteorite compass

/// CompassService's search: up to 174 chunks out (MeteoriteCompassItem's request).
pub(crate) const COMPASS_RANGE: i32 = 174;

/// What the needle shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Needle {
    /// ON_THE_SPOT: a meteorite in this chunk (spins fast).
    Here,
    /// The chunk the nearest meteorite is in.
    Toward([i32; 2]),
    /// INDETERMINATE (spins slowly).
    None,
}

/// CompassService.calculateCompassDirection over the chunks known to hold a mysterious cube: this chunk, else the
/// nearest ring (chunks `offset` out) holding one, and in it the nearest by distance (pure: unit tested).
pub(crate) fn nearest(chunks: &[[i32; 2]], from: [i32; 2], max_range: i32) -> Needle {
    if chunks.contains(&from) {
        return Needle::Here;
    }
    let mut best: Option<(i32, i64, [i32; 2])> = None;
    for c in chunks {
        let (dx, dz) = (c[0] - from[0], c[1] - from[1]);
        let ring = dx.abs().max(dz.abs());
        if ring < 1 || ring >= max_range {
            continue;
        }
        let d = i64::from(dx * 16).pow(2) + i64::from(dz * 16).pow(2);
        if best.is_none_or(|(r, bd, _)| ring < r || (ring == r && d < bd)) {
            best = Some((ring, d, *c));
        }
    }
    best.map_or(Needle::None, |(_, _, c)| Needle::Toward(c))
}

/// MeteoriteCompassBakedModel.getAnimatedRotation: the needle's angle (radians), facing `yaw` (degrees), at `millis`.
pub(crate) fn needle_angle(needle: Needle, from: [i32; 2], yaw: f32, millis: u64) -> f64 {
    use std::f64::consts::PI;
    match needle {
        Needle::Here => (millis % 500) as f64 / 500.0 * PI * 2.0,
        Needle::None => (millis % 3000) as f64 / 3000.0 * PI * 2.0,
        Needle::Toward(c) => {
            let (up, side) = (f64::from(c[1] - from[1]), f64::from(c[0] - from[0]));
            let rad = (-up).atan2(side) - PI / 2.0;
            rad + f64::from(yaw) / 180.0 * PI + PI
        }
    }
}

/// The compass model (`hz/compass_full_<n>`) for an angle: sixteen needles a turn.
pub(crate) fn needle_model(angle: f64) -> u32 {
    ((angle / (std::f64::consts::PI / 8.0)).round() as i64).rem_euclid(16) as u32
}

#[derive(Default)]
struct Cubes {
    world: Option<std::path::PathBuf>,
    /// The mysterious cubes by level.
    at: HashMap<String, HashSet<Pos>>,
    dirty: bool,
}

static CUBES: Mutex<Option<Cubes>> = Mutex::new(None);

fn with_cubes<R>(host: &dyn ServerHost, f: impl FnOnce(&mut Cubes) -> R) -> R {
    let mut g = CUBES.lock().unwrap_or_else(|e| e.into_inner());
    let c = g.get_or_insert_with(Cubes::default);
    let dir = host.world_dir();
    if c.world != dir {
        // The world's list (`data/ae2_compass.txt`: `level x y z` a line).
        *c = Cubes {
            world: dir.clone(),
            ..Cubes::default()
        };
        if let Some(d) = &dir {
            let text =
                std::fs::read_to_string(d.join("data").join("ae2_compass.txt")).unwrap_or_default();
            for l in text.lines() {
                let w: Vec<&str> = l.split_whitespace().collect();
                if let [dim, x, y, z] = w.as_slice()
                    && let (Ok(x), Ok(y), Ok(z)) = (x.parse(), y.parse(), z.parse())
                {
                    c.at.entry((*dim).to_owned()).or_default().insert([x, y, z]);
                }
            }
        }
    }
    f(c)
}

fn save_cubes(host: &dyn ServerHost) {
    let text = with_cubes(host, |c| {
        if !c.dirty {
            return None;
        }
        c.dirty = false;
        let mut lines: Vec<String> =
            c.at.iter()
                .flat_map(|(d, s)| {
                    s.iter()
                        .map(move |p| format!("{d} {} {} {}", p[0], p[1], p[2]))
                })
                .collect();
        lines.sort();
        Some(lines.join("\n") + "\n")
    });
    if let (Some(text), Some(dir)) = (text, host.world_dir()) {
        let _ = std::fs::create_dir_all(dir.join("data"));
        let _ = std::fs::write(dir.join("data").join("ae2_compass.txt"), text);
    }
}

/// CompassService.notifyBlockChange: a mysterious cube placed (a meteorite) or gone.
pub(crate) fn note_cube(host: &dyn ServerHost, dim: &str, pos: Pos, present: bool) {
    with_cubes(host, |c| {
        let set = c.at.entry(dim.to_owned()).or_default();
        let changed = if present {
            set.insert(pos)
        } else {
            set.remove(&pos)
        };
        c.dirty |= changed;
    });
}

/// Mysterious cubes placed or gone (`block_changed` has no server host), noted at the level's tick end.
static CUBE_CHANGES: Mutex<Vec<(&'static str, Pos, bool)>> = Mutex::new(Vec::new());

/// `block_changed`: the mysterious cubes the compass looks for.
pub(crate) fn block_changed(dim: &'static str, pos: Pos, old: &str, new: &str) {
    const CUBE: &str = "ae2:mysterious_cube";
    let present = if new == CUBE && old != CUBE {
        true
    } else if old == CUBE && new != CUBE {
        false
    } else {
        return;
    };
    let mut g = CUBE_CHANGES.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() < 4096 {
        g.push((dim, pos, present));
    }
}

/// The needle each player was last sent.
static SENT_NEEDLE: Mutex<Option<HashMap<u128, String>>> = Mutex::new(None);

/// A player carrying a meteorite compass is told where its needle points, twice a second (CompassRequestPacket's
/// answer), when that changed.
fn compass_tick(host: &mut dyn ServerHost, player: u128) {
    let Some(p) = host.player(player) else { return };
    let carries = p
        .inventory_slots_owned(&*host)
        .unwrap_or_default()
        .iter()
        .any(|(_, s)| sid(s) == "ae2:meteorite_compass");
    if !carries {
        return;
    }
    let Some(at) = p.entity().position(&*host) else {
        return;
    };
    let from = [(at[0].floor() as i32) >> 4, (at[2].floor() as i32) >> 4];
    let dim = host.dimension();
    let chunks: Vec<[i32; 2]> = with_cubes(&*host, |c| {
        c.at.get(dim)
            .map(|s| s.iter().map(|p| [p[0] >> 4, p[2] >> 4]).collect())
            .unwrap_or_default()
    });
    let msg = match nearest(&chunks, from, COMPASS_RANGE) {
        Needle::Here => "compass here".to_owned(),
        Needle::None => "compass none".to_owned(),
        Needle::Toward(c) => format!("compass {} {} {} {}", from[0], from[1], c[0], c[1]),
    };
    let changed = {
        let mut g = SENT_NEEDLE.lock().unwrap_or_else(|e| e.into_inner());
        let m = g.get_or_insert_with(HashMap::new);
        if m.get(&player) == Some(&msg) {
            false
        } else {
            m.insert(player, msg.clone());
            true
        }
    };
    if changed {
        let _ = send_to_player(host, player, NET_TOOLS, msg.as_bytes());
    }
}

/// The client's needle (from the server), and where it was worked out from.
static NEEDLE: Mutex<(Needle, [i32; 2])> = Mutex::new((Needle::None, [0, 0]));

pub(crate) const COMPASS_PROPERTY: &str = "ae2:compass";

/// `item_model_range_property` for `ae2:compass`: the model of the needle as the player faces now.
pub(crate) fn compass_property(host: &dyn ClientHost) -> f32 {
    let (needle, from) = *NEEDLE.lock().unwrap_or_else(|e| e.into_inner());
    let yaw = host
        .local_player()
        .and_then(|p| p.entity().rotation(host))
        .map_or(0.0, |r| r[0]);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    needle_model(needle_angle(needle, from, yaw, millis)) as f32
}

// ---- Energy cells as items

/// The energy cells' capacity (EnergyCellBlock.getMaxPower).
pub(crate) fn energy_cell_capacity(item: &str) -> Option<f64> {
    match item {
        "ae2:energy_cell" => Some(200_000.0),
        "ae2:dense_energy_cell" => Some(1_600_000.0),
        _ => None,
    }
}

/// AEBaseEntityBlock.getDrops with EnergyCellBlockEntity.exportSettings(DISMANTLE_ITEM): an energy cell broken (by a
/// player, a blast, anything) drops with its share of its network's power (`internalCurrentPower`), which leaves the
/// network with it.
pub(crate) fn block_drops(dim: &'static str, pos: Pos, block: &str, drops: &mut PreparedBlockDrops<'_>) {
    if energy_cell_capacity(block).is_none() {
        return;
    }
    let Some(share) = crate::net::stored_share(dim, pos).filter(|s| *s > 0.0) else {
        return;
    };
    for i in 0..drops.len() {
        let Some(mut d) = drops.get_mut(i) else { continue };
        let Some((id, count)) = d.stack().map(|s| (key_id(s.item()), s.count())) else { continue };
        if id != block || count != 1 {
            continue;
        }
        let taken = crate::net::extract_power(dim, pos, share);
        if taken > 0.0 {
            if let Some(mut stack) = d.stack().and_then(|s| s.project()) {
                edit_custom_data(&mut stack, |c| {
                    c.insert(java("internalCurrentPower"), Tag::Double(taken.to_bits()));
                });
                let _ = d.set_stack(stack);
            }
        }
        break;
    }
}

/// An energy cell item's charge (its `internalCurrentPower`, or a dismantled one's settings).
pub(crate) fn energy_cell_power(stack: &ItemStack) -> f64 {
    let Some(d) = custom_data(stack) else {
        return 0.0;
    };
    let read = |t: Option<&Tag>| match t {
        Some(Tag::Double(v)) => Some(f64::from_bits(*v)),
        _ => None,
    };
    read(d.get(&java("internalCurrentPower")))
        .or_else(|| match d.get(&java("ae2:settings")) {
            Some(Tag::Compound(s)) => read(s.get(&java("internalCurrentPower"))),
            _ => None,
        })
        .unwrap_or(0.0)
}

/// The tooltip lines these items add: an energy cell's stored energy (EnergyCellBlockItem), a name press's name
/// (NamePressItem), the applicator's colour.
pub(crate) fn tooltip(host: &dyn ClientHost, stack: &ItemStack) -> Vec<String> {
    let id = sid(stack);
    let mut out = Vec::new();
    if let Some(most) = energy_cell_capacity(id) {
        let label = crate::screens::tr(host, "gui.ae2.StoredEnergy", "Stored Energy");
        out.push(crate::powered::energy_line(
            &label,
            energy_cell_power(stack),
            most,
        ));
    }
    if id == "ae2:name_press"
        && let Some(Tag::String(n)) = custom_data(stack).and_then(|d| d.get(&java("InscribeName")))
    {
        out.push(n.to_string_lossy());
    }
    out
}

// ---- The channel

/// The clients' messages, kept for each player's next tick.
static INBOX: Mutex<Vec<(u128, Vec<u8>)>> = Mutex::new(Vec::new());

pub(crate) fn from_client(_host: &mut dyn ServerHost, player: u128, data: &[u8]) {
    if data.len() > 160 {
        return;
    }
    let mut inbox = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    if inbox.iter().filter(|(p, _)| *p == player).count() < 32 {
        inbox.push((player, data.to_vec()));
    }
}

fn take_inbox(player: u128) -> Vec<Vec<u8>> {
    let mut inbox = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let mut mine = Vec::new();
    inbox.retain(|(p, d)| {
        if *p == player {
            mine.push(d.clone());
            false
        } else {
            true
        }
    });
    mine
}

/// The player's tick: their messages (the wheel, the knife), the compass, the knife's checks.
pub(crate) fn player_tick(host: &mut dyn ServerHost, player: u128, tick: u64) {
    for msg in take_inbox(player) {
        let text = String::from_utf8_lossy(&msg).to_string();
        // A close always goes through; the rest is rate limited as the terminals' clicks.
        if !text.starts_with("kclose") && !crate::storage::rate_ok(player) {
            continue;
        }
        if let Some(rest) = text.strip_prefix("wheel ") {
            crate::applicator::wheel(host, player, rest.trim() == "1");
        } else if text.starts_with('k') {
            crate::knife::message(host, player, &text);
        }
    }
    crate::knife::tick(host, player);
    if tick % 10 == player as u64 % 10 {
        compass_tick(host, player);
    }
}

/// The level's tick end: the tables of what the tool cells take, once a second; the compass's list saved.
pub(crate) fn level_tick(host: &mut dyn ServerHost, tick: u64) {
    let changes = std::mem::take(&mut *CUBE_CHANGES.lock().unwrap_or_else(|e| e.into_inner()));
    for (dim, pos, present) in changes {
        note_cube(&*host, dim, pos, present);
    }
    if tick % 20 == 0 {
        crate::tool_cells::refresh(&*host);
        save_cubes(&*host);
    }
}

pub(crate) fn logged_out(player: u128) {
    drop(take_inbox(player));
    if let Some(m) = SENT_NEEDLE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        m.remove(&player);
    }
    crate::knife::logged_out(player);
}

pub(crate) fn forget_all() {
    *CUBES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *SENT_NEEDLE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    INBOX.lock().unwrap_or_else(|e| e.into_inner()).clear();
    crate::tool_cells::forget_all();
    crate::knife::forget_all();
}

/// The server's messages: the compass's needle, the knife's screen.
pub(crate) fn from_server(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != NET_TOOLS {
        return false;
    }
    let text = String::from_utf8_lossy(data).to_string();
    let w: Vec<&str> = text.split_whitespace().collect();
    match w.first().copied() {
        Some("compass") => {
            let n = |i: usize| w.get(i).and_then(|v| v.parse::<i32>().ok());
            let v = match w.get(1).copied() {
                Some("here") => (Needle::Here, [0, 0]),
                Some("none") | None => (Needle::None, [0, 0]),
                _ => match (n(1), n(2), n(3), n(4)) {
                    (Some(a), Some(b), Some(c), Some(d)) => (Needle::Toward([c, d]), [a, b]),
                    _ => (Needle::None, [0, 0]),
                },
            };
            *NEEDLE.lock().unwrap_or_else(|e| e.into_inner()) = v;
        }
        Some(k) if k.starts_with('k') => crate::knife::from_server(host, &text),
        _ => {}
    }
    true
}

/// Messages for the server from the client's hooks (sent at the next client tick; screen hooks have no mutable host).
static OUTBOX: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub(crate) fn queue(msg: String) {
    let mut g = OUTBOX.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() < 32 {
        g.push(msg);
    }
}

/// A message to the server now (a hook with a mutable host).
pub(crate) fn send(host: &mut dyn ClientHost, msg: &str) {
    let _ = send_to_server(host, NET_TOOLS, msg.as_bytes());
}

pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    crate::knife::client_tick(host);
    let out = std::mem::take(&mut *OUTBOX.lock().unwrap_or_else(|e| e.into_inner()));
    for m in out {
        send(host, &m);
    }
}

// ---- Registration

pub(crate) fn register(r: &mut dyn Registrar) {
    let _ = SPARK.set(r.particle_key("minecraft:electric_spark"));
    let _ = SMOKE.set(r.particle_key("minecraft:large_smoke"));
    let _ = crate::cannon::CRIT.set(r.particle_key("minecraft:crit"));
    let k = r.item_key("ae2:charged_staff");
    r.item_part(k, Staff);
    let k = r.item_key("ae2:entropy_manipulator");
    r.item_part(k, Entropy);
    let k = r.item_key(crate::tool_cells::CANNON);
    r.item_part(k, crate::cannon::Cannon);
    let k = r.item_key(crate::tool_cells::APPLICATOR);
    r.item_part(k, crate::applicator::Applicator);
    for knife in crate::knife::KNIVES {
        let k = r.item_key(knife);
        r.item_part(k, crate::knife::Knife);
    }
    crate::paint::register(r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staff_numbers() {
        assert_eq!(STAFF_AE, 300.0);
        assert_eq!(STAFF_DAMAGE, 6.0);
        assert_eq!(ENTROPY_AE, 1600.0);
    }

    #[test]
    fn heat_order() {
        assert_eq!(heat_kind("minecraft:tnt"), Heat::Tnt);
        assert_eq!(heat_kind("ae2:tiny_tnt"), Heat::TinyTnt);
        assert_eq!(heat_kind("minecraft:iron_ore"), Heat::SmeltOrFire);
    }

    #[test]
    fn compass_finds_the_nearest_ring() {
        let chunks = [[10, 0], [3, 3], [-4, 1], [0, 200]];
        assert_eq!(nearest(&chunks, [3, 3], 174), Needle::Here);
        // Ring 3 holds [3, 3]; ring 4 holds [-4, 1]: the nearer ring wins.
        assert_eq!(nearest(&chunks, [0, 0], 174), Needle::Toward([3, 3]));
        // Out of range.
        assert_eq!(nearest(&[[0, 200]], [0, 0], 174), Needle::None);
        assert_eq!(nearest(&[], [0, 0], 174), Needle::None);
        // In one ring, the nearest by distance.
        assert_eq!(
            nearest(&[[2, 2], [2, 0]], [0, 0], 174),
            Needle::Toward([2, 0])
        );
    }

    #[test]
    fn needle_points() {
        use std::f64::consts::PI;
        // Due east (+x) facing south (yaw 0): rad = atan2(0, 1) - pi/2 = -pi/2, plus pi.
        let a = needle_angle(Needle::Toward([1, 0]), [0, 0], 0.0, 0);
        assert!((a - PI / 2.0).abs() < 1e-9);
        assert_eq!(needle_model(a), 4);
        assert_eq!(needle_model(0.0), 0);
        assert_eq!(needle_model(2.0 * PI), 0);
        assert_eq!(needle_model(-PI / 8.0), 15);
        // Turning the player turns the needle the other way round the model.
        let b = needle_angle(Needle::Toward([1, 0]), [0, 0], 90.0, 0);
        assert_eq!(needle_model(b), 8);
        assert!(needle_angle(Needle::Here, [0, 0], 0.0, 250) > 3.0);
    }

    #[test]
    fn energy_cells() {
        assert_eq!(
            energy_cell_capacity("ae2:dense_energy_cell"),
            Some(1_600_000.0)
        );
        assert_eq!(energy_cell_capacity("ae2:creative_energy_cell"), None);
    }
}
