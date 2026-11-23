//! AE2's screens on the client (stage 3): the ME drive, the ME chest (its cell slot, and its own terminal from the
//! top), the terminal and the crafting terminal, at the positions of AE2's screen files (`assets/ae2/screens/*.json`
//! in the 15.4.12 jar) over AE2's own GUI textures.
//!
//! The fork's block-entity screen hooks: `screen` lays the slots out (both sides: a terminal's rows follow the GUI's
//! height, as AE2's TerminalStyle), `screen_draw` draws, `screen_click` turns a press on the item grid into a menu
//! button for the server (`storage::menu_button`), `screen_tooltip` names a grid entry. The list comes on the
//! `ae2:items` channel (indexes the server keeps: the client filters and sorts it, AE2's RepoSearch and SortOrder),
//! the crafting grid's result on `ae2:extras`. The wheel, drags and the search box's keys come through the global
//! `screen_*_pre` hooks. The client's state is per thread: in a world played alone the server's thread has its own.

use crate::cells::*;
use crate::core::{AeKey, AmountFormat, KeyType};
use crate::storage::*;
use crate::util::*;
use mod_api::*;
use std::sync::Mutex;
use std::time::Instant;

const TERM_COLS: usize = 9;
const TERM_GRID: [f32; 2] = [8.0, 18.0];
/// A slot the menu has but nobody sees or clicks (a terminal's insert slot).
const HIDDEN_SLOT: [f32; 2] = [-10_000.0, -10_000.0];
/// MEStorageScreen's left toolbar (VerticalButtonBar): sort by, view, type filter, sort order, terminal settings,
/// terminal style.
const TOOLBAR: [[f32; 2]; 6] = [[-20.0, 8.0], [-20.0, 28.0], [-20.0, 48.0], [-20.0, 68.0], [-20.0, 88.0], [-20.0, 108.0]];
const TOOL_SETTINGS: usize = 4;
/// The crafting status tab (screens/terminals/base_terminal.json).
const STATUS_TAB: [f32; 4] = [170.0, -4.0, 25.0, 22.0];
/// PinnedKeys: the grid's first row.
const PINNED: usize = TERM_COLS;
/// base_terminal.json's `search`.
const SEARCH_BOX: [f32; 4] = [80.0, 4.0, 89.0, 12.0];
/// crafting_terminal.json's clear buttons (from the bottom part's top) and result.
const STASH_X: [f32; 2] = [92.0, 102.0];
const STASH_Y: f32 = 14.0;
/// A press the client keeps (a toolbar setting, outside the window): a button no server acts on, so the game
/// neither throws the cursor's stack nor clicks anything.
pub(crate) const CLIENT_ONLY: i32 = -1;
/// AEConfig's `terminalMargin`, top and bottom.
const TERMINAL_MARGIN: f32 = 25.0;
const TEXT: u32 = 0xFF40_4040;

#[derive(Clone)]
pub(crate) struct Entry {
    key: AeKey,
    amount: i64,
    stack: Option<ItemStack>,
    /// The name, lower case, for the search and the sort ("" until it was asked for).
    name: String,
    /// Its tooltip's lines, lower case, for `#` in the search (made when a search first asks).
    tips: Option<String>,
}

/// What `$` in the search found for one word: the tags it names (TagKey for items, names for fluids), the entries
/// asked about and those in one of the tags, by key.
#[derive(Default)]
struct TagHits {
    tags: Vec<(TagKey, String)>,
    checked: std::collections::HashSet<String>,
    hits: std::collections::HashSet<String>,
}

#[derive(Default)]
pub(crate) struct Client {
    items: Vec<Option<Entry>>,
    view: Option<Vec<usize>>,
    online: bool,
    search: String,
    search_focus: bool,
    /// SortOrder (name, amount, mod), ViewItems (all, stored, craftable), TypeFilter (all, items, fluids), SortDir,
    /// TerminalStyle (small, medium, tall, full).
    sort: u8,
    view_mode: u8,
    type_filter: u8,
    sort_desc: bool,
    style: u8,
    scroll: usize,
    rows: usize,
    gui: [f32; 2],
    /// The player used an ME chest's top: its screen is its terminal.
    chest_top: Option<Instant>,
    chest_term: bool,
    craft_result: Option<ItemStack>,
    dragging: bool,
    /// Menu buttons to send on the next client tick (from hooks that can't).
    queued: Vec<i32>,
    /// PinnedKeys: what the player auto-crafted, shown first (oldest first, the first row's worth).
    pinned: Vec<AeKey>,
    /// `$` words of the search and what they found.
    tag_hits: std::collections::HashMap<String, TagHits>,
    /// When a terminal was last drawn (a terminal is open: no finished-job toast).
    drawn: Option<Instant>,
}

/// The client's state: one for the process (the network thread fills it, the render thread draws it).
static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

thread_local! {
    /// On the server's thread: the ME chest whose menu is opening now is its terminal.
    static SERVER_CHEST_TERM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The open terminal's client repository by item id (AE2's `getClientRepo`): the amount stored, and whether it is
/// listed as craftable with none stored (the server lists a craftable with amount 0).
pub(crate) fn repo_items() -> std::collections::HashMap<String, (i64, bool)> {
    with_client(|c| {
        c.items
            .iter()
            .flatten()
            .filter(|e| e.key.key_type() == KeyType::Items)
            .map(|e| (e.key.id().to_owned(), (e.amount.max(0), e.amount == 0)))
            .collect()
    })
}

pub(crate) fn with_client<R>(f: impl FnOnce(&mut Client) -> R) -> R {
    let mut g = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Client::default))
}

/// The server's thread (its `screen` calls must not take the client's state).
static SERVER_THREAD: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

pub(crate) fn server_thread_here() {
    *SERVER_THREAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::thread::current().id());
}

fn on_server_thread() -> bool {
    *SERVER_THREAD.lock().unwrap_or_else(|e| e.into_inner()) == Some(std::thread::current().id())
}

pub(crate) fn set_server_chest_term(on: bool) {
    SERVER_CHEST_TERM.with(|c| c.set(on));
}

// ---- Layouts

fn term_rows(c: &Client, bottom: f32) -> usize {
    // TerminalStyle.getRows: a quarter of the rows that fit for each step (never fewer than three).
    let fit = ((c.gui[1].max(240.0) - 17.0 - bottom - 2.0 * TERMINAL_MARGIN) / 18.0).floor().max(3.0) as usize;
    (fit * (usize::from(c.style) + 1) / 4).clamp(3, 18)
}

/// The view cell slots, in the panel beside the terminal (base_terminal.json `viewCells`, right -2).
fn view_cells(width: f32) -> impl Iterator<Item = Option<[f32; 2]>> {
    (0..VIEW_CELLS).map(move |i| Some([width + 2.0 + 8.0, 8.0 + 18.0 * i as f32]))
}

/// `screen`: the block's layout, built by the client as the screen opens (`devices::menu_screen`). What it hides
/// the server refuses in its slot rules (`devices::hidden_slot`, `bus::on_open_side`).
pub(crate) fn screen(block: &str) -> Option<MenuScreenLayout> {
    let base = |width: f32, height: f32, slots: Vec<Option<[f32; 2]>>, inventory_y: f32| MenuScreenLayout {
        width,
        height,
        slots: slots.into_iter().map(|s| s.map(crate::util::slot_at)).collect(),
        inventory_y,
        inventory_x: 8.0,
        own_labels: true,
        own_background: true,
        ..MenuScreenLayout::default()
    };
    match block {
        // drive.json: 176 by 199, cells from 71, 14 in two columns.
        // spatial_io_port.json: 176 by 197, the cell in at 52,48 and out at 113,48.
        "ae2:spatial_io_port" => Some(base(176.0, 197.0, vec![Some([52.0, 48.0]), Some([113.0, 48.0])], 115.0)),
        "ae2:drive" => Some(base(176.0, 199.0, (0..10).map(|i| Some([71.0 + (i % 2) as f32 * 18.0, 14.0 + (i / 2) as f32 * 18.0])).collect(), 117.0)),
        "ae2:chest" => {
            let term = if on_server_thread() {
                SERVER_CHEST_TERM.with(std::cell::Cell::get)
            } else {
                with_client(|c| {
                    // The screen is laid out once as it opens: a top click just before means its terminal.
                    if c.chest_top.take().is_some_and(|t| t.elapsed().as_secs_f32() < 2.0) {
                        c.chest_term = true;
                    }
                    c.chest_term
                })
            };
            if term {
                let rows = with_client(|c| {
                    c.rows = term_rows(c, 97.0);
                    c.rows
                });
                let h = 17.0 + 18.0 * rows as f32 + 97.0;
                Some(base(195.0, h, vec![None], h - 82.0))
            } else {
                // chest.json: the cell at 80, 37.
                Some(base(176.0, 166.0, vec![Some([80.0, 37.0])], 84.0))
            }
        }
        "ae2:terminal" => {
            let rows = with_client(|c| {
                c.rows = term_rows(c, 97.0);
                c.rows
            });
            let h = 17.0 + 18.0 * rows as f32 + 97.0;
            Some(base(195.0, h, std::iter::once(Some(HIDDEN_SLOT)).chain(view_cells(195.0)).collect(), h - 82.0))
        }
        "ae2:pattern_encoding_terminal" => {
            // pattern_encoding_terminal.json over pattern.png: the rows, then 178 pixels: the blank pattern at 147, 17
            // below the rows' foot, the encoded one at 147, 60 below it, the inventory 96 below it.
            let rows = with_client(|c| {
                c.rows = term_rows(c, 178.0);
                c.rows
            });
            let foot = 17.0 + 18.0 * rows as f32;
            let mut slots = vec![Some(HIDDEN_SLOT), Some([147.0, foot + 17.0]), Some([147.0, foot + 60.0])];
            slots.extend(view_cells(195.0));
            Some(base(195.0, foot + 178.0, slots, foot + 96.0))
        }
        "ae2:crafting_terminal" => {
            let rows = with_client(|c| {
                c.rows = term_rows(c, 170.0);
                c.rows
            });
            let h = 17.0 + 18.0 * rows as f32 + 170.0;
            // crafting_terminal.json: the grid from 37 at 154 above the foot, the result (no slot) at 131, 136 above.
            let mut slots = vec![Some(HIDDEN_SLOT)];
            slots.extend((0..9).map(|i| Some([37.0 + (i % 3) as f32 * 18.0, h - 154.0 + (i / 3) as f32 * 18.0])));
            slots.push(None);
            slots.extend(view_cells(195.0));
            Some(base(195.0, h, slots, h - 82.0))
        }
        _ => None,
    }
}

/// Whether the open ME chest screen is its cell's terminal (not its cell slot).
pub(crate) fn chest_term_open() -> bool {
    with_client(|c| c.chest_term)
}

fn is_term_screen(block: &str) -> bool {
    is_terminal(block) || crate::wireless::is_item_term(block) || (block == "ae2:chest" && with_client(|c| c.chest_term))
}

/// Whether this terminal screen has the crafting grid (the crafting terminal, the wireless crafting terminal).
fn has_crafting(block: &str) -> bool {
    block == "ae2:crafting_terminal" || block == crate::wireless::WCT
}

/// Whether this terminal screen has the crafting status tab (a network's terminals; not the ME chest's own, nor a
/// portable cell's).
fn has_status_tab(block: &str) -> bool {
    block != "ae2:chest" && !crate::wireless::is_portable(block)
}

/// The rows of an item's terminal screen (wireless terminals follow the terminal style, a portable cell has `fixed`
/// rows), laid out now for a screen `bottom` pixels below the rows.
pub(crate) fn item_rows(gui: [f32; 2], bottom: f32, fixed: Option<usize>) -> usize {
    with_client(|c| {
        c.gui = gui;
        c.rows = fixed.unwrap_or_else(|| term_rows(c, bottom));
        c.rows
    })
}

// ---- Drawing

pub(crate) fn spr(d: &mut dyn DrawContext, tex: &str, o: [f32; 2], at: [f32; 2], src: [f32; 4]) {
    d.sprite(tex, src, [o[0] + at[0], o[1] + at[1], src[2], src[3]], None);
}

pub(crate) fn tr(host: &dyn ClientHost, key: &str, fallback: &str) -> String {
    host.translate(key).unwrap_or_else(|| fallback.to_owned())
}

pub(crate) fn state_button(d: &mut dyn DrawContext, o: [f32; 2], at: [f32; 2], icon: [f32; 2]) {
    spr(d, "ae2:gui/states", o, at, [240.0, 240.0, 16.0, 16.0]);
    spr(d, "ae2:gui/states", o, at, [icon[0], icon[1], 16.0, 16.0]);
}

/// JEI's search text as of the last client tick; None without JEI.
static EXTERNAL: Mutex<Option<String>> = Mutex::new(None);

/// JEI's search text changed hands (each client tick): with "Use JEI" an open terminal filters by it, its own box
/// showing the same words (AE2's `useExternalSearch`).
pub(crate) fn external_search(text: String) {
    let mut last = EXTERNAL.lock().unwrap_or_else(|e| e.into_inner());
    if last.as_deref() == Some(text.as_str()) {
        return;
    }
    *last = Some(text.clone());
    drop(last);
    if crate::config::config().settings[crate::config::S_EXTERNAL_SEARCH] {
        with_client(|c| {
            if c.search != text {
                c.search = text;
                c.scroll = 0;
                c.tag_hits.clear();
            }
        });
    }
}

/// The side panels drawn this frame, and those of the frame before: what `screen_exclusion_areas` reports, so a
/// recipe viewer's item panel keeps clear of the upgrade and view cell columns (JEI's `getGuiExtraAreas`).
static PANELS: Mutex<(Vec<[f32; 4]>, Vec<[f32; 4]>)> = Mutex::new((Vec::new(), Vec::new()));

/// Where an AE2 screen draws beyond its window: the upgrade and view cell columns, and the toolbox belt. What a
/// recipe viewer's item list keeps clear of, reported both to the game (`screen_exclusion_areas`) and to JEI
/// (`jei::api::register_global_extra_areas`, JEI's `getGuiExtraAreas`).
pub(crate) fn extra_areas(host: &dyn ClientHost, screen: &ScreenView<'_>) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    if screen.menu.is_some_and(|m| m.starts_with("ae2:")) {
        out.extend(panels());
        out.extend(crate::toolbox::belt_area(host, screen));
    } else if crate::wireless::view(host, screen).is_some() {
        // A wireless terminal's screen opens on another menu type: its panels too.
        out.extend(panels());
    }
    out
}

/// The frame's drawing ended: its panels become the ones reported.
pub(crate) fn panels_frame_end() {
    let mut p = PANELS.lock().unwrap_or_else(|e| e.into_inner());
    p.1 = std::mem::take(&mut p.0);
}

/// The screen closed: no panels.
pub(crate) fn panels_clear() {
    let mut p = PANELS.lock().unwrap_or_else(|e| e.into_inner());
    p.0.clear();
    p.1.clear();
}

/// The side panels of the open AE2 screen, in GUI pixels (`[x, y, width, height]`).
pub(crate) fn panels() -> Vec<[f32; 4]> {
    PANELS.lock().unwrap_or_else(|e| e.into_inner()).1.clone()
}

/// The panel AE2 draws beside a screen for upgrades and view cells (extra_panels.png).
pub(crate) fn side_panel(d: &mut dyn DrawContext, o: [f32; 2], width: f32, n: usize) {
    let x = width + 2.0;
    PANELS.lock().unwrap_or_else(|e| e.into_inner()).0.push([o[0] + x, o[1], 32.0, 14.0 + 18.0 * n as f32]);
    spr(d, "ae2:gui/extra_panels", o, [x, 0.0], [0.0, 0.0, 32.0, 7.0]);
    for i in 0..n {
        spr(d, "ae2:gui/extra_panels", o, [x, 7.0 + 18.0 * i as f32], [0.0, 7.0, 32.0, 18.0]);
    }
    spr(d, "ae2:gui/extra_panels", o, [x, 7.0 + 18.0 * n as f32], [0.0, 25.0, 32.0, 7.0]);
}

pub(crate) fn screen_draw(host: &dyn ClientHost, block: &str, d: &mut dyn DrawContext, o: [f32; 2]) {
    with_client(|c| c.gui = d.size());
    let inv_label = tr(host, "container.inventory", "Inventory");
    match block {
        "ae2:drive" => {
            spr(d, "ae2:gui/drive", o, [0.0, 0.0], [0.0, 0.0, 176.0, 199.0]);
            d.text(&tr(host, "gui.ae2.Drive", "ME Drive"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
            d.text(&inv_label, [o[0] + 8.0, o[1] + 199.0 - 93.0], TEXT, false);
        }
        "ae2:spatial_io_port" => {
            spr(d, "ae2:gui/spatialio", o, [0.0, 0.0], [0.0, 0.0, 176.0, 197.0]);
            d.text(&tr(host, "gui.ae2.SpatialIOPort", "Spatial IO Port"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
            d.text(&inv_label, [o[0] + 8.0, o[1] + 197.0 - 93.0], TEXT, false);
        }
        "ae2:chest" if !with_client(|c| c.chest_term) => {
            spr(d, "ae2:gui/chest", o, [0.0, 0.0], [0.0, 0.0, 176.0, 166.0]);
            d.text(&tr(host, "gui.ae2.Chest", "ME Chest"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
            d.text(&inv_label, [o[0] + 8.0, o[1] + 166.0 - 93.0], TEXT, false);
        }
        _ if is_term_screen(block) => draw_terminal(host, block, d, o),
        _ => {}
    }
}

/// `text` cut to `width` GUI pixels, ending in "…" when it was cut.
pub(crate) fn fit_text(d: &dyn DrawContext, text: &str, width: f32) -> String {
    if d.text_width(text) <= width {
        return text.to_owned();
    }
    let mut cut: String = text.to_owned();
    while !cut.is_empty() && d.text_width(&format!("{cut}…")) > width {
        cut.pop();
    }
    format!("{}…", cut.trim_end())
}

/// MEStorageScreen: the terminal style's header, rows and bottom, the toolbar, search box, grid and scroll bar.
fn draw_terminal(host: &dyn ClientHost, block: &str, d: &mut dyn DrawContext, o: [f32; 2]) {
    let crafting = has_crafting(block);
    let encoder = block == "ae2:pattern_encoding_terminal";
    let tex = if crafting {
        "ae2:gui/crafting"
    } else if encoder {
        "ae2:gui/pattern"
    } else {
        "ae2:gui/terminal"
    };
    let bottom = if crafting {
        170.0
    } else if encoder {
        178.0
    } else {
        97.0
    };
    let rows = with_client(|c| c.rows.max(3));
    if is_terminal(block) {
        side_panel(d, o, 195.0, VIEW_CELLS);
    }
    spr(d, tex, o, [0.0, 0.0], [0.0, 0.0, 195.0, 17.0]);
    for r in 0..rows {
        let src = if r == 0 { 17.0 } else if r + 1 == rows { 53.0 } else { 35.0 };
        spr(d, tex, o, [0.0, 17.0 + 18.0 * r as f32], [0.0, src, 195.0, 18.0]);
    }
    let foot = 17.0 + 18.0 * rows as f32;
    spr(d, tex, o, [0.0, foot], [0.0, 71.0, 195.0, bottom]);
    let height = foot + bottom;
    let title = if block == "ae2:chest" {
        tr(host, "gui.ae2.Chest", "ME Chest")
    } else if let Some(t) = crate::wireless::title(host, block) {
        t
    } else if encoder {
        tr(host, "gui.ae2.PatternEncodingTerminal", "Pattern Encoding Terminal")
    } else {
        tr(host, "gui.ae2.Terminal", "Terminal")
    };
    // The search box starts at x 80: a longer title (the wireless terminals') ends in "…" before it.
    d.text(&fit_text(d, &title, 70.0), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
    d.text(&tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + height - 93.0], TEXT, false);
    if encoder {
        crate::craft_screens::draw_encoder(host, d, o, foot);
    }
    if crafting {
        d.text(&tr(host, "gui.ae2.CraftingTerminal", "Crafting Terminal"), [o[0] + 8.0, o[1] + height - 168.0], TEXT, false);
        for (x, icon) in [(STASH_X[0], [0.0, 48.0]), (STASH_X[1], [16.0, 48.0])] {
            let at = [o[0] + x, o[1] + foot + STASH_Y];
            d.sprite("ae2:gui/states", [240.0, 240.0, 16.0, 16.0], [at[0], at[1], 8.0, 8.0], None);
            d.sprite("ae2:gui/states", [icon[0], icon[1], 16.0, 16.0], [at[0], at[1], 8.0, 8.0], None);
        }
        if let Some(result) = with_client(|c| c.craft_result.clone()) {
            d.gui_draw_item_with(&result, [o[0] + 131.0, o[1] + height - 136.0], None);
        }
    }
    let (search, focus, icons) = with_client(|c| (c.search.clone(), c.search_focus, toolbar_icons(c)));
    // AETextField: text_field.png's ends and middle, the focused look 24 down.
    let [sx, sy, sw, _] = SEARCH_BOX;
    let v = if focus { 24.0 } else { 0.0 };
    d.sprite("ae2:gui/text_field", [0.0, v, 1.0, 12.0], [o[0] + sx, o[1] + sy, 1.0, 12.0], None);
    d.sprite("ae2:gui/text_field", [1.0, v, sw - 2.0, 12.0], [o[0] + sx + 1.0, o[1] + sy, sw - 2.0, 12.0], None);
    d.sprite("ae2:gui/text_field", [127.0, v, 1.0, 12.0], [o[0] + sx + sw - 1.0, o[1] + sy, 1.0, 12.0], None);
    if search.is_empty() && !focus {
        d.text(&tr(host, "gui.ae2.SearchPlaceholder", "Search..."), [o[0] + sx + 2.0, o[1] + sy + 2.0], 0xFFAE_AEAE, false);
    } else {
        let shown = if focus { format!("{search}_") } else { search };
        d.text(&shown, [o[0] + sx + 2.0, o[1] + sy + 2.0], 0xFFFF_FFFF, true);
    }
    for (at, icon) in TOOLBAR.iter().zip(icons) {
        state_button(d, o, *at, icon);
    }
    let (jobs, job_keys) = crate::craft_screens::jobs();
    if has_status_tab(block) {
        // The crafting status tab, with how many of the network's CPUs are busy.
        let [tx, ty, tw, th] = STATUS_TAB;
        d.sprite("ae2:gui/states", [128.0, 192.0, tw, th], [o[0] + tx, o[1] + ty, tw, th], None);
        d.sprite("ae2:gui/states", [48.0, 144.0, 16.0, 16.0], [o[0] + tx + 4.0, o[1] + ty + 3.0, 16.0, 16.0], None);
        crate::craft_screens::slot_amount(d, &jobs.to_string(), [o[0] + tx + 4.0, o[1] + ty + 3.0]);
    }
    with_client(|c| c.drawn = Some(Instant::now()));
    let online = with_client(|c| c.online || c.items.iter().any(Option::is_some));
    if !online {
        // AE2 dims an unconnected terminal's grid.
        d.fill([o[0] + TERM_GRID[0], o[1] + TERM_GRID[1], 18.0 * TERM_COLS as f32 - 2.0, 18.0 * rows as f32 - 2.0], 0xB010_1010);
        let text = tr(host, "gui.ae2.NoNetwork", "Network offline");
        let w = d.text_width(&text);
        d.text(&text, [o[0] + 8.0 + 81.0 - w / 2.0, o[1] + 18.0 + 9.0 * rows as f32 - 4.0], 0xFFFF_6060, true);
        return;
    }
    prepare(host);
    let pinned = with_client(|c| c.pinned.clone());
    let (shown, scroll) = with_client(|c| {
        let shown = visible(c);
        let most = shown.len().div_ceil(TERM_COLS).saturating_sub(rows);
        c.scroll = c.scroll.min(most);
        let start = c.scroll * TERM_COLS;
        let list: Vec<Entry> = shown.iter().skip(start).take(TERM_COLS * rows).filter_map(|&i| c.items.get(i).cloned().flatten()).collect();
        (list, (c.scroll, most))
    });
    {
        // The page as listed (names in order), for the tests' clicks.
        static LAST: Mutex<String> = Mutex::new(String::new());
        let names = shown.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join("|");
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if *last != names {
            debug(&format!("visible: {names}"));
            *last = names;
        }
    }
    for (i, e) in shown.iter().enumerate() {
        let at = [o[0] + TERM_GRID[0] + (i % TERM_COLS) as f32 * 18.0, o[1] + TERM_GRID[1] + (i / TERM_COLS) as f32 * 18.0];
        // AE2's amounts in the slot's corner (AmountFormat.SLOT_LARGE_FONT: the fork's GUI text has one size).
        // A craftable with none stored: AE2's "Craft" label.
        let label = if e.amount == 0 { tr(host, "gui.ae2.Craft", "Craft") } else { e.key.format_amount(e.amount.max(0), AmountFormat::Slot) };
        // AE2's small amounts (StackSizeRenderer at half size).
        if e.key.key_type() == KeyType::Fluids {
            d.fluid(e.key.id(), at, 16.0);
        } else if let Some(stack) = &e.stack {
            d.gui_draw_item_with(stack, at, None);
        }
        crate::craft_screens::slot_amount(d, &label, at);
        // A pinned entry a CPU is crafting: the assembler's lights around it (AE2's pinned crafting animation).
        if pinned.contains(&e.key) && job_keys.contains(&e.key) {
            static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
            let frame = (START.get_or_init(Instant::now).elapsed().as_millis() / 100 % 12) as f32;
            d.sprite("ae2:gui/hz/molecular_assembler_lights", [2.0, 2.0 + frame * 16.0, 12.0, 12.0], [at[0] - 1.0, at[1] - 1.0, 18.0, 18.0], None);
        }
    }
    // The scroll bar's handle (vanilla's creative scroller, as AE2's Scrollbar), grey with nothing to scroll.
    let track = 18.0 * rows as f32 - 2.0;
    let (sprite, dy) = if scroll.1 == 0 {
        ("container/creative_inventory/scroller_disabled", 0.0)
    } else {
        ("container/creative_inventory/scroller", ((track - 15.0) * scroll.0 as f32 / scroll.1 as f32).floor())
    };
    d.sprite(sprite, [0.0, 0.0, 12.0, 15.0], [o[0] + 175.0, o[1] + 18.0 + dy, 12.0, 15.0], None);
}

/// SettingToggleButton's icons as the settings stand (states.png), the gear (Icon.COG) among them.
fn toolbar_icons(c: &Client) -> [[f32; 2]; 6] {
    [
        [[0.0, 64.0], [16.0, 64.0], [96.0, 64.0]][usize::from(c.sort.min(2))],
        [[32.0, 16.0], [0.0, 16.0], [48.0, 16.0]][usize::from(c.view_mode.min(2))],
        [[160.0, 16.0], [128.0, 16.0], [144.0, 16.0]][usize::from(c.type_filter.min(2))],
        if c.sort_desc { [16.0, 48.0] } else { [0.0, 48.0] },
        [32.0, 64.0],
        [[0.0, 208.0], [16.0, 208.0], [32.0, 208.0], [48.0, 208.0]][usize::from(c.style.min(3))],
    ]
}

/// The toolbar buttons' tooltips: the setting's name and its value as it stands.
fn toolbar_words(c: &Client) -> [((&'static str, &'static str), &'static str); 6] {
    [
        (("gui.tooltips.ae2.SortBy", "Sort By"), ["Item name", "Number of items", "Mod"][usize::from(c.sort.min(2))]),
        (("gui.tooltips.ae2.View", "View"), ["Stored / Craftable", "Stored Items", "Craftable"][usize::from(c.view_mode.min(2))]),
        (("gui.tooltips.ae2.TypeFilter", "Filter Types"), ["Show All", "Show Items only", "Show Fluids only"][usize::from(c.type_filter.min(2))]),
        (("gui.tooltips.ae2.SortOrder", "Sort Order"), if c.sort_desc { "Descending" } else { "Ascending" }),
        (("gui.tooltips.ae2.TerminalSettings", "Terminal Settings"), ""),
        (
            ("gui.tooltips.ae2.TerminalStyle", "Terminal Style"),
            ["Small Centered Terminal", "Medium Centered Terminal", "Tall Centered Terminal", "Full-Height Terminal"][usize::from(c.style.min(3))],
        ),
    ]
}

/// Names and stacks for entries that haven't them yet (the search and drawing need a host); for a `#` search their
/// tooltips, for a `$` search whether they are in the tags its words name (upstream's `search_prepare`).
fn prepare(host: &dyn ClientHost) {
    with_client(|c| {
        let tips = c.search.contains('#');
        let mut changed = false;
        for e in c.items.iter_mut().flatten() {
            if e.name.is_empty() {
                e.name = key_name(host, &e.key).to_lowercase();
                if e.stack.is_none() {
                    e.stack = stack_of(host, &e.key, 1);
                }
            }
            if tips && e.tips.is_none() {
                // The standard tooltip lines (no mod's), as AE2's tooltip search reads them.
                let lines = e.stack.as_ref().and_then(|s| ItemResource::of(s).tooltip(host, false, false)).unwrap_or_default();
                let mut text: Vec<String> = lines
                    .iter()
                    .filter_map(|l| match l {
                        TooltipElement::Text(t, _) => Some(host.text_string(t)),
                        _ => None,
                    })
                    .collect();
                if text.is_empty() {
                    text.push(e.name.clone());
                }
                e.tips = Some(text.join("\n").to_lowercase());
                changed = true;
            }
        }
        // `$` words: each one's tags resolved once, each entry asked once.
        let words: Vec<String> = c.search.to_lowercase().split(['|', ' ']).filter_map(|w| w.strip_prefix('$')).filter(|w| !w.is_empty()).map(str::to_owned).collect();
        c.tag_hits.retain(|w, _| words.contains(w));
        for w in words {
            let hits = c.tag_hits.entry(w.clone()).or_insert_with(|| TagHits {
                tags: tag_candidates(&w)
                    .into_iter()
                    .filter_map(|name| {
                        let id = Identifier::parse(&name).ok()?;
                        Some((host.tags().key(&id)?, name))
                    })
                    .collect(),
                ..TagHits::default()
            });
            for e in c.items.iter().flatten() {
                let id = tag_id(&e.key);
                if hits.checked.contains(id.as_ref()) {
                    continue;
                }
                let tagged = if e.key.key_type() == KeyType::Fluids {
                    hits.tags.iter().any(|(_, name)| host.fluid_in_tag(e.key.id(), name) == Some(true))
                } else {
                    crate::cells::item_key(host, e.key.id()).is_some_and(|item| hits.tags.iter().any(|(tag, _)| host.tags().is_in_tag(item, *tag) == Some(true)))
                };
                if tagged {
                    hits.hits.insert(id.to_string());
                }
                hits.checked.insert(id.into_owned());
                changed = true;
            }
        }
        if changed {
            c.view = None;
        }
    });
}

/// What tags are asked of: the item's id, a fluid's marked apart.
fn tag_id(key: &AeKey) -> std::borrow::Cow<'_, str> {
    if key.key_type() == KeyType::Fluids { format!("~{}", key.id()).into() } else { key.id().into() }
}

/// The tags a `$` word names. Sunrise lists no tag's name (Tags reads by name only), so AE2's "any tag whose name holds
/// the word" is narrowed to the names it most likely means: the word itself as a tag id (`$c:ingots`), else in the
/// common, vanilla and AE2 namespaces, singular and plural (`$ingot` is `c:ingots`, `$logs` is `minecraft:logs`), with
/// any path (`$ingots/iron`).
fn tag_candidates(word: &str) -> Vec<String> {
    let word = word.trim_start_matches('#');
    if word.len() < 2 || word.len() > 64 || !word.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':')) {
        return Vec::new();
    }
    if word.contains(':') {
        return vec![word.to_owned()];
    }
    let mut forms = vec![word.to_owned()];
    match word.strip_suffix('s') {
        Some(one) if !one.is_empty() && !one.ends_with('/') => forms.push(one.to_owned()),
        _ => forms.push(format!("{word}s")),
    }
    ["c", "minecraft", "ae2"].iter().flat_map(|ns| forms.iter().map(move |f| format!("{ns}:{f}"))).collect()
}

/// RepoSearch: `|` between alternatives, spaces between the words each needs; `@mod`, `#tooltip`, `$tag`, `*id`, or
/// the name. `query` lower case; `tips` the tooltip (None: not read yet, the name stands in); `tagged` answers `$`.
fn search_matches(query: &str, name: &str, id: &str, tips: Option<&str>, tagged: &dyn Fn(&str) -> bool) -> bool {
    let ns = id.split_once(':').map_or("minecraft", |(n, _)| n);
    query.split('|').any(|part| {
        part.split_whitespace().all(|w| {
            if let Some(m) = w.strip_prefix('@') {
                ns.contains(m)
            } else if let Some(t) = w.strip_prefix('#') {
                tips.unwrap_or(name).contains(t)
            } else if let Some(t) = w.strip_prefix('$') {
                t.is_empty() || tagged(t)
            } else if let Some(t) = w.strip_prefix('*') {
                id.contains(t)
            } else {
                name.contains(w)
            }
        })
    })
}

/// An item's or fluid's name as the lang has it.
pub(crate) fn key_name(host: &dyn ClientHost, key: &AeKey) -> String {
    if key.key_type() == KeyType::Fluids {
        return host.fluid_name(key.id());
    }
    let (ns, path) = key.id().split_once(':').unwrap_or(("minecraft", key.id()));
    host.translate(&format!("item.{ns}.{path}"))
        .or_else(|| host.translate(&format!("block.{ns}.{path}")))
        .unwrap_or_else(|| path.replace('_', " "))
}

/// What the grid shows, as indexes into the list: made again only after a change.
fn visible(c: &mut Client) -> Vec<usize> {
    if let Some(v) = &c.view {
        return v.clone();
    }
    let query = c.search.to_lowercase();
    let keeps = |e: &Entry| {
        let id = tag_id(&e.key);
        let tagged = |w: &str| c.tag_hits.get(w).is_some_and(|h| h.hits.contains(id.as_ref()));
        search_matches(&query, &e.name, e.key.id(), e.tips.as_deref(), &tagged)
    };
    let mut out: Vec<usize> = (0..c.items.len())
        .filter(|&i| {
            let Some(e) = &c.items[i] else { return false };
            keeps(e)
                && match c.view_mode {
                    1 => e.amount > 0,
                    2 => e.amount == 0,
                    _ => true,
                }
                && match c.type_filter {
                    1 => e.key.key_type() == KeyType::Items,
                    2 => e.key.key_type() == KeyType::Fluids,
                    _ => true,
                }
        })
        .collect();
    let entry = |i: usize| c.items[i].as_ref().expect("filtered");
    match c.sort {
        1 => out.sort_by(|&a, &b| entry(a).amount.cmp(&entry(b).amount).then_with(|| entry(a).name.cmp(&entry(b).name))),
        2 => out.sort_by(|&a, &b| entry(a).key.mod_id().cmp(entry(b).key.mod_id()).then_with(|| entry(a).name.cmp(&entry(b).name))),
        _ => out.sort_by(|&a, &b| entry(a).name.cmp(&entry(b).name)),
    }
    if c.sort_desc {
        out.reverse();
    }
    // Pinned auto-crafted items first, in the order they were asked for.
    if !c.pinned.is_empty() {
        out.sort_by_key(|&i| c.pinned.iter().position(|p| *p == entry(i).key).unwrap_or(usize::MAX));
    }
    c.view = Some(out.clone());
    out
}

/// PinnedKeys.pinKey for a crafting request: the key first in the grid, the oldest pin giving way past the first row.
pub(crate) fn pin(key: &AeKey) {
    with_client(|c| {
        if c.pinned.contains(key) {
            return;
        }
        if c.pinned.len() >= PINNED {
            c.pinned.remove(0);
        }
        c.pinned.push(key.clone());
        c.view = None;
    });
}

/// The GUI's size as last drawn (overlays laid out on it take clicks in hooks with no draw context).
pub(crate) fn gui() -> [f32; 2] {
    with_client(|c| c.gui)
}

/// The open menu's block, for its icon on a sub-screen's back button.
pub(crate) fn menu_block(screen: &ScreenView<'_>) -> String {
    if screen.menu == Some("ae2:chest") {
        return "ae2:chest".to_owned();
    }
    crate::bus::menu(screen.menu).unwrap_or("ae2:terminal").to_owned()
}

/// A terminal's screen is open (drawn within the last moment).
pub(crate) fn terminal_open() -> bool {
    with_client(|c| c.drawn.is_some_and(|t| t.elapsed().as_millis() < 250))
}

/// The terminal settings changed: the grid is listed again (the pins' order, the search).
pub(crate) fn settings_changed() {
    with_client(|c| {
        if !crate::config::config().settings[crate::config::S_PIN] {
            c.pinned.clear();
        }
        c.view = None;
    });
}

// ---- Clicks

pub(crate) fn screen_click(host: &dyn ClientHost, block: &str, at: [f32; 2], right: bool) -> Option<i32> {
    if !is_term_screen(block) {
        return None;
    }
    let inside = |r: [f32; 4]| at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3];
    let shift = host.is_key_down("key.keyboard.left.shift") || host.is_key_down("key.keyboard.right.shift");
    let rows = with_client(|c| c.rows.max(3));
    let foot = 17.0 + 18.0 * rows as f32;
    if has_crafting(block) {
        for (i, x) in STASH_X.iter().enumerate() {
            if inside([*x, foot + STASH_Y, 8.0, 8.0]) {
                return Some(if i == 0 { CT_STASH } else { CT_STASH_PLAYER });
            }
        }
        let height = foot + 170.0;
        if inside([131.0, height - 136.0, 16.0, 16.0]) {
            return Some(if shift { CT_TAKE_ALL } else { CT_TAKE });
        }
    }
    if block == "ae2:pattern_encoding_terminal"
        && let Some(id) = crate::craft_screens::encoder_click(host, at, foot, right)
    {
        return Some(id);
    }
    let in_search = inside(SEARCH_BOX);
    with_client(|c| {
        c.search_focus = in_search;
        // Right-click clears it, as AE2's does.
        if in_search && right {
            c.search.clear();
            c.view = None;
        }
    });
    if in_search {
        return None;
    }
    // The crafting status tab: CraftingStatusScreen over the terminal.
    if has_status_tab(block) && inside(STATUS_TAB) {
        if !right {
            with_client(|c| c.search_focus = false);
            crate::craft_screens::open_status();
        }
        return Some(CLIENT_ONLY);
    }
    for (i, b) in TOOLBAR.iter().enumerate() {
        if inside([b[0], b[1], 16.0, 16.0]) {
            if i == TOOL_SETTINGS {
                with_client(|c| c.search_focus = false);
                crate::term_ui::open_settings();
                return Some(CLIENT_ONLY);
            }
            with_client(|c| {
                // SettingToggleButton: a right-click the one before.
                let cycle = |v: u8, n: u8| if right { (v + n - 1) % n } else { (v + 1) % n };
                match i {
                    0 => c.sort = cycle(c.sort, 3),
                    1 => c.view_mode = cycle(c.view_mode, 3),
                    2 => c.type_filter = cycle(c.type_filter, 3),
                    3 => c.sort_desc = !c.sort_desc,
                    _ => c.style = cycle(c.style, 4),
                }
                c.view = None;
                c.scroll = 0;
            });
            return Some(CLIENT_ONLY);
        }
    }
    if inside([175.0, 18.0, 12.0, 18.0 * rows as f32 - 2.0]) {
        with_client(|c| {
            c.dragging = true;
            scroll_to(c, at[1], rows);
        });
        return None;
    }
    let [x, y] = [at[0] - TERM_GRID[0], at[1] - TERM_GRID[1]];
    if x < 0.0 || y < 0.0 || x >= TERM_COLS as f32 * 18.0 || y >= rows as f32 * 18.0 {
        return None;
    }
    let cell = with_client(|c| c.scroll * TERM_COLS) + (y / 18.0) as usize * TERM_COLS + (x / 18.0) as usize;
    let index = with_client(|c| visible(c).get(cell).copied()).unwrap_or(EMPTY_CELL);
    // Space held over the grid: MOVE_REGION (AE2 reads the raw key).
    let action = if host.is_key_down("key.keyboard.space") {
        MOVE_REGION
    } else {
        (if right { RIGHT } else { 0 }) | (if shift { SHIFT } else { 0 })
    };
    debug(&format!("grid click {index} action {action} ({} entries, {} shown)", with_client(|c| c.items.len()), with_client(|c| visible(c).len())));
    Some(((index as i32) << 3) | action)
}

fn scroll_to(c: &mut Client, mouse_y: f32, rows: usize) {
    let shown = visible(c).len();
    let most = shown.div_ceil(TERM_COLS).saturating_sub(rows);
    let track = 18.0 * rows as f32 - 2.0;
    let travel = (track - 15.0).max(1.0);
    c.scroll = (((mouse_y - 18.0 - 7.5) / travel).clamp(0.0, 1.0) * most as f32).round() as usize;
}

/// The entry under the pointer: its index in the server's list, its key and amount.
pub(crate) fn entry_index_at(at: [f32; 2]) -> Option<(usize, AeKey, i64)> {
    let rows = with_client(|c| c.rows.max(3));
    let [x, y] = [at[0] - TERM_GRID[0], at[1] - TERM_GRID[1]];
    if x < 0.0 || y < 0.0 || x >= TERM_COLS as f32 * 18.0 || y >= rows as f32 * 18.0 || x % 18.0 >= 16.0 || y % 18.0 >= 16.0 {
        return None;
    }
    with_client(|c| {
        let cell = c.scroll * TERM_COLS + (y / 18.0) as usize * TERM_COLS + (x / 18.0) as usize;
        let i = *visible(c).get(cell)?;
        let e = c.items.get(i).cloned().flatten()?;
        Some((i, e.key, e.amount))
    })
}

/// The terminal's rows as laid out.
pub(crate) fn rows_now() -> usize {
    with_client(|c| c.rows.max(3))
}

// ---- enc: shared with the encoder's panel and the pattern access terminal

/// The rows' foot (where a terminal's bottom part starts) as laid out.
pub(crate) fn foot_now() -> f32 {
    17.0 + 18.0 * rows_now() as f32
}

/// The rows the terminal style gives a screen `bottom` pixels below its rows (TerminalStyle.getRows).
pub(crate) fn style_rows(bottom: f32) -> usize {
    with_client(|c| term_rows(c, bottom))
}

/// The terminal style (small, medium, tall, full).
pub(crate) fn style() -> u8 {
    with_client(|c| c.style)
}

/// The terminal style's toolbar button (right: the one before).
pub(crate) fn cycle_style(back: bool) {
    with_client(|c| {
        c.style = if back { (c.style + 3) % 4 } else { (c.style + 1) % 4 };
        c.view = None;
        c.scroll = 0;
    });
}

/// The server's list index of `key`, for a request (`plan`).
pub(crate) fn index_of(key: &AeKey) -> Option<usize> {
    with_client(|c| c.items.iter().position(|e| e.as_ref().is_some_and(|e| e.key == *key)))
}

/// A menu button for the next client tick (a hook that can't press it).
pub(crate) fn queue_button(id: i32) {
    with_client(|c| c.queued.push(id));
}

/// The entry under the pointer.
fn entry_at(at: [f32; 2]) -> Option<Entry> {
    let rows = with_client(|c| c.rows.max(3));
    let [x, y] = [at[0] - TERM_GRID[0], at[1] - TERM_GRID[1]];
    if x < 0.0 || y < 0.0 || x >= TERM_COLS as f32 * 18.0 || y >= rows as f32 * 18.0 || x % 18.0 >= 16.0 || y % 18.0 >= 16.0 {
        return None;
    }
    with_client(|c| {
        let cell = c.scroll * TERM_COLS + (y / 18.0) as usize * TERM_COLS + (x / 18.0) as usize;
        visible(c).get(cell).and_then(|&i| c.items.get(i).cloned().flatten())
    })
}

pub(crate) fn screen_tooltip(host: &dyn ClientHost, block: &str, at: [f32; 2], lines: &mut Vec<String>) {
    if !is_term_screen(block) || crate::craft_screens::covered() || crate::term_ui::settings_open() {
        return;
    }
    if let Some(e) = entry_at(at) {
        lines.push(key_name(host, &e.key));
        lines.push(format!("§7{}", e.key.format_amount(e.amount, AmountFormat::Full)));
        return;
    }
    // enc: the encoder's panel.
    if block == "ae2:pattern_encoding_terminal" && crate::encoder::tooltip(host, at, foot_now(), lines) {
        return;
    }
    let inside = |r: [f32; 4]| at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3];
    if has_status_tab(block) && inside(STATUS_TAB) {
        lines.push(tr(host, "gui.ae2.CraftingStatus", "Crafting Status"));
        return;
    }
    // MEStorageScreen.updateSearch: what the box understands.
    if inside(SEARCH_BOX) {
        for (key, fallback) in [
            ("gui.ae2.SearchTooltip", "Search in Name"),
            ("gui.ae2.SearchTooltipModId", "Use @ to search by mod (@ae2)"),
            ("gui.ae2.SearchTooltipTag", "Use $ to search by tags ($ingot)"),
            ("gui.ae2.SearchTooltipToolTips", "Use # to search in tooltips (#looting)"),
            ("gui.ae2.SearchTooltipItemId", "Use * to search by id (*cell)"),
        ] {
            lines.push(tr(host, key, fallback));
        }
        return;
    }
    let words = with_client(|c| toolbar_words(c));
    for (b, ((key, fallback), value)) in TOOLBAR.iter().zip(words) {
        if inside([b[0], b[1], 16.0, 16.0]) {
            lines.push(tr(host, key, fallback));
            if !value.is_empty() {
                lines.push(format!("§7{value}"));
            }
            return;
        }
    }
}

// ---- Input through the global screen hooks

pub(crate) fn our_menu(screen: &ScreenView<'_>) -> bool {
    matches!(crate::bus::menu(screen.menu), Some("ae2:terminal" | "ae2:crafting_terminal" | "ae2:pattern_encoding_terminal"))
        || screen.menu.is_some_and(crate::wireless::is_item_term)
        || (screen.menu == Some("ae2:chest") && with_client(|c| c.chest_term))
}

/// The wheel over a terminal: the grid's rows; with shift over an entry, one in or out (ROLL_UP / ROLL_DOWN).
pub(crate) fn scrolled(host: &dyn ClientHost, screen: &ScreenView<'_>, scroll: f64) -> bool {
    if !our_menu(screen) || scroll == 0.0 {
        return false;
    }
    let at = [screen.mouse[0] - screen.rect[0], screen.mouse[1] - screen.rect[1]];
    let shift = host.is_key_down("key.keyboard.left.shift") || host.is_key_down("key.keyboard.right.shift");
    if shift && let Some(e) = entry_at(at) {
        let index = with_client(|c| c.items.iter().position(|x| x.as_ref().is_some_and(|x| x.key == e.key)));
        if let Some(i) = index {
            with_client(|c| c.queued.push(((i as i32) << 3) | if scroll > 0.0 { ROLL_UP } else { ROLL_DOWN }));
        }
        return true;
    }
    with_client(|c| {
        let rows = c.rows.max(3);
        let most = visible(c).len().div_ceil(TERM_COLS).saturating_sub(rows);
        c.scroll = if scroll > 0.0 { c.scroll.saturating_sub(1) } else { (c.scroll + 1).min(most) };
    });
    true
}

pub(crate) fn dragged(screen: &ScreenView<'_>) -> bool {
    if crate::bus::menu(screen.menu) == Some("ae2:pattern_encoding_terminal") && crate::encoder::dragged(screen.mouse[1] - screen.rect[1], foot_now()) {
        return true;
    }
    if !our_menu(screen) || !with_client(|c| c.dragging) {
        return false;
    }
    let y = screen.mouse[1] - screen.rect[1];
    with_client(|c| {
        let rows = c.rows.max(3);
        scroll_to(c, y, rows);
    });
    true
}

pub(crate) fn released() -> bool {
    with_client(|c| std::mem::take(&mut c.dragging))
}

/// Keys while the search box has them: they type there (`e` doesn't close the screen).
pub(crate) fn key(screen: &ScreenView<'_>, key: &str) -> bool {
    if !our_menu(screen) || !with_client(|c| c.search_focus) {
        return false;
    }
    match key {
        "key.keyboard.escape" => return false,
        "key.keyboard.backspace" => with_client(|c| {
            c.search.pop();
            c.view = None;
            c.scroll = 0;
        }),
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => with_client(|c| c.search_focus = false),
        _ => {}
    }
    true
}

pub(crate) fn typed(screen: &ScreenView<'_>, ch: char) -> bool {
    if !our_menu(screen) || !with_client(|c| c.search_focus) || ch.is_control() {
        return false;
    }
    with_client(|c| {
        c.search.push(ch);
        c.view = None;
        c.scroll = 0;
    });
    true
}

pub(crate) fn closing() {
    debug("screen closing");
    with_client(|c| {
        c.search_focus = false;
        c.craft_result = None;
        c.dragging = false;
    });
}

/// The client used a block: an ME chest's top (not sneaking) opens its terminal (ChestBlock.useWithoutItem).
pub(crate) fn clicked_block(host: &dyn ClientHost, pos: Pos, face: [i32; 3]) {
    let Some(level) = host.level(host.dimension()) else { return };
    if level.block(host, pos).and_then(|b| b.id(host).map(str::to_owned)).as_deref() != Some("ae2:chest") {
        return;
    }
    let facing = facing_dir(level.block_property(host, pos, "facing").as_deref());
    let top = if facing[1] != 0 { [0, 0, -1] } else { [0, 1, 0] };
    let sneak = host.is_key_down("key.keyboard.left.shift");
    with_client(|c| {
        if face == top && !sneak {
            c.chest_top = Some(Instant::now());
        } else {
            // Its side: the cell slot's screen.
            c.chest_top = None;
            c.chest_term = false;
        }
    });
}

/// The buttons queued by the wheel, taken by an item screen (which has no menu to press them on).
pub(crate) fn take_queued() -> Vec<i32> {
    with_client(|c| std::mem::take(&mut c.queued))
}

/// The client's tick: menu buttons queued by hooks that can't send them.
pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    let queued = with_client(|c| std::mem::take(&mut c.queued));
    if queued.is_empty() {
        return;
    }
    let Some(menu) = host.local_player().and_then(|p| p.container_menu(&*host)) else { return };
    for b in queued {
        menu.click_button(host, b);
    }
}

/// The GUI's size, kept for laying out the next terminal.
pub(crate) fn gui_size(size: [f32; 2]) {
    with_client(|c| c.gui = size);
}

// ---- From the server

pub(crate) fn from_server(host: &dyn ClientHost, channel: &str, data: &[u8]) {
    if channel == NET_EXTRAS {
        let result = Tag::from_network_bytes(data, None).ok().and_then(|(t, _)| match t {
            Tag::Compound(c) => ItemStack::from_nbt(host, &c),
            _ => None,
        });
        with_client(|c| c.craft_result = result);
        return;
    }
    if channel != NET_ITEMS {
        return;
    }
    let text = String::from_utf8_lossy(data);
    debug(&format!("items message {} bytes", data.len()));
    let mut opened = false;
    with_client(|c| {
        for l in text.lines() {
            if l == "#reset" {
                c.items.clear();
                // The crafting result is not reset here: the server sends it (`ae2:extras`) before the list's
                // `#reset`, so a grid that already makes something shows it on opening. `closing` clears it.
                // A terminal opened: AE2's terminal settings, the search box focused, the last search kept or not.
                let s = crate::config::config().settings;
                if !s[crate::config::S_REMEMBER_SEARCH] {
                    c.search.clear();
                }
                // MEStorageScreen.init: with "Sync with JEI search" (or JEI's search in use) the box takes JEI's text.
                if s[crate::config::S_EXTERNAL_SEARCH] || s[crate::config::S_SYNC_EXTERNAL] {
                    if let Some(text) = EXTERNAL.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                        c.search = text;
                    }
                }
                c.search_focus = s[crate::config::S_AUTO_FOCUS] && !s[crate::config::S_EXTERNAL_SEARCH];
                c.scroll = 0;
                c.tag_hits.clear();
                opened = true;
                continue;
            }
            if let Some(on) = l.strip_prefix("#on ") {
                c.online = on == "1";
                continue;
            }
            let mut parts = l.splitn(3, ' ');
            let (Some(i), Some(n)) = (parts.next().and_then(|i| i.parse::<usize>().ok()), parts.next().and_then(|n| n.parse::<i64>().ok())) else {
                continue;
            };
            if i > 1 << 22 {
                continue;
            }
            if c.items.len() <= i {
                c.items.resize(i + 1, None);
            }
            match parts.next().and_then(key_from_string) {
                Some(key) => c.items[i] = Some(Entry { key, amount: n, stack: None, name: String::new(), tips: None }),
                None if n < 0 => c.items[i] = None,
                None => {
                    if let Some(e) = c.items[i].as_mut() {
                        e.amount = n;
                    }
                }
            }
        }
        c.view = None;
    });
    if opened {
        // The settings the server acts on, for this terminal.
        crate::term_ui::send_prefs();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(query: &str, name: &str, id: &str, tips: Option<&str>) -> bool {
        let tagged = |t: &str| t == "ingot" && id == "minecraft:iron_ingot";
        search_matches(&query.to_lowercase(), name, id, tips, &tagged)
    }

    #[test]
    fn search_words_and_prefixes() {
        assert!(hit("", "iron ingot", "minecraft:iron_ingot", None));
        assert!(hit("iron", "iron ingot", "minecraft:iron_ingot", None));
        assert!(hit("ingot iron", "iron ingot", "minecraft:iron_ingot", None));
        assert!(!hit("gold", "iron ingot", "minecraft:iron_ingot", None));
        assert!(hit("@minecraft", "iron ingot", "minecraft:iron_ingot", None));
        assert!(!hit("@ae2", "iron ingot", "minecraft:iron_ingot", None));
        assert!(hit("*iron_in", "iron ingot", "minecraft:iron_ingot", None));
        assert!(hit("gold|iron", "iron ingot", "minecraft:iron_ingot", None));
        assert!(!hit("gold|@ae2", "iron ingot", "minecraft:iron_ingot", None));
    }

    #[test]
    fn search_tooltips_and_tags() {
        let tips = Some("enchanted book\nlooting iii");
        assert!(hit("#looting", "enchanted book", "minecraft:enchanted_book", tips));
        assert!(!hit("#sharpness", "enchanted book", "minecraft:enchanted_book", tips));
        // A tooltip not read yet: the name stands in.
        assert!(hit("#book", "enchanted book", "minecraft:enchanted_book", None));
        assert!(hit("$ingot", "iron ingot", "minecraft:iron_ingot", None));
        assert!(!hit("$ingot", "gold block", "minecraft:gold_block", None));
        assert!(hit("$ingot @minecraft", "iron ingot", "minecraft:iron_ingot", None));
        assert!(hit("$", "gold block", "minecraft:gold_block", None));
    }

    #[test]
    fn tag_words() {
        assert_eq!(tag_candidates("ingot"), ["c:ingot", "c:ingots", "minecraft:ingot", "minecraft:ingots", "ae2:ingot", "ae2:ingots"]);
        assert_eq!(tag_candidates("logs"), ["c:logs", "c:log", "minecraft:logs", "minecraft:log", "ae2:logs", "ae2:log"]);
        assert_eq!(tag_candidates("c:ingots/iron"), ["c:ingots/iron"]);
        assert!(tag_candidates("a").is_empty());
        assert!(tag_candidates("bad word!").is_empty());
    }
}
