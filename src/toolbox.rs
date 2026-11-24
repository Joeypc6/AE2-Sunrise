//! The network tool's toolbox (NetworkToolItem, ToolboxMenu): nine slots of upgrade cards carried on the tool, opened by
//! its use in the air (`wireless`'s item screen), and the upgrade toolbelt (ToolboxPanel): the same nine slots beside a
//! machine's upgrade panel while the player carries a network tool, so cards go from the tool to the machine through
//! the cursor. Upstream's `toolbox_of`, `toolbox_tick`, `network_tool_slot`, `toolbelt_draw` and `toolbelt_click`
//! (`../ae2-horizon/mixins/ae2/me.rs`).
//!
//! The cards are on the tool (`ae2:toolbox` in its custom data, a stack's NBT a slot). Every click is the server's: the
//! tool is read from the player's slot, changed and written in one edit, and only then is the cursor given what came
//! out; a slot takes upgrade cards only, up to a stack.

use crate::cells::*;
use crate::screens::{spr, tr};
use crate::util::*;
use crate::wireless::{Kind, Session, edit_held, give_back, pickup, put_cursor};
use mod_api::*;

const TOOL: &str = "ae2:network_tool";
const KEY: &str = "ae2:toolbox";
const TEXT: u32 = 0xFF40_4040;
/// toolbox.png's slots (as a dispenser's: three by three from 62, 17).
const SLOTS: [f32; 2] = [62.0, 17.0];
/// The toolbelt's size beside a machine's screen, and its first slot inside it.
const BELT: [f32; 2] = [68.0, 68.0];
const BELT_INSET: f32 = 7.0;
/// The menus with an upgrade panel (the toolbelt shows beside them).
const BELT_MENUS: [&str; 10] = [
    "ae2:molecular_assembler",
    "ae2:vibration_chamber",
    "ae2:inscriber",
    "ae2:io_port",
    "ae2:io_bus",
    "ae2:storage_bus",
    "ae2:interface",
    "ae2:level_emitter",
    "ae2:formation_plane",
    "ae2:pattern_provider",
];

/// Whether a stack goes in the toolbox (RestrictedInputSlot of UPGRADES: an upgrade card).
pub(crate) fn is_card(item: &str) -> bool {
    item.strip_prefix("ae2:").is_some_and(|n| n.ends_with("_card"))
}

/// The tool's nine slots.
pub(crate) fn items_of(host: &dyn Host, tool: &ItemStack) -> Vec<Option<ItemStack>> {
    let mut out = vec![None; 9];
    if let Some(Tag::List(list)) = custom_data(tool).and_then(|d| d.get(&java(KEY))) {
        for (i, t) in list.iter().enumerate().take(9) {
            if let Tag::Compound(c) = t
                && !c.is_empty()
            {
                out[i] = ItemStack::from_nbt(host, c);
            }
        }
    }
    out
}

/// The slots as saved on the tool (made before the tool is edited: the edit holds the host).
fn toolbox_tag(host: &dyn Host, items: &[Option<ItemStack>]) -> Option<Tag> {
    if items.iter().all(Option::is_none) {
        return None;
    }
    Some(Tag::List((0..9).map(|i| Tag::Compound(items.get(i).cloned().flatten().and_then(|s| s.to_nbt(host)).unwrap_or_default())).collect()))
}

fn set_toolbox(tool: &mut ItemStack, tag: Option<Tag>) {
    edit_custom_data(tool, |d| match tag {
        Some(t) => {
            d.insert(java(KEY), t);
        }
        None => {
            d.remove(&java(KEY));
        }
    });
}

/// Cards from a shift-click in the inventory into the toolbox (onto the same card first, then empty slots, a stack each).
/// How many went in.
pub(crate) fn store(host: &mut dyn ServerHost, player: u128, s: &Session, st: &ItemStack) -> u32 {
    if s.kind != Kind::Toolbox || !is_card(sid(st)) {
        return 0;
    }
    let Some(tool) = crate::wireless::held(host, player, s) else { return 0 };
    let mut items = items_of(&*host, &tool);
    let most = max_stack(&*host, st).min(64);
    let mut left = st.count();
    for pass in 0..2 {
        for slot in items.iter_mut() {
            if left == 0 {
                break;
            }
            match slot {
                Some(x) if pass == 0 && x.is_stackable_with(st) => {
                    let add = left.min(most.saturating_sub(x.count()));
                    if add > 0 {
                        *slot = with_count(x, x.count() + add);
                        left -= add;
                    }
                }
                None if pass == 1 => {
                    let add = left.min(most);
                    *slot = with_count(st, add);
                    left -= add;
                }
                _ => {}
            }
        }
    }
    let stored = st.count() - left;
    if stored == 0 {
        return 0;
    }
    let tag = toolbox_tag(&*host, &items);
    if edit_held(host, player, s, |t| set_toolbox(t, tag)) { stored } else { 0 }
}

/// A click on a toolbox slot of the open toolbox screen: PICKUP with the cursor (cards only), or with shift the slot's
/// stack into the inventory.
pub(crate) fn slot_click(host: &mut dyn ServerHost, player: u128, i: usize, right: bool, shift: bool) {
    let Some((s, tool)) = crate::wireless::checked(host, player) else { return };
    if s.kind != Kind::Toolbox {
        return;
    }
    let mut items = items_of(&*host, &tool);
    if shift {
        let Some(out) = items[i].take() else { return };
        let tag = toolbox_tag(&*host, &items);
        if edit_held(host, player, &s, |t| set_toolbox(t, tag)) {
            give_back(host, player, out);
        }
        return;
    }
    let Ok(cursor) = crate::wireless::take_cursor(host, player) else { return };
    let may = cursor.as_ref().is_none_or(|c| is_card(sid(c)));
    let (cur, new) = pickup(&*host, cursor.clone(), items[i].clone(), right, 64, may);
    if new != items[i] {
        items[i] = new;
        let tag = toolbox_tag(&*host, &items);
        if !edit_held(host, player, &s, |t| set_toolbox(t, tag)) {
            put_cursor(host, player, cursor);
            return;
        }
    }
    put_cursor(host, player, cur);
    crate::wireless::send_cursor(host, player);
}

/// NetworkToolItem.findNetworkToolInv: the first network tool the player carries.
fn tool_slot(host: &mut dyn ServerHost, player: u128) -> Option<i32> {
    (0..36).find(|&slot| ItemPlace::PlayerSlot { player, slot }.stack_owned(host).is_some_and(|s| sid(&s) == TOOL))
}

/// A click on the toolbelt beside an AE2 machine's screen: PICKUP between that menu's cursor and the carried tool's
/// slot `i` (cards only).
pub(crate) fn belt_click(host: &mut dyn ServerHost, player: u128, i: usize, right: bool) {
    let Some(menu) = host.player(player).and_then(|p| p.container_menu(&*host)) else { return };
    // An AE2 machine's own menu (the inventory's cursor is the item screens').
    if menu.container_id(&*host).is_none_or(|id| id == 0) || !menu.kind(&*host).flatten().is_some_and(|k| k.starts_with("ae2:")) {
        return;
    }
    let Some(slot) = tool_slot(host, player) else { return };
    let place = ItemPlace::PlayerSlot { player, slot };
    let Some(tool) = place.stack_owned(host) else { return };
    let cursor_place = ItemPlace::PlayerCursor { menu };
    let cursor = cursor_place.stack_owned(host);
    let mut items = items_of(&*host, &tool);
    let may = cursor.as_ref().is_none_or(|c| is_card(sid(c)));
    let (cur, new) = pickup(&*host, cursor.clone(), items[i].clone(), right, 64, may);
    if new == items[i] {
        return;
    }
    items[i] = new;
    let tag = toolbox_tag(&*host, &items);
    // The cursor and the tool change together, or neither.
    let _ = host.transaction(|host, tx| {
        if let Some(c) = &cursor
            && cursor_place.extract(host, tx, &ItemResource::of(c), c.count()).unwrap_or(0) != c.count()
        {
            return Err(());
        }
        if crate::cells::edit_stack(&place, host, tx, |t| set_toolbox(t, tag)).is_err() {
            return Err(());
        }
        if let Some(c) = &cur
            && cursor_place.insert(host, tx, &ItemResource::of(c), c.count()).unwrap_or(0) != c.count()
        {
            return Err(());
        }
        Ok(())
    });
}

// ---- Client

/// The toolbox slot at `at` on its screen.
pub(crate) fn slot_at(at: [f32; 2]) -> Option<usize> {
    let (x, y) = (at[0] - SLOTS[0], at[1] - SLOTS[1]);
    if x < 0.0 || y < 0.0 || x >= 54.0 || y >= 54.0 || x % 18.0 >= 16.0 || y % 18.0 >= 16.0 {
        return None;
    }
    Some((y / 18.0) as usize * 3 + (x / 18.0) as usize)
}

/// The toolbox screen (NetworkToolScreen over toolbox.png): its nine slots, the title.
pub(crate) fn draw(host: &dyn ClientHost, d: &mut dyn DrawContext, o: [f32; 2], tool: Option<&ItemStack>, at: [f32; 2], tip: &mut Option<Vec<Text>>) {
    spr(d, "ae2:gui/toolbox", o, [0.0, 0.0], [0.0, 0.0, 176.0, 166.0]);
    d.text(&tr(host, "gui.ae2.NetworkTool", "Network Tool"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
    d.text(&tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 166.0 - 93.0], TEXT, false);
    let items = tool.map(|t| items_of(host, t)).unwrap_or_default();
    for (i, s) in items.iter().enumerate() {
        let p = [SLOTS[0] + (i % 3) as f32 * 18.0, SLOTS[1] + (i / 3) as f32 * 18.0];
        if let Some(s) = s {
            let label = (s.count() > 1).then(|| s.count().to_string());
            d.gui_draw_item_with(s, [o[0] + p[0], o[1] + p[1]], label.as_deref());
            if at[0] >= p[0] && at[0] < p[0] + 16.0 && at[1] >= p[1] && at[1] < p[1] + 16.0 {
                *tip = Some(vec![Text::literal(crate::screens::key_name(host, &key_of(host, s)))]);
            }
        }
        if slot_at(at) == Some(i) {
            d.fill([o[0] + p[0], o[1] + p[1], 16.0, 16.0], 0x80FF_FFFF);
        }
    }
}

/// The network tool the local player carries (the toolbelt shows only then).
fn carried_tool(host: &dyn ClientHost) -> Option<ItemStack> {
    host.local_player()?.inventory_slots_owned(host)?.into_iter().find(|(i, s)| *i < 36 && sid(s) == TOOL).map(|(_, s)| s)
}

/// Where the toolbelt is beside this screen (its bottom right), if the screen has an upgrade panel.
fn belt_rect(screen: &ScreenView<'_>) -> Option<[f32; 4]> {
    let menu = crate::bus::menu(screen.menu)?;
    if !BELT_MENUS.contains(&menu) {
        return None;
    }
    let r = screen.rect;
    Some([r[0] + r[2] + 2.0, r[1] + r[3] - BELT[1] - 4.0, BELT[0], BELT[1]])
}

/// ToolboxPanel: the carried tool's nine slots beside the machine's screen.
pub(crate) fn belt_draw(host: &dyn ClientHost, d: &mut dyn DrawContext, screen: &ScreenView<'_>) {
    let Some(r) = belt_rect(screen) else { return };
    let Some(tool) = carried_tool(host) else { return };
    d.fill(r, 0xFF55_5555);
    d.fill([r[0] + 1.0, r[1] + 1.0, r[2] - 2.0, r[3] - 2.0], 0xFFC6_C6C6);
    let items = items_of(host, &tool);
    let m = screen.mouse;
    let mut tip = None;
    for (i, s) in items.iter().enumerate() {
        let p = [r[0] + BELT_INSET + (i % 3) as f32 * 18.0, r[1] + BELT_INSET + (i / 3) as f32 * 18.0];
        d.fill([p[0] - 1.0, p[1] - 1.0, 18.0, 18.0], 0xFF37_3737);
        d.fill([p[0], p[1], 17.0, 17.0], 0xFFFF_FFFF);
        d.fill([p[0], p[1], 16.0, 16.0], 0xFF8B_8B8B);
        if let Some(s) = s {
            let label = (s.count() > 1).then(|| s.count().to_string());
            d.gui_draw_item_with(s, p, label.as_deref());
        }
        if m[0] >= p[0] && m[0] < p[0] + 16.0 && m[1] >= p[1] && m[1] < p[1] + 16.0 {
            d.fill([p[0], p[1], 16.0, 16.0], 0x80FF_FFFF);
            if let Some(s) = s {
                tip = Some(vec![Text::literal(crate::screens::key_name(host, &key_of(host, s)))]);
            }
        }
    }
    if let Some(lines) = tip {
        let _ = d.gui_tooltip(&lines, m);
    }
}

/// A press on the toolbelt: its slot's click for the server (sent at the next client tick).
pub(crate) fn belt_pressed(host: &dyn ClientHost, screen: &ScreenView<'_>, button: i32) -> Option<String> {
    let r = belt_rect(screen)?;
    carried_tool(host)?;
    let m = screen.mouse;
    if !(m[0] >= r[0] && m[0] < r[0] + r[2] && m[1] >= r[1] && m[1] < r[1] + r[3]) {
        return None;
    }
    let (x, y) = (m[0] - r[0] - BELT_INSET, m[1] - r[1] - BELT_INSET);
    if x < 0.0 || y < 0.0 || x >= 54.0 || y >= 54.0 || !(0..=1).contains(&button) {
        return Some(String::new());
    }
    let i = (y / 18.0) as usize * 3 + (x / 18.0) as usize;
    Some(format!("belt {i} {}", u8::from(button == 1)))
}

/// The toolbelt's area beside the open screen (recipe viewers keep clear of it).
pub(crate) fn belt_area(host: &dyn ClientHost, screen: &ScreenView<'_>) -> Option<[f32; 4]> {
    carried_tool(host)?;
    belt_rect(screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbox_slots() {
        assert_eq!(slot_at([62.0, 17.0]), Some(0));
        assert_eq!(slot_at([62.0 + 36.0, 17.0 + 36.0]), Some(8));
        assert_eq!(slot_at([62.0 + 17.0, 17.0]), None);
        assert_eq!(slot_at([61.0, 17.0]), None);
        assert!(is_card("ae2:energy_card") && is_card("ae2:speed_card") && !is_card("ae2:network_tool") && !is_card("minecraft:card"));
    }
}
