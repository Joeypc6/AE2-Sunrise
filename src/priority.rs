//! AE2's priority dialog (PriorityScreen, IPriorityHost) for the ME drive, the ME chest, the storage bus, the formation
//! plane and the interface: a tab at the screen's top right opens a panel over it with the same steps as the pattern
//! provider's (`craft_screens`), +1 +10 +100 +1000 and back.
//!
//! The value lives in the block's (or hosted part's) saved `priority`, which storage already reads
//! (`storage::holder_priority`, the storage buses' and planes' ticks). A step is a menu button (`PRIO + i`), worked
//! out on the server from what is saved there, never from a number the client sends; the server answers each step and
//! each `PRIO_QUERY` with the value on `ae2:priority`.

use crate::screens::{spr, tr};
use crate::util::*;
use mod_api::*;
use std::sync::Mutex;

pub(crate) const NET_PRIORITY: &str = "ae2:priority";
/// Menu buttons: `PRIO + i` steps up by 1, 10, 100, 1000 (i 0..3) or down (i 4..7); `PRIO_QUERY` asks for the value.
pub(crate) const PRIO: i32 = 1 << 29;
pub(crate) const PRIO_QUERY: i32 = PRIO | 15;
const STEP_SIZES: [i64; 4] = [1, 10, 100, 1000];
const STEPS: [(f32, f32); 4] = [(20.0, 22.0), (48.0, 28.0), (82.0, 32.0), (120.0, 38.0)];
const TAB: [f32; 4] = [154.0, 0.0, 22.0, 22.0];
const PANEL: [f32; 4] = [0.0, 22.0, 176.0, 107.0];
const TEXT: u32 = 0xFF40_4040;

/// The blocks (and hosted parts) with a priority (IPriorityHost).
pub(crate) fn has_priority(block: &str) -> bool {
    matches!(block, "ae2:drive" | "ae2:chest" | "ae2:storage_bus" | "ae2:formation_plane" | "ae2:interface" | "ae2:cable_interface")
}

/// Whether a menu button id is one of the dialog's (any id in its range, valid or not, is the dialog's to refuse).
pub(crate) fn is_priority_button(id: i32) -> bool {
    id & !0xF == PRIO
}

/// The value after button `id` from `current` (None for no change): steps saturate at the ends of `i32`, as
/// PriorityScreen's number field holds an int.
pub(crate) fn step(current: i64, id: i32) -> Option<i64> {
    if !is_priority_button(id) {
        return None;
    }
    let i = (id & 0xF) as usize;
    let delta = match i {
        0..=3 => STEP_SIZES[i],
        4..=7 => -STEP_SIZES[i - 4],
        _ => return None,
    };
    let cur = current.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    Some((cur + delta).clamp(i64::from(i32::MIN), i64::from(i32::MAX)))
}

// ---- Server

/// A dialog button on `block`'s menu, on its fields (`fields` is the block's, or the hosted part's side): true when the
/// id was the dialog's (taken, whether it changed anything or not).
pub(crate) fn menu_button(host: &mut dyn ServerHost, player: u128, block: &str, id: i32, fields: &mut Fields) -> bool {
    if !is_priority_button(id) {
        return false;
    }
    if !has_priority(block) {
        return true;
    }
    let current = nbt_i64(&fields.nbt, "priority").clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    let value = match step(current, id) {
        Some(v) if v != current => {
            set_nbt_i64(&mut fields.nbt, "priority", v);
            // The grid's cells were mounted with the old one: mount them again (a storage bus or plane reads its
            // priority each tick).
            if matches!(block, "ae2:drive" | "ae2:chest") {
                crate::storage::remount(host.dimension());
            }
            v
        }
        _ => current,
    };
    let _ = send_to_player(host, player, NET_PRIORITY, value.to_string().as_bytes());
    true
}

// ---- Client

#[derive(Default)]
struct PrioClient {
    /// The block whose screen drew the tab last ("" when the open screen has none).
    block: String,
    open: bool,
    value: Option<i64>,
    queued: Vec<i32>,
}

static CLIENT: Mutex<Option<PrioClient>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut PrioClient) -> R) -> R {
    let mut g = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(PrioClient::default))
}

fn inside(at: [f32; 2], r: [f32; 4]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// Whether `block`'s open screen shows the tab (the ME chest's own terminal doesn't).
fn shows_tab(block: &str) -> bool {
    has_priority(block) && !(block == "ae2:chest" && crate::screens::chest_term_open())
}

/// `screen_draw`, after the screen: the tab (Icon.WRENCH over a button).
pub(crate) fn draw_tab(block: &str, d: &mut dyn DrawContext, o: [f32; 2]) {
    let shown = shows_tab(block);
    with(|c| {
        if c.block != block {
            c.open = false;
            c.value = None;
        }
        c.block = if shown { block.to_owned() } else { String::new() };
    });
    if !shown {
        return;
    }
    d.gui_nine_slice("widget/button", [o[0] + TAB[0], o[1] + TAB[1], TAB[2], TAB[3]], 0xFFFF_FFFF);
    d.sprite("ae2:gui/states", [32.0, 64.0, 16.0, 16.0], [o[0] + 157.0, o[1] + 3.0, 16.0, 16.0], None);
}

/// `screen_click` on the tab: it opens the panel and asks the server for the value.
pub(crate) fn click_tab(block: &str, at: [f32; 2]) -> Option<Option<i32>> {
    if !shows_tab(block) || !inside(at, TAB) {
        return None;
    }
    with(|c| c.open = true);
    Some(Some(PRIO_QUERY))
}

pub(crate) fn tooltip(host: &dyn ClientHost, block: &str, at: [f32; 2], lines: &mut Vec<String>) -> bool {
    if !shows_tab(block) {
        return false;
    }
    if inside(at, TAB) {
        lines.push(tr(host, "gui.ae2.Priority", "Priority"));
        return true;
    }
    // Under the open panel nothing else shows.
    with(|c| c.open) && inside(at, PANEL)
}

fn ours(screen: &ScreenView<'_>) -> bool {
    screen.menu.is_some_and(|m| m.starts_with("ae2:")) && with(|c| !c.block.is_empty())
}

/// The panel, over the screen and its slots (`screen_render_post`).
pub(crate) fn render_post(host: &dyn ClientHost, d: &mut dyn DrawContext, screen: &ScreenView<'_>) {
    if !ours(screen) || !with(|c| c.open) {
        return;
    }
    let o = [screen.rect[0], screen.rect[1]];
    spr(d, "ae2:gui/craft_amt", o, [PANEL[0], PANEL[1]], [0.0, 0.0, PANEL[2], PANEL[3]]);
    d.text(&tr(host, "gui.ae2.Priority", "Priority"), [o[0] + 8.0, o[1] + 28.0], TEXT, false);
    for (i, (x, w)) in STEPS.iter().enumerate() {
        button(d, [o[0] + x, o[1] + 52.0, *w, 20.0], &format!("+{}", STEP_SIZES[i]));
        button(d, [o[0] + x, o[1] + 94.0, *w, 20.0], &format!("-{}", STEP_SIZES[i]));
    }
    d.fill([o[0] + 54.0, o[1] + 76.0, 72.0, 14.0], 0xFFFF_FFFF);
    d.fill([o[0] + 55.0, o[1] + 77.0, 70.0, 12.0], 0xFF00_0000);
    let text = with(|c| c.value).map_or_else(|| "...".to_owned(), |v| v.to_string());
    d.text(&text, [o[0] + 57.0, o[1] + 79.0], 0xFFFF_FFFF, false);
    d.text(&tr(host, "gui.ae2.PriorityInsertionHint", "Insertion: Higher priority first"), [o[0] + 8.0, o[1] + 117.0], TEXT, false);
}

fn button(d: &mut dyn DrawContext, r: [f32; 4], label: &str) {
    d.gui_nine_slice("widget/button", r, 0xFFFF_FFFF);
    let w = d.text_width(label);
    d.text(label, [r[0] + (r[2] - w) / 2.0, r[1] + (r[3] - 8.0) / 2.0], 0xFFFF_FFFF, true);
}

/// A press while the panel is open: its steps, the tab closes it; the panel takes every press over it.
pub(crate) fn pressed(screen: &ScreenView<'_>) -> bool {
    if !ours(screen) || !with(|c| c.open) {
        return false;
    }
    let at = [screen.mouse[0] - screen.rect[0], screen.mouse[1] - screen.rect[1]];
    if inside(at, TAB) {
        with(|c| c.open = false);
        return true;
    }
    for (i, (x, w)) in STEPS.iter().enumerate() {
        if inside(at, [*x, 52.0, *w, 20.0]) {
            with(|c| c.queued.push(PRIO + i as i32));
            return true;
        }
        if inside(at, [*x, 94.0, *w, 20.0]) {
            with(|c| c.queued.push(PRIO + 4 + i as i32));
            return true;
        }
    }
    inside(at, PANEL)
}

/// Escape closes the panel first.
pub(crate) fn key(screen: &ScreenView<'_>, key: &str) -> bool {
    if key == "key.keyboard.escape" && ours(screen) && with(|c| c.open) {
        with(|c| c.open = false);
        return true;
    }
    false
}

pub(crate) fn closing() {
    with(|c| *c = PrioClient::default());
}

/// The client's tick: the steps pressed, as menu buttons.
pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    let queued = with(|c| std::mem::take(&mut c.queued));
    if queued.is_empty() {
        return;
    }
    let Some(menu) = host.local_player().and_then(|p| p.container_menu(&*host)) else { return };
    for b in queued {
        menu.click_button(host, b);
    }
}

pub(crate) fn from_server(channel: &str, data: &[u8]) -> bool {
    if channel != NET_PRIORITY {
        return false;
    }
    let v = std::str::from_utf8(data).ok().and_then(|s| s.trim().parse::<i64>().ok());
    if let Some(v) = v {
        with(|c| c.value = Some(v.clamp(i64::from(i32::MIN), i64::from(i32::MAX))));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_and_limits() {
        assert_eq!(step(0, PRIO), Some(1));
        assert_eq!(step(0, PRIO + 3), Some(1000));
        assert_eq!(step(5, PRIO + 5), Some(-5));
        assert_eq!(step(i64::from(i32::MAX), PRIO + 3), Some(i64::from(i32::MAX)));
        assert_eq!(step(i64::from(i32::MIN), PRIO + 7), Some(i64::from(i32::MIN)));
        // A saved value past an int (an old world, a hand edit) comes back in range.
        assert_eq!(step(i64::MAX, PRIO), Some(i64::from(i32::MAX)));
        // The query and unknown ids change nothing.
        assert_eq!(step(7, PRIO_QUERY), None);
        assert_eq!(step(7, PRIO + 9), None);
        assert_eq!(step(7, 3), None);
    }

    #[test]
    fn button_range() {
        assert!(is_priority_button(PRIO));
        assert!(is_priority_button(PRIO_QUERY));
        assert!(!is_priority_button(PRIO | 16));
        assert!(!is_priority_button(crate::autocraft::PP_PRIORITY));
        assert!(!is_priority_button(-1));
    }
}
