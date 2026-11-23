//! AE2's tiny TNT (TinyTNTBlock, TinyTNTPrimedEntity, TinyTNTDispenseItemBehavior; gap-list item 8):
//! - lit by flint and steel (one use of it), by redstone, by a blast (a short fuse) or from a dispenser, it becomes a
//!   primed tiny TNT: vanilla's primed TNT entity (the same hop, gravity and drag as TinyTNTPrimedEntity), drawn as
//!   the tiny TNT block, tagged `ae2.tiny_tnt`, with AE2's fuse of 80;
//! - in water it goes out and drops as an item;
//! - it blows with AE2's own small blast instead of vanilla's (`explosion_start` cancels that): 6 explosion damage to
//!   what is within 1.5 blocks (a singularity and ender dust there entangle), and, with `tinyTntBlockDamage`, its
//!   crater: each block within 2 of a point a quarter block lower whose strength `2.3 - d² - (resistance + 0.3) *
//!   0.11` is left above 0.01 breaks as a blast breaks it (its drops; tiny TNT in it is primed).

use crate::cells::sid;
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub(crate) const TINY_TNT: &str = "ae2:tiny_tnt";
const TAG: &str = "ae2.tiny_tnt";
pub(crate) const FUSE: i32 = 80;
/// The `explosion_power` a primed tiny TNT is summoned with: its blast's radius as `explosion_start` hears it.
const POWER: f32 = 0.2;
/// How far (blocks) a blast's centre may be from where a primed tiny TNT was last seen and still be its blast.
const MATCH_RANGE: f64 = 3.0;

static EXPLOSION_PARTICLE: OnceLock<ParticleKey> = OnceLock::new();

pub(crate) fn register(r: &mut dyn Registrar) {
    let _ = EXPLOSION_PARTICLE.set(r.particle_key("minecraft:explosion"));
    let k = r.item_key(TINY_TNT);
    r.item_part(k, Dispense);
    let k = r.item_key("ae2:entropy_manipulator");
    r.item_part(k, EntropyDispense);
}

#[derive(Default)]
struct State {
    /// Primed tiny TNT entities by level (their ids): where each was last seen, and its fuse left (our count).
    lit: HashMap<(String, u32), Lit>,
    /// Blocks to prime at the level's tick end: position and fuse.
    primes: Vec<(&'static str, Pos, i32)>,
    /// A blast of ours is breaking blocks: its own `explosion_start` isn't one to replace.
    in_blast: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Lit {
    at: [f64; 3],
    fuse: i32,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

pub(crate) fn forget_all() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// TinyTNTPrimedEntity's start: a random sideways nudge and 0.2 up.
pub(crate) fn start_motion(angle: f64) -> [f64; 3] {
    [-angle.sin() * 0.02, 0.2, -angle.cos() * 0.02]
}

/// The crater's strength at a block (centre `d2` squared blocks from the blast, resistance `r`): it breaks above 0.01.
pub(crate) fn crater_strength(d2: f64, resistance: f64) -> f64 {
    2.3 - d2 - (resistance + 0.3) * 0.11
}

/// The blocks the crater round `c` takes, given each one's resistance (None: air, nothing to break).
pub(crate) fn crater(c: [f64; 3], resistance: &mut dyn FnMut(Pos) -> Option<f64>) -> Vec<Pos> {
    let mut out = Vec::new();
    // `(int)` truncates toward zero, as AE2's loop bounds do.
    let (x0, y0, z0) = (
        (c[0] - 2.0) as i32,
        (c[1] - 2.0) as i32,
        (c[2] - 2.0) as i32,
    );
    let mut x = x0;
    while f64::from(x) <= c[0] + 2.0 {
        let mut y = y0;
        while f64::from(y) <= c[1] + 2.0 {
            let mut z = z0;
            while f64::from(z) <= c[2] + 2.0 {
                let p = [x, y, z];
                if let Some(r) = resistance(p) {
                    let d2 = (f64::from(x) + 0.5 - c[0]).powi(2)
                        + (f64::from(y) + 0.5 - c[1]).powi(2)
                        + (f64::from(z) + 0.5 - c[2]).powi(2);
                    if crater_strength(d2, r) > 0.01 {
                        out.push(p);
                    }
                }
                z += 1;
            }
            y += 1;
        }
        x += 1;
    }
    out
}

/// A primed tiny TNT at `at` (TinyTNTBlock.startFuse): the entity, and the fuse's sound.
pub(crate) fn spawn_primed(host: &mut dyn ServerHost, at: [f64; 3], fuse: i32) {
    let dim = host.dimension();
    let m = start_motion(crate::world::rand() * std::f64::consts::TAU);
    let mut nbt = Compound::new();
    nbt.insert(
        java("fuse"),
        Tag::Short(fuse.clamp(0, i32::from(i16::MAX)) as i16),
    );
    nbt.insert(
        java("Motion"),
        Tag::List(m.iter().map(|v| Tag::Double(v.to_bits())).collect()),
    );
    nbt.insert(java("Tags"), Tag::List(vec![Tag::String(java(TAG))]));
    nbt.insert(java("explosion_power"), Tag::Float(POWER.to_bits()));
    let mut state = Compound::new();
    state.insert(java("Name"), Tag::String(java(TINY_TNT)));
    nbt.insert(java("block_state"), Tag::Compound(state));
    let id = host.summon(dim, "minecraft:tnt", at, nbt);
    debug(&format!(
        "tiny_tnt: summon at {at:?} in {dim} fuse {fuse} -> {id:?} (entity found {})",
        id.is_some_and(|id| host.entity(id).is_some())
    ));
    if let Some(id) = id {
        with_state(|s| s.lit.insert((dim.to_owned(), id), Lit { at, fuse }));
    }
    if let Some(level) = here(&*host) {
        let _ = level.play_sound(
            host,
            "minecraft:entity.tnt.primed",
            at,
            SoundSource::Blocks,
            1.0,
            1.0,
            None,
        );
    }
}

/// A tiny TNT block lit: it goes, and a primed one takes its place (at the level's tick end).
pub(crate) fn queue_prime(dim: &'static str, pos: Pos, fuse: i32) {
    debug(&format!("tiny_tnt: lit at {pos:?} in {dim} fuse {fuse}"));
    with_state(|s| {
        if !s.primes.iter().any(|p| p.0 == dim && p.1 == pos) {
            s.primes.push((dim, pos, fuse));
        }
    });
}

/// The block's tick: redstone lights it (TinyTNTBlock.neighborChanged / onPlace).
pub(crate) fn block_tick(host: &mut dyn ServerHost, pos: Pos) {
    let Some(level) = here(&*host) else { return };
    if level.has_neighbor_signal(&*host, pos) {
        queue_prime(host.dimension(), pos, FUSE);
    }
}

/// Flint and steel on the block (TinyTNTBlock.use): it is lit, the flint and steel used once. Some when it took it.
pub(crate) fn block_use(
    host: &mut dyn ServerHost,
    click: &BlockClick<'_>,
) -> Option<InteractionResult> {
    let level = here(&*host)?;
    if crate::net::block_id(&*host, level, click.pos) != TINY_TNT {
        return None;
    }
    let held = click.stack?;
    if sid(held) != "minecraft:flint_and_steel" {
        return None;
    }
    queue_prime(host.dimension(), click.pos, FUSE);
    let player = host.player(click.player)?;
    if !player.is_creative(&*host).unwrap_or(false) {
        let slot = match click.hand {
            Hand::Main => EquipmentSlot::MainHand,
            Hand::Off => EquipmentSlot::OffHand,
        };
        let _ = LivingRef::new(player.entity()).hurt_and_break(host, slot, 1);
    }
    Some(InteractionResult::SuccessServer)
}

/// A blast's block list: tiny TNT in it is spared the break (it drops nothing: `dropFromExplosion` false) and primed
/// with a short fuse (TinyTNTBlock.wasExploded: a quarter of the fuse at random, plus an eighth).
pub(crate) fn blasted(host: &mut dyn ServerHost, blocks: &mut Vec<Pos>) {
    let Some(level) = here(&*host) else { return };
    let dim = host.dimension();
    blocks.retain(|p| {
        if crate::net::block_id(&*host, level, *p) != TINY_TNT {
            return true;
        }
        let fuse = (crate::world::rand() * f64::from(FUSE / 4)) as i32 + FUSE / 8;
        queue_prime(dim, *p, fuse);
        false
    });
}

/// The end of the level's tick: blocks lit become primed; primed ones in water go out.
pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let primes: Vec<(Pos, i32)> = with_state(|s| {
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut s.primes)
            .into_iter()
            .partition(|p| p.0 == dim);
        s.primes = rest;
        here.into_iter().map(|p| (p.1, p.2)).collect()
    });
    for (pos, fuse) in primes {
        let block = crate::net::block_id(&*host, level, pos);
        if block != TINY_TNT {
            debug(&format!(
                "tiny_tnt: prime at {pos:?} skipped, the block is {block}"
            ));
            continue;
        }
        let removed = level.remove_block(host, pos, false);
        debug(&format!("tiny_tnt: block at {pos:?} removed {removed:?}"));
        spawn_primed(
            host,
            [
                f64::from(pos[0]) + 0.5,
                f64::from(pos[1]) + 0.5,
                f64::from(pos[2]) + 0.5,
            ],
            fuse,
        );
    }
    // After a reload the list is empty: the level's primed TNT with the tag, every second.
    let tick = crate::net::with_net(dim, |n| n.tick);
    if tick % 20 == 0 {
        for e in level.all_entities_owned(&*host).unwrap_or_default() {
            if e.tags(&*host).is_some_and(|t| t.contains(TAG)) {
                let at = e.position(&*host).unwrap_or([0.0; 3]);
                with_state(|s| {
                    s.lit
                        .entry((dim.to_owned(), e.id()))
                        .or_insert(Lit { at, fuse: FUSE });
                });
            }
        }
    }
    let lit: Vec<(u32, Lit)> = with_state(|s| {
        s.lit
            .iter()
            .filter(|((d, _), _)| d == dim)
            .map(|((_, id), l)| (*id, *l))
            .collect()
    });
    for (id, l) in lit {
        let Some(e) = host.entity(id) else {
            // Gone: kept a few ticks past its fuse, for its blast (`explosion_start`) to find it by where it was.
            if l.fuse < -5 {
                debug(&format!(
                    "tiny_tnt: entity {id} gone (fuse {}), forgotten",
                    l.fuse
                ));
                with_state(|s| s.lit.remove(&(dim.to_owned(), id)));
            } else {
                with_state(|s| {
                    if let Some(x) = s.lit.get_mut(&(dim.to_owned(), id)) {
                        x.fuse -= 1;
                    }
                });
            }
            continue;
        };
        let at = e.position(&*host).unwrap_or(l.at);
        with_state(|s| {
            if let Some(x) = s.lit.get_mut(&(dim.to_owned(), id)) {
                x.at = at;
                x.fuse -= 1;
            }
        });
        if l.fuse % 10 == 0 {
            debug(&format!(
                "tiny_tnt: entity {id} at {at:?} fuse {} (our count)",
                l.fuse
            ));
        }
        // Sunrise reads `isInWater` for players and mobs only: for primed TNT, the block it is in.
        let block_at = [
            at[0].floor() as i32,
            (at[1] + 0.1).floor() as i32,
            at[2].floor() as i32,
        ];
        let wet = e.is_in_water(&*host).unwrap_or_else(|| {
            crate::world::fluid_at(&*host, level, block_at) == Some("minecraft:water")
        });
        if wet {
            debug(&format!("tiny_tnt: entity {id} in water at {at:?}, out"));
            // TinyTNTPrimedEntity.tick: in water it goes out, an item with its motion.
            let _ = e.discard(host);
            with_state(|s| s.lit.remove(&(dim.to_owned(), id)));
            if let Some(item) = crate::cells::new_stack(&*host, TINY_TNT, 1) {
                let _ = level.spawn_item_with_motion(
                    host,
                    at[0],
                    at[1],
                    at[2],
                    item,
                    [0.0, 0.0, 0.0],
                    10,
                );
            }
        }
    }
}

/// `explosion_start`: a primed tiny TNT's blast is AE2's own. True to cancel vanilla's.
pub(crate) fn explosion_start(host: &mut dyn ServerHost, ex: &ExplosionStart<'_>) -> bool {
    if with_state(|s| s.in_blast) {
        return false;
    }
    let dim = host.dimension();
    // Sunrise's primed TNT blast reports no `source` (it sets the direct entity, not the source, as of 2d81263): a
    // blast of our power where a primed tiny TNT just was is its blast.
    let src = with_state(|s| {
        let candidates: Vec<(u32, [f64; 3])> = s
            .lit
            .iter()
            .filter(|((d, _), _)| d == dim)
            .map(|((_, id), l)| (*id, l.at))
            .collect();
        let found = ours(ex.source, ex.radius, ex.center, &candidates);
        if let Some(id) = found {
            s.lit.remove(&(dim.to_owned(), id));
        }
        found
    });
    if ex.radius < 1.0 || src.is_some() {
        debug(&format!(
            "tiny_tnt: explosion_start at {:?} radius {} source {:?} -> ours {src:?}",
            ex.center, ex.radius, ex.source
        ));
    }
    let Some(src) = src else { return false };
    explode(host, ex.center, src);
    true
}

/// Which primed tiny TNT (`lit`: id and where it was last seen) a blast is (pure: unit tested): its source when that
/// is one, else, for a blast of a tiny TNT's power with no source, the nearest one within reach of its centre.
fn ours(
    source: Option<u32>,
    radius: f32,
    center: [f64; 3],
    lit: &[(u32, [f64; 3])],
) -> Option<u32> {
    if let Some(src) = source {
        return lit.iter().any(|(id, _)| *id == src).then_some(src);
    }
    if (radius - POWER).abs() > 1e-3 {
        return None;
    }
    lit.iter()
        .map(|(id, at)| {
            let d2 = (0..3).map(|i| (at[i] - center[i]).powi(2)).sum::<f64>();
            (*id, d2)
        })
        .filter(|(_, d2)| *d2 <= MATCH_RANGE * MATCH_RANGE)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// TinyTNTPrimedEntity.explode.
fn explode(host: &mut dyn ServerHost, at: [f64; 3], source: u32) {
    debug(&format!("tiny_tnt: explode {source} at {at:?}"));
    let Some(level) = here(&*host) else { return };
    let pitch = ((1.0 + (crate::world::rand() - crate::world::rand()) * 0.2) * 32.9) as f32;
    let _ = level.play_sound(
        host,
        "minecraft:entity.generic.explode",
        at,
        SoundSource::Blocks,
        4.0,
        pitch,
        None,
    );
    if let Some(p) = EXPLOSION_PARTICLE.get().copied() {
        let _ = level.send_particles(
            host,
            &ParticleOptions::simple(p),
            None,
            at,
            [0.0; 3],
            0.0,
            1,
        );
    }
    let src = host.entity(source);
    let block_at = [
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    ];
    let wet = match src {
        Some(e) => e.is_in_water(&*host),
        None => None,
    }
    .unwrap_or_else(|| crate::world::fluid_at(&*host, level, block_at) == Some("minecraft:water"));
    if wet {
        debug("tiny_tnt: explode in water, no damage");
        return;
    }
    // What is within 1.5 blocks: an entangling pair first (the items' hurt), then 6 explosion damage each.
    let mut near: Vec<EntityRef> = level
        .entities_in_owned(
            &*host,
            [at[0] - 1.5, at[1] - 1.5, at[2] - 1.5],
            [at[0] + 1.5, at[1] + 1.5, at[2] + 1.5],
        )
        .unwrap_or_default();
    near.retain(|e| Some(e.id()) != Some(source));
    crate::world::explosion_transform(host, &mut near);
    for e in near {
        let _ = e.hurt(host, "minecraft:explosion", 6.0, src, None);
    }
    if !crate::config::config().tiny_tnt_block_damage {
        return;
    }
    let c = [at[0], at[1] - 0.25, at[2]];
    let mut blocks = crater(c, &mut |p| match level.explosion_resistance(
        host,
        p,
        None,
        c,
        0.2,
        false,
        ExplosionInteraction::Block,
    ) {
        Some(ExplosionResistance::Resists(r)) => Some(f64::from(r)),
        _ => None,
    });
    blasted(host, &mut blocks);
    debug(&format!("tiny_tnt: crater {} blocks", blocks.len()));
    with_state(|s| s.in_blast = true);
    let _ = level.explosion_destroy_blocks(
        host,
        &blocks,
        None,
        c,
        0.2,
        false,
        ExplosionInteraction::Block,
    );
    with_state(|s| s.in_blast = false);
}

/// TinyTNTDispenseItemBehavior: a primed tiny TNT in front of the dispenser, one item used.
pub(crate) struct Dispense;

#[mod_api::sunrise_part]
impl ItemPart for Dispense {
    fn dispense_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut DispenseItemEvent<'_>,
    ) -> Option<Dispensed> {
        let d = dir_pos(event.facing);
        let p = add(event.pos, d);
        spawn_primed(
            host,
            [
                f64::from(p[0]) + 0.5,
                f64::from(p[1]) + 0.5,
                f64::from(p[2]) + 0.5,
            ],
            FUSE,
        );
        Some((
            crate::cells::with_count(event.stack, event.stack.count().saturating_sub(1)),
            1000,
        ))
    }
}

/// The entropy manipulator from a dispenser (BlockToolDispenseItemBehavior).
pub(crate) struct EntropyDispense;

#[mod_api::sunrise_part]
impl ItemPart for EntropyDispense {
    fn dispense_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut DispenseItemEvent<'_>,
    ) -> Option<Dispensed> {
        let d = dir_pos(event.facing);
        match crate::world::entropy_dispense(host, event.pos, d, event.stack) {
            Some(left) => Some((Some(left), 1000)),
            None => Some((Some(event.stack.clone()), 1001)),
        }
    }
}

pub(crate) fn dir_pos(d: Direction) -> Pos {
    match d {
        Direction::Down => [0, -1, 0],
        Direction::Up => [0, 1, 0],
        Direction::North => [0, 0, -1],
        Direction::South => [0, 0, 1],
        Direction::West => [-1, 0, 0],
        _ => [1, 0, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crater_shape() {
        // Stone (resistance 6) round a blast in the open: only the nearest blocks go.
        let c = [0.5, 64.25, 0.5];
        let mut stone = |_: Pos| Some(6.0);
        let got = crater(c, &mut stone);
        assert!(got.contains(&[0, 64, 0]));
        assert!(!got.contains(&[2, 64, 0]));
        // Obsidian (1200) never.
        let mut obsidian = |_: Pos| Some(1200.0);
        assert!(crater(c, &mut obsidian).is_empty());
        // Air is skipped.
        let mut air = |_: Pos| None;
        assert!(crater(c, &mut air).is_empty());
        // Dirt (0.5): a wider hole than stone's.
        let mut dirt = |_: Pos| Some(0.5);
        assert!(crater(c, &mut dirt).len() > got.len());
        assert!(crater_strength(0.0, 0.0) > 2.0);
    }

    #[test]
    fn blast_is_ours() {
        let lit = [(7, [0.5, 64.0, 0.5]), (9, [10.5, 64.0, 0.5])];
        // A source that is ours, or one that isn't.
        assert_eq!(ours(Some(9), 4.0, [0.0; 3], &lit), Some(9));
        assert_eq!(ours(Some(3), POWER, [0.5, 64.0, 0.5], &lit), None);
        // No source (Sunrise's primed TNT): our power, the nearest within reach.
        assert_eq!(ours(None, POWER, [0.6, 64.06, 0.5], &lit), Some(7));
        assert_eq!(ours(None, POWER, [10.0, 63.5, 0.5], &lit), Some(9));
        assert_eq!(ours(None, POWER, [5.5, 64.0, 0.5], &lit), None);
        // A vanilla TNT's blast (power 4) beside it is not ours.
        assert_eq!(ours(None, 4.0, [0.5, 64.0, 0.5], &lit), None);
        assert_eq!(ours(None, POWER, [0.5, 64.0, 0.5], &[]), None);
    }

    #[test]
    fn hop() {
        let m = start_motion(0.0);
        assert_eq!(m[1], 0.2);
        assert!((m[2] + 0.02).abs() < 1e-9);
    }
}
