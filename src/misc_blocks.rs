//! AE2's light detector and crank (gap-list item 8):
//! - LightDetectorBlock / LightDetectorBlockEntity: every 31 ticks the light where it stands (the brighter of block
//!   light and the sky light less the sky's darkening, `getMaxLocalRawBrightness`); while that is above 0 it gives
//!   it less 6 as redstone on every side; its neighbours are told when it changes. With nothing to hold it, it drops;
//! - CrankBlock / CrankBlockEntity: a use turns it (18 ticks a turn, a new turn only as the last ends); each turn
//!   gives the machine it sits on 160 AE (`ICrankable.applyTurn`: the charger's and inscriber's buffer, the growth
//!   accelerator's stored power, each up to 1600); more than ten turns against a full machine and it breaks off. It
//!   is placed on a crankable machine only (facing out of the clicked face) and drops when that goes.

use crate::util::*;
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub(crate) const LIGHT_DETECTOR: &str = "ae2:light_detector";
pub(crate) const CRANK: &str = "ae2:crank";

/// Block entities this module ticks.
pub(crate) fn ticks(block: &str) -> bool {
    matches!(block, LIGHT_DETECTOR | CRANK | crate::tiny_tnt::TINY_TNT)
}

#[derive(Default)]
struct State {
    /// Light detectors' signals.
    signals: HashMap<(&'static str, Pos), i32>,
    /// Signals changed: neighbours to tell (at the level's tick end).
    updates: Vec<(&'static str, Pos)>,
    /// Blocks to break with their drops at the level's tick end (a block entity can't remove itself in its tick).
    breaks: Vec<(&'static str, Pos)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

pub(crate) fn forget_all() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, block: &str, fields: &mut Fields) {
    match block {
        LIGHT_DETECTOR => light_tick(host, pos, fields),
        CRANK => crank_tick(host, pos, fields),
        crate::tiny_tnt::TINY_TNT => crate::tiny_tnt::block_tick(host, pos),
        _ => {}
    }
}

/// The end of the level's tick: neighbours told of new signals, broken-off blocks dropped.
pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let (updates, breaks) = with_state(|s| {
        let (u, ru): (Vec<_>, Vec<_>) = std::mem::take(&mut s.updates)
            .into_iter()
            .partition(|u| u.0 == dim);
        s.updates = ru;
        let (b, rb): (Vec<_>, Vec<_>) = std::mem::take(&mut s.breaks)
            .into_iter()
            .partition(|u| u.0 == dim);
        s.breaks = rb;
        (u, b)
    });
    for (_, p) in updates {
        if let Some(state) = level.block_state(&*host, p) {
            let _ = level.update_neighbors(host, p, state, NeighbourUpdate::Neighbours);
        }
    }
    for (_, p) in breaks {
        if level
            .block(&*host, p)
            .is_some_and(|b| matches!(b.id(&*host), Some(LIGHT_DETECTOR | CRANK)))
        {
            let _ = level.destroy_block(host, p, true, None, false);
        }
    }
}

// ---- The cable anchor

/// Cells with a cable anchor on a side (CableAnchorPart.isLadder: a horizontal side), as each side last saw them
/// (the server's cell tick, the client's drawing). `is_ladder` has no host to read the cell's parts with.
static LADDERS: Mutex<Option<HashSet<Pos>>> = Mutex::new(None);

/// Whether a cell's parts make it climbable: a cable anchor on a side that isn't up or down.
pub(crate) fn anchors_climb(parts: &[Option<String>; 6]) -> bool {
    (2..6).any(|s| parts[s].as_deref() == Some("ae2:cable_anchor"))
}

pub(crate) fn note_cell(pos: Pos, parts: &[Option<String>; 6]) {
    let climb = anchors_climb(parts);
    let mut g = LADDERS.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    if climb {
        set.insert(pos);
    } else {
        set.remove(&pos);
    }
}

pub(crate) fn is_ladder(pos: Pos) -> bool {
    LADDERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|s| s.contains(&pos))
}

// ---- The light detector

/// Level.getSkyDarken from the day time and weather (0 to 11), for a level that doesn't answer it.
pub(crate) fn sky_darken(day_time: i64, rain: f32, thunder: f32) -> i32 {
    let day = day_time.rem_euclid(24000) as f64;
    // DimensionType.timeOfDay.
    let frac = (day / 24000.0 - 0.25).rem_euclid(1.0);
    let f = 0.5 - (frac * std::f64::consts::PI).cos() / 2.0;
    let angle = (frac * 2.0 + f) / 3.0;
    let mut d = 1.0 - ((angle * std::f64::consts::TAU).cos() * 2.0 + 0.5);
    d = d.clamp(0.0, 1.0);
    d = 1.0 - d;
    d *= 1.0 - f64::from(rain) * 5.0 / 16.0;
    d *= 1.0 - f64::from(thunder) * 5.0 / 16.0;
    d = 1.0 - d;
    (d * 11.0) as i32
}

/// getMaxLocalRawBrightness: the brighter of block light and the darkened sky.
pub(crate) fn raw_light(block: i32, sky: i32, darken: i32) -> i32 {
    block.max(sky - darken).clamp(0, 15)
}

/// LightDetectorBlock.getSignal: the light less 6, while it has seen any light.
pub(crate) fn light_signal(light: i32) -> i32 {
    if light > 0 { (light - 6).max(0) } else { 0 }
}

fn light_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let Some(level) = here(&*host) else { return };
    let dim = host.dimension();
    // A fixture with nothing behind it drops (canSurvive, asked as its support changes).
    let facing = facing_dir(level.block_property(&*host, pos, "facing").as_deref());
    let behind = add(pos, [-facing[0], -facing[1], -facing[2]]);
    let held = level
        .block_face_sturdy(&*host, behind, face_of(facing), SupportType::Full)
        .unwrap_or(true);
    if !held {
        with_state(|s| s.breaks.push((dim, pos)));
        return;
    }
    let n = nbt_i64(&fields.nbt, "check") + 1;
    let first = !with_state(|s| s.signals.contains_key(&(dim, pos)));
    if n <= 30 && !first {
        set_nbt_i64(&mut fields.nbt, "check", n);
        return;
    }
    set_nbt_i64(&mut fields.nbt, "check", 0);
    let block = level
        .brightness(&*host, LightLayer::Block, pos)
        .unwrap_or(0);
    let sky = level.brightness(&*host, LightLayer::Sky, pos).unwrap_or(0);
    let darken = level.get_sky_darken(&*host).unwrap_or_else(|| {
        let (rain, thunder) = level.weather(&*host).unwrap_or((0.0, 0.0));
        sky_darken(level.day_time(&*host).unwrap_or(6000), rain, thunder)
    });
    let signal = light_signal(raw_light(block, sky, darken));
    let was = with_state(|s| s.signals.insert((dim, pos), signal));
    if was != Some(signal) {
        with_state(|s| s.updates.push((dim, pos)));
    }
}

/// The face of the block behind a fixture that holds it (the one toward the fixture).
fn face_of(d: Pos) -> BlockFace {
    match d {
        [0, -1, 0] => BlockFace::Down,
        [0, 1, 0] => BlockFace::Up,
        [0, 0, -1] => BlockFace::North,
        [0, 0, 1] => BlockFace::South,
        [-1, 0, 0] => BlockFace::West,
        _ => BlockFace::East,
    }
}

/// `block_signal` for a light detector: its signal on every side.
pub(crate) fn signal(dim: &'static str, pos: Pos) -> Option<i32> {
    with_state(|s| s.signals.get(&(dim, pos)).copied())
}

// ---- The crank

/// A turn takes this many ticks (CrankBlockEntity.ticksPerRotation) and gives this much AE.
pub(crate) const TURN_TICKS: i64 = 18;
pub(crate) const AE_PER_TURN: f64 = 160.0;
/// The crankables' buffers (the charger's and inscriber's internal power, the accelerator's MAX_STORED_POWER).
pub(crate) const CRANK_MAX: f64 = 1600.0;
/// The growth accelerator's power from a crank, in its own nbt.
pub(crate) const ACCEL_POWER: &str = "crankPower";

/// ICrankable.get: whether `block` (facing `front`) takes a crank on its side `d` (from it toward the crank). The
/// charger and inscriber take one anywhere but their front, the growth accelerator on its front and back only.
pub(crate) fn takes_crank(block: &str, front: Option<Pos>, d: Pos) -> bool {
    match block {
        "ae2:charger" | "ae2:inscriber" => front.is_none_or(|f| f != d),
        "ae2:growth_accelerator" => front.is_none_or(|f| d == f || d == [-f[0], -f[1], -f[2]]),
        _ => false,
    }
}

/// What a crank at `pos` facing `facing` turns: the block behind it, if it takes a crank there.
fn target(host: &dyn Host, level: LevelRef, pos: Pos) -> Option<(Pos, String)> {
    let facing = facing_dir(level.block_property(host, pos, "facing").as_deref());
    let t = add(pos, [-facing[0], -facing[1], -facing[2]]);
    let block = crate::net::block_id(host, level, t);
    let front = level
        .block_property(host, t, "facing")
        .map(|f| facing_dir(Some(f.as_str())));
    takes_crank(&block, front, facing).then_some((t, block))
}

fn stored(nbt: &Compound, block: &str) -> f64 {
    if block == "ae2:growth_accelerator" {
        nbt_i64(nbt, ACCEL_POWER) as f64
    } else {
        crate::net::internal_power(nbt)
    }
}

/// ICrankable.canTurn: the machine's buffer has room.
fn can_turn(host: &dyn Host, level: LevelRef, t: Pos, block: &str) -> bool {
    host.block_entity(level, t)
        .and_then(|b| b.data(host))
        .is_some_and(|nbt| stored(&nbt, block) < CRANK_MAX)
}

/// CrankBlock.onActivated → CrankBlockEntity.power. Some when the crank took the click.
pub(crate) fn crank_use(
    host: &mut dyn ServerHost,
    click: &BlockClick<'_>,
) -> Option<InteractionResult> {
    let level = here(&*host)?;
    let pos = click.pos;
    if crate::net::block_id(&*host, level, pos) != CRANK {
        return None;
    }
    let be = host.block_entity(level, pos)?;
    let mut nbt = be.data(&*host).unwrap_or_default();
    if nbt_i64(&nbt, "rotation") < 3
        && let Some((t, block)) = target(&*host, level, pos)
    {
        if can_turn(&*host, level, t, &block) {
            set_nbt_i64(&mut nbt, "hits", 0);
            let (rotation, turns) = (nbt_i64(&nbt, "rotation"), nbt_i64(&nbt, "turns"));
            set_nbt_i64(&mut nbt, "rotation", rotation + TURN_TICKS);
            set_nbt_i64(&mut nbt, "turns", turns + 1);
        } else {
            let hits = nbt_i64(&nbt, "hits") + 1;
            set_nbt_i64(&mut nbt, "hits", hits);
            if hits > 10 {
                let _ = level.destroy_block(host, pos, true, None, false);
                return Some(InteractionResult::SuccessServer);
            }
        }
        let _ = be.replace_data(host, nbt);
    }
    Some(InteractionResult::SuccessServer)
}

/// CrankBlockEntity.tick: a turn pays at its end; with nothing to turn (CrankBlock.neighborChanged) it drops.
fn crank_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let Some(level) = here(&*host) else { return };
    let dim = host.dimension();
    let Some((t, block)) = target(&*host, level, pos) else {
        with_state(|s| s.breaks.push((dim, pos)));
        return;
    };
    let rotation = nbt_i64(&fields.nbt, "rotation");
    if rotation <= 0 {
        return;
    }
    set_nbt_i64(&mut fields.nbt, "rotation", rotation - 1);
    let charge = nbt_i64(&fields.nbt, "charge") + 1;
    if charge < TURN_TICKS {
        set_nbt_i64(&mut fields.nbt, "charge", charge);
        return;
    }
    set_nbt_i64(&mut fields.nbt, "charge", charge - TURN_TICKS);
    // ICrankable.applyTurn: 160 AE into the machine's buffer (injectExternalPower), as far as it has room.
    let Some(be) = host.block_entity(level, t) else {
        return;
    };
    let Some(mut nbt) = be.data(&*host) else {
        return;
    };
    let now = (stored(&nbt, &block) + AE_PER_TURN).min(CRANK_MAX);
    if block == "ae2:growth_accelerator" {
        set_nbt_i64(&mut nbt, ACCEL_POWER, now as i64);
    } else {
        crate::net::set_internal_power(&mut nbt, now);
    }
    let _ = be.replace_data(host, nbt);
}

/// The crank's placement (OrientationStrategies.facing, CrankBlock.canSurvive): facing out of the clicked face, on a
/// machine that takes a crank there; elsewhere it isn't placed.
pub(crate) fn placement(
    level: &dyn LevelHost,
    event: &StateForPlacementEvent<'_>,
) -> Option<Option<BlockState>> {
    let block = level.state_block(event.state)?;
    // fixE: the light detector (now on a beacon template, for its sky light) faces out of the clicked face, as the
    // shulker box template placed it (LightDetectorBlock.getStateForPlacement; light_tick drops it unsupported).
    if block == LIGHT_DETECTOR {
        return Some(level.state_with_property(event.state, "facing", SIDE_NAMES[light_detector_side(event.context.face)]));
    }
    if block != CRANK {
        return None;
    }
    let cx = event.context;
    let facing = dir_pos(cx.face);
    let p = [cx.pos.x, cx.pos.y, cx.pos.z];
    let t = add(p, [-facing[0], -facing[1], -facing[2]]);
    let ts = level.block_state(t)?;
    let block = level.state_block(ts)?.to_string();
    let front = level
        .state_string(ts)
        .and_then(|s| state_prop(&s, "facing"))
        .map(|f| facing_dir(Some(f.as_str())));
    if !takes_crank(&block, front, facing) {
        return Some(None);
    }
    Some(level.state_with_property(
        event.state,
        "facing",
        SIDE_NAMES[SIDES.iter().position(|s| *s == facing).unwrap_or(1)],
    ))
}

/// fixE: the light detector's facing for a click on `face` (`SIDES` index; up if unknown).
fn light_detector_side(face: Direction) -> usize {
    let facing = dir_pos(face);
    SIDES.iter().position(|s| *s == facing).unwrap_or(1)
}

/// A property's value in a state's string (`ae2:charger[facing=north,...]`).
pub(crate) fn state_prop(state: &str, key: &str) -> Option<String> {
    let inner = state.split_once('[')?.1.trim_end_matches(']');
    inner.split(',').find_map(|kv| {
        kv.split_once('=')
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.to_owned())
    })
}

fn dir_pos(d: Direction) -> Pos {
    match d {
        Direction::Down => [0, -1, 0],
        Direction::Up => [0, 1, 0],
        Direction::North => [0, 0, -1],
        Direction::South => [0, 0, 1],
        Direction::West => [-1, 0, 0],
        _ => [1, 0, 0],
    }
}

/// For clients: the crank's turns and the ticks left of the current one.
pub(crate) fn update_tag(nbt: &Compound) -> Compound {
    let mut out = Compound::new();
    for k in ["turns", "rotation"] {
        out.insert(java(k), Tag::Long(nbt_i64(nbt, k)));
    }
    out
}

/// CrankRenderer: the handle (`ae2:hz_crank_handle`) turned to the crank's facing, then a full turn about it for each
/// turn (20 degrees a tick).
pub(crate) fn crank_submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let Some(level) = host.level(host.dimension()) else {
        return;
    };
    let Some(key) = host
        .block_state_id("ae2:hz_crank_handle")
        .map(|s| s.block())
    else {
        return;
    };
    let turns = nbt_i64(&event.entity.nbt, "turns");
    let left = nbt_i64(&event.entity.nbt, "rotation");
    let draw = &mut *event.draw;
    let moving = if left > 0 { draw.partial() } else { 0.0 };
    let angle = -(360.0 / TURN_TICKS as f32) * ((turns * TURN_TICKS - left) as f32 + moving);
    let facing = level
        .block_property(host, event.pos, "facing")
        .unwrap_or_else(|| "north".to_owned());
    let c = [0.5, 0.5, 0.5];
    draw.push();
    match facing.as_str() {
        "south" => draw.rotate_about([0.0, 1.0, 0.0], 180.0, c),
        "east" => draw.rotate_about([0.0, 1.0, 0.0], -90.0, c),
        "west" => draw.rotate_about([0.0, 1.0, 0.0], 90.0, c),
        "up" => draw.rotate_about([1.0, 0.0, 0.0], 90.0, c),
        "down" => draw.rotate_about([1.0, 0.0, 0.0], -90.0, c),
        _ => {}
    }
    draw.rotate_about([0.0, 0.0, 1.0], angle.rem_euclid(360.0), c);
    draw.block(key, &[]);
    draw.pop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_levels() {
        // Noon is not darkened, midnight by 11 (clear weather); rain darkens the day.
        assert_eq!(sky_darken(6000, 0.0, 0.0), 0);
        assert_eq!(sky_darken(18000, 0.0, 0.0), 11);
        assert!(sky_darken(6000, 1.0, 0.0) > 0);
        assert_eq!(raw_light(0, 15, 0), 15);
        assert_eq!(raw_light(7, 15, 11), 7);
        assert_eq!(raw_light(0, 3, 11), 0);
        assert_eq!(light_signal(15), 9);
        assert_eq!(light_signal(6), 0);
        assert_eq!(light_signal(0), 0);
    }

    #[test]
    fn crankables() {
        let north = Some([0, 0, -1]);
        // A charger takes it on top, not on its front.
        assert!(takes_crank("ae2:charger", north, [0, 1, 0]));
        assert!(!takes_crank("ae2:charger", north, [0, 0, -1]));
        assert!(takes_crank("ae2:inscriber", north, [0, 0, 1]));
        // The accelerator: front and back only.
        assert!(takes_crank(
            "ae2:growth_accelerator",
            Some([0, 1, 0]),
            [0, 1, 0]
        ));
        assert!(takes_crank(
            "ae2:growth_accelerator",
            Some([0, 1, 0]),
            [0, -1, 0]
        ));
        assert!(!takes_crank(
            "ae2:growth_accelerator",
            Some([0, 1, 0]),
            [1, 0, 0]
        ));
        assert!(!takes_crank("minecraft:stone", None, [0, 1, 0]));
    }

    #[test]
    fn anchors() {
        let mut parts: [Option<String>; 6] = Default::default();
        assert!(!anchors_climb(&parts));
        parts[1] = Some("ae2:cable_anchor".into());
        assert!(!anchors_climb(&parts), "on top: not a ladder");
        parts[4] = Some("ae2:cable_anchor".into());
        assert!(anchors_climb(&parts));
    }
}
