//! AE2's looks and sounds that are more than a block model (gap list section 7): the molecular assembler's power light,
//! its sparks while crafting and the item in its glass (MolecularAssemblerRenderer, CraftingFx), the item on the
//! charger (ChargerBlockEntityRenderer), the ambient particles of the vibration chamber, the charger and the growth
//! accelerator (their blocks' `animateTick`), and the sounds of placing a part and of the entropy manipulator.
//!
//! Upstream's `assembler_effects`, `me_animate.rs` and `spend_placed` (`ae2-horizon/mixins/ae2/me.rs`). The server writes
//! what a client draws into the block entity's update tag (`device_looks`, `update_tag`); the client draws it in
//! `block_entity_submit` and adds the particles in `block_entity_client_tick`.

use crate::cells::sid;
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The particle types used here, resolved at registration.
struct Particles {
    smoke: ParticleKey,
    large_smoke: ParticleKey,
    flame: ParticleKey,
    /// AE2's own (`assets/ae2/particles/*.json`, with their `sunrise:behaviour`).
    lightning: ParticleKey,
    crafting: ParticleKey,
}

static KEYS: OnceLock<Particles> = OnceLock::new();

pub(crate) fn register(r: &mut dyn Registrar) {
    let keys = Particles {
        smoke: r.particle_key("minecraft:smoke"),
        large_smoke: r.particle_key("minecraft:large_smoke"),
        flame: r.particle_key("minecraft:flame"),
        lightning: r.particle_key("ae2:lightning_fx"),
        crafting: r.particle_key("ae2:crafting_fx"),
    };
    let _ = KEYS.set(keys);
}

const ASSEMBLER: &str = "ae2:molecular_assembler";
const CHARGER: &str = "ae2:charger";
const ACCELERATOR: &str = "ae2:growth_accelerator";

// ---- The server: what the clients draw

/// MolecularAssemblerBlockEntity's speed by its Acceleration Cards (the `speed` of AssemblerAnimationPacket).
pub(crate) fn assembler_speed(cards: usize) -> i64 {
    match cards {
        0 => 10,
        1 => 13,
        2 => 17,
        3 => 20,
        4 => 25,
        _ => 50,
    }
}

/// MolecularAssemblerRenderer: `ceil(speed / 5)` sparks every four ticks.
pub(crate) fn sparks_for(speed: i64) -> u32 {
    (speed.max(1) as u32).div_ceil(5)
}

fn set_string(nbt: &mut Compound, key: &str, value: &str) {
    let now = match nbt.get(&java(key)) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    if now == value {
        return;
    }
    if value.is_empty() {
        nbt.remove(&java(key));
    } else {
        nbt.insert(java(key), Tag::String(java(value)));
    }
}

fn set_int(nbt: &mut Compound, key: &str, value: i64) {
    if nbt_i64(nbt, key) != value || (value != 0 && !nbt.contains_key(&java(key))) {
        if value == 0 {
            nbt.remove(&java(key));
        } else {
            nbt.insert(java(key), Tag::Int(value as i32));
        }
    }
}

/// A device's tick (`devices::run_device`): the charger's item and whether it works, the assembler's craft and speed,
/// the growth accelerator's lit look.
pub(crate) fn device_looks(host: &mut dyn ServerHost, pos: Pos, block: &str, fields: &mut Fields) {
    match block {
        CHARGER => {
            let held = fields.items.first().cloned().flatten();
            let id = held.as_ref().map(|s| sid(s).to_owned()).unwrap_or_default();
            let working = held
                .as_ref()
                .is_some_and(|s| !crate::world::charger_done(&*host, s));
            set_string(&mut fields.nbt, "holds", &id);
            set_int(&mut fields.nbt, "hz_working", i64::from(working));
        }
        ASSEMBLER => {
            let dim = host.dimension();
            let making = crate::autocraft::assembling_output(dim, pos).unwrap_or_default();
            let cards = fields
                .items
                .iter()
                .skip(1)
                .take(5)
                .flatten()
                .filter(|s| sid(s) == "ae2:speed_card")
                .count();
            set_string(&mut fields.nbt, "crafting", &making);
            set_int(
                &mut fields.nbt,
                "speed",
                if making.is_empty() {
                    0
                } else {
                    assembler_speed(cards)
                },
            );
        }
        ACCELERATOR => {
            // GrowthAcceleratorBlock's POWERED: while its network has power.
            let dim = host.dimension();
            let on = crate::net::device_state(dim, pos).1;
            if let Some(level) = here(&*host)
                && level
                    .block_property(&*host, pos, "powered")
                    .is_some_and(|v| v != on.to_string())
            {
                crate::net::with_net(dim, |n| n.prop_sets.push((pos, "powered", on.to_string())));
            }
        }
        _ => {}
    }
}

/// What the clients are sent for the charger and the assembler (their renderers' data).
pub(crate) fn update_tag(id: &str, nbt: &Compound) -> Option<Compound> {
    let keys: &[&str] = match id {
        CHARGER => &["holds", "hz_working"],
        ASSEMBLER => &["crafting", "speed"],
        _ => return None,
    };
    let mut out = Compound::new();
    for k in keys {
        if let Some(v) = nbt.get(&java(k)) {
            out.insert(java(k), v.clone());
        }
    }
    Some(out)
}

// ---- Sounds (the server's: every player near hears them)

fn centre(pos: Pos) -> [f64; 3] {
    [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.5,
        f64::from(pos[2]) + 0.5,
    ]
}

/// A sound at a block's centre, for everyone near.
pub(crate) fn sound_at(host: &mut dyn ServerHost, pos: Pos, sound: &str, volume: f32, pitch: f32) {
    if let Some(level) = here(&*host) {
        let _ = level.play_sound(
            host,
            sound,
            centre(pos),
            SoundSource::Blocks,
            volume,
            pitch,
            None,
        );
    }
}

/// PartPlacement: the part's sound type's place sound (metal), as a block placed (volume (1 + 1) / 2, pitch 0.8).
pub(crate) fn part_placed(host: &mut dyn ServerHost, pos: Pos) {
    crate::commands::PLACES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    sound_at(host, pos, "minecraft:block.metal.place", 1.0, 0.8);
}

/// The fizz of water put where it boils away (FluidPlacement in an ultra-warm level, as a bucket poured there):
/// `fire.extinguish` at 0.5, pitch 2.6 + (r - r) * 0.8, and eight large smoke puffs.
pub(crate) fn fizz(host: &mut dyn ServerHost, pos: Pos) {
    let pitch = 2.6 + (crate::world::rand() - crate::world::rand()) as f32 * 0.8;
    sound_at(host, pos, "minecraft:block.fire.extinguish", 0.5, pitch);
    if let (Some(level), Some(k)) = (here(&*host), KEYS.get()) {
        let _ = level.send_particles(
            host,
            &ParticleOptions::simple(k.large_smoke),
            None,
            centre(pos),
            [0.3, 0.3, 0.3],
            0.0,
            8,
        );
    }
}

/// EntropyManipulatorItem: cooling (or a heat recipe) fizzes; lighting a fire is flint and steel's sound.
pub(crate) fn entropy_used(host: &mut dyn ServerHost, pos: Pos, lit_fire: bool) {
    if lit_fire {
        let pitch = crate::world::rand() as f32 * 0.4 + 0.8;
        sound_at(host, pos, "minecraft:item.flintandsteel.use", 1.0, pitch);
    } else {
        fizz(host, pos);
    }
}

/// Whether water placed in level `dim` boils away (`DimensionType.ultraWarm`: the Nether).
pub(crate) fn boils_water(dim: &str) -> bool {
    dim == "minecraft:the_nether"
}

// ---- The client: particles (Block.animateTick) and the renderers

/// The block entities with a client tick here.
pub(crate) fn client_ticks(block: &str) -> bool {
    matches!(
        block,
        ASSEMBLER | CHARGER | ACCELERATOR | crate::generators::VIBRATION
    )
}

/// How likely ClientLevel.animateTick picks the block `d` away from the player in one tick: 667 tries in a 16 radius
/// and 667 in a 32 radius, each offset `nextInt(r) - nextInt(r)` per axis.
pub(crate) fn animate_chance(d: [i32; 3]) -> f64 {
    let pick = |r: i32| -> f64 {
        d.iter()
            .map(|&k| f64::from((r - k.abs()).max(0)) / f64::from(r * r))
            .product::<f64>()
    };
    (667.0 * (pick(16) + pick(32))).min(1.0)
}

/// The front and top of a block that keeps a facing and a spin (BlockOrientation: the spin turns the top about the
/// front, a quarter clockwise each).
pub(crate) fn orientation(facing: Option<&str>, spin: i32) -> ([f64; 3], [f64; 3]) {
    let f = facing_dir(facing);
    let front = [f64::from(f[0]), f64::from(f[1]), f64::from(f[2])];
    let mut up = if f[1] == 0 {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, -1.0]
    };
    for _ in 0..spin.rem_euclid(4) {
        // up' = front x up (a quarter turn about the front).
        up = cross(front, up);
    }
    (front, up)
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn rand() -> f64 {
    crate::world::rand()
}

/// Each assembler's ticks since its last sparks (every four ticks while it crafts).
static SPARK_WAIT: Mutex<Option<HashMap<Pos, u32>>> = Mutex::new(None);

pub(crate) fn client_tick(event: &mut BlockEntityClientTickEvent<'_>) {
    if !crate::config::config().enable_effects {
        return;
    }
    let Some(k) = KEYS.get() else { return };
    // The two saved fields read here: the assembler's `speed`, the charger's `hz_working`.
    let Some((pos, id, speed, working)) = event
        .subject
        .with_view(|v| (v.position(), v.id().to_owned(), nbt_i64(v.nbt(), "speed"), nbt_i64(v.nbt(), "hz_working")))
    else {
        return;
    };
    let host = event.subject.host();
    if id == ASSEMBLER {
        // MolecularAssemblerRenderer: while it crafts, every four ticks, sparks that shrink into its middle.
        let due = {
            let mut g = SPARK_WAIT.lock().unwrap_or_else(|e| e.into_inner());
            let m = g.get_or_insert_with(HashMap::new);
            if speed <= 0 {
                m.remove(&pos);
                false
            } else {
                let n = m.entry(pos).or_insert(0);
                *n += 1;
                if *n >= 4 {
                    *n = 0;
                    true
                } else {
                    false
                }
            }
        };
        if due {
            let opts = ParticleOptions::simple(k.crafting);
            for _ in 0..sparks_for(speed) {
                let off = [rand() - 0.5, rand() - 0.5, rand() - 0.5];
                let len = (off[0] * off[0] + off[1] * off[1] + off[2] * off[2])
                    .sqrt()
                    .max(0.001);
                let r = 0.1 + rand() * 0.25;
                let at = std::array::from_fn(|i| f64::from(pos[i]) + 0.5 + off[i] / len * r);
                let _ = host.add_particle(&opts, at, [0.0; 3]);
            }
        }
        return;
    }
    // Block.animateTick runs when the client's random pick lands on the block.
    let Some(me) = host
        .local_player()
        .and_then(|p| p.entity().position(&*host))
    else {
        return;
    };
    let d = [0, 1, 2].map(|i| pos[i] - me[i].floor() as i32);
    if rand() >= animate_chance(d) {
        return;
    }
    let Some(level) = host.level(host.dimension()) else {
        return;
    };
    let prop = |key: &str| level.block_property(&*host, pos, key);
    let mid = centre(pos);
    match id.as_str() {
        // VibrationChamberBlock.animateTick: smoke and a flame at its vent while it burns.
        crate::generators::VIBRATION if prop("powered").as_deref() == Some("true") => {
            let spin = prop("spin").and_then(|s| s.parse().ok()).unwrap_or(0);
            let (front, up) = orientation(prop("facing").as_deref(), spin);
            let west = cross(front, up);
            let (ox, oy) = (rand(), rand() * 0.2);
            let at: [f64; 3] = std::array::from_fn(|i| {
                mid[i] + front[i] * 0.6 + up[i] * (-0.3 + oy) + west[i] * (0.3 * ox - 0.15)
            });
            let _ = host.add_particle(&ParticleOptions::simple(k.smoke), at, [0.0; 3]);
            let _ = host.add_particle(&ParticleOptions::simple(k.flame), at, [0.0; 3]);
        }
        // ChargerBlock.animateTick: half the time, three bolts over what it charges.
        CHARGER if working != 0 && rand() >= 0.5 => {
            let opts = ParticleOptions::simple(k.lightning);
            for _ in 0..3 {
                let (xo, zo) = (rand() * 0.3 - 0.15, rand() * 0.3 - 0.15);
                let yo = if rand() < 0.5 { -0.3 } else { 0.3 };
                let _ = host.add_particle(&opts, [mid[0] + xo, mid[1] + yo, mid[2] + zo], [0.0; 3]);
            }
        }
        // GrowthAcceleratorBlock.animateTick: lightning off a side with air beside it, while powered.
        ACCELERATOR if prop("powered").as_deref() == Some("true") => {
            let (front, up) = orientation(prop("facing").as_deref(), 0);
            let west = cross(front, up);
            let (d0, d1) = (rand() - 0.5, rand() - 0.5);
            let neg = |v: [f64; 3]| v.map(|x| -x);
            let (dx, dz, side) = match (rand() * 4.0) as u32 {
                0 => (0.6, d1, west),
                1 => (d1, 0.6, front),
                2 => (d1, -0.6, neg(front)),
                _ => (-0.6, d1, neg(west)),
            };
            let beside = [0, 1, 2].map(|i| pos[i] + side[i] as i32);
            let air = level
                .block(&*host, beside)
                .and_then(|b| b.id(&*host).map(str::to_owned))
                .is_none_or(|b| b.ends_with(":air"));
            if air {
                let at: [f64; 3] =
                    std::array::from_fn(|i| mid[i] + up[i] * d0 + west[i] * dx + front[i] * dz);
                let _ = host.add_particle(&ParticleOptions::simple(k.lightning), at, [0.0; 3]);
            }
        }
        _ => {}
    }
}

/// Turns the pose so its -z is the block's front (as the crafting monitor's renderer does), then by its spin.
fn face_front(draw: &mut dyn BlockEntityDrawContext, facing: &str, spin: f32) {
    let c = [0.5, 0.5, 0.5];
    match facing {
        "south" => draw.rotate_about([0.0, 1.0, 0.0], 180.0, c),
        "east" => draw.rotate_about([0.0, 1.0, 0.0], -90.0, c),
        "west" => draw.rotate_about([0.0, 1.0, 0.0], 90.0, c),
        "up" => draw.rotate_about([1.0, 0.0, 0.0], 90.0, c),
        "down" => draw.rotate_about([1.0, 0.0, 0.0], -90.0, c),
        _ => {}
    }
    if spin != 0.0 {
        draw.rotate_about([0.0, 0.0, 1.0], -90.0 * spin, c);
    }
}

/// Full brightness (15 sky, 15 block), as AE2's emissive parts draw.
const FULL_BRIGHT: u32 = 0x00F0_00F0;

/// MolecularAssemblerRenderer: the animated power light inside the shell (full bright) while powered, and the item
/// it crafts in the glass.
pub(crate) fn assembler_submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let Some(level) = host.level(host.dimension()) else {
        return;
    };
    let powered = level.block_property(host, event.pos, "powered").as_deref() == Some("true");
    let making = match event.entity.nbt.get(&java("crafting")) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let draw = &mut *event.draw;
    let light = draw.light();
    if powered {
        draw.set_light(FULL_BRIGHT);
        let sprite = "ae2:block/molecular_assembler_lights";
        let face = || Some(CubeFace::new(sprite).with_uv([2.6, 2.6, 13.4, 13.4]));
        draw.cube(
            [2.0, 2.0, 2.0],
            [14.0, 14.0, 14.0],
            [face(), face(), face(), face(), face(), face()],
            0xFF_FFFF,
        );
        draw.set_light(light);
    }
    if let Some(stack) =
        crate::cells::key_from_string(&making).and_then(|k| crate::cells::stack_of(host, &k, 1))
    {
        draw.push();
        // As a dropped item, a little below the middle (renderItem's -0.3 / -0.2 for blocks).
        draw.translate([0.5, 0.25, 0.5]);
        draw.item(&stack, ItemDisplayContext::Ground, None, 0);
        draw.pop();
    }
}

/// ChargerBlockEntityRenderer: the item it holds, upright in its middle, turned with the block.
pub(crate) fn charger_submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let id = match event.entity.nbt.get(&java("holds")) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => return,
    };
    let Some(stack) = crate::cells::new_stack(host, &id, 1) else {
        return;
    };
    let Some(level) = host.level(host.dimension()) else {
        return;
    };
    let facing = level
        .block_property(host, event.pos, "facing")
        .unwrap_or_else(|| "north".to_owned());
    let spin: f32 = level
        .block_property(host, event.pos, "spin")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let draw = &mut *event.draw;
    draw.push();
    face_front(draw, &facing, spin);
    draw.translate([0.5, 0.375, 0.5]);
    draw.scale([0.5, 0.5, 0.5]);
    draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
    draw.pop();
}

/// The renderers here, by block (`devices`'s `block_entity_submit`). True when the block is one of them.
pub(crate) fn submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) -> bool {
    match event.block {
        ASSEMBLER => assembler_submit(host, event),
        CHARGER => charger_submit(host, event),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembler_sparks_follow_its_cards() {
        // MolecularAssemblerBlockEntity's speeds; ceil(speed / 5) sparks.
        let counts: Vec<u32> = (0..=5).map(|c| sparks_for(assembler_speed(c))).collect();
        assert_eq!(counts, vec![2, 3, 4, 4, 5, 10]);
        assert_eq!(assembler_speed(9), 50);
    }

    #[test]
    fn animate_tick_odds() {
        // The player's own block is picked most; nothing past 32 blocks on any axis.
        let here = animate_chance([0, 0, 0]);
        assert!(here > 0.15 && here < 0.2, "{here}");
        assert!(animate_chance([3, 1, 3]) < here);
        assert_eq!(animate_chance([32, 0, 0]), 0.0);
        assert!(animate_chance([20, 0, 0]) > 0.0);
    }

    #[test]
    fn spin_turns_the_top_about_the_front() {
        let (front, up) = orientation(Some("north"), 0);
        assert_eq!(front, [0.0, 0.0, -1.0]);
        assert_eq!(up, [0.0, 1.0, 0.0]);
        let (_, up2) = orientation(Some("north"), 2);
        assert_eq!(up2.map(|v| v + 0.0), [0.0, -1.0, 0.0]);
        let (_, up4) = orientation(Some("north"), 4);
        assert_eq!(up4, [0.0, 1.0, 0.0]);
        // Facing up: the top points north.
        assert_eq!(orientation(Some("up"), 0).1, [0.0, 0.0, -1.0]);
    }

    #[test]
    fn only_the_nether_boils_water() {
        assert!(boils_water("minecraft:the_nether"));
        assert!(!boils_water("minecraft:overworld"));
    }
}
