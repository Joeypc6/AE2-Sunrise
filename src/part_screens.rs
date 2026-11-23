//! The stage 4 screens on the client: import, export and storage buses (io_bus.json, storage_bus.json over
//! storagebus.png), the interface (interface.json), the level emitters (level_emitter.json, energy_level_emitter.json)
//! and the cell workbench (cell_workbench.json), at the positions of AE2 15.4.12's screen files, over AE2's textures.
//!
//! Their config cells are ghosts (no slots): the server sends them on `ae2:part` (`parts::stream`), a click on one is a
//! menu button (`parts::GHOST`), the left toolbar's buttons too; the emitters' level goes to the server on `ae2:part`.

use crate::cells::*;
use crate::core::{AeKey, AmountFormat, KeyType};
use crate::parts::*;
use crate::screens::{key_name, side_panel, spr, state_button, tr};
use mod_api::*;
use std::sync::Mutex;

const TEXT: u32 = 0xFF40_4040;
/// A press the client keeps (a step of the level, the level box): no server acts on it.
const CLIENT_ONLY: i32 = -1;
/// The upgrade panel's slots beside a screen `width` wide (common.json `upgrades`: right -2).
fn panel(width: f32, n: usize) -> impl Iterator<Item = Option<[f32; 2]>> {
    (0..n).map(move |i| Some([width + 2.0 + 8.0, 8.0 + 18.0 * i as f32]))
}

/// The level emitter's NumberEntryWidget: +1 +10 +100 +1000 above the box, the same less below.
const STEPS: [(f32, f32, i64); 4] = [(20.0, 22.0, 1), (48.0, 28.0, 10), (82.0, 32.0, 100), (120.0, 38.0, 1000)];
const LEVEL_ROWS: [f32; 2] = [17.0, 59.0];
/// The fuzzy mode's icons in states.png (Icon.FUZZY_IGNORE .. FUZZY_PERCENT_25) and AE2's words.
const FUZZY_BUTTON: [([f32; 2], &str); 5] = [
    ([64.0, 96.0], "Match Any"),
    ([48.0, 96.0], "Split Damaged Items"),
    ([32.0, 96.0], "Split Damage at 75%"),
    ([16.0, 96.0], "Split Damage at 50%"),
    ([0.0, 96.0], "Split Damage at 25%"),
];

#[derive(Default)]
pub(crate) struct PartClient {
    block: String,
    ghosts: Vec<Option<(AeKey, i64, Option<ItemStack>)>>,
    level: i64,
    level_text: String,
    level_focus: bool,
    target: String,
    queued: Vec<i32>,
    outgoing: Vec<String>,
}

static CLIENT: Mutex<Option<PartClient>> = Mutex::new(None);

fn with_client<R>(f: impl FnOnce(&mut PartClient) -> R) -> R {
    let mut g = CLIENT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(PartClient::default))
}

// ---- Layouts

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
    Some(match kind(block)? {
        // io_bus.json / storage_bus.json: storagebus.png, 176 by 251, the upgrade panel beside it.
        Kind::Import | Kind::Export | Kind::StorageBus => base(176.0, 251.0, panel(176.0, 5).collect(), 169.0),
        // interface.json: the stored items from 8, 71; its card beside.
        Kind::Interface => base(176.0, 205.0, (0..9).map(|i| Some([8.0 + 18.0 * i as f32, 71.0])).chain(panel(176.0, 1)).collect(), 123.0),
        Kind::LevelEmitter => base(176.0, 184.0, std::iter::once(None).chain(panel(176.0, 1)).collect(), 102.0),
        Kind::EnergyEmitter => base(176.0, 184.0, vec![None], 102.0),
        // cell_workbench.json: the cell at 152, 8; its cards beside.
        Kind::Workbench => base(176.0, 251.0, std::iter::once(Some([152.0, 8.0])).chain(panel(176.0, 4)).collect(), 169.0),
        Kind::P2p => return None,
    })
}

/// Where config cell `i` is, and how many cells the screen has.
fn cell_at(block: &str, i: usize) -> [f32; 2] {
    match kind(block) {
        Some(Kind::Interface) => [8.0 + 18.0 * i as f32, 53.0],
        Some(Kind::LevelEmitter) => [137.0, 40.0],
        _ => [8.0 + 18.0 * (i % 9) as f32, 29.0 + 18.0 * (i / 9) as f32],
    }
}

fn cell_count(block: &str) -> usize {
    match kind(block) {
        Some(Kind::Import | Kind::Export | Kind::Interface) => 9,
        Some(Kind::StorageBus | Kind::Workbench) => 63,
        Some(Kind::LevelEmitter) => 1,
        _ => 0,
    }
}

/// Open config cells (the server's data: capacity cards).
fn open_cells(block: &str, data: &[i32]) -> usize {
    match kind(block) {
        Some(Kind::Import | Kind::Export | Kind::StorageBus) => data.first().copied().unwrap_or(1).max(1) as usize,
        _ => cell_count(block),
    }
}

/// IOBusScreen's and StorageBusScreen's left toolbar, those that show: id, icon, title, value.
fn bus_buttons(block: &str, data: &[i32]) -> Vec<(i32, [f32; 2], &'static str, String)> {
    let d = |i: usize| data.get(i).copied().unwrap_or(0);
    let mut out = Vec::new();
    let storage = block == "ae2:storage_bus";
    if storage {
        out.push((BTN_CLEAR, [96.0, 0.0], "Clear", "Clear Config/Settings".to_owned()));
        out.push((BTN_PARTITION, [32.0, 64.0], "Partition Storage", "Configures Partition based on currently stored items.".to_owned()));
        out.push(if d(8) != 0 {
            (BTN_REPORT, [96.0, 48.0], "Report Inaccessible Items", "Yes: Items that cannot be extracted will be visible.".to_owned())
        } else {
            (BTN_REPORT, [80.0, 48.0], "Report Inaccessible Items", "No: Only extractable items will be visible.".to_owned())
        });
        out.push(if d(7) != 0 {
            (BTN_EXTRACT_FILTER, [176.0, 48.0], "Filtered Operations", "Filter on insertion only.".to_owned())
        } else {
            (BTN_EXTRACT_FILTER, [160.0, 48.0], "Filtered Operations", "Filter on insert and extract.".to_owned())
        });
    } else if d(1) >= 0 {
        let v = d(1).clamp(0, 2) as usize;
        out.push((BTN_REDSTONE, [[16.0, 0.0], [0.0, 0.0], [32.0, 0.0]][v], "Redstone Mode", REDSTONE_MODES[v].to_owned()));
    }
    // FormationPlaneScreen: Block Placement (Settings.PLACE_BLOCK), then fuzzy comparison.
    if block == "ae2:formation_plane" {
        out.push(if d(6) != 0 {
            (BTN_PLACE, [16.0, 224.0], "Block Placement", "Blocks will be dropped as item.".to_owned())
        } else {
            (BTN_PLACE, [0.0, 224.0], "Block Placement", "Blocks will be placed as block.".to_owned())
        });
    }
    if d(5) >= 0 {
        let (icon, word) = FUZZY_BUTTON[d(5).clamp(0, 4) as usize];
        out.push((BTN_FUZZY, icon, "Fuzzy Comparison", word.to_owned()));
    }
    if block == "ae2:export_bus" {
        if d(4) >= 0 {
            out.push(if d(4) != 0 {
                (BTN_CRAFT_ONLY, [48.0, 16.0], "Crafting Behavior", "Do not use stocked items, only craft items while exporting.".to_owned())
            } else {
                (BTN_CRAFT_ONLY, [32.0, 16.0], "Crafting Behavior", "Use stocked items, or craft items while exporting.".to_owned())
            });
        }
        let v = d(2).clamp(0, 2) as usize;
        out.push((
            BTN_SCHEDULE,
            [[0.0, 240.0], [16.0, 240.0], [32.0, 240.0]][v],
            "Scheduling Mode",
            ["Export the first item until the network is empty, then try the next ones.", "Export using round robin mode.", "Export items in random mode."][v].to_owned(),
        ));
    }
    if storage {
        let v = d(6).clamp(0, 2) as usize;
        out.push((BTN_ACCESS, [[32.0, 144.0], [16.0, 144.0], [0.0, 144.0]][v], "Input/Output Mode", ["Bi-Directional", "Extract Only", "Insert Only"][v].to_owned()));
    }
    out
}

/// Any part screen's toolbar (16 pixels, 4 between, from -20, 8).
fn toolbar(block: &str, data: &[i32]) -> Vec<(i32, [f32; 2], &'static str, String)> {
    let d = |i: usize| data.get(i).copied().unwrap_or(0);
    let list = match kind(block) {
        Some(Kind::Import | Kind::Export | Kind::StorageBus) => bus_buttons(block, data),
        Some(Kind::Interface) if d(1) >= 0 => {
            let (icon, word) = FUZZY_BUTTON[d(1).clamp(0, 4) as usize];
            vec![(BTN_FUZZY, icon, "Fuzzy Comparison", word.to_owned())]
        }
        // LevelEmitterScreen with a Crafting Card: Settings.CRAFT_VIA_REDSTONE in the redstone mode's place.
        Some(Kind::LevelEmitter) if block == "ae2:level_emitter" && d(2) != 0 => {
            let mut v = vec![if d(3) != 0 {
                (BTN_CRAFT_RS, [48.0, 16.0], "Crafting Emitter Mode", "Emit Redstone to craft item.".to_owned())
            } else {
                (BTN_CRAFT_RS, [32.0, 16.0], "Crafting Emitter Mode", "Emit Redstone while item is crafting.".to_owned())
            }];
            if d(4) >= 0 {
                let (icon, word) = FUZZY_BUTTON[d(4).clamp(0, 4) as usize];
                v.push((BTN_FUZZY, icon, "Fuzzy Comparison", word.to_owned()));
            }
            v
        }
        Some(Kind::LevelEmitter | Kind::EnergyEmitter) => {
            let below = d(1) != 0;
            let mut v = vec![(
                BTN_EMIT_MODE,
                if below { [16.0, 0.0] } else { [0.0, 0.0] },
                "Redstone Mode",
                if below { "Emit when levels are below limit." } else { "Emit when levels are above or equal to limit." }.to_owned(),
            )];
            if block == "ae2:level_emitter" && d(4) >= 0 {
                let (icon, word) = FUZZY_BUTTON[d(4).clamp(0, 4) as usize];
                v.push((BTN_FUZZY, icon, "Fuzzy Comparison", word.to_owned()));
            }
            v
        }
        Some(Kind::Workbench) => {
            let mut v = Vec::new();
            if d(1) >= 0 {
                let (icon, word) = FUZZY_BUTTON[d(1).clamp(0, 4) as usize];
                v.push((BTN_FUZZY, icon, "Fuzzy Comparison", word.to_owned()));
            }
            v.push((BTN_PARTITION, [32.0, 64.0], "Partition Storage", "Configures Partition based on currently stored items.".to_owned()));
            v.push((BTN_CLEAR, [96.0, 0.0], "Clear", "Clear Config/Settings".to_owned()));
            v.push(if d(0) == 0 {
                (BTN_WB_COPY, [80.0, 176.0], "Copy Mode", "Clear on remove".to_owned())
            } else {
                (BTN_WB_COPY, [80.0, 192.0], "Copy Mode", "Keep on remove".to_owned())
            });
            v
        }
        _ => Vec::new(),
    };
    list
}

fn bar_place(i: usize) -> [f32; 2] {
    [-20.0, 8.0 + 20.0 * i as f32]
}

// ---- Drawing

fn draw_slot(d: &mut dyn DrawContext, x: f32, y: f32) {
    d.fill([x - 1.0, y - 1.0, 18.0, 18.0], 0xFF37_3737);
    d.fill([x, y, 17.0, 17.0], 0xFFFF_FFFF);
    d.fill([x, y, 16.0, 16.0], 0xFF8B_8B8B);
}

fn draw_key(host: &dyn ClientHost, d: &mut dyn DrawContext, key: &AeKey, stack: Option<&ItemStack>, at: [f32; 2], amount: Option<i64>) {
    let label = amount.map(|n| key.format_amount(n, AmountFormat::SlotLargeFont));
    if key.key_type() == KeyType::Fluids {
        d.fluid(key.id(), at, 16.0);
        if let Some(l) = label {
            let w = d.text_width(&l);
            d.text(&l, [at[0] + 17.0 - w, at[1] + 9.0], 0xFFFF_FFFF, true);
        }
    } else if let Some(s) = stack.cloned().or_else(|| stack_of(host, key, 1)) {
        d.gui_draw_item_with(&s, at, label.as_deref());
    }
}

pub(crate) fn screen_draw(host: &dyn ClientHost, block: &str, data: &[i32], d: &mut dyn DrawContext, o: [f32; 2]) -> bool {
    let Some(k) = kind(block) else { return false };
    if k == Kind::P2p {
        return false;
    }
    with_client(|c| {
        if c.block != block {
            c.block = block.to_owned();
        }
    });
    let at = |p: [f32; 2]| [o[0] + p[0], o[1] + p[1]];
    let (title_key, title, height) = match k {
        Kind::Import => ("gui.ae2.ImportBus", "Import Bus", 251.0),
        Kind::Export => ("gui.ae2.ExportBus", "Export Bus", 251.0),
        Kind::StorageBus if block == "ae2:formation_plane" => ("gui.ae2.FormationPlane", "Formation Plane", 251.0),
        Kind::StorageBus => ("gui.ae2.StorageBus", "Storage Bus", 251.0),
        Kind::Interface => ("gui.ae2.Interface", "ME Interface", 205.0),
        Kind::LevelEmitter => ("gui.ae2.LevelEmitter", "ME Level Emitter", 184.0),
        Kind::EnergyEmitter => ("gui.ae2.EnergyLevelEmitter", "ME Energy Level Emitter", 184.0),
        Kind::Workbench => ("gui.ae2.CellWorkbench", "Cell Workbench", 251.0),
        Kind::P2p => return false,
    };
    match k {
        Kind::Import | Kind::Export | Kind::StorageBus => {
            spr(d, "ae2:gui/storagebus", o, [0.0, 0.0], [0.0, 0.0, 176.0, 251.0]);
            let cells = cell_count(block);
            for i in 18..cells {
                let p = at(cell_at(block, i));
                draw_slot(d, p[0], p[1]);
            }
            if cells == 9 {
                // The import/export bus has one row: the rest of AE2's texture stays plain.
                d.fill([o[0] + 7.0, o[1] + 46.0, 162.0, 110.0], 0xFFC6_C6C6);
            }
            let open = open_cells(block, data);
            for i in open..cells {
                let p = at(cell_at(block, i));
                d.fill([p[0], p[1], 16.0, 16.0], 0x8040_4040);
            }
            side_panel(d, o, 176.0, 5);
            // The storage bus's attached line (FormationPlaneScreen has none).
            if k == Kind::StorageBus && block != "ae2:formation_plane" {
                let target = with_client(|c| c.target.clone());
                let text = if target.is_empty() || target.ends_with(":air") {
                    tr(host, "gui.ae2.Unattached", "Unattached")
                } else {
                    format!("{}: {}", tr(host, "gui.ae2.AttachedTo", "Attached to"), block_name(host, &target))
                };
                d.text(&text, at([10.0, 17.0]), TEXT, false);
            }
        }
        Kind::Interface => {
            spr(d, "ae2:gui/interface", o, [0.0, 0.0], [0.0, 0.0, 176.0, 205.0]);
            d.text(&tr(host, "gui.ae2.Config", "Config"), at([8.0, 24.0]), TEXT, false);
            d.text(&tr(host, "gui.ae2.StoredItems", "Stored Items"), at([8.0, 91.0]), TEXT, false);
            side_panel(d, o, 176.0, 1);
            // The fluid stock in the stored row (`iface_fluids`).
            crate::iface_fluids::draw(host, d, o);
            // InterfaceScreen.SetAmountButton over each configured cell (Icon.WRENCH).
            with_client(|c| {
                for (i, g) in c.ghosts.iter().enumerate().take(9) {
                    if g.is_some() {
                        d.sprite("ae2:gui/states", [48.0, 64.0, 16.0, 16.0], [o[0] + 8.0 + 18.0 * i as f32, o[1] + 35.0, 16.0, 16.0], None);
                    }
                }
            });
        }
        Kind::LevelEmitter | Kind::EnergyEmitter => {
            let tex = if k == Kind::LevelEmitter { "ae2:gui/level_emitter" } else { "ae2:gui/energy_level_emitter" };
            spr(d, tex, o, [0.0, 0.0], [0.0, 0.0, 176.0, 184.0]);
            draw_level_widget(d, o, if k == Kind::LevelEmitter { 103.0 } else { 133.0 });
            if k == Kind::LevelEmitter {
                side_panel(d, o, 176.0, 1);
                let p = at(cell_at(block, 0));
                draw_slot(d, p[0], p[1]);
            }
        }
        Kind::Workbench => {
            spr(d, "ae2:gui/cellworkbench", o, [0.0, 0.0], [0.0, 0.0, 176.0, 251.0]);
            for i in 0..63 {
                let p = at(cell_at(block, i));
                draw_slot(d, p[0], p[1]);
            }
            side_panel(d, o, 176.0, 4);
        }
        Kind::P2p => {}
    }
    d.text(&tr(host, title_key, title), at([8.0, 6.0]), TEXT, false);
    d.text(&tr(host, "container.inventory", "Inventory"), at([8.0, height - 93.0]), TEXT, false);
    // The ghosts.
    let interface = k == Kind::Interface;
    let ghosts = with_client(|c| {
        for g in c.ghosts.iter_mut().flatten() {
            if g.2.is_none() && g.0.key_type() == KeyType::Items {
                g.2 = stack_of(host, &g.0, 1);
            }
        }
        c.ghosts.clone()
    });
    for (i, g) in ghosts.iter().enumerate().take(cell_count(block)) {
        if let Some((key, n, stack)) = g {
            draw_key(host, d, key, stack.as_ref(), at(cell_at(block, i)), interface.then_some(*n));
        }
    }
    for (n, (_, icon, _, _)) in toolbar(block, data).iter().enumerate() {
        state_button(d, o, bar_place(n), *icon);
    }
    true
}

/// A block's name for the storage bus's line.
fn block_name(host: &dyn ClientHost, id: &str) -> String {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    host.translate(&format!("block.{ns}.{path}")).unwrap_or_else(|| path.replace('_', " "))
}

/// AE2's NumberEntryWidget: the step buttons and the box.
fn draw_level_widget(d: &mut dyn DrawContext, o: [f32; 2], box_w: f32) {
    for (x, w, n) in STEPS {
        for (row, sign) in LEVEL_ROWS.iter().zip(["+", "-"]) {
            d.gui_nine_slice("widget/button", [o[0] + x, o[1] + row, w, 20.0], 0xFFFF_FFFF);
            let label = format!("{sign}{n}");
            let tw = d.text_width(&label);
            d.text(&label, [o[0] + x + (w - tw) / 2.0, o[1] + row + 6.0], 0xFFFF_FFFF, true);
        }
    }
    let (text, focus) = with_client(|c| (c.level_text.clone(), c.level_focus));
    d.fill([o[0] + 22.0, o[1] + 41.0, box_w + 2.0, 14.0], if focus { 0xFFFF_FFFF } else { 0xFFA0_A0A0 });
    d.fill([o[0] + 23.0, o[1] + 42.0, box_w, 12.0], 0xFF00_0000);
    // An expression that can't be read (yet) in red.
    let color = if text.trim().is_empty() || crate::mathexpr::whole(&text, 0, LEVEL_MOST).is_some() { 0xFFFF_FFFF } else { 0xFFFF_6060 };
    d.text(&if focus { format!("{text}_") } else { text }, [o[0] + 26.0, o[1] + 44.0], color, false);
}

// ---- Clicks

fn inside(at: [f32; 2], r: [f32; 4]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

fn cell_under(block: &str, at: [f32; 2]) -> Option<usize> {
    (0..cell_count(block)).find(|i| {
        let p = cell_at(block, *i);
        inside(at, [p[0], p[1], 16.0, 16.0])
    })
}

/// The level's most (the server clamps it there too).
const LEVEL_MOST: i64 = i64::MAX / 2;

/// The level typed so far (a number or an expression, MathExpressionParser) to the server as a number; nothing while
/// the expression is unfinished.
fn set_level_text(c: &mut PartClient, text: String) {
    c.level_text = text;
    let n = if c.level_text.trim().is_empty() { Some(0) } else { crate::mathexpr::whole(&c.level_text, 0, LEVEL_MOST) };
    if let Some(n) = n {
        c.outgoing.push(format!("level {n}"));
    }
}

pub(crate) fn screen_click(_host: &dyn ClientHost, block: &str, data: &[i32], at: [f32; 2], right: bool) -> Option<Option<i32>> {
    let k = kind(block)?;
    if k == Kind::P2p {
        return None;
    }
    for (n, (id, _, _, _)) in toolbar(block, data).iter().enumerate() {
        let p = bar_place(n);
        if inside(at, [p[0], p[1], 16.0, 16.0]) {
            return Some(Some(if right { id | BACK } else { *id }));
        }
    }
    if matches!(k, Kind::LevelEmitter | Kind::EnergyEmitter) {
        let box_w = if k == Kind::LevelEmitter { 103.0 } else { 133.0 };
        for (x, w, n) in STEPS {
            for (row, sign) in LEVEL_ROWS.iter().zip([1i64, -1]) {
                if inside(at, [x, *row, w, 20.0]) {
                    with_client(|c| {
                        let v = crate::mathexpr::whole(&c.level_text, 0, LEVEL_MOST).unwrap_or(0);
                        set_level_text(c, v.saturating_add(sign * n).clamp(0, LEVEL_MOST).to_string());
                    });
                    return Some(Some(CLIENT_ONLY));
                }
            }
        }
        let in_box = inside(at, [23.0, 42.0, box_w, 12.0]);
        with_client(|c| {
            if in_box && right {
                set_level_text(c, String::new());
            }
            c.level_focus = in_box;
        });
        if in_box {
            return Some(Some(CLIENT_ONLY));
        }
    }
    if let Some(i) = cell_under(block, at) {
        if i >= open_cells(block, data) {
            return Some(Some(CLIENT_ONLY));
        }
        return Some(Some(GHOST | ((i as i32) << 3) | i32::from(right)));
    }
    // A stored cell holding (or configured for) a fluid: a bucket on it fills or empties its tank.
    if k == Kind::Interface
        && let Some(i) = crate::iface_fluids::tank_under(at)
        && (crate::iface_fluids::client_tank(i).is_some() || with_client(|c| c.ghosts.get(i).cloned().flatten()).is_some_and(|g| g.0.key_type() == KeyType::Fluids))
    {
        return Some(Some(crate::iface_fluids::TANK_BUTTON | i as i32));
    }
    Some(None)
}

pub(crate) fn screen_tooltip(host: &dyn ClientHost, block: &str, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) -> bool {
    let Some(k) = kind(block) else { return false };
    if k == Kind::P2p {
        return false;
    }
    for (n, (_, _, title, value)) in toolbar(block, data).iter().enumerate() {
        let p = bar_place(n);
        if inside(at, [p[0], p[1], 16.0, 16.0]) {
            lines.push((*title).to_owned());
            lines.push(format!("§7{value}"));
            return true;
        }
    }
    if k == Kind::Interface && crate::iface_fluids::tooltip(host, at, lines) {
        return true;
    }
    if let Some(i) = cell_under(block, at)
        && let Some((key, n, _)) = with_client(|c| c.ghosts.get(i).cloned().flatten())
    {
        lines.push(key_name(host, &key));
        if k == Kind::Interface {
            lines.push(format!("§7{}", key.format_amount(n, AmountFormat::Full)));
        }
        return true;
    }
    false
}

// ---- Input through the global screen hooks

fn our_menu(screen: &ScreenView<'_>) -> bool {
    matches!(crate::bus::menu(screen.menu), Some("ae2:io_bus" | "ae2:storage_bus" | "ae2:interface" | "ae2:level_emitter" | "ae2:energy_level_emitter" | "ae2:cell_workbench"))
}

/// The wheel over an interface's config cell: one more or less of it.
pub(crate) fn scrolled(screen: &ScreenView<'_>, scroll: f64) -> bool {
    if screen.menu != Some("ae2:interface") || scroll == 0.0 {
        return false;
    }
    let at = [screen.mouse[0] - screen.rect[0], screen.mouse[1] - screen.rect[1]];
    let Some(i) = cell_under("ae2:interface", at) else { return false };
    with_client(|c| c.queued.push(GHOST | ((i as i32) << 3) | if scroll > 0.0 { 2 } else { 3 }));
    true
}

pub(crate) fn key(screen: &ScreenView<'_>, key: &str) -> bool {
    if !our_menu(screen) || !with_client(|c| c.level_focus) {
        return false;
    }
    match key {
        "key.keyboard.escape" => return false,
        "key.keyboard.backspace" => with_client(|c| {
            let mut t = c.level_text.clone();
            t.pop();
            set_level_text(c, t);
        }),
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => with_client(|c| c.level_focus = false),
        _ => {}
    }
    true
}

pub(crate) fn typed(screen: &ScreenView<'_>, ch: char) -> bool {
    if !our_menu(screen) || !with_client(|c| c.level_focus) {
        return false;
    }
    if crate::mathexpr::allowed(ch) {
        with_client(|c| {
            if c.level_text.len() < 32 {
                let t = format!("{}{ch}", c.level_text);
                // A plain number loses its leading zeros; an expression is kept as typed.
                let t = if t.chars().all(|x| x.is_ascii_digit()) { t.trim_start_matches('0').to_owned() } else { t };
                set_level_text(c, t);
            }
        });
    }
    true
}

pub(crate) fn closing() {
    crate::iface_fluids::client_clear();
    with_client(|c| {
        c.level_focus = false;
        c.ghosts.clear();
        c.block.clear();
        c.target.clear();
    });
}

/// The client's tick: menu buttons queued by hooks that can't send them, and the level typed.
pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    let (queued, outgoing) = with_client(|c| (std::mem::take(&mut c.queued), std::mem::take(&mut c.outgoing)));
    if let Some(last) = outgoing.last() {
        let _ = crate::util::send_to_server(host, NET_PART, last.as_bytes());
    }
    if queued.is_empty() {
        return;
    }
    let Some(menu) = host.local_player().and_then(|p| p.container_menu(&*host)) else { return };
    for b in queued {
        menu.click_button(host, b);
    }
}

// ---- From the server

pub(crate) fn from_server(channel: &str, data: &[u8]) -> bool {
    if channel != NET_PART {
        return false;
    }
    let text = String::from_utf8_lossy(data);
    crate::iface_fluids::client_message(&text);
    with_client(|c| {
        c.ghosts.clear();
        for l in text.lines() {
            if let Some(b) = l.strip_prefix("#part ") {
                c.block = b.to_owned();
            } else if let Some(rest) = l.strip_prefix("g ") {
                let mut parts = rest.splitn(3, ' ');
                let (Some(i), Some(n), Some(k)) = (parts.next().and_then(|i| i.parse::<usize>().ok()), parts.next().and_then(|n| n.parse::<i64>().ok()), parts.next().and_then(key_from_string)) else {
                    continue;
                };
                if i >= 63 {
                    continue;
                }
                if c.ghosts.len() <= i {
                    c.ghosts.resize(i + 1, None);
                }
                c.ghosts[i] = Some((k, n, None));
            } else if let Some(n) = l.strip_prefix("level ").and_then(|n| n.parse::<i64>().ok()) {
                c.level = n;
                if !c.level_focus {
                    c.level_text = n.to_string();
                }
            } else if let Some(t) = l.strip_prefix("target ") {
                c.target = t.to_owned();
            }
        }
    });
    true
}
