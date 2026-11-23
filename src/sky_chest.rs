//! AE2's sky stone blocks with contents: the sky stone chest and smooth sky stone chest (SkyChestBlockEntity: 36
//! slots, AE2's screen, the lid that opens while anyone has it open, with a chest's sounds) and the sky stone tank
//! (SkyStoneTankBlockEntity: 16 buckets of one fluid, buckets fill and empty it by a use, its fluid drawn inside).
//!
//! Upstream's `sky_chest_submit`, `container_viewers`, `tank_bucket` and `tank_submit` (`ae2-horizon/mixins/ae2/me.rs`)
//! on Sunrise's API: the chest's slots are a `ModContainer` (hoppers and pipes reach them as a chest's; they drop when
//! it breaks), the tank's fluid is a `ModFluidTanks` tank (pipes fill and drain it), and a bucket's use goes through
//! `fluid_util::interact_with_fluid_handler`, which moves exactly one bucket or nothing.

use crate::screens::{spr, tr};
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) const CHEST_MENU: &str = "ae2:sky_chest";
pub(crate) const CHEST_SLOTS: usize = 36;
pub(crate) const TANK: &str = "ae2:sky_stone_tank";
/// SkyStoneTankBlockEntity.TANK_CAPACITY: 16 buckets.
pub(crate) const TANK_MB: i32 = 16_000;
const TEXT: u32 = 0xFF40_4040;

pub(crate) fn is_sky_chest(block: &str) -> bool {
    matches!(block, "ae2:sky_stone_chest" | "ae2:smooth_sky_stone_chest")
}

/// Blocks of this file with a block entity that ticks (the chests, for their lid).
pub(crate) fn ticks(block: &str) -> bool {
    is_sky_chest(block)
}

pub(crate) fn has_screen(block: &str) -> bool {
    is_sky_chest(block)
}

pub(crate) fn container(block: &str) -> Option<ModContainer> {
    is_sky_chest(block).then(|| ModContainer {
        slots: CHEST_SLOTS,
        faces: std::array::from_fn(|_| (0..CHEST_SLOTS).collect()),
        inputs: (0..CHEST_SLOTS).collect(),
        outputs: (0..CHEST_SLOTS).collect(),
        insert_rules: Vec::new(),
        menu: Some(CHEST_MENU.to_owned()),
    })
}

pub(crate) fn fluids(block: &str) -> Option<ModFluidTanks> {
    (block == TANK).then(|| {
        let mut t = ModFluidTanks::default();
        t.tanks = vec![TANK_MB];
        t.faces = std::array::from_fn(|_| vec![0]);
        t.inputs = vec![0];
        t.outputs = vec![0];
        t
    })
}

// ---- Who has a chest open (the server's ChestLidController counts)

/// Players with a sky chest open: where, and the menu's window once learnt (None until the menu answers), and the
/// tick it opened.
#[derive(Default)]
struct Openers {
    by_player: HashMap<u128, ((&'static str, Pos), Option<i32>, u64)>,
}

static OPENERS: Mutex<Option<Openers>> = Mutex::new(None);

fn with_openers<R>(f: impl FnOnce(&mut Openers) -> R) -> R {
    let mut g = OPENERS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Openers::default))
}

pub(crate) fn forget_all() {
    *OPENERS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *LIDS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// A player opened a sky chest's menu (`devices::use_block`).
pub(crate) fn opened(player: u128, dim: &'static str, pos: Pos, window: Option<i32>) {
    let t = crate::net::with_net(dim, |n| n.tick);
    with_openers(|o| {
        o.by_player.insert(player, ((dim, pos), window, t));
    });
}

/// How many still have the chest at `pos` open: a record whose player left, walked away, or whose menu is no longer
/// the one opened (closed, or another opened) goes.
fn viewers(host: &dyn Host, dim: &'static str, pos: Pos) -> usize {
    let here_now: Vec<(u128, Option<i32>, u64)> =
        with_openers(|o| o.by_player.iter().filter(|(_, (at, _, _))| *at == (dim, pos)).map(|(p, (_, w, t))| (*p, *w, *t)).collect());
    let now = crate::net::with_net(dim, |n| n.tick);
    let mut count = 0;
    for (player, window, opened) in here_now {
        let ok = (|| {
            let p = host.player(player)?;
            if p.entity().level(host) != here(host) {
                return None;
            }
            let at = p.entity().position(host)?;
            let d2: f64 = (0..3).map(|i| (at[i] - f64::from(pos[i]) - 0.5).powi(2)).sum();
            if d2 > 64.0 {
                return None;
            }
            let menu = p.container_menu(host).and_then(|m| m.container_id(host));
            match (window, menu) {
                (Some(w), Some(m)) if w == m => Some(()),
                (Some(_), _) => None,
                (None, Some(m)) if m != 0 => {
                    with_openers(|o| {
                        if let Some(r) = o.by_player.get_mut(&player) {
                            r.1 = Some(m);
                        }
                    });
                    Some(())
                }
                // The menu opens at the tick's end: a while to learn its window.
                (None, _) => (now.saturating_sub(opened) <= 10).then_some(()),
            }
        })()
        .is_some();
        if ok {
            count += 1;
        } else {
            with_openers(|o| {
                if o.by_player.get(&player).is_some_and(|r| r.0 == (dim, pos)) {
                    o.by_player.remove(&player);
                }
            });
        }
    }
    count
}

/// The chest's tick: open while anyone has it open (its update tag carries `open` to clients, which swing the lid), with
/// the chest's sounds as it opens and shuts; its comparator reading.
pub(crate) fn chest_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let dim = host.dimension();
    let open = viewers(&*host, dim, pos) > 0;
    let was = nbt_i64(&fields.nbt, "open") != 0;
    if open != was {
        fields.nbt.insert(java("open"), Tag::Byte(i8::from(open)));
        if let Some(level) = here(&*host) {
            let sound = if open { "minecraft:block.chest.open" } else { "minecraft:block.chest.close" };
            let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
            let pitch = 0.9 + 0.1 * ((pos[0] ^ pos[2] ^ dim.len() as i32).rem_euclid(10) as f32 / 10.0);
            let _ = level.play_sound(host, sound, at, SoundSource::Blocks, 0.5, pitch, None);
        }
    }
    let signal = crate::comparator::of_slots(&*host, &fields.items, CHEST_SLOTS);
    crate::comparator::set(dim, pos, signal);
}

// ---- The tank's bucket

/// SkyStoneTankBlock.useItemOn: a fluid container in hand fills from the tank, or empties into it (one bucket, all or
/// nothing). None when the hand holds no fluid container that moves anything.
pub(crate) fn tank_use(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let level = here(&*host)?;
    if crate::net::block_id(&*host, level, click.pos) != TANK {
        return None;
    }
    click.stack?;
    let player = host.player(click.player)?;
    let side = match click.hit.face {
        [0, -1, 0] => Direction::Down,
        [0, 1, 0] => Direction::Up,
        [0, 0, -1] => Direction::North,
        [0, 0, 1] => Direction::South,
        [-1, 0, 0] => Direction::West,
        _ => Direction::East,
    };
    fluid_util::interact_with_fluid_handler(host, player, click.hand, level, click.pos, Some(side)).then_some(InteractionResult::SuccessServer)
}

/// The tank's fluid and amount from its saved `fluids` (the game's tank list).
pub(crate) fn tank_fluid(nbt: &Compound) -> Option<(String, i32)> {
    let Some(Tag::List(list)) = nbt.get(&java("fluids")) else { return None };
    let Some(Tag::Compound(t)) = list.first() else { return None };
    let fluid = match t.get(&java("Fluid")) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => return None,
    };
    let amount = match t.get(&java("Amount")) {
        Some(Tag::Int(n)) => *n,
        _ => 0,
    };
    (amount > 0 && !fluid.is_empty() && fluid != "minecraft:empty").then_some((fluid, amount))
}

/// What the clients are sent: the chest's `open`, the tank's `fluids`.
pub(crate) fn update_tag(block_nbt: &Compound) -> Option<Compound> {
    let mut out = Compound::new();
    for key in ["open", "fluids"] {
        if let Some(v) = block_nbt.get(&java(key)) {
            out.insert(java(key), v.clone());
        }
    }
    (!out.is_empty()).then_some(out)
}

// ---- Client: the lid and the tank's fluid

/// Each chest's lid on this client: how far open (0 to 1) and the time that was at.
static LIDS: Mutex<Option<HashMap<Pos, (f32, f64)>>> = Mutex::new(None);

/// The lid's openness moving towards `target` at a tenth a tick (ChestLidController), from `was` at `then` to `now`.
pub(crate) fn lid_step(was: f32, target: f32, then: f64, now: f64) -> f32 {
    let dt = (now - then).clamp(0.0, 20.0) as f32;
    if was < target { (was + 0.1 * dt).min(target) } else { (was - 0.1 * dt).max(target) }
}

/// SkyChestTESR: the lid (`ae2:hz_<chest>_lid`) swings open about its back edge, eased as vanilla's chest lid.
pub(crate) fn chest_submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let open = nbt_i64(&event.entity.nbt, "open") != 0;
    let now = event.draw.game_time() as f64 + f64::from(event.draw.partial());
    let lid = {
        let mut g = LIDS.lock().unwrap_or_else(|e| e.into_inner());
        let map = g.get_or_insert_with(HashMap::new);
        let (was, then) = map.get(&event.pos).copied().unwrap_or((if open { 1.0 } else { 0.0 }, now));
        let v = lid_step(was, if open { 1.0 } else { 0.0 }, then, now);
        map.insert(event.pos, (v, now));
        if map.len() > 4096 {
            map.retain(|_, (_, t)| now - *t < 200.0);
        }
        v
    };
    let eased = 1.0 - (1.0 - lid).powi(3);
    let lid_block = if event.block == "ae2:smooth_sky_stone_chest" { "ae2:hz_smooth_sky_stone_chest_lid" } else { "ae2:hz_sky_stone_chest_lid" };
    let Some(key) = host.block_key(lid_block) else { return };
    let facing = host.level(host.dimension()).and_then(|l| l.block_property(host, event.pos, "facing")).unwrap_or_else(|| "north".to_owned());
    let draw = &mut *event.draw;
    let c = [0.5, 0.5, 0.5];
    draw.push();
    match facing.as_str() {
        "south" => draw.rotate_about([0.0, 1.0, 0.0], 180.0, c),
        "east" => draw.rotate_about([0.0, 1.0, 0.0], -90.0, c),
        "west" => draw.rotate_about([0.0, 1.0, 0.0], 90.0, c),
        _ => {}
    }
    // The hinge: the lid's back edge (y 10, z 15 of 16), the front lifting.
    draw.rotate_about([1.0, 0.0, 0.0], 90.0 * eased, [0.5, 10.0 / 16.0, 15.0 / 16.0]);
    draw.block(key, &[]);
    draw.pop();
}

/// SkyStoneTankBlockEntityRenderer: the fluid's still texture in a box inside the glass, from the floor up to its fill.
pub(crate) fn tank_submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let Some((fluid, amount)) = tank_fluid(&event.entity.nbt) else { return };
    let (lo, hi) = (0.0635_f32, 0.9365_f32);
    let fill = lo + (hi - lo) * (amount as f32 / TANK_MB as f32).clamp(0.0, 1.0);
    let draw = &mut *event.draw;
    if let Some(key) = host.fluid_key(&fluid)
        && draw.fluid_box(&FluidStack::new(key, amount.max(1) as u32), [lo, lo, lo, hi, fill, hi], FluidBoxOptions::new(true, 0.0)).is_ok()
    {
        return;
    }
    // Another mod's fluid: its still sprite by name (left out when it has none).
    let (ns, path) = fluid.split_once(':').unwrap_or(("minecraft", fluid.as_str()));
    let sprite = format!("{ns}:block/{path}_still");
    let face = || Some(CubeFace::new(sprite.as_str()));
    let (a, b) = (lo * 16.0, hi * 16.0);
    draw.cube([a, a, a], [b, fill * 16.0, b], [face(), face(), face(), face(), face(), face()], 0xFFFF_FFFF);
}

// ---- The chest's screen (sky_chest.json over skychest.png)

pub(crate) fn screen(block: &str) -> Option<MenuScreenLayout> {
    if !is_sky_chest(block) {
        return None;
    }
    let slots = (0..CHEST_SLOTS).map(|i| Some(slot_at([8.0 + 18.0 * (i % 9) as f32, 24.0 + 18.0 * (i / 9) as f32]))).collect();
    Some(MenuScreenLayout { width: 176.0, height: 195.0, slots, inventory_y: 113.0, inventory_x: 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() })
}

pub(crate) fn screen_draw(host: &dyn ClientHost, block: &str, d: &mut dyn DrawContext, o: [f32; 2]) -> bool {
    if !is_sky_chest(block) {
        return false;
    }
    spr(d, "ae2:gui/skychest", o, [0.0, 0.0], [0.0, 0.0, 176.0, 195.0]);
    let title = if block == "ae2:smooth_sky_stone_chest" {
        tr(host, "block.ae2.smooth_sky_stone_chest", "Sky Stone Block Chest")
    } else {
        tr(host, "gui.ae2.SkyChest", "Sky Stone Chest")
    };
    d.text(&title, [o[0] + 8.0, o[1] + 8.0], TEXT, false);
    d.text(&tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 195.0 - 93.0], TEXT, false);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lid_moves_a_tenth_a_tick() {
        assert_eq!(lid_step(0.0, 1.0, 0.0, 1.0), 0.1);
        assert_eq!(lid_step(0.0, 1.0, 0.0, 30.0), 1.0);
        assert_eq!(lid_step(1.0, 0.0, 5.0, 10.0), 0.5);
        assert_eq!(lid_step(0.3, 0.0, 10.0, 5.0), 0.3);
    }

    #[test]
    fn tank_reads_the_game_tank() {
        let mut t = Compound::new();
        t.insert(java("Fluid"), Tag::String(java("minecraft:water")));
        t.insert(java("Amount"), Tag::Int(3000));
        let mut nbt = Compound::new();
        nbt.insert(java("fluids"), Tag::List(vec![Tag::Compound(t.clone())]));
        assert_eq!(tank_fluid(&nbt), Some(("minecraft:water".to_owned(), 3000)));
        t.insert(java("Amount"), Tag::Int(0));
        nbt.insert(java("fluids"), Tag::List(vec![Tag::Compound(t)]));
        assert_eq!(tank_fluid(&nbt), None);
        assert!(update_tag(&nbt).is_some());
        assert!(update_tag(&Compound::new()).is_none());
    }
}
