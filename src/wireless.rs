//! The wireless access point (WirelessAccessPointBlockEntity: its booster slot, range, look and the link slots) and
//! the item screens: the wireless terminal and wireless crafting terminal (WirelessTerminalItem,
//! WirelessCraftingTerminalItem), the portable item and fluid cells (`portable`) and the network tool's toolbox
//! (`toolbox`). Upstream's `wap_boosters`, `wap_range`, `set_link`, `link_of`, `open_wireless`, `wireless_click`,
//! `wireless_drain`, `hotkey`, `hotkey_slot` and the item screens' section (`../ae2-horizon/mixins/ae2/me.rs`).
//!
//! An item's screen is the mod's own screen (`mod_screen_open`): Sunrise shows a mod's menu only over its block, and
//! these items open anywhere. Over it the terminal is the terminal's own drawing and clicks (`screens`, `craft_screens`,
//! `term_ui`, given a `ScreenView` whose menu names the item's kind), and the wireless terminal is a viewer of the
//! linked grid in `storage` like a terminal block's (`storage::opened_remote`): the same list, the same grid clicks
//! (`storage::menu_button`), crafting requests and the crafting status tab. The player's inventory is drawn by this
//! module and its clicks are the server's; the cursor stack is the player's inventory menu's (`ItemPlace::
//! PlayerCursor`), taken off before each operation and put back after, so nothing is made twice or lost.
//!
//! Server rules: the item is checked in its slot (by a tag written on it as it opens) before every operation and
//! every tick, with the link, the range and the power for a wireless terminal; a failed check closes the screen
//! (moving or dropping the item closes it). Clicks are rate limited (`storage::rate_ok`).

use crate::cells::*;
use crate::config::config;
use crate::screens::{side_panel, spr, tr};
use crate::storage::{self, Scope};
use crate::util::*;
use crate::world::{item_power, set_item_power};
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) const WAP: &str = "ae2:wireless_access_point";
pub(crate) const BOOSTER: &str = "ae2:wireless_booster";
pub(crate) const NET_WIRELESS: &str = "ae2:wireless";
/// The mod screen the item screens draw on.
pub(crate) const SCREEN: &str = "ae2:item_screen";
/// The item screens' kinds (what `ScreenView.menu` names for the terminal's code).
pub(crate) const WT: &str = "ae2:wireless_terminal";
pub(crate) const WCT: &str = "ae2:wireless_crafting_terminal";
pub(crate) const PORTABLE_ITEM: &str = "ae2:portable_item_cell";
pub(crate) const PORTABLE_FLUID: &str = "ae2:portable_fluid_cell";
pub(crate) const TOOLBOX: &str = "ae2:network_tool";

const TEXT: u32 = 0xFF40_4040;
/// The item's custom data: its link (`ae2:link`, the access point's position, and `ae2:link_dim`), the open screen's
/// tag (`ae2:open`), the wireless crafting terminal's grid (`ae2:craft_grid`).
const LINK: &str = "ae2:link";
const LINK_DIM: &str = "ae2:link_dim";
const OPEN_TAG: &str = "ae2:open";
const GRID: &str = "ae2:craft_grid";
/// The wireless crafting terminal's grid and result (crafting_terminal.json, from the screen's foot).
const GRID_X: f32 = 37.0;
const GRID_UP: f32 = 154.0;
/// The key mappings (Hotkeys: AE2 binds none by default): id, label, the item kind they open.
const KEYS: [(&str, &str, &str); 3] = [
    ("wireless_terminal", "key.ae2.wireless_terminal", "wireless"),
    ("portable_item_cell", "key.ae2.portable_item_cell", "portable_item"),
    ("portable_fluid_cell", "key.ae2.portable_fluid_cell", "portable_fluid"),
];

// ---- Kinds

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Wireless,
    WirelessCrafting,
    PortableItem,
    PortableFluid,
    Toolbox,
}

impl Kind {
    pub(crate) fn of_item(item: &str) -> Option<Kind> {
        Some(match item {
            WT => Kind::Wireless,
            WCT => Kind::WirelessCrafting,
            TOOLBOX => Kind::Toolbox,
            i if i.starts_with("ae2:portable_item_cell_") => Kind::PortableItem,
            i if i.starts_with("ae2:portable_fluid_cell_") => Kind::PortableFluid,
            _ => return None,
        })
    }

    pub(crate) fn screen(self) -> &'static str {
        match self {
            Kind::Wireless => WT,
            Kind::WirelessCrafting => WCT,
            Kind::PortableItem => PORTABLE_ITEM,
            Kind::PortableFluid => PORTABLE_FLUID,
            Kind::Toolbox => TOOLBOX,
        }
    }

    fn from_screen(s: &str) -> Option<Kind> {
        [Kind::Wireless, Kind::WirelessCrafting, Kind::PortableItem, Kind::PortableFluid, Kind::Toolbox].into_iter().find(|k| k.screen() == s)
    }

    pub(crate) fn wireless(self) -> bool {
        matches!(self, Kind::Wireless | Kind::WirelessCrafting)
    }

    pub(crate) fn portable(self) -> bool {
        matches!(self, Kind::PortableItem | Kind::PortableFluid)
    }

    /// The item's upgrade slots (its UpgradeInventory): two on a wireless terminal, four on a portable item cell,
    /// three on a portable fluid cell.
    pub(crate) fn card_slots(self) -> usize {
        match self {
            Kind::Wireless | Kind::WirelessCrafting => 2,
            Kind::PortableItem => 4,
            Kind::PortableFluid => 3,
            Kind::Toolbox => 0,
        }
    }

    /// The cards the item takes and how many of each (Upgrades.add for the item).
    pub(crate) fn card_limit(self, card: &str) -> usize {
        match (self, card) {
            (Kind::Wireless | Kind::WirelessCrafting, "ae2:energy_card") => 2,
            // [tools] InitUpgrades: two Energy Cards on a portable cell.
            (Kind::PortableItem | Kind::PortableFluid, "ae2:energy_card") => 2,
            (Kind::PortableItem, "ae2:fuzzy_card" | "ae2:inverter_card" | "ae2:equal_distribution_card" | "ae2:void_card") => 1,
            (Kind::PortableFluid, "ae2:inverter_card" | "ae2:equal_distribution_card" | "ae2:void_card") => 1,
            _ => 0,
        }
    }

    /// The terminal's rows below its grid (the wireless crafting terminal has the crafting part).
    fn bottom(self) -> f32 {
        if self == Kind::WirelessCrafting { 170.0 } else { 97.0 }
    }
}

/// Whether `block` is an item screen's terminal kind (the terminal's drawing and clicks take it).
pub(crate) fn is_item_term(block: &str) -> bool {
    matches!(block, WT | WCT | PORTABLE_ITEM | PORTABLE_FLUID)
}

pub(crate) fn is_portable(block: &str) -> bool {
    matches!(block, PORTABLE_ITEM | PORTABLE_FLUID)
}

/// An item terminal's title.
pub(crate) fn title(host: &dyn ClientHost, block: &str) -> Option<String> {
    Some(match block {
        WT => tr(host, "item.ae2.wireless_terminal", "Wireless Terminal"),
        WCT => tr(host, "item.ae2.wireless_crafting_terminal", "Wireless Crafting Terminal"),
        PORTABLE_ITEM | PORTABLE_FLUID => tr(host, "gui.ae2.PortableCell", "Portable Cell"),
        _ => return None,
    })
}

// ---- Range and drain (pure: unit tested)

/// AEConfig.wireless_getMaxRange: base + multiplier * boosters^exp.
pub(crate) fn max_range(base: f64, multiplier: f64, exp: f64, boosters: f64) -> f64 {
    base + multiplier * boosters.max(0.0).powf(exp)
}

/// AEConfig.wireless_getPowerDrain: an access point's idle draw for its boosters.
pub(crate) fn wap_drain(base: f64, multiplier: f64, high: f64, boosters: f64) -> f64 {
    base + multiplier * boosters.powf(1.0 + boosters / high.max(1.0))
}

/// From the player's position to an access point's middle.
pub(crate) fn distance(at: [f64; 3], wap: Pos) -> f64 {
    (0..3).map(|i| (at[i] - f64::from(wap[i]) - 0.5).powi(2)).sum::<f64>().sqrt()
}

/// WirelessTerminalMenuHost.rangeCheck: of the grid's access points that work, the nearest that reaches `at`:
/// its position and distance.
pub(crate) fn best_wap(at: [f64; 3], waps: &[(Pos, f64, bool)]) -> Option<(Pos, f64)> {
    waps.iter()
        .filter(|(_, _, active)| *active)
        .map(|(p, range, _)| (*p, distance(at, *p), *range))
        .filter(|(_, d, range)| d <= range)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, d, _)| (p, d))
}

/// ItemMenuHost.drainPower for a wireless terminal: ten ticks of AEConfig.wireless_getDrainRate (the drain multiplier
/// a block of distance), with the power usage multiplier.
pub(crate) fn drain_ten_ticks(distance: f64, drain_multiplier: f64, usage: f64) -> f64 {
    10.0 * drain_multiplier * distance * usage
}

// ---- The access point

pub(crate) fn has_screen(block: &str) -> bool {
    block == WAP
}

/// Its three slots: the boosters, a terminal to link, the linked terminal (hoppers reach none).
pub(crate) fn container_of(block: &str) -> Option<ModContainer> {
    (block == WAP).then(|| ModContainer { slots: 3, faces: std::array::from_fn(|_| Vec::new()), inputs: (0..3).collect(), outputs: (0..3).collect(), insert_rules: Vec::new(), menu: Some(WAP.to_owned()) })
}

pub(crate) fn slot_rules(menu: &str, slot: usize, stack: &ItemStack, may_place: &mut bool, max_stack: &mut u32) {
    if menu != WAP {
        return;
    }
    match slot {
        0 => {
            *may_place = sid(stack) == BOOSTER;
            *max_stack = 64;
        }
        1 => {
            *may_place = crate::powered::is_wireless_terminal(sid(stack));
            *max_stack = 1;
        }
        _ => *may_place = false,
    }
}

fn boosters(stack: Option<&ItemStack>) -> f64 {
    stack.filter(|s| sid(s) == BOOSTER).map_or(0.0, |s| f64::from(s.count()))
}

fn range_for(boosters: f64) -> f64 {
    let c = config();
    max_range(c.wireless_base_range, c.wireless_booster_range_multiplier, c.wireless_booster_exp, boosters)
}

/// WirelessAccessPointBlockEntity.updatePower: the access point's idle draw with its boosters (the grid's drain).
pub(crate) fn wap_idle(host: &dyn Host, level: LevelRef, pos: Pos) -> f64 {
    let c = config();
    wap_drain(c.wireless_base_cost, c.wireless_cost_multiplier, c.wireless_high_wireless_count, boosters(crate::bus::item_slot(host, level, pos, 0).as_ref()))
}

/// The access point's tick: its range and state for the screen (data slots), its look, and a terminal in the link slot
/// linked here and moved to the out slot.
pub(crate) fn wap_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let dim = host.dimension();
    if fields.items.len() < 3 {
        fields.items.resize(3, None);
    }
    if fields.data.len() < 2 {
        fields.data.resize(2, 0);
    }
    let (_, online, active) = crate::net::device_state(dim, pos);
    let n = boosters(fields.items[0].as_ref());
    fields.data[0] = i32::from(active) | ((n as i32) << 1);
    fields.data[1] = range_for(n).min(32_000.0) as i32;
    let look = if active { "has_channel" } else if online { "on" } else { "off" };
    if let Some(level) = here(&*host)
        && level.block_property(&*host, pos, "state").is_some_and(|v| v != look)
    {
        host.set_block_property_with(pos, "state", look, 2);
    }
    let linkable = fields.items[1].as_ref().is_some_and(|s| crate::powered::is_wireless_terminal(sid(s)));
    if linkable && fields.items[2].is_none() {
        if let Some(mut t) = fields.items[1].take() {
            set_link(&mut t, dim, pos);
            fields.items[2] = Some(t);
        }
    }
}

/// A use of the access point (not sneaking) opens its screen, claimed at the block's phase as the charger's is (its
/// beacon template would answer the use first).
pub(crate) fn wap_use(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let level = here(&*host)?;
    if crate::net::block_id(&*host, level, click.pos) != WAP {
        return None;
    }
    let entity = host.block_entity(level, click.pos)?;
    let player = host.player(click.player)?;
    crate::devices::pad_slots(host, entity, WAP);
    let _ = player.open_menu(host, entity);
    Some(InteractionResult::SuccessServer)
}

pub(crate) fn screen(block: &str) -> Option<MenuScreenLayout> {
    if block != WAP {
        return None;
    }
    // screens/wireless_access_point.json: the boosters at 80, 47; the link panel's in and out on the right.
    let slots = vec![Some(slot_at([80.0, 47.0])), Some(slot_at([193.0, 10.0])), Some(slot_at([193.0, 68.0]))];
    Some(MenuScreenLayout { width: 222.0, height: 166.0, slots, inventory_y: 84.0, inventory_x: 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() })
}

pub(crate) fn screen_draw(host: &dyn ClientHost, block: &str, data: &[i32], d: &mut dyn DrawContext, o: [f32; 2]) -> bool {
    if block != WAP {
        return false;
    }
    let tex = "ae2:gui/wireless_access_point";
    spr(d, tex, o, [0.0, 0.0], [0.0, 0.0, 176.0, 166.0]);
    // The link panel, run down to the window's foot.
    let px = 178.0;
    spr(d, tex, o, [px, 0.0], [198.0, 0.0, 44.0, 94.0]);
    d.sprite(tex, [198.0, 90.0, 44.0, 1.0], [o[0] + px, o[1] + 94.0, 44.0, 68.0], None);
    spr(d, tex, o, [px, 162.0], [198.0, 94.0, 44.0, 4.0]);
    d.text(&tr(host, "gui.ae2.Wireless", "Wireless Access Point"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
    d.text(&tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 166.0 - 93.0], TEXT, false);
    let packed = i32::from(data.first().copied().unwrap_or(0) as u16);
    let range = i32::from(data.get(1).copied().unwrap_or(16) as u16);
    let text = if packed & 1 != 0 {
        tr(host, "gui.ae2.WirelessRange", "Range: %s m").replace("%s", &range.to_string())
    } else {
        tr(host, "gui.ae2.Unlinked", "Offline")
    };
    let w = d.text_width(&text);
    d.text(&text, [o[0] + 88.0 - w / 2.0, o[1] + 20.0], if packed & 1 != 0 { TEXT } else { 0xFFAA_0000 }, false);
    let c = config();
    let drain = wap_drain(c.wireless_base_cost, c.wireless_cost_multiplier, c.wireless_high_wireless_count, f64::from(packed >> 1));
    let text = tr(host, "gui.ae2.PowerUsageRate", "Energy Usage: %s").replace("%s", &format!("{:.1} AE/t", drain));
    let w = d.text_width(&text);
    d.text(&text, [o[0] + 88.0 - w / 2.0, o[1] + 32.0], TEXT, false);
    true
}

pub(crate) fn screen_tooltip(host: &dyn ClientHost, block: &str, at: [f32; 2], lines: &mut Vec<String>) -> bool {
    if block != WAP {
        return false;
    }
    let inside = |x: f32, y: f32| at[0] >= x && at[0] < x + 16.0 && at[1] >= y && at[1] < y + 16.0;
    if inside(80.0, 47.0) {
        lines.push(tr(host, "gui.tooltips.ae2.PlaceWirelessBooster", "Increase range with Wireless Boosters"));
        return true;
    }
    if inside(193.0, 10.0) || inside(193.0, 68.0) {
        lines.push(tr(host, "gui.tooltips.ae2.LinkWirelessTerminal", "Link Wireless Terminals here"));
        return true;
    }
    false
}

// ---- What the item carries

fn set_link(stack: &mut ItemStack, dim: &str, pos: Pos) {
    edit_custom_data(stack, |d| {
        d.insert(java(LINK), Tag::IntArray(pos.to_vec()));
        d.insert(java(LINK_DIM), Tag::String(java(dim)));
    });
}

/// The access point a wireless terminal is linked to: its level and position.
pub(crate) fn link_of(stack: &ItemStack) -> Option<(String, Pos)> {
    let d = custom_data(stack)?;
    let pos = match d.get(&java(LINK)) {
        Some(Tag::IntArray(v)) if v.len() == 3 => [v[0], v[1], v[2]],
        _ => return None,
    };
    let dim = match d.get(&java(LINK_DIM)) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => "minecraft:overworld".to_owned(),
    };
    Some((dim, pos))
}

fn open_tag(stack: &ItemStack) -> Option<i64> {
    match custom_data(stack)?.get(&java(OPEN_TAG)) {
        Some(Tag::Long(t)) => Some(*t),
        _ => None,
    }
}

/// The wireless crafting terminal's grid, as saved on it.
pub(crate) fn grid_of(host: &dyn Host, stack: &ItemStack) -> Vec<Option<ItemStack>> {
    let mut out = vec![None; 9];
    if let Some(Tag::List(cells)) = custom_data(stack).and_then(|d| d.get(&java(GRID))) {
        for (i, c) in cells.iter().enumerate().take(9) {
            if let Tag::Compound(c) = c
                && !c.is_empty()
            {
                out[i] = ItemStack::from_nbt(host, c);
            }
        }
    }
    out
}

/// The grid as saved on the item (none for an empty grid): made before the item is edited, as the edit holds the host.
pub(crate) fn grid_tag(host: &dyn Host, grid: &[Option<ItemStack>]) -> Option<Tag> {
    if grid.iter().all(Option::is_none) {
        return None;
    }
    Some(Tag::List((0..9).map(|i| Tag::Compound(grid.get(i).cloned().flatten().and_then(|s| s.to_nbt(host)).unwrap_or_default())).collect()))
}

pub(crate) fn set_grid(stack: &mut ItemStack, tag: Option<Tag>) {
    edit_custom_data(stack, |d| match tag {
        Some(t) => {
            d.insert(java(GRID), t);
        }
        None => {
            d.remove(&java(GRID));
        }
    });
}

/// The item's upgrade cards (`ae2:upgrades`, as the cell workbench writes a cell's).
pub(crate) fn set_cards(stack: &mut ItemStack, cards: &[String]) {
    edit_custom_data(stack, |d| {
        if cards.is_empty() {
            d.remove(&java("ae2:upgrades"));
        } else {
            d.insert(java("ae2:upgrades"), Tag::List(cards.iter().map(|c| Tag::String(java(c))).collect()));
        }
    });
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn stack_hex(host: &dyn Host, s: &ItemStack) -> String {
    s.to_nbt(host).and_then(|c| Tag::Compound(c).to_network_bytes().ok()).map(|b| hex(&b)).unwrap_or_default()
}

fn stack_from_hex(host: &dyn Host, h: &str) -> Option<ItemStack> {
    let b = unhex(h)?;
    match Tag::from_network_bytes(&b, None).ok()?.0 {
        Tag::Compound(c) => ItemStack::from_nbt(host, &c),
        _ => None,
    }
}

// ---- Sessions (server)

#[derive(Clone, Debug)]
pub(crate) struct Session {
    pub(crate) kind: Kind,
    /// The item's slot (`ItemPlace::PlayerSlot`'s numbers: 0..36, the off hand -106).
    pub(crate) slot: i32,
    tag: i64,
    item: &'static str,
    dim: &'static str,
    /// The access point it reaches its grid through now (wireless kinds).
    pub(crate) wap: Option<Pos>,
    distance: f64,
    ticks: u64,
}

#[derive(Default)]
struct Server {
    sessions: HashMap<u128, Session>,
    next_tag: i64,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);

fn with_server<R>(f: impl FnOnce(&mut Server) -> R) -> R {
    let mut g = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Server::default))
}

pub(crate) fn forget_all() {
    *SERVER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    crate::portable::forget_all();
}

fn session(player: u128) -> Option<Session> {
    with_server(|s| s.sessions.get(&player).cloned())
}

/// Whether the player has a wireless terminal open that passed its checks (storage's viewer record stands on it).
pub(crate) fn session_ok(player: u128) -> bool {
    with_server(|s| s.sessions.get(&player).is_some_and(|x| x.kind.wireless()))
}

fn new_tag() -> i64 {
    with_server(|s| {
        if s.next_tag == 0 {
            s.next_tag = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as i64 & 0x3FFF_FFFF_FFFF_FFFF);
        }
        s.next_tag += 1;
        s.next_tag
    })
}

/// The session's item, if it is still in its slot (the same item, with the tag written as it opened).
pub(crate) fn held(host: &mut dyn ServerHost, player: u128, s: &Session) -> Option<ItemStack> {
    let st = ItemPlace::PlayerSlot { player, slot: s.slot }.stack_owned(host)?;
    (sid(&st) == s.item && open_tag(&st) == Some(s.tag)).then_some(st)
}

/// Changes the session's item in its slot (count and item kept): false, and nothing changed, when it isn't there.
pub(crate) fn edit_held(host: &mut dyn ServerHost, player: u128, s: &Session, f: impl FnOnce(&mut ItemStack)) -> bool {
    if held(host, player, s).is_none() {
        return false;
    }
    let place = ItemPlace::PlayerSlot { player, slot: s.slot };
    host.transaction(|host, tx| crate::cells::edit_stack(&place, host, tx, f)).is_ok()
}

fn creative(host: &dyn Host, player: u128) -> bool {
    host.player(player).and_then(|p| p.is_creative(host)).unwrap_or(false)
}

fn say(host: &mut dyn ServerHost, player: u128, key: &str) {
    if let Some(p) = host.player(player) {
        let _ = p.action_bar(host, Text::translate(key, Vec::new()));
    }
}

/// The access point the player reaches the terminal's network through: WirelessTerminalItem's checks (the link and its
/// network), then the range (the nearest working access point of that grid in reach). The message key on a failure.
fn reach(host: &mut dyn ServerHost, player: u128, stack: &ItemStack) -> Result<(Pos, f64), &'static str> {
    let (dim, link) = link_of(stack).ok_or("chat.ae2.DeviceNotLinked")?;
    let here_dim = host.dimension();
    let level = here(&*host).ok_or("chat.ae2.LinkedNetworkNotFound")?;
    if dim != here_dim {
        // Another level: its network is out of reach.
        return Err("chat.ae2.OutOfRange");
    }
    if crate::net::block_id(&*host, level, link) != WAP {
        return Err("chat.ae2.LinkedNetworkNotFound");
    }
    let devices = crate::net::grid_devices(here_dim, link, false).ok_or("chat.ae2.LinkedNetworkNotFound")?;
    let waps: Vec<(Pos, f64, bool)> = devices
        .into_iter()
        .filter(|(_, b)| b == WAP)
        .map(|(p, _)| {
            let n = boosters(crate::bus::item_slot(&*host, level, p, 0).as_ref());
            (p, range_for(n), crate::net::device_state(here_dim, p).2)
        })
        .collect();
    let at = host.player(player).and_then(|p| p.entity().position(&*host)).ok_or("chat.ae2.OutOfRange")?;
    best_wap(at, &waps).ok_or("chat.ae2.OutOfRange")
}

/// Opens the item in `slot` (its use, or a hotkey): AE2's checks, then its screen.
pub(crate) fn open(host: &mut dyn ServerHost, player: u128, slot: i32) -> bool {
    crate::cells::ids(&*host);
    let Some(stack) = ItemPlace::PlayerSlot { player, slot }.stack_owned(host) else { return false };
    let item = sid(&stack);
    let Some(kind) = Kind::of_item(item) else { return false };
    if session(player).is_some() {
        close(host, player, None);
    }
    let creative = creative(&*host, player);
    let dim = host.dimension();
    let mut wap = None;
    let mut dist = 0.0;
    if kind.wireless() {
        // WirelessTerminalItem.checkPreconditions: linked, powered; then the network in range.
        if link_of(&stack).is_none() {
            say(host, player, "chat.ae2.DeviceNotLinked");
            return false;
        }
        if !creative && item_power(&stack) < 0.5 {
            say(host, player, "chat.ae2.DeviceNotPowered");
            return false;
        }
        match reach(host, player, &stack) {
            Ok((p, d)) => {
                wap = Some(p);
                dist = d;
            }
            Err(key) => {
                say(host, player, key);
                return false;
            }
        }
    } else if kind.portable() && !creative && item_power(&stack) < 0.5 {
        say(host, player, "chat.ae2.DeviceNotPowered");
        return false;
    }
    let tag = new_tag();
    let s = Session { kind, slot, tag, item, dim, wap, distance: dist, ticks: 0 };
    let place = ItemPlace::PlayerSlot { player, slot };
    if host.transaction(|host, tx| crate::cells::edit_stack(&place, host, tx, |st| edit_custom_data(st, |d| {
        d.insert(java(OPEN_TAG), Tag::Long(tag));
    }))).is_err()
    {
        return false;
    }
    with_server(|sv| sv.sessions.insert(player, s.clone()));
    match kind {
        Kind::Wireless => storage::opened_remote(player, dim, wap.unwrap_or_default(), "ae2:terminal"),
        Kind::WirelessCrafting => storage::opened_remote(player, dim, wap.unwrap_or_default(), "ae2:crafting_terminal"),
        Kind::PortableItem | Kind::PortableFluid => crate::portable::opened(player),
        Kind::Toolbox => {}
    }
    debug(&format!("wireless: {player:x} opened {item} in slot {slot}"));
    let _ = send_to_player(host, player, NET_WIRELESS, format!("open {} {slot} {}", kind.screen(), kind.card_slots()).as_bytes());
    send_cursor(host, player);
    true
}

/// Closes the player's item screen: the crafting grid back into the network when they asked for that, the cursor
/// back into the inventory, and AE2's message for why when there is one.
pub(crate) fn close(host: &mut dyn ServerHost, player: u128, why: Option<&str>) {
    let Some(s) = with_server(|sv| sv.sessions.remove(&player)) else { return };
    if s.kind == Kind::WirelessCrafting
        && why.is_none()
        && crate::autocraft::clears_grid(player)
        && let Some(wap) = s.wap
        && let Some(stack) = held(host, player, &s)
    {
        let mut grid = grid_of(&*host, &stack);
        for cell in grid.iter_mut() {
            let Some(st) = cell.take() else { continue };
            let key = key_of(&*host, &st);
            let stored = storage::insert(host, Scope::Grid(wap), None, &key, i64::from(st.count())).clamp(0, i64::from(st.count())) as u32;
            *cell = with_count(&st, st.count() - stored);
        }
        // Put back what didn't go in (the item is in place: checked above in this call).
        let tag = grid_tag(&*host, &grid);
        let _ = edit_held(host, player, &s, |st| set_grid(st, tag));
    }
    // The open mark comes off again, so the closed item stacks and compares as before it was opened.
    // (A plain slot write: this runs from the client's message too, outside a tick's transactions.)
    let edited = edit_held(host, player, &s, |st| {
        edit_custom_data(st, |d| {
            d.remove(&java(OPEN_TAG));
        })
    });
    {
        let there = ItemPlace::PlayerSlot { player, slot: s.slot }.stack_owned(host);
        debug(&format!(
            "wireless: untag through the transaction: {edited}; slot {} holds {:?} tagged {:?}, session {} tagged {}",
            s.slot,
            there.as_ref().map(|t| sid(t).to_owned()),
            there.as_ref().and_then(open_tag),
            s.item,
            s.tag
        ));
    }
    if !edited
        && let Some(mut st) = held(host, player, &s)
    {
        edit_custom_data(&mut st, |d| {
            d.remove(&java(OPEN_TAG));
        });
        if s.slot >= 0
            && let Some(p) = host.player(player)
        {
            let _ = p.set_inventory_slot(host, s.slot as u32, Some(st));
        }
    }
    if s.kind.wireless() && storage::is_remote(player) {
        storage::closed(player);
    }
    if s.kind.portable() {
        crate::portable::closed(player);
    }
    if let Ok(Some(c)) = take_cursor(host, player) {
        give_back(host, player, c);
    }
    let _ = send_to_player(host, player, NET_WIRELESS, b"close");
    if let Some(key) = why {
        say(host, player, key);
    }
    debug(&format!("wireless: {player:x} closed ({why:?})"));
}

pub(crate) fn logged_out(player: u128) {
    drop(take_inbox(player));
    with_server(|sv| sv.sessions.remove(&player));
    crate::portable::closed(player);
}

/// The session and its item, after every check of the kind (the item in its slot; a wireless terminal's power, link
/// and range, the range each operation and every fifth tick when not `full`): a failed check closes the screen.
pub(crate) fn checked(host: &mut dyn ServerHost, player: u128) -> Option<(Session, ItemStack)> {
    checked_as(host, player, true)
}

fn checked_as(host: &mut dyn ServerHost, player: u128, full: bool) -> Option<(Session, ItemStack)> {
    let mut s = session(player)?;
    let Some(stack) = held(host, player, &s) else {
        close(host, player, None);
        return None;
    };
    if host.dimension() != s.dim && !s.kind.wireless() {
        close(host, player, None);
        return None;
    }
    let creative = creative(&*host, player);
    if (s.kind.wireless() || s.kind.portable()) && !creative && item_power(&stack) <= 0.0 {
        close(host, player, Some("chat.ae2.DeviceNotPowered"));
        return None;
    }
    if s.kind.wireless() {
        if !storage::is_remote(player) || (host.dimension() != s.dim && !full) {
            close(host, player, if storage::is_remote(player) { Some("chat.ae2.OutOfRange") } else { None });
            return None;
        }
        if !full {
            return Some((s, stack));
        }
        match reach(host, player, &stack) {
            Ok((wap, d)) => {
                if s.wap != Some(wap) {
                    storage::set_remote_pos(player, wap);
                }
                s.wap = Some(wap);
                s.distance = d;
                with_server(|sv| {
                    if let Some(x) = sv.sessions.get_mut(&player) {
                        x.wap = Some(wap);
                        x.distance = d;
                    }
                });
            }
            Err(key) => {
                close(host, player, Some(key));
                return None;
            }
        }
    }
    Some((s, stack))
}

/// The player's tick: the open item checked, its drain taken every ten ticks, a portable cell's list sent.
pub(crate) fn player_tick(host: &mut dyn ServerHost, player: u128) {
    for msg in take_inbox(player) {
        handle_client(host, player, &msg);
    }
    let Some(ticks) = with_server(|sv| {
        let x = sv.sessions.get_mut(&player)?;
        x.ticks += 1;
        Some(x.ticks)
    }) else {
        return;
    };
    let Some((s, stack)) = checked_as(host, player, ticks % 5 == 1) else { return };
    if ticks % 10 == 0 && !creative(&*host, player) {
        let c = config();
        let cost = match s.kind {
            Kind::Wireless | Kind::WirelessCrafting => drain_ten_ticks(s.distance, c.wireless_terminal_drain_multiplier, c.power_usage_multiplier),
            // PortableCellItem.getIdleDrain: half an AE a tick.
            Kind::PortableItem | Kind::PortableFluid => 5.0 * c.power_usage_multiplier,
            Kind::Toolbox => 0.0,
        };
        if cost > 0.0 {
            let mut ok = true;
            let edited = edit_held(host, player, &s, |st| ok = crate::powered::use_power(st, cost));
            if !edited || !ok {
                close(host, player, Some("chat.ae2.DeviceNotPowered"));
                return;
            }
        }
    }
    if s.kind.portable()
        && let Some(stack) = held(host, player, &s).or(Some(stack))
    {
        crate::portable::stream(host, player, &stack);
    }
}

// ---- The cursor and the inventory (server)

fn cursor_place(host: &dyn Host, player: u128) -> Option<ItemPlace> {
    let menu = host.player(player)?.container_menu(host)?;
    (menu.container_id(host) == Some(0)).then_some(ItemPlace::PlayerCursor { menu })
}

/// The cursor's stack, taken off it (None for an empty cursor); an error when the player has no inventory menu up.
pub(crate) fn take_cursor(host: &mut dyn ServerHost, player: u128) -> Result<Option<ItemStack>, ()> {
    let place = cursor_place(&*host, player).ok_or(())?;
    let Some(st) = place.stack_owned(host) else { return Ok(None) };
    let n = st.count();
    host.transaction(|host, tx| if place.extract(host, tx, &ItemResource::of(&st), n).unwrap_or(0) == n { Ok(()) } else { Err(()) }).map_err(|_| ())?;
    Ok(Some(st))
}

/// A stack back onto the (empty) cursor; into the inventory or at the player's feet when that can't be.
pub(crate) fn put_cursor(host: &mut dyn ServerHost, player: u128, st: Option<ItemStack>) {
    let Some(st) = st else { return };
    let n = st.count();
    let ok = cursor_place(&*host, player).is_some_and(|place| {
        host.transaction(|host, tx| if place.insert(host, tx, &ItemResource::of(&st), n).unwrap_or(0) == n { Ok(()) } else { Err(()) }).is_ok()
    });
    if !ok {
        give_back(host, player, st);
    }
}

/// A stack into the player's inventory (Inventory.add), what doesn't fit at their feet.
pub(crate) fn give_back(host: &mut dyn ServerHost, player: u128, st: ItemStack) {
    let Some(p) = host.player(player) else { return };
    if p.give_stack(host, st.clone()).is_err() {
        let _ = p.drop_stack(host, st, false);
    }
}

pub(crate) fn send_cursor(host: &mut dyn ServerHost, player: u128) {
    let c = cursor_place(&*host, player).and_then(|p| p.stack_owned(host));
    let text = match c {
        Some(s) => format!("cursor {}", stack_hex(&*host, &s)),
        None => "cursor -".to_owned(),
    };
    let _ = send_to_player(host, player, NET_WIRELESS, text.as_bytes());
}

/// PICKUP between the cursor and a slot holding up to `limit` (AbstractContainerMenu.doClick): an empty cursor takes all
/// (or half, `right`); a full one puts all (or one) in, tops up the same item, or swaps with another. What the cursor
/// and the slot hold after.
pub(crate) fn pickup(host: &dyn Host, cursor: Option<ItemStack>, slot: Option<ItemStack>, right: bool, limit: u32, may_place: bool) -> (Option<ItemStack>, Option<ItemStack>) {
    match (cursor, slot) {
        (None, None) => (None, None),
        (None, Some(s)) => {
            let take = if right { s.count().div_ceil(2) } else { s.count() };
            (with_count(&s, take), with_count(&s, s.count() - take))
        }
        (Some(c), None) => {
            if !may_place {
                return (Some(c), None);
            }
            let most = limit.min(max_stack(host, &c));
            let put = if right { 1 } else { c.count() }.min(most);
            (with_count(&c, c.count() - put), with_count(&c, put))
        }
        (Some(c), Some(s)) => {
            if c.is_stackable_with(&s) {
                if !may_place {
                    // A slot that takes nothing: the cursor takes from it, as far as it holds.
                    let room = max_stack(host, &c).saturating_sub(c.count());
                    let take = s.count().min(room);
                    return (with_count(&c, c.count() + take), with_count(&s, s.count() - take));
                }
                let most = limit.min(max_stack(host, &s));
                let room = most.saturating_sub(s.count());
                let put = if right { 1 } else { c.count() }.min(room);
                (with_count(&c, c.count() - put), with_count(&s, s.count() + put))
            } else if may_place && c.count() <= limit.min(max_stack(host, &c)) {
                (Some(s), Some(c))
            } else {
                (Some(c), Some(s))
            }
        }
    }
}

/// A click on the player's inventory slot `slot` (0..36) on an item screen: PICKUP with the cursor, or with shift the
/// stack into `store` (which answers how many it took). The session's own slot takes no click.
pub(crate) fn inventory_click(host: &mut dyn ServerHost, player: u128, s: &Session, slot: i32, right: bool, shift: bool, store: &mut dyn FnMut(&mut dyn ServerHost, &ItemStack) -> u32) {
    if !(0..36).contains(&slot) || slot == s.slot {
        return;
    }
    let Some(p) = host.player(player) else { return };
    let place = ItemPlace::PlayerSlot { player, slot };
    if shift {
        let Some(st) = place.stack_owned(host) else { return };
        if p.set_inventory_slot(host, slot as u32, None).is_err() {
            return;
        }
        let stored = store(host, &st).min(st.count());
        if let Some(left) = with_count(&st, st.count() - stored)
            && p.set_inventory_slot(host, slot as u32, Some(left.clone())).is_err()
        {
            give_back(host, player, left);
        }
        return;
    }
    let Ok(cursor) = take_cursor(host, player) else { return };
    let there = place.stack_owned(host);
    let (cur, new) = pickup(&*host, cursor.clone(), there.clone(), right, 99, true);
    if new != there && p.set_inventory_slot(host, slot as u32, new.clone()).is_err() {
        // The slot wouldn't change: everything back as it was.
        put_cursor(host, player, cursor);
        return;
    }
    put_cursor(host, player, cur);
}

// ---- Clicks from the client (server)

/// The client's messages, kept for the player's next server tick: a payload handler can't reach the player's
/// inventory slots (the item's slot reads empty there and transactions are refused), so every check and edit of the
/// item runs from `player_tick`.
static INBOX: std::sync::Mutex<Vec<(u128, Vec<u8>)>> = std::sync::Mutex::new(Vec::new());

/// At most this many waiting messages per player; more are dropped (a flood, not play).
const INBOX_PER_PLAYER: usize = 64;

pub(crate) fn from_client(_host: &mut dyn ServerHost, player: u128, data: &[u8]) {
    if data.len() > 128 {
        return;
    }
    let mut inbox = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    if inbox.iter().filter(|(p, _)| *p == player).count() < INBOX_PER_PLAYER {
        inbox.push((player, data.to_vec()));
    }
}

/// The player's waiting messages, oldest first.
fn take_inbox(player: u128) -> Vec<Vec<u8>> {
    let mut inbox = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let mut mine = Vec::new();
    inbox.retain(|(p, d)| {
        if *p == player {
            mine.push(d.clone());
            false
        } else {
            true
        }
    });
    mine
}

fn handle_client(host: &mut dyn ServerHost, player: u128, data: &[u8]) {
    if data.len() > 128 {
        return;
    }
    // A close is never dropped by the rate limit: the session (and the item's open mark) must end with the screen.
    if data != b"close" && !storage::rate_ok(player) {
        return;
    }
    crate::cells::ids(&*host);
    let text = String::from_utf8_lossy(data).to_string();
    let w: Vec<&str> = text.split_whitespace().collect();
    let num = |i: usize| w.get(i).and_then(|v| v.parse::<i64>().ok());
    let flag = |i: usize| w.get(i) == Some(&"1");
    match w.first().copied() {
        Some("key") => hotkey(host, player, w.get(1).copied().unwrap_or("")),
        Some("close") => close(host, player, None),
        Some("btn") => {
            if let Some(id) = num(1).and_then(|n| i32::try_from(n).ok()) {
                button(host, player, id);
            }
        }
        Some("inv") => {
            if let Some(slot) = num(1).and_then(|n| i32::try_from(n).ok()) {
                inventory(host, player, slot, flag(2), flag(3));
            }
        }
        Some("cell") => {
            if let Some(cell) = num(1).filter(|c| (0..9).contains(c)) {
                grid_cell(host, player, cell as usize, flag(2), flag(3));
            }
        }
        Some("card") => {
            if let Some(i) = num(1).filter(|c| (0..8).contains(c)) {
                card(host, player, i as usize);
            }
        }
        Some("tb") => {
            if let Some(i) = num(1).filter(|c| (0..9).contains(c)) {
                crate::toolbox::slot_click(host, player, i as usize, flag(2), flag(3));
            }
        }
        Some("belt") => {
            if let Some(i) = num(1).filter(|c| (0..9).contains(c)) {
                crate::toolbox::belt_click(host, player, i as usize, flag(2));
            }
        }
        // jeix: JEI's "+" on the wireless crafting terminal (`wct_transfer`).
        Some(crate::wct_transfer::HEAD) => {
            if let Some((ctrl, recipe)) = crate::wct_transfer::parse(&w) {
                crate::wct_transfer::fill(host, player, ctrl, &recipe);
            }
        }
        _ => {}
    }
}

/// Hotkeys.checkHotkeys: the first such item the player carries opens (the inventory, then the off hand).
fn hotkey(host: &mut dyn ServerHost, player: u128, name: &str) {
    let fits = |item: &str| match name {
        "wireless" => crate::powered::is_wireless_terminal(item),
        "portable_item" => item.starts_with("ae2:portable_item_cell_"),
        "portable_fluid" => item.starts_with("ae2:portable_fluid_cell_"),
        _ => false,
    };
    let slot = (0..36).chain(std::iter::once(-106)).find(|&slot| ItemPlace::PlayerSlot { player, slot }.stack_owned(host).is_some_and(|s| fits(sid(&s))));
    if let Some(slot) = slot {
        open(host, player, slot);
    }
}

/// A terminal button (the grid, the crafting part): a wireless terminal's is the terminal's (`storage::menu_button`,
/// on the grid the access point reaches; the crafting grid from the item), a portable cell's its own.
fn button(host: &mut dyn ServerHost, player: u128, id: i32) {
    let Some((s, stack)) = checked(host, player) else { return };
    match s.kind {
        Kind::Wireless | Kind::WirelessCrafting => {
            let Some(wap) = s.wap else { return };
            let Ok(mut carried) = take_cursor(host, player) else { return };
            let mut fields = Fields::default();
            fields.items = vec![None; 11];
            let grid = if s.kind == Kind::WirelessCrafting { grid_of(&*host, &stack) } else { vec![None; 9] };
            for (i, g) in grid.iter().enumerate() {
                fields.items[1 + i] = g.clone();
            }
            storage::menu_button(host, player, wap, id, &mut fields, &mut carried);
            put_cursor(host, player, carried);
            if s.kind == Kind::WirelessCrafting {
                let now: Vec<Option<ItemStack>> = fields.items[1..10].to_vec();
                if now != grid {
                    let tag = grid_tag(&*host, &now);
                    let _ = edit_held(host, player, &s, |st| set_grid(st, tag));
                }
            }
        }
        Kind::PortableItem | Kind::PortableFluid => {
            let Ok(mut carried) = take_cursor(host, player) else { return };
            crate::portable::button(host, player, &s, stack, id, &mut carried);
            put_cursor(host, player, carried);
        }
        Kind::Toolbox => {}
    }
    send_cursor(host, player);
}

fn inventory(host: &mut dyn ServerHost, player: u128, slot: i32, right: bool, shift: bool) {
    let Some((s, stack)) = checked(host, player) else { return };
    let wap = s.wap;
    let kind = s.kind;
    let mut store = |host: &mut dyn ServerHost, st: &ItemStack| -> u32 {
        match kind {
            Kind::Wireless | Kind::WirelessCrafting => {
                let Some(wap) = wap else { return 0 };
                let key = key_of(&*host, st);
                storage::insert(host, Scope::Grid(wap), None, &key, i64::from(st.count())).clamp(0, i64::from(st.count())) as u32
            }
            Kind::PortableItem | Kind::PortableFluid => crate::portable::store(host, player, &s, &stack, st),
            Kind::Toolbox => crate::toolbox::store(host, player, &s, st),
        }
    };
    inventory_click(host, player, &s, slot, right, shift, &mut store);
    send_cursor(host, player);
}

/// A click on a cell of the wireless crafting terminal's grid: PICKUP with the cursor, with shift into the network.
fn grid_cell(host: &mut dyn ServerHost, player: u128, cell: usize, right: bool, shift: bool) {
    let Some((s, stack)) = checked(host, player) else { return };
    if s.kind != Kind::WirelessCrafting {
        return;
    }
    let Some(wap) = s.wap else { return };
    let mut grid = grid_of(&*host, &stack);
    let before = grid.clone();
    if shift {
        let Some(st) = grid[cell].take() else { return };
        let key = key_of(&*host, &st);
        let stored = storage::insert(host, Scope::Grid(wap), None, &key, i64::from(st.count())).clamp(0, i64::from(st.count())) as u32;
        grid[cell] = with_count(&st, st.count() - stored);
        let tag = grid_tag(&*host, &grid);
        if !edit_held(host, player, &s, |st| set_grid(st, tag)) {
            // The item went (it was checked this call: not expected): what went into the network comes back out.
            let _ = storage::extract(host, Scope::Grid(wap), None, &key, i64::from(stored));
        }
        return;
    }
    let Ok(cursor) = take_cursor(host, player) else { return };
    let (cur, new) = pickup(&*host, cursor.clone(), grid[cell].clone(), right, 99, true);
    grid[cell] = new;
    if grid != before {
        let tag = grid_tag(&*host, &grid);
        if !edit_held(host, player, &s, |st| set_grid(st, tag)) {
            put_cursor(host, player, cursor);
            return;
        }
    }
    put_cursor(host, player, cur);
    send_cursor(host, player);
}

/// A click on one of the item's upgrade slots: a card from the cursor in, or the card there onto an empty cursor (an
/// Energy Card taken out takes its charge above the new capacity with it, as AE2's).
fn card(host: &mut dyn ServerHost, player: u128, i: usize) {
    let Some((s, stack)) = checked(host, player) else { return };
    if i >= s.kind.card_slots() {
        return;
    }
    let mut cards = cell_cards(&stack);
    let Ok(cursor) = take_cursor(host, player) else { return };
    let mut cur = cursor.clone();
    match cursor.as_ref() {
        Some(c) => {
            let id = sid(c);
            let have = cards.iter().filter(|x| x.as_str() == id).count();
            if cards.len() < s.kind.card_slots() && have < s.kind.card_limit(id) {
                cards.push(id.to_owned());
                cur = with_count(c, c.count() - 1);
            }
        }
        None => {
            if i < cards.len() {
                let id = cards.remove(i);
                cur = new_stack(&*host, &id, 1);
            }
        }
    }
    if cur != cursor {
        let ok = edit_held(host, player, &s, |st| {
            set_cards(st, &cards);
            if let Some(most) = crate::powered::capacity(st) {
                let p = item_power(st);
                if p > most {
                    set_item_power(st, most);
                }
            }
        });
        if !ok {
            put_cursor(host, player, cursor);
            return;
        }
    }
    put_cursor(host, player, cur);
    send_cursor(host, player);
}

/// The wireless crafting terminal's result now, for storage's stream (none when the grid makes nothing).
pub(crate) fn grid_result(host: &mut dyn ServerHost, player: u128) -> Option<ItemStack> {
    let s = session(player).filter(|s| s.kind == Kind::WirelessCrafting)?;
    let stack = held(host, player, &s)?;
    let cells = grid_of(&*host, &stack);
    if cells.iter().all(Option::is_none) {
        return None;
    }
    let level = here(&*host)?;
    let crafting = Identifier::parse("minecraft:crafting").ok()?;
    level.recipe_for(&*host, &crafting, &RecipeInput::Crafting { width: 3, height: 3, slots: cells }).ok().flatten().map(|m| m.output)
}

// ---- Item parts

/// The wireless terminals', portable cells' and network tool's use in the air: their screen (the network tool's toolbox;
/// its use on a block is the network status and the wrench).
pub(crate) struct ItemScreen;

#[mod_api::sunrise_part]
impl ItemPart for ItemScreen {
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => {
                crate::cells::ids(&*host);
                // Sneaking, an empty portable cell is taken apart (PortableCellItem.disassembleDrive) instead.
                if let Some(result) = crate::cell_tools::use_item(host, event.stack, event.hand, event.player) {
                    return Some(result);
                }
                let slot = match event.hand {
                    Hand::Main => host.player(event.player).and_then(|p| p.selected_slot(&*host)).unwrap_or(0) as i32,
                    Hand::Off => -106,
                };
                open(host, event.player, slot);
                Some(InteractionResult::Success)
            }
            HostSide::Client(_) => Some(InteractionResult::Success),
        }
    }
}

pub(crate) fn register(r: &mut dyn Registrar) {
    let mut items: Vec<String> = vec![WT.to_owned(), WCT.to_owned(), TOOLBOX.to_owned()];
    for size in ["1k", "4k", "16k", "64k", "256k"] {
        items.push(format!("ae2:portable_item_cell_{size}"));
        items.push(format!("ae2:portable_fluid_cell_{size}"));
    }
    for id in &items {
        let k = r.item_key(id);
        r.item_part(k, ItemScreen);
    }
    for (id, label, _) in KEYS {
        r.register_key_mapping(KeyRegistration::new(id, "key.keyboard.unknown").label(label).category("key.ae2.category").context(KeyConflictContext::InGame));
    }
}

// ---- The screen (client)

#[derive(Default)]
struct Ui {
    kind: Option<Kind>,
    slot: i32,
    cards: usize,
    cursor: Option<ItemStack>,
    /// Messages for the server, sent at the next client tick (the screen hooks have no mutable host).
    queued: Vec<String>,
    /// The screen was opened (the client tick tells the server when it closed).
    shown: bool,
}

static UI: Mutex<Option<Ui>> = Mutex::new(None);

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> R {
    let mut g = UI.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Ui::default))
}

/// A message for the server from a screen hook (the toolbelt's clicks too).
pub(crate) fn queue(msg: String) {
    with_ui(|u| {
        if u.queued.len() < 32 {
            u.queued.push(msg);
        }
    });
}

pub(crate) fn from_server(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != NET_WIRELESS {
        return false;
    }
    crate::cells::ids(&*host);
    let text = String::from_utf8_lossy(data).to_string();
    let w: Vec<&str> = text.split_whitespace().collect();
    match w.first().copied() {
        Some("open") => {
            let kind = w.get(1).and_then(|k| Kind::from_screen(k));
            let slot = w.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
            let cards = w.get(3).and_then(|v| v.parse().ok()).unwrap_or(0usize).min(8);
            with_ui(|u| {
                u.kind = kind;
                u.slot = slot;
                u.cards = cards;
                u.cursor = None;
            });
            if kind.is_some() {
                let open = host.mod_screen() == Some(SCREEN) || host.mod_screen_open(SCREEN);
                with_ui(|u| u.shown = open);
                if !open {
                    with_ui(|u| u.kind = None);
                    let _ = send_to_server(host, NET_WIRELESS, b"close");
                }
            }
        }
        Some("close") => {
            let was = with_ui(|u| {
                u.shown = false;
                u.kind.take().is_some()
            });
            if was && host.mod_screen() == Some(SCREEN) {
                host.close_screen();
            }
        }
        Some("cursor") => {
            let c = w.get(1).filter(|h| **h != "-").and_then(|h| stack_from_hex(&*host, h));
            with_ui(|u| u.cursor = c);
        }
        _ => {}
    }
    true
}

/// The open item screen's kind.
fn open_kind(host: &dyn ClientHost) -> Option<Kind> {
    if host.mod_screen() != Some(SCREEN) {
        return None;
    }
    with_ui(|u| u.kind)
}

/// jeix: the open item screen's kind, for JEI's "+" (`wct_transfer`).
pub(crate) fn open_kind_of(host: &dyn ClientHost) -> Option<Kind> {
    open_kind(host)
}

/// jeix: the stack under the pointer on an item screen, for JEI's R and U: its id and, for a facade, its components
/// as JEI's facade stacks write them (the inventory, the crafting grid, the terminal's list).
pub(crate) fn hovered_stack(host: &dyn ClientHost, v: &ScreenView<'_>) -> Option<(String, String)> {
    let kind = open_kind(host)?;
    if kind == Kind::Toolbox {
        return None;
    }
    let o = [v.rect[0], v.rect[1]];
    let h = v.rect[3];
    let at = [v.mouse[0] - o[0], v.mouse[1] - o[1]];
    let of_stack = |s: &ItemStack| {
        let id = sid(s).to_owned();
        let components = crate::jei_facades::facade_block(s).map(|b| crate::jei_facades::facade_json(&b)).unwrap_or_default();
        (id, components)
    };
    if let Some(slot) = inventory_slot_at(at, inv_top(kind, h)) {
        let inv = host.local_player().and_then(|p| p.inventory_slots_owned(host))?;
        return inv.into_iter().find(|(i, _)| *i as i32 == slot).map(|(_, s)| of_stack(&s));
    }
    if kind == Kind::WirelessCrafting {
        for i in 0..9 {
            let p = [GRID_X + (i % 3) as f32 * 18.0, h - GRID_UP + (i / 3) as f32 * 18.0];
            if inside(at, [p[0], p[1], 16.0, 16.0]) {
                let grid = local_item(host).map(|s| grid_of(host, &s)).unwrap_or_default();
                return grid.get(i).cloned().flatten().map(|s| of_stack(&s));
            }
        }
    }
    let (_, key, _) = crate::screens::entry_index_at(at)?;
    (key.key_type() == crate::core::KeyType::Items).then(|| (key.id().to_owned(), String::new()))
}

/// The item screen's size and rows (`gui` the GUI's size).
fn layout(kind: Kind, gui: [f32; 2]) -> ([f32; 2], usize) {
    if kind == Kind::Toolbox {
        return ([176.0, 166.0], 0);
    }
    let fixed = if kind.portable() { Some(3) } else { None };
    let rows = crate::screens::item_rows(gui, kind.bottom(), fixed);
    ([195.0, 17.0 + 18.0 * rows as f32 + kind.bottom()], rows)
}

/// The open item screen as the terminal's code sees it: its kind as the menu, its window as the rectangle.
pub(crate) fn view<'a>(host: &dyn ClientHost, screen: &ScreenView<'a>) -> Option<ScreenView<'a>> {
    let kind = open_kind(host)?;
    let gui = crate::screens::gui();
    let ([w, h], _) = layout(kind, gui);
    let mut v = *screen;
    v.menu = Some(kind.screen());
    v.rect = [((gui[0] - w) / 2.0).floor(), ((gui[1] - h) / 2.0).floor(), w, h];
    Some(v)
}

fn inside(at: [f32; 2], r: [f32; 4]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// The player's inventory slot (hotbar 0..9, then the rows) at `at`, the inventory's top at `inv_y`.
fn inventory_slot_at(at: [f32; 2], inv_y: f32) -> Option<i32> {
    let (cx, cy) = ((at[0] - 8.0) / 18.0, at[1] - inv_y);
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

fn inventory_slot_pos(slot: i32, inv_y: f32) -> [f32; 2] {
    if slot < 9 { [8.0 + 18.0 * slot as f32, inv_y + 58.0] } else { [8.0 + 18.0 * ((slot - 9) % 9) as f32, inv_y + 18.0 * ((slot - 9) / 9) as f32] }
}

fn inv_top(kind: Kind, h: f32) -> f32 {
    if kind == Kind::Toolbox { 84.0 } else { h - 82.0 }
}

/// The client's copy of the open item (cards, the crafting grid, the toolbox are read from it).
fn local_item(host: &dyn ClientHost) -> Option<ItemStack> {
    let slot = with_ui(|u| u.slot);
    let want: u32 = if slot == -106 { 40 } else { u32::try_from(slot).ok()? };
    host.local_player()?.inventory_slots_owned(host)?.into_iter().find(|(i, _)| *i == want).map(|(_, s)| s)
}

fn stack_lines(host: &dyn ClientHost, s: &ItemStack) -> Vec<Text> {
    let lines = ItemResource::of(s).tooltip(host, false, false).unwrap_or_default();
    let mut out: Vec<Text> = lines
        .into_iter()
        .filter_map(|l| match l {
            TooltipElement::Text(t, _) => Some(t),
            _ => None,
        })
        .collect();
    if out.is_empty() {
        out.push(Text::literal(crate::screens::key_name(host, &key_of(host, s))));
    }
    out
}

fn draw_stack(d: &mut dyn DrawContext, s: &ItemStack, at: [f32; 2]) {
    let n = s.count();
    let label = (n > 1).then(|| n.to_string());
    d.gui_draw_item_with(s, at, label.as_deref());
}

/// `screen_render_post` for an item screen: the terminal (or the toolbox), the cards, the crafting grid, the
/// inventory, the cursor and the tooltip under the pointer.
pub(crate) fn draw(host: &dyn ClientHost, d: &mut dyn DrawContext, v: &ScreenView<'_>) {
    let Some(kind) = open_kind(host) else { return };
    let o = [v.rect[0], v.rect[1]];
    let h = v.rect[3];
    let m = v.mouse;
    let at = [m[0] - o[0], m[1] - o[1]];
    let item = local_item(host);
    let mut tip: Option<Vec<Text>> = None;
    // Screen.renderTransparentBackground: the world and the HUD dimmed behind the window, as behind a menu screen.
    let size = d.size();
    d.fill_gradient([0.0, 0.0, size[0], size[1]], 0xC010_1010, 0xD010_1010);
    if kind == Kind::Toolbox {
        crate::toolbox::draw(host, d, o, item.as_ref(), at, &mut tip);
    } else {
        crate::screens::screen_draw(host, kind.screen(), d, o);
        // The upgrade panel (the item's cards).
        let n = with_ui(|u| u.cards);
        if n > 0 {
            side_panel(d, o, 195.0, n);
            let cards = item.as_ref().map(cell_cards).unwrap_or_default();
            for i in 0..n {
                let p = [o[0] + 205.0, o[1] + 8.0 + 18.0 * i as f32];
                if let Some(s) = cards.get(i).and_then(|c| new_stack(host, c, 1)) {
                    d.gui_draw_item_with(&s, p, None);
                    if inside(at, [205.0, 8.0 + 18.0 * i as f32, 16.0, 16.0]) {
                        tip = Some(stack_lines(host, &s));
                    }
                } else if inside(at, [205.0, 8.0 + 18.0 * i as f32, 16.0, 16.0]) {
                    tip = Some(vec![Text::literal(tr(host, "gui.ae2.CompatibleUpgrades", "Compatible Upgrades:"))]);
                }
            }
        }
        if kind == Kind::WirelessCrafting {
            let grid = item.as_ref().map(|s| grid_of(host, s)).unwrap_or_default();
            for (i, g) in grid.iter().enumerate() {
                let p = [GRID_X + (i % 3) as f32 * 18.0, h - GRID_UP + (i / 3) as f32 * 18.0];
                if let Some(s) = g {
                    draw_stack(d, s, [o[0] + p[0], o[1] + p[1]]);
                    if inside(at, [p[0], p[1], 16.0, 16.0]) {
                        tip = Some(stack_lines(host, s));
                    }
                }
                if inside(at, [p[0], p[1], 16.0, 16.0]) {
                    d.fill([o[0] + p[0], o[1] + p[1], 16.0, 16.0], 0x80FF_FFFF);
                }
            }
        }
        if tip.is_none() && !crate::craft_screens::covered() && !crate::term_ui::settings_open() {
            let mut lines = Vec::new();
            crate::screens::screen_tooltip(host, kind.screen(), at, &mut lines);
            if !lines.is_empty() {
                tip = Some(lines.iter().map(|l| Text::literal(l.trim_start_matches("§7"))).collect());
            }
        }
    }
    // The inventory, its slot under the pointer lit, the item's own slot dimmed (it takes no click).
    let inv_y = inv_top(kind, h);
    let own = with_ui(|u| u.slot);
    if let Some(inv) = host.local_player().and_then(|p| p.inventory_slots_owned(host)) {
        for (i, s) in inv.iter().filter(|(i, _)| *i < 36) {
            let p = inventory_slot_pos(*i as i32, inv_y);
            draw_stack(d, s, [o[0] + p[0], o[1] + p[1]]);
            if inside(at, [p[0], p[1], 16.0, 16.0]) && !crate::craft_screens::covered() {
                tip = Some(stack_lines(host, s));
            }
        }
    }
    if (0..36).contains(&own) {
        let p = inventory_slot_pos(own, inv_y);
        d.fill([o[0] + p[0], o[1] + p[1], 16.0, 16.0], 0x80_202020);
    }
    if let Some(slot) = inventory_slot_at(at, inv_y) {
        let p = inventory_slot_pos(slot, inv_y);
        d.fill([o[0] + p[0], o[1] + p[1], 16.0, 16.0], 0x80FF_FFFF);
    }
    if let Some(c) = with_ui(|u| u.cursor.clone()) {
        draw_stack(d, &c, [m[0] - 8.0, m[1] - 8.0]);
        tip = None;
    }
    if let Some(lines) = tip {
        let _ = d.gui_tooltip(&lines, m);
    }
}

/// A press on the item screen (the terminal's own overlays had their turn): the cards, the crafting grid and the
/// inventory are this module's; the rest is the terminal's (`screens::screen_click`), its buttons sent to the server.
/// Every press is taken.
pub(crate) fn pressed(host: &dyn ClientHost, v: &ScreenView<'_>, button: i32) -> bool {
    let Some(kind) = open_kind(host) else { return false };
    let o = [v.rect[0], v.rect[1]];
    let h = v.rect[3];
    let at = [v.mouse[0] - o[0], v.mouse[1] - o[1]];
    if !(0..=1).contains(&button) {
        return true;
    }
    let right = button == 1;
    let shift = host.is_key_down("key.keyboard.left.shift") || host.is_key_down("key.keyboard.right.shift");
    let b = |x: bool| u8::from(x);
    if let Some(slot) = inventory_slot_at(at, inv_top(kind, h)) {
        queue(format!("inv {slot} {} {}", b(right), b(shift)));
        return true;
    }
    if kind == Kind::Toolbox {
        if let Some(i) = crate::toolbox::slot_at(at) {
            queue(format!("tb {i} {} {}", b(right), b(shift)));
        }
        return true;
    }
    let n = with_ui(|u| u.cards);
    for i in 0..n {
        if inside(at, [205.0, 8.0 + 18.0 * i as f32, 16.0, 16.0]) {
            queue(format!("card {i}"));
            return true;
        }
    }
    if kind == Kind::WirelessCrafting {
        for i in 0..9 {
            let p = [GRID_X + (i % 3) as f32 * 18.0, h - GRID_UP + (i / 3) as f32 * 18.0];
            if inside(at, [p[0], p[1], 16.0, 16.0]) {
                queue(format!("cell {i} {} {}", b(right), b(shift)));
                return true;
            }
        }
    }
    if let Some(id) = crate::screens::screen_click(host, kind.screen(), at, right)
        && id != crate::screens::CLIENT_ONLY
    {
        queue(format!("btn {id}"));
    }
    true
}

/// A middle press on a portable cell's grid asks nothing of the network (it has none): taken before the crafting
/// dialogs see it.
pub(crate) fn portable_middle(host: &dyn ClientHost, button: i32) -> bool {
    button == 2 && open_kind(host).is_some_and(Kind::portable)
}

/// The wheel with shift over an entry queues ROLL_UP/ROLL_DOWN buttons in `screens` (sent as menu buttons there); an
/// item screen has no menu, so they come here as buttons for the server.
pub(crate) fn take_screen_buttons(host: &dyn ClientHost) {
    if open_kind(host).is_none() {
        return;
    }
    for id in crate::screens::take_queued() {
        queue(format!("btn {id}"));
    }
}

/// The client's tick: the hotkeys (with no screen open), the queued messages, and a closed screen told to the server.
pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    take_screen_buttons(&*host);
    for (id, _, name) in KEYS {
        let clicked = host.mod_key_mapping("ae2", id).is_ok_and(|m| m.consume_click(host).unwrap_or(false));
        if clicked {
            queue(format!("key {name}"));
        }
    }
    let gone = with_ui(|u| u.shown && u.kind.is_some()) && host.mod_screen() != Some(SCREEN);
    if gone {
        with_ui(|u| {
            u.kind = None;
            u.shown = false;
            u.cursor = None;
        });
        queue("close".to_owned());
    }
    let queued = with_ui(|u| std::mem::take(&mut u.queued));
    for msg in queued {
        let _ = send_to_server(host, NET_WIRELESS, msg.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_follows_boosters() {
        assert_eq!(max_range(16.0, 1.0, 1.5, 0.0), 16.0);
        assert_eq!(max_range(16.0, 1.0, 1.5, 4.0), 24.0);
        assert!((max_range(16.0, 1.0, 1.5, 64.0) - 528.0).abs() < 1e-9);
    }

    #[test]
    fn access_point_drain_grows_with_boosters() {
        assert_eq!(wap_drain(8.0, 1.0, 64.0, 0.0), 8.0);
        assert!((wap_drain(8.0, 1.0, 64.0, 1.0) - (8.0 + 1f64.powf(1.0 + 1.0 / 64.0))).abs() < 1e-9);
        assert!(wap_drain(8.0, 1.0, 64.0, 32.0) > wap_drain(8.0, 1.0, 64.0, 16.0));
    }

    #[test]
    fn nearest_working_access_point_in_range() {
        let at = [0.5, 64.5, 0.5];
        // Far and in range, near but off, near and in range, out of range.
        let waps = [([0, 64, 20], 24.0, true), ([0, 64, 2], 16.0, false), ([0, 64, 10], 16.0, true), ([0, 64, 40], 16.0, true)];
        assert_eq!(best_wap(at, &waps).map(|(p, _)| p), Some([0, 64, 10]));
        assert_eq!(best_wap(at, &waps[3..]), None);
        assert_eq!(best_wap(at, &[]), None);
        assert!((distance(at, [0, 64, 10]) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn drain_by_distance() {
        assert_eq!(drain_ten_ticks(0.0, 1.0, 1.0), 0.0);
        assert_eq!(drain_ten_ticks(12.0, 1.0, 1.0), 120.0);
        assert_eq!(drain_ten_ticks(12.0, 0.5, 2.0), 120.0);
    }

    #[test]
    fn inventory_slots_on_screen() {
        let inv_y = 100.0;
        assert_eq!(inventory_slot_at([8.0, inv_y + 58.0], inv_y), Some(0));
        assert_eq!(inventory_slot_at([8.0 + 18.0 * 8.0 + 1.0, inv_y + 60.0], inv_y), Some(8));
        assert_eq!(inventory_slot_at([9.0, inv_y + 1.0], inv_y), Some(9));
        assert_eq!(inventory_slot_at([8.0 + 18.0 * 2.0, inv_y + 36.0], inv_y), Some(29));
        assert_eq!(inventory_slot_at([7.0, inv_y], inv_y), None);
        assert_eq!(inventory_slot_at([8.0, inv_y + 55.0], inv_y), None);
        for slot in 0..36 {
            let p = inventory_slot_pos(slot, inv_y);
            assert_eq!(inventory_slot_at([p[0] + 1.0, p[1] + 1.0], inv_y), Some(slot));
        }
    }

    #[test]
    fn kinds_and_cards() {
        assert_eq!(Kind::of_item("ae2:portable_fluid_cell_4k"), Some(Kind::PortableFluid));
        assert_eq!(Kind::of_item("ae2:wireless_crafting_terminal"), Some(Kind::WirelessCrafting));
        assert_eq!(Kind::of_item("ae2:terminal"), None);
        assert_eq!(Kind::Wireless.card_limit("ae2:energy_card"), 2);
        assert_eq!(Kind::PortableFluid.card_limit("ae2:fuzzy_card"), 0);
        assert_eq!(Kind::PortableItem.card_slots(), 4);
        assert!(is_item_term(WCT) && is_portable(PORTABLE_FLUID) && !is_item_term(TOOLBOX));
    }

    #[test]
    fn hex_round_trip() {
        let b = vec![0u8, 1, 0xAB, 0xFF];
        assert_eq!(unhex(&hex(&b)), Some(b));
        assert_eq!(unhex("abc"), None);
    }
}
