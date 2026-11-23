//! Stage 5's client screens: the pattern provider (pattern_provider.json), a crafting CPU's status (CraftingCPUScreen,
//! craftingcpu.png), the pattern encoding terminal's encoding panel (pattern_modes.png), and the crafting request
//! dialogs over a terminal: the amount (CraftAmountScreen, craft_amt.png) and the plan (CraftConfirmScreen,
//! craftingreport.png). The fork opens no second screen over a menu's (`mod_screen_open` while one is open is an API
//! gap), so the two dialogs are drawn over the terminal in `screen_render_post` and take its clicks and keys.

use crate::autocraft::*;
use crate::cells::*;
use crate::core::{AeKey, AmountFormat, KeyType};
use crate::screens::{key_name, spr, state_button, tr};
use mod_api::*;
use mod_api::client::hud_layout::{GuiDraw, PoseOp};
use std::sync::Mutex;

const TEXT: u32 = 0xFF40_4040;
const HIDDEN: [f32; 2] = [-10_000.0, -10_000.0];

pub(crate) const STEPS: [(f32, f32, i64); 4] = [(20.0, 22.0, 1), (48.0, 28.0, 10), (82.0, 32.0, 100), (120.0, 38.0, 1000)];
pub(crate) const NEXT_BUTTON: [f32; 4] = [120.0, 51.0, 38.0, 20.0];
const CONFIRM_CANCEL: [f32; 4] = [6.0, 181.0, 50.0, 20.0];
const CONFIRM_START: [f32; 4] = [162.0, 181.0, 50.0, 20.0];
const STATUS_CANCEL: [f32; 4] = [163.0, 159.0, 50.0, 20.0];
const CONFIRM_CPU: [f32; 4] = [19.0, 138.0, 180.0, 20.0];
/// Blocking mode, lock crafting, and (enc) show in pattern access terminals.
const PP_TOOLBAR: [[f32; 2]; 3] = [[-18.0, 8.0], [-18.0, 28.0], [-18.0, 48.0]];
const PP_PRIORITY_TAB: [f32; 4] = [154.0, 0.0, 22.0, 22.0];
/// LockCraftingMode's icons (states.png): none, while high, while low, until pulse, until result.
const LOCK_ICONS: [[f32; 2]; 5] = [[160.0, 0.0], [16.0, 0.0], [0.0, 0.0], [32.0, 0.0], [112.0, 0.0]];
const LOCK_NAMES: [&str; 5] = ["Not locked", "Locked while redstone is high", "Locked while redstone is low", "Locked until a redstone pulse", "Locked until the result returns"];
const CELL_ACTIVE: u32 = 0x5A45_A021;
const CELL_PENDING: u32 = 0x5AFF_F7AA;
const CELL_MISSING: u32 = 0x1AFF_0000;
// screens/crafting_cpu.json and crafting_status.json: the back tab, the left toolbar's one button (a crafting block's
// screen), the CPU list down the left side (CPUSelectionList, from a terminal).
const STATUS_BACK: [f32; 4] = [213.0, -4.0, 25.0, 22.0];
const STATUS_MODE: [f32; 4] = [-20.0, 8.0, 16.0, 16.0];
const CPU_LIST: [f32; 4] = [-94.0, 4.0, 94.0, 164.0];
const CPU_BUTTON: [f32; 2] = [67.0, 22.0];
const CPU_ROWS: usize = 6;
/// palette.json's SELECTION_COLOR.
const SELECTION: u32 = 0xFF00_D5FF;
const WARNING: u32 = 0xFFFF_5555;
/// The amount dialog's longest expression.
const AMOUNT_CHARS: usize = 32;

#[derive(Clone)]
pub(crate) enum Overlay {
    Amount { index: usize, key: AeKey, text: String },
    Waiting,
    Plan(PlanView),
    Error(String),
}

#[derive(Clone, Default)]
pub(crate) struct PlanView {
    bytes: i64,
    simulation: bool,
    amount: i64,
    key: Option<AeKey>,
    cpus: usize,
    /// The free CPUs' numbers, and the one picked (None: automatic).
    cpu_list: Vec<usize>,
    picked: Option<usize>,
    /// Key, from storage, to craft, missing.
    rows: Vec<(AeKey, i64, i64, i64)>,
    scroll: usize,
}

#[derive(Default)]
pub(crate) struct CpuView {
    formed: bool,
    number: usize,
    bytes: i64,
    co: i64,
    job: bool,
    secs: i64,
    /// The job's time left in seconds (this port's), -1 when unknown.
    left: i64,
    amount: i64,
    key: Option<AeKey>,
    /// Key, stored, crafting, scheduled.
    rows: Vec<(AeKey, i64, i64, i64)>,
    /// What holds the job up: 0 nothing, 1 no power, 2 its machines, 3 nothing can make it go on.
    stall: u8,
    /// CpuSelectionMode: any, players only, automation only.
    mode: u8,
    /// CantStoreItems: what it holds doesn't fit back into the network.
    cant_store: bool,
}

/// A CPU in the terminal's crafting status list (CraftingStatusMenu.CraftingCpuListEntry).
#[derive(Clone, Default)]
pub(crate) struct CpuEntry {
    number: usize,
    bytes: i64,
    co: i64,
    mode: u8,
    permille: i64,
    amount: i64,
    key: Option<AeKey>,
}

#[derive(Default)]
pub(crate) struct CraftClient {
    /// The terminal's crafting status tab is open (CraftingStatusScreen over the terminal): the grid's CPUs, the one
    /// shown and the list's scroll.
    status_open: bool,
    cpu_list: Vec<CpuEntry>,
    cpu_sel: usize,
    list_scroll: usize,
    /// The open terminal's network: how many CPUs are busy, and what they make.
    jobs: usize,
    job_keys: Vec<AeKey>,
    /// The open assembler's craft: its pattern and grid cells.
    asm: Option<(AeKey, Vec<(usize, AeKey)>)>,
    cpu_scroll: usize,
    /// The provider screen's priority panel is open.
    priority_open: bool,
    overlay: Option<Overlay>,
    /// enc: what Ctrl + JEI's "+" asks to craft, one plan after the other (CraftConfirmMenu.openWithCraftingList).
    craft_queue: Vec<(AeKey, i64)>,
    cpu: CpuView,
    outgoing: Vec<String>,
}

static CLIENT: Mutex<Option<CraftClient>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut CraftClient) -> R) -> R {
    let mut g = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(CraftClient::default))
}

pub(crate) fn inside(at: [f32; 2], r: [f32; 4]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

pub(crate) fn button(d: &mut dyn DrawContext, r: [f32; 4], label: &str, enabled: bool) {
    let sprite = if enabled { "widget/button" } else { "widget/button_disabled" };
    d.gui_nine_slice(sprite, r, 0xFFFF_FFFF);
    let w = d.text_width(label);
    d.text(label, [r[0] + (r[2] - w) / 2.0, r[1] + (r[3] - 8.0) / 2.0], if enabled { 0xFFFF_FFFF } else { 0xFFA0_A0A0 }, true);
}

/// Text at half size (AE2's small font: `pose.scale(0.5)`), `at` its top left in GUI pixels.
pub(crate) fn half_text(d: &mut dyn DrawContext, text: &str, at: [f32; 2], color: u32, shadow: bool) {
    let draws = vec![
        GuiDraw::Push(vec![PoseOp::Scale(0.5, 0.5)]),
        GuiDraw::Text { text: Text::literal(text), x: (at[0] * 2.0).round() as i32, y: (at[1] * 2.0).round() as i32, color, shadow },
        GuiDraw::Pop,
    ];
    let _ = d.gui_paint(&draws);
}

/// StackSizeRenderer's small amount in a slot's corner.
pub(crate) fn slot_amount(d: &mut dyn DrawContext, text: &str, at: [f32; 2]) {
    let w = d.text_width(text) * 0.5;
    half_text(d, text, [at[0] + 16.0 - w, at[1] + 12.5], 0xFFFF_FFFF, true);
}

pub(crate) fn draw_key(host: &dyn ClientHost, d: &mut dyn DrawContext, key: &AeKey, at: [f32; 2], label: Option<&str>) {
    if key.key_type() == KeyType::Fluids {
        d.fluid(key.id(), at, 16.0);
    } else if let Some(stack) = stack_of(host, key, 1) {
        d.gui_draw_item_with(&stack, at, None);
    }
    if let Some(l) = label {
        slot_amount(d, l, at);
    }
}

/// AE2's three-column table of keys (CraftingCPUScreen, CraftConfirmScreen): 67 by 22 cells from 9, 19.
fn draw_table(host: &dyn ClientHost, d: &mut dyn DrawContext, o: [f32; 2], rows: usize, cells: &[(AeKey, Vec<(String, u32)>, u32)], scroll: usize) {
    for (n, (key, lines, bg)) in cells.iter().enumerate().skip(scroll * 3).take(rows * 3) {
        let at = n - scroll * 3;
        let (x, y) = (o[0] + 9.0 + (at % 3) as f32 * 68.0, o[1] + 19.0 + (at / 3) as f32 * 23.0);
        if *bg != 0 {
            d.fill([x, y, 67.0, 22.0], *bg);
        }
        // AE2's table: the lines at half size, right-aligned before the item, centred in the cell.
        let count = lines.len() as f32;
        let height = count * 4.5 + (count - 1.0).max(0.0);
        let mut ty = (y + (22.0 - height) / 2.0).round();
        let item_x = x + 67.0 - 19.0;
        for (line, color) in lines {
            let w = d.text_width(line);
            let tx = ((item_x - 2.0 - w * 0.5) * 2.0).floor() / 2.0;
            half_text(d, line, [tx, ty], *color, false);
            ty += 5.5;
        }
        draw_key(host, d, key, [item_x, y + 3.0], None);
    }
}

/// The table cell under `at` (relative to the table's screen), from `scroll`.
fn table_cell(at: [f32; 2], rows: usize, scroll: usize) -> Option<usize> {
    let (x, y) = (at[0] - 9.0, at[1] - 19.0);
    if x < 0.0 || y < 0.0 || x >= 3.0 * 68.0 || y >= rows as f32 * 23.0 {
        return None;
    }
    Some((scroll + (y / 23.0) as usize) * 3 + (x / 68.0) as usize)
}

fn full_lines(host: &dyn ClientHost, k: &AeKey, parts: &[(i64, &str)]) -> Vec<String> {
    let mut lines = vec![key_name(host, k)];
    for (n, word) in parts {
        if *n > 0 {
            lines.push(format!("§7{word}: {}", k.format_amount(*n, AmountFormat::Full)));
        }
    }
    lines
}

// ---- Block screens

pub(crate) fn screen(block: &str) -> Option<MenuScreenLayout> {
    let base = |width: f32, height: f32, slots: Vec<Option<[f32; 2]>>, inventory_y: f32, inventory_x: f32| MenuScreenLayout {
        width,
        height,
        slots: slots.into_iter().map(|s| s.map(crate::util::slot_at)).collect(),
        inventory_y,
        inventory_x,
        own_labels: true,
        own_background: true,
        ..MenuScreenLayout::default()
    };
    if is_provider(block) {
        // pattern_provider.json: patterns in a row at 8, 45; the return inventory at 8, 97; the inventory at 131.
        let slots = (0..9).map(|i| Some([8.0 + 18.0 * i as f32, 45.0])).chain((0..9).map(|i| Some([8.0 + 18.0 * i as f32, 97.0]))).collect();
        return Some(base(176.0, 213.0, slots, 131.0, 8.0));
    }
    if block == "ae2:molecular_assembler" {
        // molecular_assembler.json: the pattern at 126,16 (shown), five cards beside it, the grid from 29,30, the
        // output at 126,48; the inventory at 116.
        let mut slots = vec![Some([126.0, 16.0])];
        slots.extend((0..5).map(|i| Some([176.0 + 2.0 + 8.0, 8.0 + 18.0 * i as f32])));
        slots.extend((0..9).map(|i| Some([29.0 + 18.0 * (i % 3) as f32, 30.0 + 18.0 * (i / 3) as f32])));
        slots.push(Some([126.0, 48.0]));
        return Some(base(176.0, 197.0, slots, 116.0, 8.0));
    }
    if block == "ae2:pattern_access_terminal" {
        // pattern_access_terminal.json: the terminal style's rows of names and slots, the inventory from 32 below
        // them (enc: `access_term`).
        let rows = crate::access_term::layout_rows() as f32;
        let h = 17.0 + 18.0 * rows + crate::access_term::BOTTOM;
        return Some(base(195.0, h, vec![Some(HIDDEN)], 32.0 + 18.0 * rows, 8.0));
    }
    if crate::net::is_crafting(block) {
        // CraftingCPUScreen: no slots, no inventory.
        return Some(base(238.0, 184.0, vec![Some(HIDDEN)], HIDDEN[1], HIDDEN[0]));
    }
    None
}

pub(crate) fn screen_draw(host: &dyn ClientHost, block: &str, data: &[i32], d: &mut dyn DrawContext, o: [f32; 2]) -> bool {
    if is_provider(block) {
        spr(d, "ae2:gui/pattern_provider", o, [0.0, 0.0], [0.0, 0.0, 176.0, 213.0]);
        d.text(&tr(host, "gui.ae2.PatternProvider", "ME Pattern Provider"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
        d.text(&tr(host, "gui.ae2.Patterns", "Patterns"), [o[0] + 8.0, o[1] + 34.0], TEXT, false);
        d.text(&tr(host, "gui.ae2.ReturnInventory", "Return Inventory"), [o[0] + 8.0, o[1] + 86.0], TEXT, false);
        d.text(&tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 120.0], TEXT, false);
        // RestrictedInputSlot ENCODED_PATTERN's background icon (an item in the slot covers it).
        for i in 0..9 {
            d.sprite("ae2:gui/states", [240.0, 112.0, 16.0, 16.0], [o[0] + 8.0 + 18.0 * i as f32, o[1] + 45.0, 16.0, 16.0], Some(0x60FF_FFFF));
        }
        let blocking = data.first().copied().unwrap_or(0) != 0;
        state_button(d, o, PP_TOOLBAR[0], if blocking { [80.0, 16.0] } else { [64.0, 16.0] });
        state_button(d, o, PP_TOOLBAR[1], LOCK_ICONS[data.get(1).copied().unwrap_or(0).clamp(0, 4) as usize]);
        // Settings.PATTERN_ACCESS_TERMINAL: Icon.PATTERN_ACCESS_SHOW / HIDE.
        state_button(d, o, PP_TOOLBAR[2], if data.get(3).copied().unwrap_or(0) != 0 { [80.0, 80.0] } else { [64.0, 80.0] });
        // The priority tab (PriorityScreen as a panel over the screen: the fork opens no screen over a menu's).
        d.gui_nine_slice("widget/button", [o[0] + PP_PRIORITY_TAB[0], o[1] + PP_PRIORITY_TAB[1], 22.0, 22.0], 0xFFFF_FFFF);
        d.sprite("ae2:gui/states", [32.0, 64.0, 16.0, 16.0], [o[0] + 157.0, o[1] + 3.0, 16.0, 16.0], None);
        if with(|c| c.priority_open) {
            spr(d, "ae2:gui/craft_amt", o, [0.0, 22.0], [0.0, 0.0, 176.0, 107.0]);
            d.text(&tr(host, "gui.ae2.Priority", "Priority"), [o[0] + 8.0, o[1] + 28.0], TEXT, false);
            for (x, w, n) in STEPS {
                button(d, [o[0] + x, o[1] + 52.0, w, 20.0], &format!("+{n}"), true);
                button(d, [o[0] + x, o[1] + 94.0, w, 20.0], &format!("-{n}"), true);
            }
            d.fill([o[0] + 59.0, o[1] + 76.0, 62.0, 14.0], 0xFFFF_FFFF);
            d.fill([o[0] + 60.0, o[1] + 77.0, 60.0, 12.0], 0xFF00_0000);
            d.text(&data.get(2).copied().unwrap_or(0).to_string(), [o[0] + 62.0, o[1] + 79.0], 0xFFFF_FFFF, false);
        }
        return true;
    }
    if block == "ae2:molecular_assembler" {
        spr(d, "ae2:gui/molecular_assembler", o, [0.0, 0.0], [0.0, 0.0, 176.0, 197.0]);
        crate::screens::side_panel(d, o, 176.0, 5);
        d.text(&tr(host, "block.ae2.molecular_assembler", "Molecular Assembler"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
        d.text(&tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 105.0], TEXT, false);
        // The progress bar (MolecularAssemblerScreen: 6 by 17 from 148, 66 up).
        if let Some((pattern, grid)) = with(|c| c.asm.clone()) {
            draw_key(host, d, &pattern, [o[0] + 126.0, o[1] + 16.0], None);
            for (i, k) in grid {
                draw_key(host, d, &k, [o[0] + 29.0 + 18.0 * (i % 3) as f32, o[1] + 30.0 + 18.0 * (i / 3) as f32], None);
            }
        }
        let h = (17.0 * data.first().copied().unwrap_or(0).clamp(0, 100) as f32 / 100.0).round();
        if h > 0.0 {
            d.sprite("ae2:gui/molecular_assembler", [148.0, 219.0 - h, 6.0, h], [o[0] + 148.0, o[1] + 66.0 - h, 6.0, h], None);
        }
        return true;
    }
    if block == "ae2:pattern_access_terminal" {
        crate::access_term::draw(host, d, o, crate::encoder::mouse());
        return true;
    }
    if crate::net::is_crafting(block) {
        draw_cpu(host, d, o);
        return true;
    }
    false
}

pub(crate) fn screen_click(host: &dyn ClientHost, block: &str, _data: &[i32], at: [f32; 2], _right: bool) -> Option<Option<i32>> {
    if is_provider(block) {
        if inside(at, [PP_TOOLBAR[0][0], PP_TOOLBAR[0][1], 16.0, 16.0]) {
            return Some(Some(PP_BLOCKING));
        }
        if inside(at, [PP_TOOLBAR[1][0], PP_TOOLBAR[1][1], 16.0, 16.0]) {
            return Some(Some(PP_LOCK));
        }
        if inside(at, [PP_TOOLBAR[2][0], PP_TOOLBAR[2][1], 16.0, 16.0]) {
            return Some(Some(PP_PAT_SHOW));
        }
        if inside(at, PP_PRIORITY_TAB) {
            with(|c| c.priority_open = !c.priority_open);
            return Some(None);
        }
        if with(|c| c.priority_open) {
            for (i, (x, w, _)) in STEPS.iter().enumerate() {
                if inside(at, [*x, 52.0, *w, 20.0]) {
                    return Some(Some(PP_PRIORITY + i as i32));
                }
                if inside(at, [*x, 94.0, *w, 20.0]) {
                    return Some(Some(PP_PRIORITY + 4 + i as i32));
                }
            }
            if inside(at, [0.0, 22.0, 176.0, 107.0]) {
                return Some(None);
            }
        }
        return None;
    }
    if block == "ae2:pattern_access_terminal" {
        return crate::access_term::click(host, at, _right);
    }
    if crate::net::is_crafting(block) {
        if inside(at, STATUS_CANCEL) && with(|c| c.cpu.job) {
            return Some(Some(CPU_CANCEL));
        }
        // SettingToggleButton on Settings.CPU_SELECTION_MODE (right: the one before). The server steps it.
        if inside(at, STATUS_MODE) && with(|c| c.cpu.formed) {
            return Some(Some(if _right { CPU_MODE_BACK } else { CPU_MODE }));
        }
        return Some(None);
    }
    None
}

fn mode_lines(host: &dyn ClientHost, mode: u8) -> Vec<String> {
    let (key, fallback) = [
        ("gui.tooltips.ae2.CpuSelectionModeAny", "For requests by players or automation"),
        ("gui.tooltips.ae2.CpuSelectionModePlayersOnly", "Only for requests by players"),
        ("gui.tooltips.ae2.CpuSelectionModeAutomationOnly", "Only for requests by automation"),
    ][usize::from(mode.min(2))];
    vec![tr(host, "gui.tooltips.ae2.CpuSelectionMode", "CPU Auto-Selection Mode"), format!("§7{}", tr(host, key, fallback))]
}

pub(crate) fn screen_tooltip(host: &dyn ClientHost, block: &str, at: [f32; 2], lines: &mut Vec<String>) -> bool {
    if is_provider(block) && inside(at, [PP_TOOLBAR[0][0], PP_TOOLBAR[0][1], 16.0, 16.0]) {
        lines.push(tr(host, "gui.tooltips.ae2.InterfaceBlockingMode", "Blocking Mode"));
        lines.push("§7Do not push new inputs while a machine still holds a pattern's input".to_owned());
        return true;
    }
    if is_provider(block) && inside(at, [PP_TOOLBAR[1][0], PP_TOOLBAR[1][1], 16.0, 16.0]) {
        lines.push(tr(host, "gui.tooltips.ae2.LockCraftingMode", "Lock Crafting"));
        return true;
    }
    if is_provider(block) && inside(at, [PP_TOOLBAR[2][0], PP_TOOLBAR[2][1], 16.0, 16.0]) {
        lines.push(tr(host, "gui.ae2.PatternAccessTerminal", "Pattern Access Terminal"));
        lines.push(format!("§7{}", tr(host, "gui.ae2.PatternAccessTerminalHint", "Show Or Hide on Pattern Access Terminal.")));
        return true;
    }
    if is_provider(block) && inside(at, PP_PRIORITY_TAB) {
        lines.push(tr(host, "gui.ae2.Priority", "Priority"));
        return true;
    }
    if block == "ae2:pattern_access_terminal" {
        return crate::access_term::tooltip(host, at, lines);
    }
    if crate::net::is_crafting(block) {
        if inside(at, STATUS_MODE) {
            let (formed, mode) = with(|c| (c.cpu.formed, c.cpu.mode));
            if formed {
                lines.extend(mode_lines(host, mode));
            }
            return true;
        }
        let (rows, scroll) = with(|c| (c.cpu.rows.clone(), c.cpu_scroll));
        if let Some(i) = table_cell(at, 6, scroll)
            && let Some((k, a, b, p)) = rows.get(i)
        {
            lines.extend(full_lines(host, k, &[(*a, "Stored"), (*b, "Crafting"), (*p, "Scheduled")]));
        }
        return true;
    }
    false
}

fn draw_cpu(host: &dyn ClientHost, d: &mut dyn DrawContext, o: [f32; 2]) {
    static AT: Mutex<[f32; 2]> = Mutex::new([0.0, 0.0]);
    if std::mem::replace(&mut *AT.lock().unwrap_or_else(|e| e.into_inner()), o) != o {
        crate::util::debug(&format!("cpu screen at {} {}", o[0], o[1]));
    }
    draw_cpu_window(host, d, o, false);
}

/// CraftingCPUScreen's window (a crafting block's, or the selected CPU's in a terminal's status): the title with the
/// time so far, the time left and what holds the job up, the table, the CPU's line and Cancel; from a crafting block,
/// its auto-selection mode button.
fn draw_cpu_window(host: &dyn ClientHost, d: &mut dyn DrawContext, o: [f32; 2], terminal: bool) {
    spr(d, "ae2:gui/craftingcpu", o, [0.0, 0.0], [0.0, 0.0, 238.0, 184.0]);
    let (title, cells, job, info, warning) = with(|c| {
        let v = &c.cpu;
        let mut title = tr(host, "gui.ae2.CraftingStatus", "Crafting Status");
        if v.job {
            title.push_str(&format!(" - {:02}:{:02}:{:02}", v.secs / 3600, v.secs % 3600 / 60, v.secs % 60));
            // The time left (this port's addition): `~` and the same clock.
            if v.left >= 0 {
                title.push_str(&format!(" (~{:02}:{:02}:{:02})", v.left / 3600, v.left % 3600 / 60, v.left % 60));
            }
        }
        // CantStoreItems, else why the job stands (upstream's stall reasons).
        let warning = if v.cant_store {
            Some(tr(host, "gui.ae2.CantStoreItems", "Can't Store Contents!"))
        } else if v.job {
            match v.stall {
                1 => Some("No power".to_owned()),
                2 => Some("Waiting for machines".to_owned()),
                3 => Some("Stalled".to_owned()),
                _ => None,
            }
        } else {
            None
        };
        let cells: Vec<(AeKey, Vec<(String, u32)>, u32)> = v
            .rows
            .iter()
            .map(|(k, stored, active, pending)| {
                let mut lines = Vec::new();
                for (n, word) in [(*stored, "Stored"), (*active, "Crafting"), (*pending, "Scheduled")] {
                    if n > 0 {
                        lines.push((format!("{word}: {}", k.format_amount(n, AmountFormat::Slot)), TEXT));
                    }
                }
                let bg = if *active > 0 {
                    CELL_ACTIVE
                } else if *pending > 0 {
                    CELL_PENDING
                } else {
                    0
                };
                (k.clone(), lines, bg)
            })
            .collect();
        let info = if v.formed {
            format!("CPU #{}: {} bytes, {} co-processors", v.number, v.bytes, v.co)
        } else if terminal {
            tr(host, "gui.ae2.NoCraftingCPUs", "No Crafting CPUs are Available")
        } else {
            "Not formed".to_owned()
        };
        (title, cells, v.job, info, warning)
    });
    d.text(&title, [o[0] + 8.0, o[1] + 7.0], TEXT, false);
    if let Some(w) = warning {
        let x = o[0] + 8.0 + d.text_width(&title);
        d.text(" - ", [x, o[1] + 7.0], TEXT, false);
        d.text(&w, [x + d.text_width(" - "), o[1] + 7.0], WARNING, false);
    }
    let scroll = with(|c| {
        c.cpu_scroll = c.cpu_scroll.min(cells.len().div_ceil(3).saturating_sub(6));
        c.cpu_scroll
    });
    draw_table(host, d, o, 6, &cells, scroll);
    d.text(&info, [o[0] + 8.0, o[1] + 165.0], TEXT, false);
    button(d, [o[0] + STATUS_CANCEL[0], o[1] + STATUS_CANCEL[1], STATUS_CANCEL[2], STATUS_CANCEL[3]], &tr(host, "gui.ae2.Cancel", "Cancel"), job);
    let (formed, mode) = with(|c| (c.cpu.formed, c.cpu.mode));
    if !terminal && formed {
        // The mode's icon: Icon.CRAFTING_CPU_ANY, or the terminal's or export bus's item (players', automation's).
        let at = [o[0] + STATUS_MODE[0], o[1] + STATUS_MODE[1]];
        match mode {
            1 | 2 => {
                d.sprite("ae2:gui/states", [240.0, 240.0, 16.0, 16.0], [at[0], at[1], 16.0, 16.0], None);
                if let Some(st) = new_stack(host, if mode == 1 { "ae2:terminal" } else { "ae2:export_bus" }, 1) {
                    d.gui_draw_item_with(&st, at, None);
                }
            }
            _ => state_button(d, o, [STATUS_MODE[0], STATUS_MODE[1]], [48.0, 144.0]),
        }
    }
}

// ---- The pattern encoding terminal's panel (enc: `encoder`)

pub(crate) fn draw_encoder(host: &dyn ClientHost, d: &mut dyn DrawContext, o: [f32; 2], foot: f32) {
    crate::encoder::draw(host, d, o, foot, crate::encoder::mouse());
}

pub(crate) fn encoder_click(host: &dyn ClientHost, at: [f32; 2], foot: f32, right: bool) -> Option<i32> {
    crate::encoder::click(host, at, foot, right)
}

// ---- The request dialogs over a terminal

fn dialog_origin(screen: &ScreenView<'_>, plan: bool) -> [f32; 2] {
    if plan { [screen.rect[0] - 21.0, screen.rect[1]] } else { [screen.rect[0] + 9.0, screen.rect[1] + 17.0] }
}

/// A press over a terminal: the open dialog takes it; a click on a craftable with none stored (or the middle button on
/// any entry) opens the amount dialog.
pub(crate) fn pressed(host: &dyn ClientHost, screen: &ScreenView<'_>, button: i32) -> bool {
    crate::encoder::note_mouse(screen.mouse);
    // enc: the access terminal's creative copy (the middle button on a slot).
    if crate::access_term::is_open(screen) && button == 2 {
        let at = [screen.mouse[0] - screen.rect[0], screen.mouse[1] - screen.rect[1]];
        if let Some(id) = crate::access_term::middle_click(host, at) {
            crate::screens::queue_button(id);
            return true;
        }
        return false;
    }
    if !crate::screens::our_menu(screen) {
        return false;
    }
    // enc: the encoder's amount dialog takes every press; the middle button on a processing cell opens it.
    if crate::encoder::dialog_open() {
        crate::encoder::dialog_pressed(screen.mouse, screen.rect);
        return true;
    }
    if button == 2
        && crate::bus::menu(screen.menu) == Some("ae2:pattern_encoding_terminal")
        && !with(|c| c.status_open || c.overlay.is_some())
        && crate::encoder::middle_click([screen.mouse[0] - screen.rect[0], screen.mouse[1] - screen.rect[1]], crate::screens::foot_now())
    {
        return true;
    }
    if with(|c| c.status_open) {
        status_pressed(screen, button);
        return true;
    }
    let overlay = with(|c| c.overlay.clone());
    let m = screen.mouse;
    match overlay {
        Some(Overlay::Amount { index, key, text }) => {
            let o = dialog_origin(screen, false);
            let at = [m[0] - o[0], m[1] - o[1]];
            // The field holds an expression (MathExpressionParser): the steps go from its value.
            let mut n: i64 = crate::mathexpr::whole(&text, 0, i64::from(i32::MAX)).unwrap_or(0);
            let mut stepped = false;
            for (x, w, step) in STEPS {
                if inside(at, [x, 30.0, w, 20.0]) {
                    n = if n <= 1 && step > 1 { step } else { n + step };
                    stepped = true;
                } else if inside(at, [x, 72.0, w, 20.0]) {
                    n = (n - step).max(1);
                    stepped = true;
                }
            }
            if inside(at, NEXT_BUTTON) {
                request_plan(index, &key, &text);
                return true;
            }
            if !inside(at, [0.0, 0.0, 176.0, 107.0]) {
                with(|c| c.overlay = None);
                return true;
            }
            if stepped {
                with(|c| c.overlay = Some(Overlay::Amount { index, key, text: n.clamp(1, i64::from(i32::MAX)).to_string() }));
            }
            true
        }
        Some(Overlay::Plan(p)) => {
            let o = dialog_origin(screen, true);
            let at = [m[0] - o[0], m[1] - o[1]];
            if inside(at, CONFIRM_CANCEL) {
                with(|c| {
                    c.outgoing.push("close".to_owned());
                    c.overlay = None;
                    c.craft_queue.clear();
                });
            } else if inside(at, CONFIRM_START) && !p.simulation && p.cpus > 0 {
                let n = p.picked.and_then(|i| p.cpu_list.get(i)).copied().unwrap_or(0);
                with(|c| {
                    c.outgoing.push(format!("start {n}"));
                    c.overlay = Some(Overlay::Waiting);
                });
            } else if inside(at, CONFIRM_CPU) && !p.cpu_list.is_empty() {
                // CraftConfirmScreen's CPU button: automatic, then each free CPU in turn (right: back).
                let n = p.cpu_list.len();
                let next = match (p.picked, button == 1) {
                    (None, false) => Some(0),
                    (None, true) => Some(n - 1),
                    (Some(i), false) => (i + 1 < n).then_some(i + 1),
                    (Some(i), true) => i.checked_sub(1),
                };
                with(|c| {
                    if let Some(Overlay::Plan(v)) = c.overlay.as_mut() {
                        v.picked = next;
                    }
                });
            }
            true
        }
        Some(Overlay::Waiting) => true,
        Some(Overlay::Error(_)) => {
            with(|c| c.overlay = None);
            true
        }
        None => {
            let at = [m[0] - screen.rect[0], m[1] - screen.rect[1]];
            let Some((index, key, amount)) = crate::screens::entry_index_at(at) else { return false };
            let shift = host.is_key_down("key.keyboard.left.shift") || host.is_key_down("key.keyboard.right.shift");
            if (button == 0 && amount == 0 && !shift) || button == 2 {
                with(|c| c.overlay = Some(Overlay::Amount { index, key, text: "1".to_owned() }));
                return true;
            }
            false
        }
    }
}

/// The wheel: the plan's table, the CPU status table, the access terminal's rows.
pub(crate) fn scrolled(screen: &ScreenView<'_>, dy: f64) -> bool {
    if dy == 0.0 {
        return false;
    }
    let up = dy > 0.0;
    let step = |v: usize| if up { v.saturating_sub(1) } else { v + 1 };
    if crate::screens::our_menu(screen) && with(|c| c.status_open) {
        let o = status_origin(crate::screens::gui());
        let at = [screen.mouse[0] - o[0], screen.mouse[1] - o[1]];
        return with(|c| {
            if at[0] < 0.0 {
                c.list_scroll = step(c.list_scroll).min(c.cpu_list.len().saturating_sub(CPU_ROWS));
            } else {
                c.cpu_scroll = step(c.cpu_scroll).min(c.cpu.rows.len().div_ceil(3).saturating_sub(6));
            }
            true
        });
    }
    if crate::screens::our_menu(screen) && crate::encoder::dialog_open() {
        return true;
    }
    if crate::bus::menu(screen.menu) == Some("ae2:pattern_encoding_terminal")
        && !with(|c| c.overlay.is_some())
        && crate::encoder::scrolled([screen.mouse[0] - screen.rect[0], screen.mouse[1] - screen.rect[1]], crate::screens::foot_now(), dy)
    {
        return true;
    }
    if crate::screens::our_menu(screen) {
        return with(|c| match c.overlay.as_mut() {
            Some(Overlay::Plan(p)) => {
                let most = p.rows.len().div_ceil(3).saturating_sub(5);
                p.scroll = step(p.scroll).min(most);
                crate::util::debug(&format!("plan scroll {} of {most}", p.scroll));
                true
            }
            Some(_) => true,
            None => false,
        });
    }
    match crate::bus::menu(screen.menu) {
        Some("ae2:crafting_cpu") => with(|c| {
            c.cpu_scroll = step(c.cpu_scroll).min(c.cpu.rows.len().div_ceil(3).saturating_sub(6));
            true
        }),
        _ => false,
    }
}

pub(crate) fn render_post(host: &dyn ClientHost, d: &mut dyn DrawContext, screen: &ScreenView<'_>) {
    crate::encoder::note_mouse(screen.mouse);
    if !crate::screens::our_menu(screen) {
        return;
    }
    if crate::encoder::dialog_open() {
        crate::encoder::dialog_draw(host, d, screen.rect, &crate::screens::menu_block(screen));
        return;
    }
    if with(|c| c.status_open) {
        draw_status(host, d, screen);
        return;
    }
    let Some(overlay) = with(|c| c.overlay.clone()) else { return };
    match overlay {
        Overlay::Amount { key, text, .. } => {
            let o = dialog_origin(screen, false);
            spr(d, "ae2:gui/craft_amt", o, [0.0, 0.0], [0.0, 0.0, 176.0, 107.0]);
            d.text(&tr(host, "gui.ae2.SelectAmount", "Select Amount"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
            for (x, w, n) in STEPS {
                button(d, [o[0] + x, o[1] + 30.0, w, 20.0], &format!("+{n}"), true);
                button(d, [o[0] + x, o[1] + 72.0, w, 20.0], &format!("-{n}"), true);
            }
            d.fill([o[0] + 47.0, o[1] + 54.0, 68.0, 14.0], 0xFFFF_FFFF);
            d.fill([o[0] + 48.0, o[1] + 55.0, 66.0, 12.0], 0xFF00_0000);
            // An expression the field can't read yet in red (NumberEntryWidget's validation).
            let color = if amount_of(&text).is_some() { 0xFFFF_FFFF } else { 0xFFFF_6060 };
            d.text(&format!("{text}_"), [o[0] + 50.0, o[1] + 57.0], color, false);
            draw_key(host, d, &key, [o[0] + 23.0, o[1] + 53.0], None);
            button(d, [o[0] + NEXT_BUTTON[0], o[1] + NEXT_BUTTON[1], NEXT_BUTTON[2], NEXT_BUTTON[3]], &tr(host, "gui.ae2.Next", "Next"), true);
        }
        Overlay::Waiting | Overlay::Error(_) | Overlay::Plan(_) => {
            let o = dialog_origin(screen, true);
            spr(d, "ae2:gui/craftingreport", o, [0.0, 0.0], [0.0, 0.0, 238.0, 206.0]);
            match overlay {
                // polish: in the game's language (GuiText.CraftingPlan, CalculatingWait, BytesUsed).
                Overlay::Waiting => {
                    let wait = tr(host, "gui.ae2.CalculatingWait", "Calculating Please Wait...");
                    d.text(&tr(host, "gui.ae2.CraftingPlan", "Crafting Plan - %s").replace("%s", &wait), [o[0] + 8.0, o[1] + 7.0], TEXT, false)
                }
                Overlay::Error(e) => {
                    let title = tr(host, "gui.ae2.CraftingPlan", "Crafting Plan - %s").replace(" - %s", "").replace("%s", "");
                    d.text(&title, [o[0] + 8.0, o[1] + 7.0], TEXT, false);
                    d.text(&e, [o[0] + 8.0, o[1] + 30.0], 0xFFFF_5555, false);
                }
                Overlay::Plan(p) => {
                    let used = tr(host, "gui.ae2.BytesUsed", "%s Bytes Used").replace("%s", &p.bytes.to_string());
                    d.text(&tr(host, "gui.ae2.CraftingPlan", "Crafting Plan - %s").replace("%s", &used), [o[0] + 8.0, o[1] + 7.0], TEXT, false);
                    let cells: Vec<(AeKey, Vec<(String, u32)>, u32)> = p
                        .rows
                        .iter()
                        .map(|(k, used, craft, missing)| {
                            let mut lines = Vec::new();
                            for (n, word, color) in [(*used, "Available", TEXT), (*missing, "Missing", TEXT), (*craft, "To Craft", TEXT)] {
                                if n > 0 {
                                    lines.push((format!("{word}: {}", k.format_amount(n, AmountFormat::Slot)), color));
                                }
                            }
                            (k.clone(), lines, if *missing > 0 { CELL_MISSING } else { 0 })
                        })
                        .collect();
                    draw_table(host, d, o, 5, &cells, p.scroll);
                    let cpu = if p.simulation {
                        "Missing ingredients".to_owned()
                    } else if p.cpus == 0 {
                        "No CPU is free and big enough".to_owned()
                    } else {
                        match p.picked.and_then(|i| p.cpu_list.get(i)) {
                            Some(n) => format!("Crafting CPU: CPU #{n}"),
                            None => format!("Crafting CPU: Automatic ({} free)", p.cpus),
                        }
                    };
                    let usable = !p.simulation && p.cpus > 0;
                    button(d, [o[0] + CONFIRM_CPU[0], o[1] + CONFIRM_CPU[1], CONFIRM_CPU[2], CONFIRM_CPU[3]], &cpu, usable);
                    // The scroll bar beside the table, and the hovered cell's full amounts.
                    let most = cells.len().div_ceil(3).saturating_sub(5);
                    let track = 5.0 * 23.0 - 15.0;
                    let dy = if most == 0 { 0.0 } else { (track * p.scroll as f32 / most as f32).floor() };
                    let sprite = if most == 0 { "container/creative_inventory/scroller_disabled" } else { "container/creative_inventory/scroller" };
                    d.sprite(sprite, [0.0, 0.0, 12.0, 15.0], [o[0] + 218.0, o[1] + 19.0 + dy, 12.0, 15.0], None);
                    let m = screen.mouse;
                    if let Some(i) = table_cell([m[0] - o[0], m[1] - o[1]], 5, p.scroll)
                        && let Some((k, used, craft, missing)) = p.rows.get(i)
                    {
                        let lines: Vec<Text> = full_lines(host, k, &[(*used, "Available"), (*missing, "Missing"), (*craft, "To Craft")]).iter().map(|l| Text::literal(l.trim_start_matches("§7"))).collect();
                        let _ = d.gui_tooltip(&lines, m);
                    }
                    if let Some(k) = &p.key {
                        d.text(&format!("{} x {}", key_name(host, k), p.amount), [o[0] + 19.0, o[1] + 162.0], TEXT, false);
                    }
                    button(d, [o[0] + CONFIRM_START[0], o[1] + CONFIRM_START[1], 50.0, 20.0], &tr(host, "gui.ae2.Start", "Start"), !p.simulation && p.cpus > 0);
                }
                Overlay::Amount { .. } => {}
            }
            button(d, [o[0] + CONFIRM_CANCEL[0], o[1] + CONFIRM_CANCEL[1], 50.0, 20.0], &tr(host, "gui.ae2.Cancel", "Cancel"), true);
        }
    }
}

/// Keys while a dialog is open: digits and backspace edit the amount, Enter is Next, Escape closes the dialog only.
pub(crate) fn key(screen: &ScreenView<'_>, key: &str) -> bool {
    if !crate::screens::our_menu(screen) {
        return false;
    }
    if crate::encoder::dialog_open() {
        crate::encoder::dialog_key(key);
        return true;
    }
    if with(|c| c.status_open) {
        // Escape goes back to the terminal; other keys stay with the status.
        if key == "key.keyboard.escape" {
            close_status();
        }
        return true;
    }
    let Some(overlay) = with(|c| c.overlay.clone()) else { return false };
    match key {
        "key.keyboard.escape" => with(|c| {
            if matches!(c.overlay, Some(Overlay::Plan(_))) {
                c.outgoing.push("close".to_owned());
            }
            c.overlay = None;
            c.craft_queue.clear();
        }),
        "key.keyboard.backspace" => with(|c| {
            if let Some(Overlay::Amount { text, .. }) = c.overlay.as_mut() {
                text.pop();
            }
        }),
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => {
            if let Overlay::Amount { index, key, text } = overlay {
                request_plan(index, &key, &text);
            }
        }
        _ => {}
    }
    true
}

/// The amount dialog's field as a request: a whole number of at least one (an expression's value), at most what a plan
/// is made for. None while it isn't one.
fn amount_of(text: &str) -> Option<i64> {
    let n = crate::mathexpr::whole(text, i64::MIN, i64::MAX)?;
    (n >= 1).then(|| n.min(i64::from(i32::MAX)))
}

/// Next (or Enter) on the amount dialog: the plan asked for; with "Pin auto-crafted items", the key pinned to the grid's
/// first row (PinnedKeys).
fn request_plan(index: usize, key: &AeKey, text: &str) {
    let Some(n) = amount_of(text) else { return };
    with(|c| {
        c.outgoing.push(format!("plan {index} {n}"));
        c.overlay = Some(Overlay::Waiting);
    });
    if crate::config::config().settings[crate::config::S_PIN] {
        crate::screens::pin(key);
    }
}

pub(crate) fn typed(screen: &ScreenView<'_>, ch: char) -> bool {
    if !crate::screens::our_menu(screen) {
        return false;
    }
    if crate::encoder::dialog_open() {
        crate::encoder::dialog_typed(ch);
        return true;
    }
    with(|c| {
        if c.status_open {
            return true;
        }
        match c.overlay.as_mut() {
            Some(Overlay::Amount { text, .. }) => {
                if crate::mathexpr::allowed(ch) && text.len() < AMOUNT_CHARS {
                    if text == "0" {
                        text.clear();
                    }
                    text.push(ch);
                }
                true
            }
            Some(_) => true,
            None => false,
        }
    })
}

pub(crate) fn closing() {
    with(|c| {
        c.priority_open = false;
        c.craft_queue.clear();
        if c.overlay.take().is_some() {
            c.outgoing.push("close".to_owned());
        }
        if std::mem::take(&mut c.status_open) {
            c.outgoing.push("status off".to_owned());
        }
    });
}

/// A message for the server on the next client tick (`ae2:craft`).
pub(crate) fn send(message: String) {
    with(|c| c.outgoing.push(message));
}

/// The open terminal network's busy CPUs and what they make (the status tab's count, the pinned row's animation).
pub(crate) fn jobs() -> (usize, Vec<AeKey>) {
    with(|c| (c.jobs, c.job_keys.clone()))
}

/// Whether a dialog or the crafting status covers the terminal.
pub(crate) fn covered() -> bool {
    with(|c| c.status_open || c.overlay.is_some()) || crate::encoder::dialog_open()
}

/// enc: the next ingredient Ctrl + "+" asks to craft: its plan, as the amount dialog's Next asks for one.
fn craft_next(c: &mut CraftClient) {
    while let Some((key, amount)) = c.craft_queue.pop() {
        let Some(index) = crate::screens::index_of(&key) else { continue };
        c.outgoing.push(format!("plan {index} {}", amount.clamp(1, i64::from(i32::MAX))));
        c.overlay = Some(Overlay::Waiting);
        return;
    }
}

// ---- The terminal's crafting status tab (CraftingStatusScreen over the terminal)

/// The status tab clicked: the CPUs of the terminal's network, the first busy one shown.
pub(crate) fn open_status() {
    with(|c| {
        c.status_open = true;
        c.cpu = CpuView::default();
        c.cpu_list.clear();
        c.cpu_sel = 0;
        c.cpu_scroll = 0;
        c.list_scroll = 0;
        c.outgoing.push("status auto".to_owned());
    });
}

fn close_status() {
    with(|c| {
        if std::mem::take(&mut c.status_open) {
            c.outgoing.push("status off".to_owned());
        }
    });
}

/// The window's corner: the window and the CPU list beside it centred together.
fn status_origin(size: [f32; 2]) -> [f32; 2] {
    [((size[0] - 238.0 - CPU_LIST[0]) / 2.0).floor(), ((size[1] - 184.0) / 2.0).floor()]
}

fn cpu_row(row: usize) -> [f32; 4] {
    [CPU_LIST[0] + 9.0, CPU_LIST[1] + 19.0 + row as f32 * (CPU_BUTTON[1] + 1.0), CPU_BUTTON[0], CPU_BUTTON[1]]
}

/// "1.5 kB" (CPUSelectionList's storage line).
fn bytes_words(bytes: i64) -> String {
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["kB", "MB", "GB", "TB"] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    let text = format!("{value:.2}");
    format!("{} {unit}", text.trim_end_matches('0').trim_end_matches('.'))
}

/// Text at `scale` (CPUSelectionList's 0.8 names, its 0.6 info bar).
fn scaled_text(d: &mut dyn DrawContext, text: &str, at: [f32; 2], scale: f32, color: u32) {
    let draws = vec![
        GuiDraw::Push(vec![PoseOp::Scale(scale, scale)]),
        GuiDraw::Text { text: Text::literal(text), x: (at[0] / scale).round() as i32, y: (at[1] / scale).round() as i32, color, shadow: false },
        GuiDraw::Pop,
    ];
    let _ = d.gui_paint(&draws);
}

fn draw_status(host: &dyn ClientHost, d: &mut dyn DrawContext, screen: &ScreenView<'_>) {
    let size = d.size();
    d.fill([0.0, 0.0, size[0], size[1]], 0xA000_0000);
    let o = status_origin(size);
    let m = screen.mouse;
    let at = [m[0] - o[0], m[1] - o[1]];
    let mut tip: Option<Vec<String>> = None;
    // The CPU list (CPUSelectionList), behind the window's left edge.
    let (list, sel, top) = with(|c| {
        let most = c.cpu_list.len().saturating_sub(CPU_ROWS);
        c.list_scroll = c.list_scroll.min(most);
        (c.cpu_list.clone(), c.cpu_sel, c.list_scroll)
    });
    spr(d, "ae2:gui/cpu_selector", o, [CPU_LIST[0], CPU_LIST[1]], [0.0, 0.0, 94.0, 164.0]);
    d.text(&tr(host, "gui.ae2.CraftingCPUs", "Crafting CPUs"), [o[0] + CPU_LIST[0] + 8.0, o[1] + 12.0], TEXT, false);
    for (i, cpu) in list.iter().enumerate().skip(top).take(CPU_ROWS) {
        let r = cpu_row(i - top);
        let (x, y) = (o[0] + r[0], o[1] + r[1]);
        let selected = i == sel;
        d.sprite("ae2:gui/cpu_selector", [100.0, 0.0, CPU_BUTTON[0], CPU_BUTTON[1]], [x, y, CPU_BUTTON[0], CPU_BUTTON[1]], Some(if selected { SELECTION } else { 0xFFFF_FFFF }));
        let name = format!("CPU #{}", cpu.number);
        scaled_text(d, &name, [x + 3.0, y + 3.0], 0.8, TEXT);
        // The info bar: storage and co-processors, or what it crafts and how far.
        let (mut bx, by) = (x + 2.0, y + CPU_BUTTON[1] - 12.0);
        fn icon(d: &mut dyn DrawContext, bx: &mut f32, by: f32, icon: [f32; 2]) {
            d.sprite("ae2:gui/states", [icon[0], icon[1], 16.0, 16.0], [*bx, by, 9.6, 9.6], None);
            *bx += 10.0;
        }
        fn text(d: &mut dyn DrawContext, bx: &mut f32, by: f32, t: &str) {
            scaled_text(d, t, [*bx, by + 3.0], 0.6, TEXT);
            *bx += (d.text_width(t) * 0.6).round() + 1.0;
        }
        match &cpu.key {
            None => {
                icon(d, &mut bx, by, [64.0, 64.0]);
                text(d, &mut bx, by, &format!("{}k", cpu.bytes / 1024));
                if cpu.co > 0 {
                    icon(d, &mut bx, by, [64.0, 16.0]);
                    text(d, &mut bx, by, &cpu.co.to_string());
                }
            }
            Some(k) => {
                icon(d, &mut bx, by, [48.0, 144.0]);
                text(d, &mut bx, by, &k.format_amount(cpu.amount, AmountFormat::Slot));
                if k.key_type() == KeyType::Fluids {
                    d.fluid(k.id(), [bx, by], 9.6);
                } else if let Some(st) = stack_of(host, k, 1) {
                    d.item_stack(&st, [bx, by], 9.6);
                }
                let progress = (cpu.permille.clamp(0, 1000) as f32 / 1000.0 * (CPU_BUTTON[0] - 1.0)).floor();
                if progress > 1.0 {
                    d.fill([x + 1.0, y + CPU_BUTTON[1] - 2.0, progress - 1.0, 1.0], if selected { 0xFFFF_FFFF } else { SELECTION });
                }
            }
        }
        if inside(at, r) {
            let mut lines = vec![name, format!("§7{} Storage", bytes_words(cpu.bytes))];
            match cpu.co {
                0 => {}
                1 => lines.push("§71 Co-Processor".to_owned()),
                n => lines.push(format!("§7{n} Co-Processors")),
            }
            if cpu.mode != 0 {
                lines.push(mode_lines(host, cpu.mode).pop().unwrap_or_default());
            }
            if let Some(k) = &cpu.key {
                lines.push(format!("Crafting {} {}", k.format_amount(cpu.amount, AmountFormat::Full), key_name(host, k)));
                lines.push(format!("§7{}%", cpu.permille.clamp(0, 1000) / 10));
            }
            tip = Some(lines);
        }
    }
    // The list's scroll bar.
    let most = list.len().saturating_sub(CPU_ROWS);
    let track = 137.0 - 15.0;
    let dy = if most == 0 { 0.0 } else { (track * top as f32 / most as f32).floor() };
    let sprite = if most == 0 { "container/creative_inventory/scroller_disabled" } else { "container/creative_inventory/scroller" };
    d.sprite(sprite, [0.0, 0.0, 12.0, 15.0], [o[0] - 16.0, o[1] + 23.0 + dy, 12.0, 15.0], None);
    draw_cpu_window(host, d, o, true);
    // AESubScreen's back button: a corner tab with the terminal's icon.
    let back = [o[0] + STATUS_BACK[0], o[1] + STATUS_BACK[1]];
    d.sprite("ae2:gui/states", [128.0, 192.0, STATUS_BACK[2], STATUS_BACK[3]], [back[0], back[1], STATUS_BACK[2], STATUS_BACK[3]], None);
    let icon = crate::screens::menu_block(screen);
    if let Some(st) = new_stack(host, &icon, 1) {
        d.gui_draw_item_with(&st, [back[0] + 4.0, back[1] + 3.0], None);
    }
    if inside(at, STATUS_BACK) {
        let path = icon.split_once(':').map_or(icon.as_str(), |(_, p)| p);
        tip = Some(vec![tr(host, &format!("block.ae2.{path}"), &icon)]);
    }
    let (rows, scroll) = with(|c| (c.cpu.rows.clone(), c.cpu_scroll));
    if let Some(i) = table_cell(at, 6, scroll)
        && let Some((k, a, b, p)) = rows.get(i)
    {
        tip = Some(full_lines(host, k, &[(*a, "Stored"), (*b, "Crafting"), (*p, "Scheduled")]));
    }
    if let Some(lines) = tip {
        let lines: Vec<Text> = lines.iter().map(|l| Text::literal(l.trim_start_matches("§7"))).collect();
        let _ = d.gui_tooltip(&lines, m);
    }
}

/// A press while the status is open: it takes them all.
fn status_pressed(screen: &ScreenView<'_>, button: i32) {
    if button != 0 {
        return;
    }
    let o = status_origin(crate::screens::gui());
    let at = [screen.mouse[0] - o[0], screen.mouse[1] - o[1]];
    if inside(at, STATUS_BACK) {
        close_status();
        return;
    }
    with(|c| {
        if inside(at, STATUS_CANCEL) {
            if c.cpu.job {
                c.outgoing.push(format!("cpucancel {}", c.cpu_sel));
            }
            return;
        }
        for row in 0..CPU_ROWS {
            let i = c.list_scroll + row;
            if inside(at, cpu_row(row)) && i < c.cpu_list.len() {
                c.cpu_sel = i;
                c.cpu_scroll = 0;
                c.outgoing.push(format!("status {i}"));
            }
        }
    });
}

pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    let out = with(|c| std::mem::take(&mut c.outgoing));
    for m in out {
        let _ = crate::util::send_to_server(host, NET_CRAFT, m.as_bytes());
    }
}

// ---- From the server

fn parse_row(line: &str) -> Option<(AeKey, i64, i64, i64)> {
    let mut f = line.splitn(4, '\t');
    let a = f.next()?.parse().ok()?;
    let b = f.next()?.parse().ok()?;
    let c = f.next()?.parse().ok()?;
    Some((key_from_string(f.next()?)?, a, b, c))
}

pub(crate) fn from_server(channel: &str, data: &[u8]) -> bool {
    let text = String::from_utf8_lossy(data);
    match channel {
        NET_PLAN => {
            let mut lines = text.lines();
            let head = lines.next().unwrap_or("");
            // A job this player asked for finished (FinishedJobToast).
            if let Some(rest) = head.strip_prefix("#jobdone ") {
                if let Some((n, k)) = rest.split_once(' ')
                    && let (Ok(n), Some(k)) = (n.parse::<i64>(), key_from_string(k))
                {
                    crate::term_ui::job_done(k, n);
                }
                return true;
            }
            // enc: Ctrl + JEI's "+": the missing ingredients to craft, one plan after the other.
            if head == crate::ct_transfer::CRAFT_LIST {
                let mut list: Vec<(AeKey, i64)> = lines.filter_map(|l| l.split_once(' ').and_then(|(n, k)| Some((key_from_string(k)?, n.parse().ok()?)))).take(9).collect();
                list.reverse();
                with(|c| {
                    if c.overlay.is_none() && !c.status_open {
                        c.craft_queue = list;
                        craft_next(c);
                    }
                });
                return true;
            }
            if head.starts_with("#started") && with(|c| !c.craft_queue.is_empty()) {
                with(craft_next);
                return true;
            }
            let overlay = if let Some(why) = head.strip_prefix("#error ") {
                Some(Overlay::Error(why.to_owned()))
            } else if head.starts_with("#started") {
                None
            } else if let Some(rest) = head.strip_prefix("#plan ") {
                let mut w = rest.splitn(4, ' ');
                let mut p = PlanView {
                    bytes: w.next().and_then(|x| x.parse().ok()).unwrap_or(0),
                    simulation: w.next() == Some("1"),
                    amount: w.next().and_then(|x| x.parse().ok()).unwrap_or(0),
                    key: w.next().and_then(key_from_string),
                    ..PlanView::default()
                };
                for l in lines {
                    if let Some(n) = l.strip_prefix("#cpus ") {
                        let mut w = n.split_whitespace();
                        p.cpus = w.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                        p.cpu_list = w.filter_map(|x| x.parse().ok()).collect();
                    } else if let Some(r) = parse_row(l) {
                        p.rows.push(r);
                    }
                }
                Some(Overlay::Plan(p))
            } else {
                return true;
            };
            with(|c| c.overlay = overlay);
            true
        }
        NET_CPU => {
            // A crafting block's status (`#cpu`, `#none`), or a terminal's (`#jobs`, and with its tab open `#list`
            // and `#sel` with the selected CPU's status); `#stall`, `#mode` and `#cant` past CraftingCPUMenu's.
            let mut v = CpuView::default();
            let (mut status, mut jobs, mut job_keys, mut list, mut sel) = (false, None, Vec::new(), Vec::new(), None);
            for l in text.lines() {
                let num = |s: &str| s.trim().parse::<i64>().ok();
                if let Some(rest) = l.strip_prefix("#cpu ") {
                    status = true;
                    let w: Vec<&str> = rest.splitn(8, ' ').collect();
                    let n = |i: usize| w.get(i).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
                    v.formed = true;
                    v.number = n(0).max(0) as usize;
                    v.bytes = n(1);
                    v.co = n(2);
                    v.job = n(3) != 0;
                    v.secs = n(4).max(0);
                    v.amount = n(5);
                    v.left = w.get(6).and_then(|x| x.parse::<i64>().ok()).unwrap_or(-1);
                    v.key = w.get(7).and_then(|k| key_from_string(k));
                } else if l == "#none" {
                    status = true;
                } else if let Some(n) = l.strip_prefix("#jobs ") {
                    jobs = num(n).map(|n| n.clamp(0, 9999) as usize);
                } else if let Some(k) = l.strip_prefix("#jk ") {
                    if job_keys.len() < 16 {
                        job_keys.extend(key_from_string(k));
                    }
                } else if let Some(r) = l.strip_prefix("#list ") {
                    status = true;
                    let w: Vec<&str> = r.splitn(7, ' ').collect();
                    let n = |i: usize| w.get(i).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
                    if list.len() < 64 {
                        list.push(CpuEntry {
                            number: n(0).max(0) as usize,
                            bytes: n(1),
                            co: n(2),
                            mode: n(3).clamp(0, 2) as u8,
                            permille: n(4),
                            amount: n(5),
                            key: w.get(6).filter(|k| **k != "-").and_then(|k| key_from_string(k)),
                        });
                    }
                } else if let Some(n) = l.strip_prefix("#sel ") {
                    sel = num(n).map(|n| n.max(0) as usize);
                } else if let Some(n) = l.strip_prefix("#stall ") {
                    v.stall = num(n).unwrap_or(0).clamp(0, 3) as u8;
                } else if let Some(n) = l.strip_prefix("#mode ") {
                    v.mode = num(n).unwrap_or(0).clamp(0, 2) as u8;
                } else if let Some(n) = l.strip_prefix("#cant ") {
                    v.cant_store = num(n) == Some(1);
                } else if let Some(r) = parse_row(l) {
                    v.rows.push(r);
                }
            }
            with(|c| {
                if let Some(j) = jobs {
                    c.jobs = j;
                    c.job_keys = job_keys;
                }
                if status {
                    c.cpu = v;
                    c.cpu_list = list;
                    if let Some(s) = sel {
                        c.cpu_sel = s;
                    }
                }
            });
            true
        }
        NET_ASM => {
            let mut lines = text.lines();
            let asm = lines.next().and_then(key_from_string).map(|p| {
                (p, lines.filter_map(|l| l.split_once(' ').and_then(|(i, k)| Some((i.parse().ok()?, key_from_string(k)?)))).collect())
            });
            with(|c| c.asm = asm);
            true
        }
        NET_PAT => {
            crate::access_term::from_server(&text);
            true
        }
        NET_ENC => {
            crate::encoder::from_server(&text);
            true
        }
        _ => false,
    }
}
