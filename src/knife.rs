//! AE2's cutting knives (QuartzCuttingKnifeItem, QuartzKnifeMenu, QuartzKnifeScreen): a use opens the knife's screen
//! (quartzknife.png, AE2's quartz_knife.json layout): a name box (32 characters), an input slot for one metal ingot
//! (`ae2:metal_ingots`), the output slot showing a Name Press of the typed name (`InscribeName`, which the inscriber
//! prints), and the inventory. Taking the press uses the ingot and wears the knife one point; what is left in the input
//! comes back on close. Upstream's `is_knife`, `knife_tick` and its "knife" item screen (`../ae2-horizon/mixins/ae2/me.rs`).
//!
//! The screen is the mod's own (`ae2:cutting_knife`, Sunrise shows a mod's menu only over a block); every slot and the
//! name are the server's: the knife is checked in its hand slot before each operation and every tick (moved or broken,
//! the screen closes), the cursor is the inventory menu's (taken off and put back around each operation), the ingot
//! kept by the session, never in two places.

use crate::cells::*;
use crate::screens::{spr, tr};
use crate::util::*;
use crate::wireless::{give_back, pickup, put_cursor, take_cursor};
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) const KNIVES: [&str; 2] = [
    "ae2:certus_quartz_cutting_knife",
    "ae2:nether_quartz_cutting_knife",
];
pub(crate) const SCREEN: &str = "ae2:cutting_knife";
const NAME_PRESS: &str = "ae2:name_press";
/// QuartzKnifeScreen: `name.setMaxLength(32)`.
pub(crate) const NAME_MAX: usize = 32;
const TEXT: u32 = 0xFF40_4040;
/// quartz_knife.json: the window, the name box, the slots, the inventory's top.
const SIZE: [f32; 2] = [176.0, 184.0];
const NAME_BOX: [f32; 4] = [21.0, 30.0, 90.0, 12.0];
const INPUT: [f32; 2] = [94.0, 44.0];
const OUTPUT: [f32; 2] = [134.0, 44.0];
const INV_Y: f32 = 102.0;

/// The name as the menu keeps it (pure: unit tested): control characters out, 32 characters at most.
pub(crate) fn clean_name(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX)
        .collect()
}

/// QuartzKniveSlot.getItem's rule (pure): a press only with an ingot in and a name typed.
pub(crate) fn makes_press(has_ingot: bool, name: &str) -> bool {
    has_ingot && !name.is_empty()
}

// ---- Server

struct Session {
    hand: Hand,
    slot: i32,
    item: &'static str,
    input: Option<ItemStack>,
    name: String,
}

static SESSIONS: Mutex<Option<HashMap<u128, Session>>> = Mutex::new(None);

fn with_sessions<R>(f: impl FnOnce(&mut HashMap<u128, Session>) -> R) -> R {
    let mut g = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

pub(crate) fn forget_all() {
    *SESSIONS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(crate) fn logged_out(player: u128) {
    with_sessions(|s| s.remove(&player));
}

fn is_metal_ingot(host: &dyn Host, stack: &ItemStack) -> bool {
    let tags = host.tags();
    Identifier::parse("ae2:metal_ingots")
        .ok()
        .and_then(|id| tags.key(&id))
        .is_some_and(|t| tags.is_in_tag(stack.item(), t) == Some(true))
}

fn press(host: &dyn Host, name: &str) -> Option<ItemStack> {
    let mut s = new_stack(host, NAME_PRESS, 1)?;
    edit_custom_data(&mut s, |d| {
        d.insert(java("InscribeName"), Tag::String(java(name)));
    });
    Some(s)
}

fn stack_hex(host: &dyn Host, s: &ItemStack) -> String {
    s.to_nbt(host)
        .and_then(|c| Tag::Compound(c).to_network_bytes().ok())
        .map(|b| b.iter().map(|x| format!("{x:02x}")).collect())
        .unwrap_or_default()
}

fn stack_from_hex(host: &dyn Host, h: &str) -> Option<ItemStack> {
    if h.len() % 2 != 0 {
        return None;
    }
    let b: Option<Vec<u8>> = (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok())
        .collect();
    match Tag::from_network_bytes(&b?, None).ok()?.0 {
        Tag::Compound(c) => ItemStack::from_nbt(host, &c),
        _ => None,
    }
}

/// The knife still in the slot it opened from.
fn knife_there(host: &mut dyn ServerHost, player: u128, slot: i32, item: &str) -> bool {
    ItemPlace::PlayerSlot { player, slot }
        .stack_owned(host)
        .is_some_and(|s| sid(&s) == item)
}

/// QuartzCuttingKnifeItem.use: the knife's screen opens (a screen open before closes first).
pub(crate) fn open(host: &mut dyn ServerHost, player: u128, hand: Hand) {
    close(host, player, false);
    let slot = match hand {
        Hand::Main => host
            .player(player)
            .and_then(|p| p.selected_slot(&*host))
            .unwrap_or(0) as i32,
        Hand::Off => -106,
    };
    let Some(stack) = ItemPlace::PlayerSlot { player, slot }.stack_owned(host) else {
        return;
    };
    let item = sid(&stack);
    if !KNIVES.contains(&item) {
        return;
    }
    with_sessions(|s| {
        s.insert(
            player,
            Session {
                hand,
                slot,
                item,
                input: None,
                name: String::new(),
            },
        )
    });
    let _ = send_to_player(
        host,
        player,
        crate::handtools::NET_TOOLS,
        format!("kopen {slot}").as_bytes(),
    );
    send_state(host, player);
}

/// QuartzKnifeMenu.removed: the input back to the player, the cursor into the inventory.
fn close(host: &mut dyn ServerHost, player: u128, tell: bool) {
    let Some(s) = with_sessions(|m| m.remove(&player)) else {
        return;
    };
    if let Some(i) = s.input {
        give_back(host, player, i);
    }
    if let Ok(Some(c)) = take_cursor(host, player) {
        give_back(host, player, c);
    }
    if tell {
        let _ = send_to_player(host, player, crate::handtools::NET_TOOLS, b"kclose");
    }
}

fn send_state(host: &mut dyn ServerHost, player: u128) {
    let Some(input) = with_sessions(|m| m.get(&player).map(|s| s.input.clone())) else {
        return;
    };
    let cursor = host
        .player(player)
        .and_then(|p| p.container_menu(&*host))
        .filter(|m| m.container_id(&*host) == Some(0))
        .and_then(|m| ItemPlace::PlayerCursor { menu: m }.stack_owned(host));
    let hex = |s: &Option<ItemStack>| {
        s.as_ref()
            .map_or_else(|| "-".to_owned(), |s| stack_hex(&*host, s))
    };
    let msg = format!("kstate {} {}", hex(&input), hex(&cursor));
    let _ = send_to_player(host, player, crate::handtools::NET_TOOLS, msg.as_bytes());
}

/// Every tick: the knife still where it was, else the screen closes.
pub(crate) fn tick(host: &mut dyn ServerHost, player: u128) {
    let Some((slot, item)) = with_sessions(|m| m.get(&player).map(|s| (s.slot, s.item))) else {
        return;
    };
    if !knife_there(host, player, slot, item) {
        close(host, player, true);
    }
}

/// A message from the knife's screen (the player's tick).
pub(crate) fn message(host: &mut dyn ServerHost, player: u128, text: &str) {
    let Some((slot, item, hand)) =
        with_sessions(|m| m.get(&player).map(|s| (s.slot, s.item, s.hand)))
    else {
        return;
    };
    if text == "kclose" {
        close(host, player, false);
        return;
    }
    if !knife_there(host, player, slot, item) {
        close(host, player, true);
        return;
    }
    let w: Vec<&str> = text.split_whitespace().collect();
    let flag = |i: usize| w.get(i) == Some(&"1");
    match w.first().copied() {
        Some("kname") => {
            let name = clean_name(
                text.strip_prefix("kname")
                    .unwrap_or("")
                    .strip_prefix(' ')
                    .unwrap_or(""),
            );
            with_sessions(|m| {
                if let Some(s) = m.get_mut(&player) {
                    s.name = name;
                }
            });
            return;
        }
        Some("kin") => input_click(host, player, flag(1)),
        Some("kout") => output_click(host, player, hand, flag(1)),
        Some("kinv") => {
            if let Some(i) = w.get(1).and_then(|v| v.parse::<i32>().ok()) {
                inventory_click(host, player, slot, i, flag(2), flag(3));
            }
        }
        _ => return,
    }
    send_state(host, player);
}

/// The input slot (RestrictedInputSlot of METAL_INGOTS, one item): PICKUP with the cursor.
fn input_click(host: &mut dyn ServerHost, player: u128, right: bool) {
    let Ok(cursor) = take_cursor(host, player) else {
        return;
    };
    let input = with_sessions(|m| m.get(&player).map(|s| s.input.clone())).flatten();
    let may = cursor.as_ref().is_none_or(|c| is_metal_ingot(&*host, c));
    let (cur, new) = pickup(&*host, cursor, input, right, 1, may);
    with_sessions(|m| {
        if let Some(s) = m.get_mut(&player) {
            s.input = new;
        }
    });
    put_cursor(host, player, cur);
}

/// QuartzKniveSlot: the press onto the cursor (or with shift into the inventory); the ingot goes and the knife wears.
fn output_click(host: &mut dyn ServerHost, player: u128, hand: Hand, shift: bool) {
    let Some((input, name)) =
        with_sessions(|m| m.get(&player).map(|s| (s.input.clone(), s.name.clone())))
    else {
        return;
    };
    let ingot = input.as_ref().is_some_and(|i| is_metal_ingot(&*host, i));
    if !makes_press(ingot, &name) {
        return;
    }
    let Some(out) = press(&*host, &name) else {
        return;
    };
    if shift {
        give_back(host, player, out);
    } else {
        let Ok(cursor) = take_cursor(host, player) else {
            return;
        };
        let cur = match cursor {
            None => Some(out),
            Some(c) if c.is_stackable_with(&out) && c.count() < max_stack(&*host, &c) => {
                with_count(&c, c.count() + 1)
            }
            Some(c) => {
                put_cursor(host, player, Some(c));
                return;
            }
        };
        put_cursor(host, player, cur);
    }
    // makePlate: one ingot used, the knife worn a point (it may break).
    with_sessions(|m| {
        if let Some(s) = m.get_mut(&player) {
            s.input = s
                .input
                .as_ref()
                .and_then(|i| with_count(i, i.count().saturating_sub(1)));
        }
    });
    if let Some(p) = host.player(player) {
        let slot = if hand == Hand::Main {
            EquipmentSlot::MainHand
        } else {
            EquipmentSlot::OffHand
        };
        let _ = LivingRef::new(p.entity()).hurt_and_break(host, slot, 1);
    }
}

/// A click on the player's inventory (0..36) on the knife's screen: PICKUP with the cursor, or with shift an ingot into
/// the empty input slot. The knife's own slot takes no click (AE2 locks it).
fn inventory_click(
    host: &mut dyn ServerHost,
    player: u128,
    own: i32,
    slot: i32,
    right: bool,
    shift: bool,
) {
    if !(0..36).contains(&slot) || slot == own {
        return;
    }
    let Some(p) = host.player(player) else { return };
    let place = ItemPlace::PlayerSlot { player, slot };
    if shift {
        let Some(st) = place.stack_owned(host) else { return };
        let empty = with_sessions(|m| m.get(&player).is_some_and(|s| s.input.is_none()));
        if !empty || !is_metal_ingot(&*host, &st) {
            return;
        }
        if p.set_inventory_slot(host, slot as u32, with_count(&st, st.count() - 1))
            .is_err()
        {
            return;
        }
        with_sessions(|m| {
            if let Some(s) = m.get_mut(&player) {
                s.input = with_count(&st, 1);
            }
        });
        return;
    }
    let Ok(cursor) = take_cursor(host, player) else {
        return;
    };
    let there = place.stack_owned(host);
    let (cur, new) = pickup(&*host, cursor.clone(), there.clone(), right, 99, true);
    if new != there && p.set_inventory_slot(host, slot as u32, new).is_err() {
        put_cursor(host, player, cursor);
        return;
    }
    put_cursor(host, player, cur);
}

/// The knives' Part: a use (in the air, or on a block that lets it through) opens the screen.
pub(crate) struct Knife;

#[mod_api::sunrise_part]
impl ItemPart for Knife {
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        if let HostSide::Server(host) = host {
            crate::cells::ids(&*host);
            open(host, event.player, event.hand);
        }
        Some(InteractionResult::Success)
    }
}

// ---- Client

#[derive(Default)]
struct Ui {
    shown: bool,
    slot: i32,
    name: String,
    focus: bool,
    input: Option<ItemStack>,
    cursor: Option<ItemStack>,
}

static UI: Mutex<Option<Ui>> = Mutex::new(None);

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> R {
    let mut g = UI.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Ui::default))
}

pub(crate) fn is_open(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(SCREEN)
}

pub(crate) fn from_server(host: &mut dyn ClientHost, text: &str) {
    let w: Vec<&str> = text.split_whitespace().collect();
    match w.first().copied() {
        Some("kopen") => {
            let slot = w.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
            with_ui(|u| {
                *u = Ui {
                    slot,
                    focus: true,
                    ..Ui::default()
                }
            });
            let open = is_open(&*host) || host.mod_screen_open(SCREEN);
            with_ui(|u| u.shown = open);
            if !open {
                crate::handtools::send(host, "kclose");
            }
        }
        Some("kclose") => {
            let was = with_ui(|u| std::mem::replace(&mut u.shown, false));
            if was && is_open(&*host) {
                host.close_screen();
            }
        }
        Some("kstate") => {
            let read = |h: Option<&&str>| {
                h.filter(|h| **h != "-")
                    .and_then(|h| stack_from_hex(&*host, h))
            };
            let (input, cursor) = (read(w.get(1)), read(w.get(2)));
            with_ui(|u| {
                u.input = input;
                u.cursor = cursor;
            });
        }
        _ => {}
    }
}

/// The screen closed (Escape, another screen): the server is told.
pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    let gone = with_ui(|u| u.shown) && !is_open(&*host);
    if gone {
        with_ui(|u| *u = Ui::default());
        crate::handtools::send(host, "kclose");
    }
}

fn origin() -> [f32; 2] {
    let g = crate::screens::gui();
    [
        ((g[0] - SIZE[0]) / 2.0).floor(),
        ((g[1] - SIZE[1]) / 2.0).floor(),
    ]
}

fn inside(at: [f32; 2], r: [f32; 4]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// The inventory slot (hotbar 0..9, then the three rows) at `at`.
pub(crate) fn inventory_slot_at(at: [f32; 2]) -> Option<i32> {
    let (cx, cy) = ((at[0] - 8.0) / 18.0, at[1] - INV_Y);
    if !(0.0..9.0).contains(&cx) || (at[0] - 8.0) % 18.0 >= 16.0 {
        return None;
    }
    if (0.0..54.0).contains(&cy) && cy % 18.0 < 16.0 {
        Some(9 + (cy / 18.0) as i32 * 9 + cx as i32)
    } else if (58.0..74.0).contains(&cy) {
        Some(cx as i32)
    } else {
        None
    }
}

fn inventory_slot_pos(slot: i32) -> [f32; 2] {
    if slot < 9 {
        [8.0 + 18.0 * slot as f32, INV_Y + 58.0]
    } else {
        [
            8.0 + 18.0 * ((slot - 9) % 9) as f32,
            INV_Y + 18.0 * ((slot - 9) / 9) as f32,
        ]
    }
}

fn draw_stack(d: &mut dyn DrawContext, s: &ItemStack, at: [f32; 2]) {
    let label = (s.count() > 1).then(|| s.count().to_string());
    d.gui_draw_item_with(s, at, label.as_deref());
}

fn lines(host: &dyn ClientHost, s: &ItemStack) -> Vec<Text> {
    let mut out: Vec<Text> = ItemResource::of(s)
        .tooltip(host, false, false)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|l| match l {
            TooltipElement::Text(t, _) => Some(t),
            _ => None,
        })
        .collect();
    if out.is_empty() {
        out.push(Text::literal(crate::screens::key_name(
            host,
            &key_of(host, s),
        )));
    }
    out
}

/// QuartzKnifeScreen: the window, the name box, the slots, the inventory, the cursor and the tooltip.
pub(crate) fn draw(host: &dyn ClientHost, d: &mut dyn DrawContext, screen: &ScreenView<'_>) {
    let o = origin();
    let m = screen.mouse;
    let at = [m[0] - o[0], m[1] - o[1]];
    let size = d.size();
    d.fill_gradient([0.0, 0.0, size[0], size[1]], 0xC010_1010, 0xD010_1010);
    spr(
        d,
        "ae2:gui/quartzknife",
        o,
        [0.0, 0.0],
        [0.0, 0.0, SIZE[0], SIZE[1]],
    );
    d.text(
        &tr(host, "gui.ae2.QuartzCuttingKnife", "Quartz Cutting Knife"),
        [o[0] + 8.0, o[1] + 6.0],
        TEXT,
        false,
    );
    d.text(
        &tr(host, "container.inventory", "Inventory"),
        [o[0] + 8.0, o[1] + INV_Y - 11.0],
        TEXT,
        false,
    );
    let (name, focus, input, cursor, own) = with_ui(|u| {
        (
            u.name.clone(),
            u.focus,
            u.input.clone(),
            u.cursor.clone(),
            u.slot,
        )
    });
    // The name box (an AE2 text field: white on black, the caret while focused).
    let [bx, by, bw, bh] = NAME_BOX;
    d.fill(
        [o[0] + bx - 1.0, o[1] + by - 1.0, bw + 2.0, bh + 2.0],
        if focus { 0xFFFF_FFFF } else { 0xFFA0_A0A0 },
    );
    d.fill([o[0] + bx, o[1] + by, bw, bh], 0xFF00_0000);
    let shown = if focus {
        format!("{name}_")
    } else {
        name.clone()
    };
    d.text(
        &shown,
        [o[0] + bx + 2.0, o[1] + by + 2.0],
        0xFFE0_E0E0,
        true,
    );
    let mut tip: Option<Vec<Text>> = None;
    if let Some(i) = &input {
        draw_stack(d, i, [o[0] + INPUT[0], o[1] + INPUT[1]]);
        if inside(at, [INPUT[0], INPUT[1], 16.0, 16.0]) {
            tip = Some(lines(host, i));
        }
    }
    if makes_press(input.is_some(), &name)
        && let Some(mut p) = new_stack(host, NAME_PRESS, 1)
    {
        edit_custom_data(&mut p, |c| {
            c.insert(java("InscribeName"), Tag::String(java(&name)));
        });
        draw_stack(d, &p, [o[0] + OUTPUT[0], o[1] + OUTPUT[1]]);
        if inside(at, [OUTPUT[0], OUTPUT[1], 16.0, 16.0]) {
            tip = Some(lines(host, &p));
        }
    }
    for r in [[INPUT[0], INPUT[1]], [OUTPUT[0], OUTPUT[1]]] {
        if inside(at, [r[0], r[1], 16.0, 16.0]) {
            d.fill([o[0] + r[0], o[1] + r[1], 16.0, 16.0], 0x80FF_FFFF);
        }
    }
    if let Some(inv) = host.local_player().and_then(|p| p.inventory_slots_owned(host)) {
        for (i, s) in inv.iter().filter(|(i, _)| *i < 36) {
            let p = inventory_slot_pos(*i as i32);
            draw_stack(d, s, [o[0] + p[0], o[1] + p[1]]);
            if inside(at, [p[0], p[1], 16.0, 16.0]) {
                tip = Some(lines(host, s));
            }
        }
    }
    if (0..36).contains(&own) {
        let p = inventory_slot_pos(own);
        d.fill([o[0] + p[0], o[1] + p[1], 16.0, 16.0], 0x80_202020);
    }
    if let Some(slot) = inventory_slot_at(at) {
        let p = inventory_slot_pos(slot);
        d.fill([o[0] + p[0], o[1] + p[1], 16.0, 16.0], 0x80FF_FFFF);
    }
    if let Some(c) = cursor {
        draw_stack(d, &c, [m[0] - 8.0, m[1] - 8.0]);
        tip = None;
    }
    if let Some(l) = tip {
        let _ = d.gui_tooltip(&l, m);
    }
}

/// A press on the knife's screen: the name box takes the focus; slots' clicks go to the server.
pub(crate) fn pressed(host: &dyn ClientHost, screen: &ScreenView<'_>, button: i32) {
    if !(0..=1).contains(&button) {
        return;
    }
    let o = origin();
    let at = [screen.mouse[0] - o[0], screen.mouse[1] - o[1]];
    let right = button == 1;
    let shift =
        host.is_key_down("key.keyboard.left.shift") || host.is_key_down("key.keyboard.right.shift");
    let b = |x: bool| u8::from(x);
    let in_box = inside(at, NAME_BOX);
    with_ui(|u| u.focus = in_box);
    if in_box {
        // A right click clears the box, as AE2's text fields do.
        if right {
            with_ui(|u| u.name.clear());
            crate::handtools::queue("kname ".to_owned());
        }
        return;
    }
    if inside(at, [INPUT[0], INPUT[1], 16.0, 16.0]) {
        crate::handtools::queue(format!("kin {}", b(right)));
    } else if inside(at, [OUTPUT[0], OUTPUT[1], 16.0, 16.0]) {
        crate::handtools::queue(format!("kout {}", b(shift)));
    } else if let Some(slot) = inventory_slot_at(at) {
        crate::handtools::queue(format!("kinv {slot} {} {}", b(right), b(shift)));
    }
}

/// Keys while the name box has the focus (Escape still closes).
pub(crate) fn key(key: &str) -> bool {
    if !with_ui(|u| u.focus) || key == "key.keyboard.escape" {
        return false;
    }
    match key {
        "key.keyboard.backspace" => {
            let name = with_ui(|u| {
                u.name.pop();
                u.name.clone()
            });
            crate::handtools::queue(format!("kname {name}"));
        }
        "key.keyboard.enter" | "key.keyboard.tab" => with_ui(|u| u.focus = false),
        _ => {}
    }
    true
}

pub(crate) fn typed(c: char) -> bool {
    if !with_ui(|u| u.focus) {
        return false;
    }
    if c.is_control() {
        return true;
    }
    let name = with_ui(|u| {
        if u.name.chars().count() < NAME_MAX {
            u.name.push(c);
        }
        u.name.clone()
    });
    crate::handtools::queue(format!("kname {name}"));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(clean_name("Fluix\n Press"), "Fluix Press");
        assert_eq!(clean_name(&"a".repeat(40)).len(), 32);
        assert!(makes_press(true, "x") && !makes_press(true, "") && !makes_press(false, "x"));
    }

    #[test]
    fn knife_screen_slots() {
        assert_eq!(inventory_slot_at([8.0, INV_Y + 58.0]), Some(0));
        assert_eq!(inventory_slot_at([9.0, INV_Y + 1.0]), Some(9));
        for slot in 0..36 {
            let p = inventory_slot_pos(slot);
            assert_eq!(inventory_slot_at([p[0] + 1.0, p[1] + 1.0]), Some(slot));
        }
        assert_eq!(inventory_slot_at([7.0, INV_Y]), None);
    }
}
