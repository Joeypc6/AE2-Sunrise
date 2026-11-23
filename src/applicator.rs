//! AE2's colour applicator (ColorApplicatorItem, BlockRecolorer, ColorApplicatorPickColorHook, MouseWheelScrolled): a
//! storage cell of paint (dyes, paint balls, snowballs: `tool_cells`) with a battery. Used on a block, it paints with the
//! colour it is set to, for one of that paint and 100 AE (more than 100 needed): a cable (or a cable bus's cable) takes
//! the colour, a vanilla colour family block (glass, panes, wool, banners, carpet, terracotta, glazed terracotta,
//! concrete) becomes that colour's, its properties kept; a snowball takes a cable's colour off or cleans the paint
//! splotches off the face used on. Used on a sheep, it dyes it. Sneaking, a use cycles to the next colour it holds; so
//! does the mouse wheel while sneaking (both directions); picking a block's colour (middle click on a coloured cable,
//! applicator in hand) selects it. Upstream's `applicator_use`, `recolor_at`, `paint_cable`, `cycle_applicator`,
//! `wheel` and `pick_item` (`../ae2-horizon/mixins/ae2/me.rs`).
//!
//! The selected paint is on the item as AE2 keeps it (`color`, the paint item's NBT, in its custom data); its model's
//! colour (ColorApplicatorBakedModel: AEColor's black, medium and white variants) is the item's custom model data, and
//! its name ends with the colour (`getName`), both written whenever the selection changes.

use crate::cells::*;
use crate::core::Actionable;
use crate::paint::{BLACK_VARIANT, COLOR_KEYS, COLORS, MEDIUM_VARIANT, TRANSPARENT, WHITE_VARIANT};
use crate::tool_cells::{APPLICATOR, paint_color};
use crate::util::*;
use crate::world::item_power;
use mod_api::*;

/// ColorApplicatorItem.POWER_PER_USE.
pub(crate) const POWER_PER_USE: f64 = 100.0;
const TAG_COLOR: &str = "color";

/// The paint items the applicator holds, in its cell's order.
fn held_paints(stack: &ItemStack) -> Vec<(String, i64)> {
    let c = read_cell(stack);
    c.keys
        .iter()
        .zip(&c.amounts)
        .filter(|(_, n)| **n > 0)
        .map(|(k, n)| (k.id().to_owned(), *n))
        .collect()
}

/// The paint it is set to (`color`), if any.
fn chosen(host: &dyn Host, stack: &ItemStack) -> Option<String> {
    match custom_data(stack).and_then(|d| d.get(&java(TAG_COLOR))) {
        Some(Tag::Compound(c)) => ItemStack::from_nbt(host, c).map(|s| sid(&s).to_owned()),
        _ => None,
    }
}

/// ColorApplicatorItem.getColor's choice: the set paint, else the first it holds (pure: unit tested).
pub(crate) fn current_paint(chosen: Option<&str>, held: &[String]) -> Option<String> {
    chosen.map(str::to_owned).or_else(|| held.first().cloned())
}

/// The order `findNextColor` cycles in: by AEColor (white first, TRANSPARENT last), anything else at the end.
fn color_order(item: &str) -> usize {
    paint_color(item).unwrap_or(usize::MAX)
}

/// ColorApplicatorItem.findNextColor: from the paint `anchor` (its colour), the next (`step` 1), previous (-1) or same
/// (0) paint held, by colour (pure: unit tested).
pub(crate) fn next_paint(held: &[String], anchor: &str, step: i32) -> Option<String> {
    let mut list: Vec<String> = held.to_vec();
    if list.is_empty() {
        return None;
    }
    list.sort_by_key(|i| color_order(i));
    let want = paint_color(anchor);
    let mut cycles = 1 + list.len();
    while cycles > 0 && paint_color(&list[0]) != want {
        list.rotate_left(1);
        cycles -= 1;
    }
    if step > 0 {
        list.rotate_left(1);
    } else if step < 0 {
        list.rotate_right(1);
    }
    list.first().cloned()
}

/// `setColor`: the selection written, with the model's colours and the name's colour.
fn set_color(host: &dyn Host, stack: &mut ItemStack, paint: Option<&str>) {
    let tag = paint
        .and_then(|p| new_stack(host, p, 1))
        .and_then(|s| s.to_nbt(host));
    edit_custom_data(stack, |d| match &tag {
        Some(t) => {
            d.insert(java(TAG_COLOR), Tag::Compound(t.clone()));
        }
        None => {
            d.remove(&java(TAG_COLOR));
        }
    });
    show_color(stack, paint.and_then(paint_color));
}

/// The model's colours (custom model data: flag 0 on, colours black, medium, white) and the name (`Color Applicator -
/// Red`), or neither with no colour.
fn show_color(stack: &mut ItemStack, color: Option<usize>) {
    let mut patch = stack
        .components()
        .map(|c| c.as_nbt().clone())
        .unwrap_or_default();
    match color {
        Some(c) => {
            let mut cmd = Compound::new();
            cmd.insert(java("flags"), Tag::List(vec![Tag::Byte(1)]));
            cmd.insert(
                java("colors"),
                Tag::List(vec![
                    Tag::Int(BLACK_VARIANT[c] as i32),
                    Tag::Int(MEDIUM_VARIANT[c] as i32),
                    Tag::Int(WHITE_VARIANT[c] as i32),
                ]),
            );
            patch.insert(java("minecraft:custom_model_data"), Tag::Compound(cmd));
        }
        None => {
            patch.remove(&java("minecraft:custom_model_data"));
        }
    }
    let _ = stack.set_components(patch);
    // The name in its own write: a text the game refuses leaves the colours.
    let mut patch = stack
        .components()
        .map(|c| c.as_nbt().clone())
        .unwrap_or_default();
    match color {
        Some(c) => {
            let mut name = Compound::new();
            name.insert(
                java("translate"),
                Tag::String(java("item.ae2.color_applicator")),
            );
            let mut col = Compound::new();
            col.insert(java("translate"), Tag::String(java(COLOR_KEYS[c])));
            name.insert(
                java("extra"),
                Tag::List(vec![Tag::String(java(" - ")), Tag::Compound(col)]),
            );
            patch.insert(java("minecraft:item_name"), Tag::Compound(name));
        }
        None => {
            patch.remove(&java("minecraft:item_name"));
        }
    }
    let _ = stack.set_components(patch);
}

/// ColorApplicatorItem.getColor: the set paint, or (writing it) the first one held.
fn get_color(host: &dyn Host, stack: &mut ItemStack) -> Option<String> {
    let held: Vec<String> = held_paints(stack).into_iter().map(|(i, _)| i).collect();
    let set = chosen(host, stack);
    if set.is_none()
        && let Some(first) = held.first()
    {
        set_color(host, stack, Some(first));
    }
    current_paint(set.as_deref(), &held)
}

/// `cycleColors`: the next colour held.
fn cycle(host: &dyn Host, stack: &mut ItemStack, step: i32) {
    let held: Vec<String> = held_paints(stack).into_iter().map(|(i, _)| i).collect();
    let paint = get_color(host, stack);
    match paint {
        None => set_color(host, stack, None),
        Some(p) => {
            let next = next_paint(&held, &p, step);
            set_color(host, stack, next.as_deref());
        }
    }
}

/// `consumeItem`: one of the paint and 100 AE, or nothing; the selection clears when the last of it went.
fn consume(host: &dyn Host, stack: &mut ItemStack, paint: &str) -> bool {
    let Some(ty) = cell_type(APPLICATOR) else {
        return false;
    };
    let Some(key) = held_paints(stack)
        .into_iter()
        .find(|(i, _)| i == paint)
        .and_then(|(i, _)| new_stack(host, &i, 1))
        .map(|s| key_of(host, &s))
    else {
        return false;
    };
    if item_power(stack) < POWER_PER_USE {
        return false;
    }
    let mut inv = cell_inventory(stack, ty);
    if inv.extract(&key, 1, Actionable::Modulate) < 1 {
        return false;
    }
    if let Some(c) = inv.persist() {
        write_cell(stack, &c);
    }
    crate::powered::use_power(stack, POWER_PER_USE);
    if chosen(host, stack).as_deref() == Some(paint)
        && !held_paints(stack).iter().any(|(i, _)| i == paint)
    {
        set_color(host, stack, None);
    }
    true
}

// ---- Recolouring blocks (pure)

/// BlockRecolorer's groups: the uncoloured block (None: always coloured) and the coloured ones' suffix.
const RECOLOR_GROUPS: [(Option<&str>, &str); 9] = [
    (Some("minecraft:glass"), "_stained_glass"),
    (Some("minecraft:glass_pane"), "_stained_glass_pane"),
    (Some("minecraft:white_wool"), "_wool"),
    (Some("minecraft:white_banner"), "_banner"),
    (Some("minecraft:white_wall_banner"), "_wall_banner"),
    (Some("minecraft:white_carpet"), "_carpet"),
    (Some("minecraft:terracotta"), "_terracotta"),
    (None, "_glazed_terracotta"),
    (None, "_concrete"),
];

/// BlockRecolorer.recolor: the block in colour `color` (TRANSPARENT: the group's uncoloured one, or itself), or the
/// block itself when it is in no group.
pub(crate) fn recolor_block(block: &str, color: usize) -> String {
    let Some(path) = block.strip_prefix("minecraft:") else {
        return block.to_owned();
    };
    for (plain, suffix) in RECOLOR_GROUPS {
        let colored = COLORS
            .iter()
            .take(16)
            .any(|c| path.strip_prefix(c).is_some_and(|r| r == suffix));
        if plain == Some(block) || colored {
            if color >= TRANSPARENT {
                return plain.unwrap_or(block).to_owned();
            }
            return format!("minecraft:{}{suffix}", COLORS[color]);
        }
    }
    block.to_owned()
}

/// A cable of another colour (`ae2:<colour>_<kind>`), or None when it is that colour already or no cable.
pub(crate) fn recolor_cable(block: &str, color: usize) -> Option<String> {
    let (now, _) = crate::net::cable(block)?;
    if crate::paint::color_index(now) == Some(color) {
        return None;
    }
    let rest = block.strip_prefix("ae2:")?.strip_prefix(now)?;
    Some(format!("ae2:{}{rest}", COLORS[color]))
}

/// The state string with its block swapped (`copyProp`: the properties both have).
fn swap_block(state: &str, to: &str) -> String {
    match state.split_once('[') {
        Some((_, props)) => format!("{to}[{props}"),
        None => to.to_owned(),
    }
}

// ---- The world (server)

/// ColorApplicatorItem.recolourBlock: true when the block took the colour.
fn recolour(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, color: usize) -> bool {
    let block = crate::net::block_id(&*host, level, pos);
    // A cable bus: its cable (CableBusBlock.recolorBlock).
    if crate::bus::is_cell_block(&block) {
        let Some(cable) = crate::bus::cable_at(&*host, level, pos) else {
            return false;
        };
        let Some(to) = recolor_cable(&cable, color) else {
            return false;
        };
        return crate::bus::recolor_cable(host, level, pos, &to);
    }
    if crate::net::cable(&block).is_some() {
        let Some(to) = recolor_cable(&block, color) else {
            return false;
        };
        return set_swapped(host, level, pos, &to);
    }
    let to = recolor_block(&block, color);
    if to == block {
        return false;
    }
    set_swapped(host, level, pos, &to)
}

fn set_swapped(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, to: &str) -> bool {
    let Some(state) = level
        .block_state(&*host, pos)
        .and_then(|s| host.block_state_string(s))
    else {
        return false;
    };
    let new = host
        .block_state_id(&swap_block(&state, to))
        .or_else(|| host.block_state_id(to));
    let Some(new) = new else { return false };
    matches!(
        level.set_block(
            host,
            pos,
            new,
            BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS
        ),
        Ok(true)
    )
}

/// The colour a block shows the applicator (IColorableBlockEntity.getColor): a cable's or a cable bus's cable's.
fn block_color(host: &dyn Host, level: LevelRef, pos: Pos) -> Option<usize> {
    let block = crate::net::block_id(host, level, pos);
    let cable = if crate::bus::is_cell_block(&block) {
        crate::bus::cable_at(host, level, pos)?
    } else {
        block
    };
    crate::net::cable(&cable).and_then(|(c, _)| crate::paint::color_index(c))
}

fn sneaking(host: &dyn Host, player: u128) -> bool {
    host.player(player).is_some_and(|p| {
        let e = p.entity();
        e.is_shift_key_down(host).unwrap_or(false) || e.is_crouching(host).unwrap_or(false)
    })
}

/// ColorApplicatorItem.useOn. Some when it took the click.
fn use_on(
    host: &mut dyn ServerHost,
    player: u128,
    hand: Hand,
    pos: Pos,
    face: Pos,
    held: &ItemStack,
) -> Option<InteractionResult> {
    let level = here(&*host)?;
    let mut stack = held.clone();
    let side = SIDES.iter().position(|s| *s == face)?;
    let paint =
        get_color(&*host, &mut stack).filter(|p| held_paints(&stack).iter().any(|(i, _)| i == p));
    let p = host.player(player)?;
    let bface = [
        BlockFace::Down,
        BlockFace::Up,
        BlockFace::North,
        BlockFace::South,
        BlockFace::West,
        BlockFace::East,
    ][side];
    if !p.may_build(&*host).unwrap_or(false)
        || !p.may_use_item_at(&*host, pos, bface, held).unwrap_or(false)
    {
        return Some(InteractionResult::Fail);
    }
    let power = item_power(&stack);
    let mut done = false;
    if let Some(paint) = &paint {
        if paint == "minecraft:snowball" && power > POWER_PER_USE {
            // A coloured cable back to fluix, or the face's splotches cleaned.
            if block_color(&*host, level, pos).is_some_and(|c| c != TRANSPARENT)
                && recolour(host, level, pos, TRANSPARENT)
            {
                done = consume(&*host, &mut stack, paint);
            } else if crate::paint::is_paint(&crate::net::block_id(&*host, level, add(pos, face)))
                && consume(&*host, &mut stack, paint)
            {
                crate::paint::clean_side(host, level, add(pos, face), side ^ 1);
                done = true;
            }
        }
        if !done
            && let Some(color) = paint_color(paint)
            && power > POWER_PER_USE
            && recolour(host, level, pos, color)
        {
            done = consume(&*host, &mut stack, paint);
        }
    }
    if done {
        crate::handtools::replace_hand(host, player, hand, held, stack);
        return Some(InteractionResult::SuccessServer);
    }
    if sneaking(&*host, player) {
        cycle(&*host, &mut stack, 1);
        crate::handtools::replace_hand(host, player, hand, held, stack);
    } else if stack != *held {
        // The first paint written as the selection (getColor).
        crate::handtools::replace_hand(host, player, hand, held, stack);
    }
    Some(InteractionResult::Fail)
}

/// The colour applicator's Part.
pub(crate) struct Applicator;

#[mod_api::sunrise_part]
impl ItemPart for Applicator {
    /// useOn: once the block let the click through (a terminal still opens; sneaking skips the block's use).
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return None;
        };
        crate::cells::ids(&*host);
        let held = event.stack.clone();
        use_on(host, event.player, event.hand, event.pos, event.face, &held)
    }

    /// ColorApplicatorItem.interactLivingEntity: a live, unsheared sheep of another colour takes the paint's colour.
    fn item_interact_mob(
        &self,
        host: HostSide,
        event: &mut ItemInteractMobEvent<'_>,
    ) -> Option<InteractionResult> {
        if event.kind != "minecraft:sheep" {
            return None;
        }
        let HostSide::Server(host) = host else {
            return Some(InteractionResult::Success);
        };
        crate::cells::ids(&*host);
        let player = host.players().acting_player()?;
        let uuid = player.entity().uuid(host)?;
        let mut stack = event.stack.clone();
        let paint = get_color(&*host, &mut stack)?;
        let color = paint_color(&paint).filter(|c| *c < TRANSPARENT)?;
        let e = host.entity(event.entity as u32)?;
        let nbt = e.nbt(host)?;
        if matches!(nbt.get(&java("Sheared")), Some(Tag::Byte(1))) {
            return None;
        }
        if matches!(nbt.get(&java("Color")), Some(Tag::Byte(c)) if *c as usize == color) {
            return None;
        }
        if item_power(&stack) > POWER_PER_USE && consume(&*host, &mut stack, &paint) {
            let mut set = Compound::new();
            set.insert(java("Color"), Tag::Byte(color as i8));
            let _ = e.merge_nbt(host, set);
            let _ = e.play_sound(host, "minecraft:item.dye.use", 1.0, 1.0);
            crate::handtools::replace_hand(host, uuid, event.hand, event.stack, stack);
        }
        Some(InteractionResult::SuccessServer)
    }
}

/// MouseWheelPacket on the server (IMouseWheelItem.onWheel): the held applicator (main hand, then off hand) cycles.
pub(crate) fn wheel(host: &mut dyn ServerHost, player: u128, up: bool) {
    if !sneaking(&*host, player) {
        return;
    }
    for hand in [Hand::Main, Hand::Off] {
        let Some(held) = host
            .player(player)
            .and_then(|p| LivingRef::new(p.entity()).hand_item_owned(&*host, hand))
        else {
            continue;
        };
        if sid(&held) != APPLICATOR {
            continue;
        }
        let mut stack = held.clone();
        cycle(&*host, &mut stack, if up { 1 } else { -1 });
        crate::handtools::replace_hand(host, player, hand, &held, stack);
        return;
    }
}

/// ColorApplicatorPickColorHook / ColorApplicatorSelectColorPacket: picking a coloured block with an applicator in a
/// hand selects that colour (the first of its paint held). True when it took the pick.
pub(crate) fn pick(host: &mut dyn ServerHost, player: u128, pos: Pos) -> bool {
    let Some(level) = here(&*host) else {
        return false;
    };
    let Some(p) = host.player(player) else {
        return false;
    };
    let living = LivingRef::new(p.entity());
    let hand = [Hand::Off, Hand::Main].into_iter().find(|h| {
        living
            .hand_item_owned(&*host, *h)
            .is_some_and(|s| sid(&s) == APPLICATOR)
    });
    let Some(hand) = hand else { return false };
    let Some(color) = block_color(&*host, level, pos) else {
        return false;
    };
    let Some(held) = living.hand_item_owned(&*host, hand) else {
        return false;
    };
    let mut stack = held.clone();
    // setActiveColor: the first paint of that colour held; the selection stays when none is.
    if let Some((paint, _)) = held_paints(&stack)
        .into_iter()
        .find(|(i, _)| paint_color(i) == Some(color))
    {
        set_color(&*host, &mut stack, Some(&paint));
        crate::handtools::replace_hand(host, player, hand, &held, stack);
    }
    true
}

/// The client's wheel (MouseWheelScrolled): sneaking with an applicator in the main hand (or off hand), the wheel cycles
/// its colour instead of the hotbar. True when it took the wheel.
pub(crate) fn client_wheel(host: &mut dyn ClientHost, scroll: f64) -> bool {
    if scroll == 0.0 || host.mod_screen().is_some() {
        return false;
    }
    let Some(p) = host.local_player() else {
        return false;
    };
    let e = p.entity();
    if !e.is_shift_key_down(&*host).unwrap_or(false) {
        return false;
    }
    let living = LivingRef::new(e);
    if ![Hand::Main, Hand::Off].into_iter().any(|h| {
        living
            .hand_item_owned(&*host, h)
            .is_some_and(|s| sid(&s) == APPLICATOR)
    }) {
        return false;
    }
    crate::handtools::send(host, &format!("wheel {}", if scroll > 0.0 { 1 } else { 0 }));
    true
}

/// The applicator's tooltip (AE2 puts the colour in the name, which `show_color` writes on the item; this line is for an
/// applicator whose selection was set before that).
pub(crate) fn tooltip(host: &dyn ClientHost, stack: &ItemStack) -> Option<String> {
    if sid(stack) != APPLICATOR {
        return None;
    }
    let c = chosen(host, stack).and_then(|p| paint_color(&p))?;
    Some(crate::screens::tr(host, COLOR_KEYS[c], COLORS[c]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn colours_cycle_in_aecolor_order() {
        let held = v(&[
            "minecraft:snowball",
            "minecraft:red_dye",
            "minecraft:white_dye",
            "ae2:blue_paint_ball",
        ]);
        // Sorted: white (0), blue (11), red (14), snowball (16).
        assert_eq!(
            next_paint(&held, "minecraft:white_dye", 1).as_deref(),
            Some("ae2:blue_paint_ball")
        );
        assert_eq!(
            next_paint(&held, "ae2:blue_paint_ball", 1).as_deref(),
            Some("minecraft:red_dye")
        );
        assert_eq!(
            next_paint(&held, "minecraft:red_dye", 1).as_deref(),
            Some("minecraft:snowball")
        );
        assert_eq!(
            next_paint(&held, "minecraft:snowball", 1).as_deref(),
            Some("minecraft:white_dye")
        );
        assert_eq!(
            next_paint(&held, "minecraft:white_dye", -1).as_deref(),
            Some("minecraft:snowball")
        );
        assert_eq!(
            next_paint(&held, "minecraft:red_dye", 0).as_deref(),
            Some("minecraft:red_dye")
        );
        // An anchor no longer held: Java's search turns the list once more than its length, then one step on.
        assert_eq!(
            next_paint(&held, "minecraft:black_dye", 1).as_deref(),
            Some("minecraft:red_dye")
        );
        assert_eq!(next_paint(&[], "minecraft:red_dye", 1), None);
        assert_eq!(
            current_paint(None, &held).as_deref(),
            Some("minecraft:snowball")
        );
        assert_eq!(
            current_paint(Some("minecraft:red_dye"), &held).as_deref(),
            Some("minecraft:red_dye")
        );
    }

    #[test]
    fn vanilla_blocks_recolour() {
        assert_eq!(
            recolor_block("minecraft:glass", 14),
            "minecraft:red_stained_glass"
        );
        assert_eq!(
            recolor_block("minecraft:red_stained_glass", 16),
            "minecraft:glass"
        );
        assert_eq!(
            recolor_block("minecraft:light_blue_stained_glass_pane", 0),
            "minecraft:white_stained_glass_pane"
        );
        assert_eq!(
            recolor_block("minecraft:orange_wool", 16),
            "minecraft:white_wool"
        );
        assert_eq!(
            recolor_block("minecraft:white_wool", 15),
            "minecraft:black_wool"
        );
        assert_eq!(
            recolor_block("minecraft:terracotta", 3),
            "minecraft:light_blue_terracotta"
        );
        assert_eq!(
            recolor_block("minecraft:cyan_glazed_terracotta", 5),
            "minecraft:lime_glazed_terracotta"
        );
        assert_eq!(
            recolor_block("minecraft:cyan_glazed_terracotta", 16),
            "minecraft:cyan_glazed_terracotta"
        );
        assert_eq!(
            recolor_block("minecraft:gray_concrete", 7),
            "minecraft:gray_concrete"
        );
        assert_eq!(
            recolor_block("minecraft:pink_carpet", 1),
            "minecraft:orange_carpet"
        );
        assert_eq!(
            recolor_block("minecraft:red_wall_banner", 2),
            "minecraft:magenta_wall_banner"
        );
        assert_eq!(recolor_block("minecraft:stone", 2), "minecraft:stone");
        assert_eq!(
            recolor_block("minecraft:red_concrete_powder", 2),
            "minecraft:red_concrete_powder"
        );
    }

    #[test]
    fn cables_recolour() {
        assert_eq!(
            recolor_cable("ae2:fluix_glass_cable", 14).as_deref(),
            Some("ae2:red_glass_cable")
        );
        assert_eq!(
            recolor_cable("ae2:light_blue_smart_dense_cable", 16).as_deref(),
            Some("ae2:fluix_smart_dense_cable")
        );
        assert_eq!(recolor_cable("ae2:red_covered_cable", 14), None);
        assert_eq!(recolor_cable("ae2:controller", 2), None);
        assert_eq!(
            swap_block("ae2:red_glass_cable[north=true]", "ae2:blue_glass_cable"),
            "ae2:blue_glass_cable[north=true]"
        );
    }
}
