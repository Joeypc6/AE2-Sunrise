//! AE2's matter cannon (MatterCannonItem, MatterCannonDispenseItemBehavior): a one-type storage cell of ammo (filled in
//! a drive or an ME chest, `tool_cells`). A use fires: 1600 AE a shot, a Speed Card adding a shot each (all from the one
//! item it fires), the shots no more than the ammo held nor the charge pays for. Its penetration is the ammo's weight
//! times the shots: a mob takes `ceil(penetration / 20)` (which comes off the penetration), a block breaks when the
//! penetration beats nine times its hardness (`matterCannonBlockDamage`), and the shot goes on through what it killed or
//! broke at 60% of what was left. A paint ball dyes a sheep it hits or leaves a splotch on the block (`paint`). A
//! dispenser fires a cannon too, 32 blocks out of its face. Upstream's `fire_cannon`, `cannon_shoot`, `cannon_mob`,
//! `cannon_block` and `cannon_paint` (`../ae2-horizon/mixins/ae2/me.rs`), on Sunrise's level rays.

use crate::cells::*;
use crate::core::Actionable;
use crate::tool_cells::CANNON;
use crate::util::*;
use crate::world::item_power;
use mod_api::*;
use std::sync::OnceLock;

/// MatterCannonItem.ENERGY_PER_SHOT.
pub(crate) const ENERGY_PER_SHOT: f64 = 1600.0;
/// InteractionUtil.getPlayerRay(p, 255).
const REACH: f64 = 255.0;
/// AEDamageTypes.MATTER_CANNON (data/ae2/damage_type/matter_cannon.json).
const DAMAGE_TYPE: &str = "ae2:matter_cannon";

pub(crate) static CRIT: OnceLock<ParticleKey> = OnceLock::new();

/// MatterCannonItem.getDamageFromPenetration.
pub(crate) fn damage_from_penetration(penetration: f32) -> i32 {
    (penetration / 20.0).ceil() as i32
}

/// What a shot takes: the shots (one, a Speed Card more each, no more than the ammo held nor the charge pays for).
/// None when the charge is short of one shot.
pub(crate) fn shots(speed_cards: usize, ammo: i64, power: f64) -> Option<i64> {
    if power < ENERGY_PER_SHOT {
        return None;
    }
    let s = (1 + speed_cards as i64).min(ammo);
    Some(s.min((power / ENERGY_PER_SHOT) as i64))
}

/// A block a shot meets: what is left of the penetration once it broke it, or None when it holds (hardness × 9 not
/// beaten, or unbreakable).
pub(crate) fn through_block(penetration: f32, hardness: f32) -> Option<f32> {
    let h = hardness * 9.0;
    (h >= 0.0 && penetration > h).then(|| (penetration - h) * 0.6)
}

/// How a use went.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fired {
    /// No ammo: AE2's "Ammo Depleted." (the use is still taken).
    NoAmmo,
    /// Short of 1600 AE: the use fails.
    NoPower,
    Shot,
}

/// MatterCannonItem.fireCannon on the server: the cannon `stack` is changed (charge and ammo) and the shot made from
/// `from` toward `to`. `shooter` is the player's entity (none from a dispenser).
pub(crate) fn fire(
    host: &mut dyn ServerHost,
    level: LevelRef,
    stack: &mut ItemStack,
    shooter: Option<EntityRef>,
    player: Option<u128>,
    from: [f64; 3],
    to: [f64; 3],
) -> Fired {
    crate::cells::ids(&*host);
    let Some(ty) = cell_type(CANNON) else {
        return Fired::NoPower;
    };
    let contents = read_cell(stack);
    let Some(i) = contents.amounts.iter().position(|n| *n > 0) else {
        return Fired::NoAmmo;
    };
    let key = contents.keys[i].clone();
    let speed = cell_cards(stack)
        .iter()
        .filter(|c| c.as_str() == "ae2:speed_card")
        .count();
    let Some(n) = shots(speed, contents.amounts[i], item_power(stack)) else {
        return Fired::NoPower;
    };
    crate::powered::use_power(stack, ENERGY_PER_SHOT * n as f64);
    let mut inv = cell_inventory(stack, ty);
    let got = inv.extract(&key, 1, Actionable::Modulate);
    if let Some(c) = inv.persist() {
        write_cell(stack, &c);
    }
    if got == 0 {
        return Fired::Shot;
    }
    let ammo = key.id().to_owned();
    let penetration = crate::tool_cells::ammo_weight(&ammo) * n as f32;
    debug(&format!(
        "cannon: shot of {n} {ammo} penetration {penetration} from {from:?} dir {:?} shooter {:?}",
        unit(from, to),
        shooter.map(|e| e.id())
    ));
    if penetration <= 0.0 {
        if crate::paint::paint_ball(&ammo).is_some() {
            paint_shot(host, level, shooter, player, from, to, &ammo);
        }
        return Fired::Shot;
    }
    standard_shot(host, level, shooter, player, from, to, penetration);
    Fired::Shot
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The unit direction from `from` to `to` (zero for a point).
fn unit(from: [f64; 3], to: [f64; 3]) -> [f64; 3] {
    let d = dist(from, to);
    if d < 1e-9 {
        return [0.0; 3];
    }
    [0, 1, 2].map(|i| (to[i] - from[i]) / d)
}

/// What the ray takes, given the nearest entity's and block's distances (pure: unit tested): the entity when it is
/// nearer than the block (or there is no block), else the block.
#[derive(Debug, PartialEq, Eq)]
enum First {
    Entity,
    Block,
    Miss,
}

fn first_hit(entity: Option<f64>, block: Option<f64>) -> First {
    match (entity, block) {
        (Some(e), Some(b)) if b > e => First::Entity,
        (Some(_), None) => First::Entity,
        (_, Some(_)) => First::Block,
        (None, None) => First::Miss,
    }
}

/// Whether a candidate the ray met can be shot (pure: unit tested): not an item, and living ones only while alive.
/// Sunrise answers no `alive` for some kinds: those count as alive (the hurt decides).
fn shootable(is_item: bool, living: bool, alive: Option<bool>) -> bool {
    !is_item && (!living || alive.unwrap_or(true))
}

/// After a hurt of the matter cannon's type answered `first`, whether to hurt again as a player attack (pure: unit
/// tested): whenever the hurt was refused, since a refused hurt dealt nothing, so the retry can't hit twice. In game
/// (Nightly 2d81263) a registered `ae2:matter_cannon` type is refused on a fresh mob; a real invulnerability or cooldown
/// refuses the player attack too. Never after a hit that landed, nor for a stale handle. `type_registered` is kept for
/// the debug line.
fn retry_as_player_attack(first: Option<bool>, type_registered: Option<bool>) -> bool {
    let _ = type_registered;
    first == Some(false)
}

/// What the ray meets first: the nearest live entity (not an item, not the shooter; its box grown by 0.3) unless a
/// block with a collision shape is nearer (ClipContext.Block.COLLIDER, fluids passed through).
enum Hit {
    Entity(EntityRef, [f64; 3], f64),
    Block(ClipHit, f64),
    Miss,
}

fn trace(
    host: &dyn Host,
    level: LevelRef,
    from: [f64; 3],
    to: [f64; 3],
    shooter: Option<EntityRef>,
) -> Hit {
    let all = level.ray_trace_entities(host, from, to, 0.3, shooter);
    if all.is_none() {
        debug("cannon: ray_trace_entities answered None");
    }
    let entity = all
        .unwrap_or_default()
        .into_iter()
        .filter(|h| {
            let kind = h.entity.kind(host).map(|k| k.id(host).to_owned());
            let is_item = h.entity.kind(host).is_some_and(is_item_entity);
            let living = host.living(h.entity);
            let alive = living.and_then(|l| l.alive(host));
            let ok = shootable(is_item, living.is_some(), alive);
            debug(&format!(
                "cannon: candidate {} {kind:?} at {:?} distance {:.2} living {} alive {alive:?} -> {ok}",
                h.entity.id(),
                h.location,
                h.distance,
                living.is_some()
            ));
            ok
        })
        .min_by(|a, b| a.distance.total_cmp(&b.distance));
    let block = level
        .clip(host, from, to, ClipBlock::Collider, ClipFluid::None)
        .filter(|c| !c.miss)
        .map(|c| (c, dist(from, c.location)));
    debug(&format!(
        "cannon: nearest entity {:?} block {:?}",
        entity.as_ref().map(|e| (e.entity.id(), e.distance)),
        block.as_ref().map(|(c, d)| (c.block, *d))
    ));
    match (
        first_hit(
            entity.as_ref().map(|e| e.distance),
            block.as_ref().map(|b| b.1),
        ),
        entity,
        block,
    ) {
        (First::Entity, Some(e), _) => Hit::Entity(e.entity, e.location, e.distance),
        (First::Block, _, Some((c, bd))) => Hit::Block(c, bd),
        _ => Hit::Miss,
    }
}

/// MatterCannonPacket's effect along the shot (32 blocks when it hit nothing): a trail of particles to what it hit
/// (AEConfig's `enableEffects`).
fn trail(host: &mut dyn ServerHost, level: LevelRef, from: [f64; 3], to: [f64; 3], hit: &Hit) {
    if !crate::config::config().enable_effects {
        return;
    }
    let Some(key) = CRIT.get() else { return };
    let len = match hit {
        Hit::Entity(_, _, d) | Hit::Block(_, d) => d + 1.0,
        Hit::Miss => 32.0,
    }
    .min(64.0);
    let d = dist(from, to).max(1e-9);
    let dir = [
        (to[0] - from[0]) / d,
        (to[1] - from[1]) / d,
        (to[2] - from[2]) / d,
    ];
    let options = ParticleOptions::simple(*key);
    let mut t = 1.0;
    while t < len {
        let at = [
            from[0] + dir[0] * t,
            from[1] + dir[1] * t,
            from[2] + dir[2] * t,
        ];
        let _ = level.send_particles(host, &options, None, at, [0.0; 3], 0.0, 1);
        t += 1.0;
    }
}

/// Platform.hasPermissions: the player may change the world there (no player: a dispenser, which may).
fn may_change(
    host: &dyn Host,
    player: Option<u128>,
    pos: Pos,
    face: BlockFace,
    stack: Option<&ItemStack>,
) -> bool {
    let Some(p) = player.and_then(|p| host.player(p)) else {
        return true;
    };
    if !p.may_build(host).unwrap_or(false) {
        return false;
    }
    match stack {
        Some(s) => p.may_use_item_at(host, pos, face, s).unwrap_or(false),
        None => true,
    }
}

fn face_index(f: BlockFace) -> usize {
    match f {
        BlockFace::Down => 0,
        BlockFace::Up => 1,
        BlockFace::North => 2,
        BlockFace::South => 3,
        BlockFace::West => 4,
        _ => 5,
    }
}

/// `standardAmmo`: on through what it killed or broke while penetration is left.
fn standard_shot(
    host: &mut dyn ServerHost,
    level: LevelRef,
    shooter: Option<EntityRef>,
    player: Option<u128>,
    from: [f64; 3],
    to: [f64; 3],
    mut penetration: f32,
) {
    let mut destroyed = true;
    // A bound on the passes (each kills or breaks one thing).
    let mut passes = 0;
    while penetration > 0.0 && destroyed && passes < 256 {
        passes += 1;
        destroyed = false;
        let hit = trace(&*host, level, from, to, shooter);
        trail(host, level, from, to, &hit);
        match hit {
            Hit::Entity(e, _, _) => {
                let dmg = damage_from_penetration(penetration);
                if let Some(l) = host.living(e) {
                    penetration -= dmg as f32;
                    hurt(host, e, dmg as f32, shooter);
                    // Dead (or gone): the shot goes on through it.
                    destroyed = !l
                        .alive(&*host)
                        .unwrap_or_else(|| host.entity(e.id()).is_some());
                } else if hurt(host, e, dmg as f32, shooter) {
                    destroyed = host.entity(e.id()).is_none();
                }
                debug(&format!(
                    "cannon: entity {} hit for {dmg}, penetration left {penetration}, killed {destroyed}",
                    e.id()
                ));
            }
            Hit::Block(c, _) => {
                if !crate::config::config().matter_cannon_block_damage {
                    penetration = 0.0;
                    continue;
                }
                let pos = c.block;
                let hardness = level
                    .block_state(&*host, pos)
                    .and_then(|st| host.block_states().destroy_speed(st))
                    .unwrap_or(-1.0);
                if let Some(left) = through_block(penetration, hardness)
                    && may_change(&*host, player, pos, c.face, None)
                {
                    // Level.destroyBlock(pos, true): its drops, then air.
                    if matches!(level.destroy_block(host, pos, true, None, false), Ok(_)) {
                        destroyed = true;
                        penetration = left;
                    }
                }
            }
            Hit::Miss => {}
        }
    }
}

/// The matter cannon's damage (AEDamageTypes.MATTER_CANNON, by the shooter); a player attack if the type is missing.
fn hurt(host: &mut dyn ServerHost, e: EntityRef, amount: f32, shooter: Option<EntityRef>) -> bool {
    let first = e.hurt(host, DAMAGE_TYPE, amount, shooter, shooter);
    // Sunrise answers false for a damage type its registry lacks: then the mod's type wasn't loaded from its data.
    let registered = (first == Some(false)).then(|| {
        host.registry_entry("minecraft:damage_type", DAMAGE_TYPE)
            .is_some()
    });
    let mut done = first == Some(true);
    if retry_as_player_attack(first, registered) {
        let again = e.hurt(host, "minecraft:player_attack", amount, shooter, shooter);
        debug(&format!(
            "cannon: {DAMAGE_TYPE} refused (registered {registered:?}), as player_attack -> {again:?}"
        ));
        done = again == Some(true);
    }
    debug(&format!(
        "cannon: hurt {} by {amount} ({DAMAGE_TYPE} -> {first:?}) dealt {done}",
        e.id()
    ));
    done
}

/// `shootPaintBalls`: a sheep hit takes the ball's colour (and every entity hit a hit of 0), a block hit gets a splotch.
fn paint_shot(
    host: &mut dyn ServerHost,
    level: LevelRef,
    shooter: Option<EntityRef>,
    player: Option<u128>,
    from: [f64; 3],
    to: [f64; 3],
    ball: &str,
) {
    let hit = trace(&*host, level, from, to, shooter);
    trail(host, level, from, to, &hit);
    let Some((color, _)) = crate::paint::paint_ball(ball) else {
        return;
    };
    match hit {
        Hit::Entity(e, _, _) => {
            if e.kind(&*host)
                .is_some_and(|k| k.id(&*host).as_deref() == Some("minecraft:sheep"))
            {
                // Sheep.setColor: its `Color` byte is the dye's id, which AEColor's order is.
                let mut nbt = Compound::new();
                nbt.insert(java("Color"), Tag::Byte(color as i8));
                let _ = e.merge_nbt(host, nbt);
            }
            let _ = e.hurt(host, "minecraft:player_attack", 0.0, shooter, shooter);
        }
        Hit::Block(c, _) => {
            let face = face_index(c.face);
            let at = add(c.block, SIDES[face]);
            if !may_change(&*host, player, at, c.face, None) {
                return;
            }
            crate::paint::add_blot(host, level, c.block, face, c.location, ball);
        }
        Hit::Miss => {}
    }
}

// ---- The item

/// The matter cannon's Part: its use (in the air, or on a block that lets the click through) fires; a dispenser fires it.
pub(crate) struct Cannon;

#[mod_api::sunrise_part]
impl ItemPart for Cannon {
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        let host = match host {
            HostSide::Client(h) => {
                // The client's guess (fireCannon on the client): taken with ammo and the charge for a shot, or with none.
                crate::cells::ids(&*h);
                let ammo = read_cell(event.stack).amounts.iter().any(|n| *n > 0);
                return Some(if !ammo || item_power(event.stack) >= ENERGY_PER_SHOT {
                    InteractionResult::Success
                } else {
                    InteractionResult::Fail
                });
            }
            HostSide::Server(h) => h,
        };
        let player = event.player;
        let p = host.player(player)?;
        let e = p.entity();
        let eye = e.eye_position(&*host)?;
        let look = e.view_vector(&*host)?;
        let to = [
            eye[0] + look[0] * REACH,
            eye[1] + look[1] * REACH,
            eye[2] + look[2] * REACH,
        ];
        let level = here(&*host)?;
        let old = event.stack.clone();
        let mut stack = old.clone();
        match fire(host, level, &mut stack, Some(e), Some(player), eye, to) {
            Fired::NoAmmo => {
                let _ = p.action_bar(host, Text::translate("chat.ae2.AmmoDepleted", Vec::new()));
                Some(InteractionResult::Success)
            }
            Fired::NoPower => Some(InteractionResult::Fail),
            Fired::Shot => {
                crate::handtools::replace_hand(host, player, event.hand, &old, stack);
                Some(InteractionResult::Success)
            }
        }
    }

    /// MatterCannonDispenseItemBehavior: the dispenser fires the cannon 32 blocks out of its face.
    fn dispense_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut DispenseItemEvent<'_>,
    ) -> Option<Dispensed> {
        let level = here(&*host)?;
        let d = match event.facing {
            Direction::Down => [0.0, -1.0, 0.0],
            Direction::Up => [0.0, 1.0, 0.0],
            Direction::North => [0.0, 0.0, -1.0],
            Direction::South => [0.0, 0.0, 1.0],
            Direction::West => [-1.0, 0.0, 0.0],
            _ => [1.0, 0.0, 0.0],
        };
        // DispenserBlock.getDispensePosition: 0.7 out of the block's centre.
        let p = event.pos;
        let from = [
            f64::from(p[0]) + 0.5 + 0.7 * d[0],
            f64::from(p[1]) + 0.5 + 0.7 * d[1],
            f64::from(p[2]) + 0.5 + 0.7 * d[2],
        ];
        let to = [
            from[0] + 32.0 * d[0],
            from[1] + 32.0 * d[1],
            from[2] + 32.0 * d[2],
        ];
        let mut stack = event.stack.clone();
        fire(host, level, &mut stack, None, None, from, to);
        Some((Some(stack), 1000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_follows_penetration() {
        assert_eq!(damage_from_penetration(32.0), 2);
        assert_eq!(damage_from_penetration(20.0), 1);
        assert_eq!(damage_from_penetration(196.96655 * 5.0), 50);
        assert_eq!(damage_from_penetration(0.5), 1);
    }

    #[test]
    fn the_nearer_hit_wins() {
        assert_eq!(first_hit(Some(3.0), Some(40.0)), First::Entity);
        assert_eq!(first_hit(Some(3.0), None), First::Entity);
        assert_eq!(first_hit(Some(5.0), Some(2.0)), First::Block);
        // A tie goes to the block (AE2: the entity only when strictly nearer).
        assert_eq!(first_hit(Some(2.0), Some(2.0)), First::Block);
        assert_eq!(first_hit(None, Some(2.0)), First::Block);
        assert_eq!(first_hit(None, None), First::Miss);
    }

    #[test]
    fn what_can_be_shot() {
        // A pig: living, alive or no answer.
        assert!(shootable(false, true, Some(true)));
        assert!(shootable(false, true, None));
        assert!(!shootable(false, true, Some(false)));
        // Items never; other entities (a boat, an item frame) always.
        assert!(!shootable(true, false, None));
        assert!(shootable(false, false, None));
    }

    #[test]
    fn player_attack_after_any_refusal() {
        assert!(!retry_as_player_attack(Some(true), None));
        assert!(retry_as_player_attack(Some(false), Some(false)));
        assert!(retry_as_player_attack(Some(false), None));
        assert!(retry_as_player_attack(Some(false), Some(true)));
        assert!(!retry_as_player_attack(None, None));
    }

    #[test]
    fn shot_direction() {
        let d = unit([0.0, 64.0, 0.0], [0.0, 64.0, 255.0]);
        assert_eq!(d, [0.0, 0.0, 1.0]);
        let d = unit([1.0, 1.0, 1.0], [4.0, 5.0, 1.0]);
        assert!((d[0] - 0.6).abs() < 1e-12 && (d[1] - 0.8).abs() < 1e-12);
        assert_eq!(unit([1.0; 3], [1.0; 3]), [0.0; 3]);
    }

    #[test]
    fn shots_need_power_and_ammo() {
        assert_eq!(shots(0, 10, 1599.0), None);
        assert_eq!(shots(0, 10, 1600.0), Some(1));
        assert_eq!(shots(4, 10, 100_000.0), Some(5));
        assert_eq!(shots(4, 2, 100_000.0), Some(2));
        assert_eq!(shots(4, 10, 4000.0), Some(2));
    }

    #[test]
    fn penetration_through_blocks() {
        // Stone (1.5): 13.5 to beat; a matter ball (32) goes through with (32 - 13.5) * 0.6 left.
        let left = through_block(32.0, 1.5).expect("broken");
        assert!((left - 11.1).abs() < 1e-4);
        assert_eq!(through_block(10.0, 1.5), None);
        // Obsidian (50) holds against one gold nugget shot; bedrock (-1) always holds.
        assert_eq!(through_block(196.97, 50.0), None);
        assert_eq!(through_block(10_000.0, -1.0), None);
        // Instant blocks (0) always go.
        assert_eq!(through_block(1.0, 0.0), Some(0.6));
    }
}
