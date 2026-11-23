//! The certus and nether quartz wrenches on AE2's machines, AE2's placement orientation, and upgrade cards used on a
//! machine:
//! - a use rotates the machine about the clicked face (BlockOrientation.rotateClockwiseAround: front and top for the
//!   FacingWithSpin blocks, the front for the facing ones); on a pattern provider it sets the side it pushes to
//!   (PatternProviderBlock's push direction); vanilla blocks and AE2 blocks with no orientation are left to their own use;
//! - a sneaking use dismantles the machine (AEBaseEntityBlock.disassembleWithWrench) into the player's inventory: its
//!   item carries its settings and upgrade cards (SettingsFrom.DISMANTLE_ITEM), what it held comes out with it (cells,
//!   patterns, stored items, fuel). A machine whose tanks hold fluid that the network won't take is not dismantled (no
//!   item can carry it); an energy cell's item carries its share of the network's power. The block goes with its
//!   block entity's side effects skipped, so nothing is dropped twice; nothing is given unless the block went;
//! - placing a FacingWithSpin or facing block sets its front back at the player and its top as AE2 does
//!   (`placement_state`); an item with settings gives them to the new block at its first tick (`placed`, `apply_pending`);
//! - a sneaking use of an upgrade card on a machine (UpgradeCardItem.onItemUseFirst) installs as many as fit, the
//!   machine's limit of each kind kept, the cards taken from the hand in the same transaction.
//!
//! Upstream: `wrench_rotate`, `wrench_turn`, `wrench_dismantle`, `machine_settings`, `provider_set_side`, `has_spin`,
//! `spin_up`, `set_orientation`, `install_card` (`../ae2-horizon/mixins/ae2/me.rs`).

use crate::cells::*;
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

// ---- Orientation (pure)

/// The blocks with FacingWithSpinStrategy: a front and a spin about it.
pub(crate) fn has_spin(name: &str) -> bool {
    matches!(name, "drive" | "chest" | "io_port" | "inscriber" | "charger" | "cell_workbench" | "vibration_chamber" | "spatial_io_port" | "crafting_monitor")
}

/// The blocks with a front only (FacingStrategy).
fn facing_only(name: &str) -> bool {
    matches!(name, "molecular_assembler" | "wireless_access_point" | "crystal_resonance_generator")
}

/// The blocks that face a horizontal way only (the sky stone chests).
fn horizontal(name: &str) -> bool {
    matches!(name, "sky_stone_chest" | "smooth_sky_stone_chest")
}

pub(crate) const FACING_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

fn facing_name(d: Pos) -> &'static str {
    SIDES.iter().position(|s| *s == d).map_or("north", |i| FACING_NAMES[i])
}

fn cross(a: Pos, b: Pos) -> Pos {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn neg(d: Pos) -> Pos {
    [-d[0], -d[1], -d[2]]
}

/// Where the top of a block's model points at `facing` and `spin`: the blockstate's `z` turn (about the front), then
/// its `x` and `y` turns.
pub(crate) fn spin_up(facing: Pos, spin: usize) -> Pos {
    let [x, y, z] = [[0, 1, 0], [1, 0, 0], [0, -1, 0], [-1, 0, 0]][spin % 4];
    match facing {
        [0, 0, 1] => [-x, y, -z],
        [-1, 0, 0] => [z, y, -x],
        [1, 0, 0] => [-z, y, x],
        [0, 1, 0] => [x, -z, y],
        [0, -1, 0] => [x, z, -y],
        _ => [x, y, z],
    }
}

/// The spin that puts the top at `up` (none when `up` isn't square to `facing`).
pub(crate) fn spin_for(facing: Pos, up: Pos) -> Option<usize> {
    (0..4).find(|s| spin_up(facing, *s) == up)
}

/// A quarter turn clockwise about the clicked `face` (seen from outside it); a direction along its axis stays.
pub(crate) fn turn_about(d: Pos, face: Pos) -> Pos {
    let c = cross(d, face);
    if c == [0, 0, 0] { d } else { c }
}

/// `PatternProviderBlock` with a wrench: the clicked face's opposite first; the side itself again turns it back to all
/// sides; any other turns a quarter about the face. None is "all".
pub(crate) fn provider_next_side(current: Option<Pos>, face: Pos) -> Option<Pos> {
    let opposite = neg(face);
    match current {
        None => Some(opposite),
        Some(s) if s == opposite => Some(face),
        Some(s) if s == face => None,
        Some(s) => Some(cross(s, face)),
    }
}

/// `BlockPlaceContext.getNearestLookingDirection` from a player's yaw and pitch (degrees).
pub(crate) fn nearest_looking(yaw: f32, pitch: f32) -> Pos {
    let (y, p) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    let v = [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()];
    let axis = (0..3).max_by(|a, b| v[*a].abs().total_cmp(&v[*b].abs())).unwrap_or(2);
    let mut d = [0; 3];
    d[axis] = if v[axis] < 0.0 { -1 } else { 1 };
    d
}

/// `Direction.fromYRot`: the horizontal way a player at `yaw` looks.
pub(crate) fn horizontal_looking(yaw: f32) -> Pos {
    match ((f64::from(yaw) / 90.0 + 0.5).floor() as i64).rem_euclid(4) {
        0 => [0, 0, 1],
        1 => [-1, 0, 0],
        2 => [0, 0, -1],
        _ => [1, 0, 0],
    }
}

/// FacingWithSpinStrategy.getStateForPlacement: the front back at the player; placed looking up or down, its top the
/// way the player looks.
pub(crate) fn placement_orientation(yaw: f32, pitch: f32) -> (Pos, Pos) {
    let facing = neg(nearest_looking(yaw, pitch));
    let up = if facing[1] != 0 { horizontal_looking(yaw) } else { [0, 1, 0] };
    (facing, up)
}

// ---- Placement

/// `state_for_placement` of AE2's devices: the orientation strategies' states. None leaves the block's own placement.
pub(crate) fn placement_state(level: &dyn LevelHost, event: &StateForPlacementEvent<'_>) -> Option<Option<BlockState>> {
    let block = level.state_block(event.state)?.to_string();
    let name = block.strip_prefix("ae2:")?.to_owned();
    let cx = event.context;
    let state = event.state;
    if has_spin(&name) {
        let (facing, up) = placement_orientation(cx.yaw, cx.pitch);
        let spin = spin_for(facing, up).unwrap_or(0);
        let s = level.state_with_property(state, "facing", facing_name(facing))?;
        let s = level.state_with_property(s, "spin", &spin.to_string())?;
        return Some(Some(s));
    }
    if matches!(name.as_str(), "molecular_assembler" | "wireless_access_point") {
        let facing = neg(nearest_looking(cx.yaw, cx.pitch));
        return Some(Some(level.state_with_property(state, "facing", facing_name(facing))?));
    }
    if horizontal(&name) {
        let facing = neg(horizontal_looking(cx.yaw));
        return Some(Some(level.state_with_property(state, "facing", facing_name(facing))?));
    }
    None
}

/// Settings from a dismantled machine's item, waiting for its block entity's first tick: the block, the settings and
/// the level tick they were placed at.
static PENDING: Mutex<Option<HashMap<(&'static str, Pos), (String, Compound, u64)>>> = Mutex::new(None);

fn with_pending<R>(f: impl FnOnce(&mut HashMap<(&'static str, Pos), (String, Compound, u64)>) -> R) -> R {
    let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

pub(crate) fn forget_all() {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

const SETTINGS: &str = "ae2:settings";
const SETTINGS_FROM: &str = "ae2:settings_from";
const UPGRADES: &str = "ae2:upgrades";
const POWER: &str = "internalCurrentPower";

/// A block placed by a player (`entity_place`): the settings on the item placed, for its block entity's first tick.
pub(crate) fn placed(host: &mut dyn ServerHost, event: &EntityPlaceEvent) {
    if event.cancelled {
        return;
    }
    let Some(placer) = event.placer else { return };
    let Some(block) = host.block_state_string(event.state).map(|b| b.split('[').next().unwrap_or("").to_owned()) else { return };
    if !block.starts_with("ae2:") {
        return;
    }
    let living = LivingRef::new(placer);
    let held = [Hand::Main, Hand::Off].into_iter().filter_map(|h| living.hand_item_owned(&*host, h)).find(|s| sid(s) == block);
    let mut settings = match held.as_ref().and_then(custom_data).and_then(|d| d.get(&java(SETTINGS))).cloned() {
        Some(Tag::Compound(s)) => s,
        _ => Compound::new(),
    };
    // [tools] An energy cell's charge is on its item as AE2 keeps it (EnergyCellBlockItem's `internalCurrentPower`).
    if matches!(block.as_str(), "ae2:energy_cell" | "ae2:dense_energy_cell")
        && let Some(p @ Tag::Double(_)) = held.as_ref().and_then(custom_data).and_then(|d| d.get(&java(POWER))).cloned()
    {
        settings.insert(java(POWER), p);
    }
    if settings.is_empty() {
        return;
    }
    // The memory card's settings are under the same key: only a dismantled machine's item (no "from").
    if held.as_ref().and_then(custom_data).is_some_and(|d| d.contains_key(&java(SETTINGS_FROM))) {
        return;
    }
    let dim = host.dimension();
    let tick = crate::net::with_net(dim, |n| n.tick);
    with_pending(|p| {
        p.retain(|_, (_, _, t)| tick.saturating_sub(*t) < 100);
        p.insert((dim, [event.pos.x, event.pos.y, event.pos.z]), (block, settings, tick));
    });
}

/// The settings of a placed item onto its block entity, at its first tick (before its power is read).
pub(crate) fn apply_pending(host: &dyn Host, pos: Pos, block: &str, fields: &mut Fields) {
    if is_hosted(pos) {
        return;
    }
    let dim = host.dimension();
    let Some((b, settings, tick)) = with_pending(|p| p.remove(&(dim, pos))) else { return };
    let now = crate::net::with_net(dim, |n| n.tick);
    if b != block || now.saturating_sub(tick) > 100 {
        return;
    }
    for (k, v) in &settings {
        let key = k.to_string_lossy();
        if key == UPGRADES {
            continue;
        }
        if SETTING_KEYS.contains(&key.as_str()) || key == POWER {
            fields.nbt.insert(k.clone(), v.clone());
        }
    }
    // The cards into the card slots, one a slot, no more than the machine takes of each.
    if let (Some(Tag::List(cards)), Some((from, n))) = (settings.get(&java(UPGRADES)), card_slots(block)) {
        let slots = crate::Mod::container_of(block).map_or(0, |c| c.slots);
        if fields.items.len() < slots {
            fields.items.resize(slots, None);
        }
        for t in cards {
            let Tag::String(id) = t else { continue };
            let id = id.to_string_lossy();
            let have = (from..from + n).filter(|i| fields.items.get(*i).cloned().flatten().is_some_and(|s| sid(&s) == id)).count();
            if have >= card_max(block, &id) {
                continue;
            }
            let Some(free) = (from..from + n).find(|i| fields.items.get(*i).is_some_and(Option::is_none)) else { break };
            fields.items[free] = new_stack(host, &id, 1);
        }
    }
    debug(&format!("wrench: settings onto {block} at {pos:?}"));
}

// ---- Cards

/// A machine's upgrade card slots: where they start and how many.
pub(crate) fn card_slots(block: &str) -> Option<(usize, usize)> {
    use crate::parts::Kind;
    match crate::parts::kind(block) {
        Some(Kind::Import | Kind::Export | Kind::StorageBus | Kind::Interface | Kind::LevelEmitter) => return Some(crate::parts::upgrade_slots(block)),
        Some(_) => return None,
        None => {}
    }
    match block {
        "ae2:inscriber" => Some((4, 4)),
        "ae2:io_port" => Some((12, 3)),
        "ae2:vibration_chamber" => Some((crate::generators::VIB_CARDS.start, crate::generators::VIB_CARDS.len())),
        "ae2:molecular_assembler" => Some((1, 5)),
        _ => None,
    }
}

/// Upgrades.getMaxInstallable: how many of `card` the machine takes.
pub(crate) fn card_max(block: &str, card: &str) -> usize {
    if crate::parts::kind(block).is_some() {
        return crate::parts::card_limit(block, card);
    }
    match (block, card) {
        ("ae2:inscriber", "ae2:speed_card") => 4,
        ("ae2:io_port", "ae2:speed_card") => 3,
        ("ae2:io_port", "ae2:redstone_card") => 1,
        ("ae2:vibration_chamber", "ae2:energy_card" | "ae2:speed_card") => 3,
        ("ae2:molecular_assembler", "ae2:speed_card") => 5,
        _ => 0,
    }
}

/// AE2's upgrade cards.
pub(crate) const CARDS: [&str; 9] = [
    "ae2:speed_card", "ae2:capacity_card", "ae2:fuzzy_card", "ae2:redstone_card", "ae2:inverter_card", "ae2:crafting_card",
    "ae2:void_card", "ae2:energy_card", "ae2:equal_distribution_card",
];

/// How many cards go in: (installed, free slots, limit, held) to how many, or the refusal's text.
pub(crate) fn cards_to_install(installed: usize, free: usize, limit: usize, held: u32) -> Result<usize, &'static str> {
    if free == 0 {
        return Err("The upgrade capacity of this machine has been reached.");
    }
    if installed >= limit {
        return Err("No further upgrade cards of this type can be installed.");
    }
    Ok(free.min(limit - installed).min(held as usize))
}

/// The world slot behind a machine's or hosted part's slot `slot`.
pub(crate) fn slot_target(pos: Pos, slot: usize) -> (Pos, u32) {
    if is_hosted(pos) {
        let (c, side) = host_cell(pos);
        (c, (side * crate::bus::SLOTS + slot) as u32)
    } else {
        (pos, slot as u32)
    }
}

fn sneaking(host: &dyn Host, player: u128) -> bool {
    host.player(player).is_some_and(|p| {
        let e = p.entity();
        e.is_shift_key_down(host).unwrap_or(false) || e.is_crouching(host).unwrap_or(false)
    })
}

fn say(host: &mut dyn ServerHost, player: u128, text: &str) {
    if let Some(p) = host.player(player) {
        let _ = p.action_bar(host, Text::literal(text));
    }
}

/// Whether the player stands within reach of the block (8 blocks, as the menus' checks).
fn in_reach(host: &dyn Host, player: u128, pos: Pos) -> bool {
    let Some(p) = host.player(player) else { return false };
    let Some(at) = p.entity().position(host) else { return false };
    let pos = real(pos);
    (0..3).map(|i| (at[i] - f64::from(pos[i]) - 0.5).powi(2)).sum::<f64>() <= 64.0
}

/// UpgradeCardItem.onItemUseFirst: sneaking, a card on a machine (or a hosted part) goes in.
pub(crate) fn card_use(host: &mut dyn ServerHost, stack: &ItemStack, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let card = sid(stack);
    if !CARDS.contains(&card) || !sneaking(&*host, click.player) {
        return None;
    }
    let level = here(&*host)?;
    let node = crate::bus::aimed_node(&*host, level, click).unwrap_or(click.pos);
    let block = crate::net::block_id(&*host, level, node);
    let (from, n) = card_slots(&block)?;
    let limit = card_max(&block, card);
    if limit == 0 || !in_reach(&*host, click.player, node) {
        return None;
    }
    let slots: Vec<Option<ItemStack>> = (from..from + n).map(|i| crate::bus::item_slot(&*host, level, node, i)).collect();
    let installed = slots.iter().flatten().filter(|s| sid(s) == card).map(|s| s.count() as usize).sum();
    let free: Vec<usize> = (0..n).filter(|i| slots[*i].is_none()).map(|i| from + i).collect();
    let count = match cards_to_install(installed, free.len(), limit, stack.count()) {
        Ok(c) => c,
        Err(why) => {
            say(host, click.player, why);
            return Some(InteractionResult::SuccessServer);
        }
    };
    let Some(one) = with_count(stack, 1) else { return None };
    let res = ItemResource::of(stack);
    let hand = ItemPlace::PlayerHand { player: click.player, hand: click.hand };
    let done = host.transaction(|host, tx| {
        if hand.extract(host, tx, &res, count as u32) != count as u32 {
            return Err(());
        }
        for slot in free.iter().take(count) {
            let (at, i) = slot_target(node, *slot);
            let be = host.block_entity(level, at).ok_or(())?;
            if !matches!(be.set_item_slot(host, tx, i, Some(one.clone())), Ok(true)) {
                return Err(());
            }
        }
        Ok(())
    });
    debug(&format!("cards: {count} {card} into {block} at {node:?}: {done:?}"));
    Some(InteractionResult::SuccessServer)
}

// ---- The wrench

/// A stack into the player's inventory (the held slot, then the rest), what doesn't fit at their feet.
pub(crate) fn give_or_drop(host: &mut dyn ServerHost, player: u128, stack: ItemStack) {
    let res = ItemResource::of(&stack);
    let total = stack.count();
    let selected = host.player(player).and_then(|p| p.selected_slot(&*host)).unwrap_or(0) as i32;
    // Inventory.add: onto the same item first (the held slot, the off hand, the rest), then into an empty slot.
    let order: Vec<i32> = [selected, -106].into_iter().chain(0..36).collect();
    let now: Vec<(i32, Option<ItemStack>)> = order.iter().map(|s| (*s, (ItemPlace::PlayerSlot { player, slot: *s }).stack_owned(host))).collect();
    let went = host
        .transaction(|host, tx| {
            let mut left = total;
            for pass in 0..2 {
                for (slot, there) in &now {
                    if left == 0 {
                        break;
                    }
                    let fits = match there {
                        Some(s) => pass == 0 && s.is_stackable_with(&stack),
                        None => pass == 1 && *slot != -106,
                    };
                    if fits {
                        left -= (ItemPlace::PlayerSlot { player, slot: *slot }).insert(host, tx, &res, left).min(left);
                    }
                }
            }
            Ok::<u32, ()>(total - left)
        })
        .unwrap_or(0);
    if went < total
        && let Some(rest) = with_count(&stack, total - went)
        && let Some(p) = host.player(player)
    {
        let _ = p.drop_stack(host, rest, false);
    }
}

/// What of a machine's saved state is its settings (exportSettings), not what it is doing now: the memory card's keys
/// and the machines' own buttons' keys.
pub(crate) const SETTING_KEYS: [&str; 21] = [
    "ghosts", "redstone", "schedule", "craft_only", "craft_redstone", "fuzzy", "access", "no_extract_filter", "report_all", "priority", "level",
    "below", "drop_items", "blocking", "lock", "auto_export", "separate_sides", "small_buffer", "mode", "full", "copy",
];

/// A use of a wrench on a block that isn't a cable bus (`bus::wrench` takes those): Some when it took the click.
pub(crate) fn use_on(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let level = here(&*host)?;
    let pos = click.pos;
    let raw = level.block(&*host, pos).and_then(|b| b.id(&*host).map(str::to_owned))?;
    if !raw.starts_with("ae2:") || raw.starts_with("ae2:hz_") || crate::bus::is_cell_block(&raw) || crate::net::cable(&raw).is_some() {
        return None;
    }
    let name = raw.trim_start_matches("ae2:").to_owned();
    if !in_reach(&*host, click.player, pos) {
        return None;
    }
    if sneaking(&*host, click.player) {
        if host.block_entity(level, pos).is_none() {
            return None;
        }
        dismantle(host, level, pos, &raw, click.player);
        return Some(InteractionResult::SuccessServer);
    }
    let face = click.hit.face;
    if raw == "ae2:pattern_provider" {
        let current = level.block_property(&*host, pos, "push_direction").filter(|d| d != "all").map(|d| facing_dir(Some(&d)));
        let next = provider_next_side(current, face).map_or("all", facing_name);
        host.set_block_property_with(pos, "push_direction", next, 3);
        return Some(InteractionResult::SuccessServer);
    }
    let facing = facing_dir(level.block_property(&*host, pos, "facing").as_deref());
    level.block_property(&*host, pos, "facing")?;
    if has_spin(&name) {
        let spin = level.block_property(&*host, pos, "spin").and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
        let (f, up) = (turn_about(facing, face), turn_about(spin_up(facing, spin), face));
        if let Some(s) = spin_for(f, up) {
            host.set_block_property_with(pos, "facing", facing_name(f), 3);
            host.set_block_property_with(pos, "spin", &s.to_string(), 3);
        }
        return Some(InteractionResult::SuccessServer);
    }
    if facing_only(&name) {
        let f = turn_about(facing, face);
        if f != facing {
            host.set_block_property_with(pos, "facing", facing_name(f), 3);
        }
        return Some(InteractionResult::SuccessServer);
    }
    if horizontal(&name) {
        // About the vertical axis whichever face was clicked (they stand upright).
        let f = turn_about(facing, [0, 1, 0]);
        host.set_block_property_with(pos, "facing", facing_name(f), 3);
        return Some(InteractionResult::SuccessServer);
    }
    if crate::bus::hostable(&raw) {
        // A part standing on its own: the next facing with an AE2 block behind it (when one is beside it at all).
        let at = SIDES.iter().position(|s| *s == facing).unwrap_or(2);
        let ae2_at = |host: &dyn Host, d: Pos| crate::net::block_id(host, level, add(pos, d)).starts_with("ae2:");
        let any = SIDES.iter().any(|d| ae2_at(&*host, *d));
        for step in 1..6 {
            let next = SIDES[(at + step) % 6];
            if any && !ae2_at(&*host, neg(next)) {
                continue;
            }
            host.set_block_property_with(pos, "facing", facing_name(next), 3);
            break;
        }
        return Some(InteractionResult::SuccessServer);
    }
    None
}

/// AEBaseEntityBlock.disassembleWithWrench: the machine's item (its settings and cards on it) and what it held into
/// the player's inventory, the block gone.
fn dismantle(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, block: &str, player: u128) {
    let Some(be) = host.block_entity(level, pos) else { return };
    let Some(mut nbt) = be.data(&*host) else { return };
    let dim = host.dimension();
    // Fluid in its tanks goes to the network first; what stays keeps the machine where it is (no item holds fluid).
    let mut tanks = crate::iface_fluids::read_tanks(&nbt);
    if tanks.iter().any(|t| t.0.is_some()) {
        let mut left = false;
        for t in tanks.iter_mut() {
            let Some(f) = t.0.clone() else { continue };
            let stored = crate::storage::insert(host, crate::storage::Scope::Grid(pos), None, &crate::core::AeKey::fluid(&f, ""), t.1).clamp(0, t.1);
            t.1 -= stored;
            if t.1 <= 0 {
                *t = (None, 0);
            } else {
                left = true;
            }
        }
        crate::iface_fluids::write_tanks(&mut nbt, &tanks);
        let _ = be.replace_data(host, nbt.clone());
        if left {
            say(host, player, "Its tanks hold fluid the network can't take: empty them first.");
            return;
        }
    }
    let Some(mut item) = new_stack(&*host, block, 1) else { return };
    let slots = crate::Mod::container_of(block).map_or(0, |c| c.slots);
    let items: Vec<Option<ItemStack>> = (0..slots).map(|i| be.item_slot(&*host, i as u32).ok().flatten()).collect();
    let cards = card_slots(block).map_or(0..0, |(from, n)| from..from + n);
    // The settings and cards on the item; the rest given.
    let mut settings = Compound::new();
    for k in SETTING_KEYS {
        if let Some(v) = nbt.get(&java(k)) {
            settings.insert(java(k), v.clone());
        }
    }
    let mut upgrades = Vec::new();
    let mut contents = Vec::new();
    for (i, s) in items.into_iter().enumerate() {
        let Some(s) = s else { continue };
        if cards.contains(&i) && sid(&s).ends_with("_card") {
            upgrades.extend((0..s.count()).map(|_| Tag::String(java(sid(&s)))));
        } else {
            contents.push(s);
        }
    }
    if !upgrades.is_empty() {
        settings.insert(java(UPGRADES), Tag::List(upgrades));
    }
    let energy_cell = matches!(block, "ae2:energy_cell" | "ae2:dense_energy_cell");
    let share = if energy_cell { crate::net::stored_share(dim, pos).unwrap_or(0.0) } else { 0.0 };
    let Some(air) = host.block_state_id("minecraft:air") else { return };
    let flags = BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS | BlockUpdateFlags::SUPPRESS_DROPS | BlockUpdateFlags::SKIP_BLOCK_ENTITY_SIDE_EFFECTS;
    if !matches!(level.set_block(host, pos, air, flags), Ok(true)) {
        return;
    }
    // EnergyCellBlockEntity: its share of the network's power leaves with it.
    if share > 0.0 {
        let taken = crate::net::extract_power(dim, pos, share);
        // [tools] On the item as AE2 keeps it (EnergyCellBlockItem), so the tooltip and placing read it.
        if taken > 0.0 {
            edit_custom_data(&mut item, |d| {
                d.insert(java(POWER), Tag::Double(taken.to_bits()));
            });
        }
    }
    if !settings.is_empty() {
        edit_custom_data(&mut item, |d| {
            d.insert(java(SETTINGS), Tag::Compound(settings));
        });
    }
    crate::net::mark_dirty(dim);
    debug(&format!("wrench: dismantled {block} at {pos:?}, {} stacks out", contents.len()));
    give_or_drop(host, player, item);
    for s in contents {
        give_or_drop(host, player, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spins_cover_every_top() {
        for f in SIDES {
            let tops: Vec<Pos> = (0..4).map(|s| spin_up(f, s)).collect();
            for t in &tops {
                // Square to the front, and four different ones.
                assert_eq!(t[0] * f[0] + t[1] * f[1] + t[2] * f[2], 0, "{f:?} {t:?}");
            }
            for (i, t) in tops.iter().enumerate() {
                assert_eq!(spin_for(f, *t), Some(i));
            }
        }
        assert_eq!(spin_up([0, 0, -1], 0), [0, 1, 0]);
        assert_eq!(spin_for([0, 0, -1], [0, 0, 1]), None);
    }

    #[test]
    fn rotation_turns_clockwise_about_the_face() {
        // Clicked on top: north turns to east, east to south; up stays up.
        assert_eq!(turn_about([0, 0, -1], [0, 1, 0]), [1, 0, 0]);
        assert_eq!(turn_about([1, 0, 0], [0, 1, 0]), [0, 0, 1]);
        assert_eq!(turn_about([0, 1, 0], [0, 1, 0]), [0, 1, 0]);
        // Four turns are a full circle.
        let mut d = [0, 0, -1];
        for _ in 0..4 {
            d = turn_about(d, [1, 0, 0]);
        }
        assert_eq!(d, [0, 0, -1]);
        // Front and top turned together stay square.
        let (f, up) = (turn_about([0, 0, -1], [1, 0, 0]), turn_about([0, 1, 0], [1, 0, 0]));
        assert!(spin_for(f, up).is_some());
    }

    #[test]
    fn provider_side_cycles() {
        let up = [0, 1, 0];
        let s = provider_next_side(None, up);
        assert_eq!(s, Some([0, -1, 0]));
        let s = provider_next_side(s, up);
        assert_eq!(s, Some(up));
        assert_eq!(provider_next_side(s, up), None);
        // A side square to the face turns a quarter.
        assert_eq!(provider_next_side(Some([0, 0, -1]), up), Some([1, 0, 0]));
    }

    #[test]
    fn placement_faces_the_player() {
        // Looking south (yaw 0) level: the front faces north, its top up.
        assert_eq!(placement_orientation(0.0, 0.0), ([0, 0, -1], [0, 1, 0]));
        // Looking west (yaw 90): faces east.
        assert_eq!(placement_orientation(90.0, 10.0), ([1, 0, 0], [0, 1, 0]));
        // Looking down at it while facing south: it faces up, its top south.
        let (f, up) = placement_orientation(0.0, 80.0);
        assert_eq!((f, up), ([0, 1, 0], [0, 0, 1]));
        assert!(spin_for(f, up).is_some());
        // Looking up: it faces down.
        assert_eq!(placement_orientation(180.0, -70.0).0, [0, -1, 0]);
        assert_eq!(horizontal_looking(-90.0), [1, 0, 0]);
    }

    #[test]
    fn cards_respect_limits() {
        assert_eq!(cards_to_install(0, 4, 4, 64), Ok(4));
        assert_eq!(cards_to_install(3, 1, 4, 64), Ok(1));
        assert_eq!(cards_to_install(1, 3, 1, 1), Err("No further upgrade cards of this type can be installed."));
        assert_eq!(cards_to_install(0, 0, 4, 1), Err("The upgrade capacity of this machine has been reached."));
        assert_eq!(cards_to_install(0, 5, 2, 1), Ok(1));
    }
}
