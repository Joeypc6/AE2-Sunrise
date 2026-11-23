//! The terminal's settings screen (TerminalSettingsScreen, from the toolbar's gear) and the finished-job toast
//! (FinishedJobToast), on the client. Upstream's `settings_draw`, `settings_click`, `draw_checkbox` and
//! `job_toast_draw` (`../ae2-horizon/mixins/ae2/me.rs`). The settings are client.json's (`config::update_client`);
//! the two the server acts on (notify about finished jobs, clear the crafting grid on close) are told to it as a
//! terminal opens and when they change.
//!
//! The fork opens no second screen over a menu's (as the crafting dialogs), so the settings are drawn over the terminal
//! in `screen_render_post` and take its clicks and keys while open.

use crate::cells::*;
use crate::config::{self, S_AUTO_FOCUS, S_CLEAR_EXTERNAL, S_CLEAR_GRID, S_EXTERNAL_SEARCH, S_NOTIFY, S_PIN, S_REMEMBER_SEARCH, S_SYNC_EXTERNAL};
use crate::core::{AeKey, AmountFormat, KeyType};
use crate::screens::{key_name, tr};
use mod_api::*;
use crate::util::InventoryCopy;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

const TEXT: u32 = 0xFF40_4040;
/// TerminalSettingsScreen's checkboxes (screens/terminals/terminal_settings.json): the setting, x, y, width, its
/// label's lang key and words, and whether it shows with AE's own search (0), with JEI's (1), or always (2).
const SETTINGS: [(usize, f32, f32, f32, &str, &str, u8); 7] = [
    (S_PIN, 10.0, 25.0, 180.0, "gui.ae2.TerminalSettingsPinAutoCraftedItems", "Pin auto-crafted items to first row", 2),
    (S_NOTIFY, 10.0, 50.0, 180.0, "gui.ae2.TerminalSettingsNotifyForFinishedJobs", "Notify about finished crafting jobs (requires wireless terminal)", 2),
    (S_CLEAR_GRID, 10.0, 82.0, 180.0, "gui.ae2.TerminalSettingsClearGridOnClose", "Automatically clear terminal grid on close (if applicable)", 2),
    (S_REMEMBER_SEARCH, 10.0, 160.0, 150.0, "gui.ae2.SearchSettingsRememberSearch", "Remember last search", 0),
    (S_AUTO_FOCUS, 10.0, 176.0, 150.0, "gui.ae2.SearchSettingsAutoFocus", "Auto-Focus on open", 0),
    (S_SYNC_EXTERNAL, 10.0, 192.0, 150.0, "gui.ae2.SearchSettingsSyncWithExternal", "Sync with %s search", 0),
    (S_CLEAR_EXTERNAL, 10.0, 160.0, 150.0, "gui.ae2.SearchSettingsClearExternal", "Clear %s search on open", 1),
];
/// The two radio buttons, "Use AE" and "Use JEI": x, y, width, lang key, words.
const RADIOS: [(f32, f32, f32, &str, &str); 2] =
    [(10.0, 124.0, 75.0, "gui.ae2.SearchSettingsUseInternalSearch", "Use AE"), (95.0, 124.0, 75.0, "gui.ae2.SearchSettingsUseExternalSearch", "Use %s")];
const SIZE: [f32; 2] = [200.0, 216.0];
const BACK: [f32; 4] = [178.0, 0.0, 22.0, 22.0];
/// FinishedJobToast's time on screen, and its slide in and out.
const TOAST_SECONDS: f32 = 2.5;
const TOAST_SLIDE: f32 = 0.25;
/// The items whose holder hears of finished jobs (PendingCraftingJobs.hasNotificationEnablingItem).
const NOTIFYING_ITEMS: [&str; 2] = ["ae2:wireless_terminal", "ae2:wireless_crafting_terminal"];

#[derive(Default)]
struct Ui {
    settings_open: bool,
    /// Finished jobs waiting for their toast (key, amount), and when the front one began showing.
    toasts: VecDeque<(AeKey, i64)>,
    shown: Option<Instant>,
}

static UI: Mutex<Option<Ui>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Ui) -> R) -> R {
    let mut g = UI.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Ui::default))
}

fn inside(at: [f32; 2], r: [f32; 4]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

// ---- The settings screen

pub(crate) fn open_settings() {
    with(|u| u.settings_open = true);
}

pub(crate) fn settings_open() -> bool {
    with(|u| u.settings_open)
}

/// The settings the server acts on: notify about finished jobs, clear the crafting grid on close.
pub(crate) fn send_prefs() {
    let s = config::config().settings;
    crate::craft_screens::send(format!("prefs {} {}", u8::from(s[S_NOTIFY]), u8::from(s[S_CLEAR_GRID])));
}

fn origin(size: [f32; 2]) -> [f32; 2] {
    [((size[0] - SIZE[0]) / 2.0).floor(), ((size[1] - SIZE[1]) / 2.0).floor()]
}

/// A label's words: `%s` is the item list mod's name (AE2's REI or JEI).
fn label(host: &dyn ClientHost, key: &str, fallback: &str) -> String {
    tr(host, key, fallback).replace("%s", "JEI")
}

/// Whether a setting's checkbox shows for the search the player uses.
fn shown(shows: u8, external: bool) -> bool {
    shows == 2 || (shows == 1) == external
}

/// BackgroundGenerator.draw: background.png's corners, edges and middle stretched to the window's size.
fn draw_background(d: &mut dyn DrawContext, at: [f32; 2], size: [f32; 2]) {
    let [x, y] = at;
    let [w, h] = size;
    let (iw, ih) = (w - 8.0, h - 8.0);
    for (src, dst) in [
        ([0.0, 0.0, 4.0, 4.0], [x, y, 4.0, 4.0]),
        ([252.0, 0.0, 4.0, 4.0], [x + w - 4.0, y, 4.0, 4.0]),
        ([0.0, 252.0, 4.0, 4.0], [x, y + h - 4.0, 4.0, 4.0]),
        ([252.0, 252.0, 4.0, 4.0], [x + w - 4.0, y + h - 4.0, 4.0, 4.0]),
        ([4.0, 0.0, iw, 4.0], [x + 4.0, y, iw, 4.0]),
        ([4.0, 252.0, iw, 4.0], [x + 4.0, y + h - 4.0, iw, 4.0]),
        ([0.0, 4.0, 4.0, ih], [x, y + 4.0, 4.0, ih]),
        ([252.0, 4.0, 4.0, ih], [x + w - 4.0, y + 4.0, 4.0, ih]),
        ([4.0, 4.0, iw, ih], [x + 4.0, y + 4.0, iw, ih]),
    ] {
        d.sprite("ae2:gui/background", src, dst, None);
    }
}

/// AECheckbox: its 14-pixel box from checkbox.png (a radio's beside it, the hovered look 14 across, the checked one
/// 14 down), the label wrapped to the rest of its width.
fn draw_checkbox(d: &mut dyn DrawContext, at: [f32; 2], width: f32, text: &str, on: bool, radio: bool, hover: bool) {
    let u = if radio { 28.0 } else { 0.0 } + if hover { 14.0 } else { 0.0 };
    let v = if on { 14.0 } else { 0.0 };
    d.sprite("ae2:gui/checkbox", [u, v, 14.0, 14.0], [at[0], at[1], 14.0, 14.0], None);
    let mut lines: Vec<String> = Vec::new();
    for word in text.split(' ') {
        match lines.last_mut() {
            Some(line) if d.text_width(&format!("{line} {word}")) <= width - 16.0 => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    let mut y = at[1] + if lines.len() <= 1 { 4.0 } else { 1.0 };
    for line in &lines {
        d.text(line, [at[0] + 16.0, y], TEXT, false);
        y += 9.0;
    }
}

/// TerminalSettingsScreen over the terminal.
pub(crate) fn render_post(host: &dyn ClientHost, d: &mut dyn DrawContext, screen: &ScreenView<'_>) {
    if !settings_open() || !crate::screens::our_menu(screen) {
        return;
    }
    let s = config::config().settings;
    let size = d.size();
    d.fill([0.0, 0.0, size[0], size[1]], 0xA000_0000);
    let o = origin(size);
    let m = screen.mouse;
    let at = [m[0] - o[0], m[1] - o[1]];
    draw_background(d, o, SIZE);
    d.text(&tr(host, "gui.ae2.TerminalSettingsTitle", "Terminal Settings"), [o[0] + 8.0, o[1] + 7.0], TEXT, false);
    d.text(&tr(host, "gui.ae2.SearchSettingsTitle", "Search Settings"), [o[0] + 8.0, o[1] + 110.0], TEXT, false);
    for (i, (x, y, w, key, fallback)) in RADIOS.iter().enumerate() {
        draw_checkbox(d, [o[0] + x, o[1] + y], *w, &label(host, key, fallback), s[S_EXTERNAL_SEARCH] == (i == 1), true, inside(at, [*x, *y, *w, 14.0]));
    }
    for (index, x, y, w, key, fallback, shows) in SETTINGS {
        if shown(shows, s[S_EXTERNAL_SEARCH]) {
            draw_checkbox(d, [o[0] + x, o[1] + y], w, &label(host, key, fallback), s[index], false, inside(at, [x, y, w, 14.0]));
        }
    }
    // The back button: TabButton's BOX style with the terminal's icon.
    let [bx, by, bw, bh] = BACK;
    d.sprite("ae2:gui/states", [160.0, 192.0, bw, bh], [o[0] + bx, o[1] + by, bw, bh], None);
    let icon = crate::screens::menu_block(screen);
    if let Some(st) = new_stack(host, &icon, 1) {
        d.gui_draw_item_with(&st, [o[0] + bx + 3.0, o[1] + by + 3.0], None);
    }
    if inside(at, BACK) {
        let path = icon.split_once(':').map_or(icon.as_str(), |(_, p)| p);
        let _ = d.gui_tooltip(&[Text::literal(tr(host, &format!("block.ae2.{path}"), &icon))], m);
    }
}

/// A press while the settings are open: they take them all. A box changed is saved at once.
pub(crate) fn pressed(host: &dyn ClientHost, screen: &ScreenView<'_>, button: i32) -> bool {
    if !settings_open() || !crate::screens::our_menu(screen) {
        return false;
    }
    if button != 0 {
        return true;
    }
    let o = origin(crate::screens::gui());
    let at = [screen.mouse[0] - o[0], screen.mouse[1] - o[1]];
    if inside(at, BACK) {
        with(|u| u.settings_open = false);
        return true;
    }
    let mut s = config::config().settings;
    let before = s;
    for (i, (x, y, w, _, _)) in RADIOS.iter().enumerate() {
        if inside(at, [*x, *y, *w, 14.0]) {
            s[S_EXTERNAL_SEARCH] = i == 1;
        }
    }
    if s == before {
        for (index, x, y, w, _, _, shows) in SETTINGS {
            if shown(shows, s[S_EXTERNAL_SEARCH]) && inside(at, [x, y, w, 14.0]) {
                s[index] = !s[index];
                break;
            }
        }
    }
    if s != before {
        config::update_client(host, |cfg| cfg.settings = s);
        if s[S_NOTIFY] != before[S_NOTIFY] || s[S_CLEAR_GRID] != before[S_CLEAR_GRID] {
            send_prefs();
        }
        crate::screens::settings_changed();
    }
    true
}

/// Keys while the settings are open: Escape goes back to the terminal, the rest stay here.
pub(crate) fn key(screen: &ScreenView<'_>, key: &str) -> bool {
    if !settings_open() || !crate::screens::our_menu(screen) {
        return false;
    }
    if key == "key.keyboard.escape" {
        with(|u| u.settings_open = false);
    }
    true
}

pub(crate) fn takes_input(screen: &ScreenView<'_>) -> bool {
    settings_open() && crate::screens::our_menu(screen)
}

pub(crate) fn closing() {
    with(|u| u.settings_open = false);
}

// ---- The finished-job toast

/// A job this player asked for finished (the server's `#jobdone`).
pub(crate) fn job_done(key: AeKey, amount: i64) {
    with(|u| {
        if u.toasts.len() < 8 {
            u.toasts.push_back((key, amount.max(0)));
        }
    });
}

/// Whether the player carries what lets them hear of finished jobs (anywhere in the inventory, the off hand too).
fn carries_notifier(host: &dyn ClientHost) -> bool {
    let Some(player) = host.local_player() else { return false };
    player.inventory_slots_owned(host).is_some_and(|inv| inv.iter().any(|(_, s)| NOTIFYING_ITEMS.contains(&sid(s))))
}

/// FinishedJobToast: top right for 2.5 seconds when "Notify about finished crafting jobs" is on, no terminal is open
/// and the player carries a wireless terminal (PendingCraftingJobs); one after another.
pub(crate) fn hud(host: &dyn ClientHost, d: &mut dyn DrawContext) {
    let Some(((key, amount), shown)) = with(|u| u.toasts.front().cloned().map(|t| (t, u.shown))) else { return };
    let since = match shown {
        Some(at) => at.elapsed().as_secs_f32(),
        None => {
            if !config::config().settings[S_NOTIFY] || crate::screens::terminal_open() || !carries_notifier(host) {
                with(|u| {
                    u.toasts.pop_front();
                });
                return;
            }
            with(|u| u.shown = Some(Instant::now()));
            0.0
        }
    };
    if since >= TOAST_SECONDS {
        with(|u| {
            u.toasts.pop_front();
            u.shown = None;
        });
        return;
    }
    let w = d.size()[0];
    // Slides in and out as toasts do.
    let slide = (since / TOAST_SLIDE).min((TOAST_SECONDS - since) / TOAST_SLIDE).clamp(0.0, 1.0);
    let x = w - 160.0 * slide;
    if !d.sprite("toast/recipe", [0.0, 0.0, 160.0, 32.0], [x, 0.0, 160.0, 32.0], None) {
        d.fill([x, 0.0, 160.0, 32.0], 0xFF20_2020);
    }
    d.text(&tr(host, "gui.ae2.ToastCraftingJobFinishedTitle", "Auto-Crafting Complete"), [x + 30.0, 7.0], 0xFF50_0050, false);
    let line = tr(host, "gui.ae2.ToastCraftingJobFinishedText", "%d %s")
        .replacen("%d", &key.format_amount(amount, AmountFormat::Slot), 1)
        .replacen("%s", &key_name(host, &key), 1);
    d.text(&line, [x + 30.0, 18.0], 0xFF00_0000, false);
    if key.key_type() == KeyType::Fluids {
        d.fluid(key.id(), [x + 8.0, 8.0], 16.0);
    } else if let Some(st) = stack_of(host, &key, 1) {
        d.gui_draw_item_with(&st, [x + 8.0, 8.0], None);
    }
}
