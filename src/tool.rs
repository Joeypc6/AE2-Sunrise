//! The network tool (NetworkToolItem): used on a block of a network, AE2's network status screen (NetworkStatusScreen:
//! stored and max power, the devices by kind with their counts, missing channels in red, passive drain, usage and
//! generation); sneak-used, a wrench on a cable bus. Upstream's `network_status_lines` and its "status" screen
//! (`../ae2-horizon/mixins/ae2/me.rs`), on the fork's `mod_screen_open`, net messages and `DrawContext`.
//! Its toolbox (a use in the air: nine upgrade slots) and the upgrade toolbelt are `toolbox`'s.

use crate::screens::{spr, tr};
use crate::util::*;
use mod_api::*;
use std::sync::Mutex;

pub(crate) const NET_STATUS: &str = "ae2:status";
pub(crate) const SCREEN: &str = "ae2:network_status";
const TEXT: u32 = 0xFF40_4040;

/// The network tool's Part.
pub(crate) struct NetworkTool;

#[mod_api::sunrise_part]
impl ItemPart for NetworkTool {
    fn on_item_use_first(&self, host: &mut dyn ServerHost, event: &mut OnItemUseFirstEvent<'_>) -> Option<InteractionResult> {
        let player = host.player(event.click.player)?;
        let e = player.entity();
        if e.is_shift_key_down(&*host).unwrap_or(false) || e.is_crouching(&*host).unwrap_or(false) {
            // The network tool is a wrench too (IAEWrench).
            return crate::bus::wrench(host, event.click);
        }
        let level = here(&*host)?;
        let pos = crate::bus::aimed_node(&*host, level, event.click).unwrap_or(event.click.pos);
        let dim = host.dimension();
        let lines = status_lines(dim, pos)?;
        let _ = crate::util::send_to_player(host, event.click.player, NET_STATUS, lines.join("\n").as_bytes());
        debug(&format!("tool: status of {pos:?}: {} lines", lines.len()));
        Some(InteractionResult::SuccessServer)
    }
}

/// polish: a controller used with an empty main hand (not sneaking) shows its network's status, as upstream's
/// controller screen did; the same screen as the network tool's.
pub(crate) fn controller_use(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    if click.stack.is_some() || click.hand != Hand::Main {
        return None;
    }
    let level = here(&*host)?;
    if crate::net::block_id(&*host, level, click.pos) != "ae2:controller" {
        return None;
    }
    let player = host.player(click.player)?;
    let e = player.entity();
    if e.is_shift_key_down(&*host).unwrap_or(false) || e.is_crouching(&*host).unwrap_or(false) {
        return None;
    }
    let lines = status_lines(host.dimension(), click.pos)?;
    let _ = crate::util::send_to_player(host, click.player, NET_STATUS, lines.join("\n").as_bytes());
    Some(InteractionResult::SuccessServer)
}

/// NetworkStatus.fromGrid: `t` power lines (AE), `c` channels, `n <item> <count> <missing> <idle> <generates>`.
fn status_lines(dim: &'static str, pos: Pos) -> Option<Vec<String>> {
    let (stored, cap) = crate::net::grid_power(dim, pos)?;
    let devices = crate::net::grid_devices(dim, pos, false)?;
    let (channel_drain, channels, active, usage, input) = crate::net::with_net(dim, |n| {
        let g = n.grids.get(*n.grid_of.get(&pos)?)?;
        let flow = g.power_root().and_then(|r| n.energy_flow.get(&r).copied()).unwrap_or_default();
        Some((g.channel_drain, g.channels, g.active.clone(), flow[3], flow[2]))
    })?;
    let mut groups: Vec<(String, bool, i64, f64, f64)> = Vec::new();
    for (p, id) in devices {
        let kind = crate::net::node_kind(&id);
        let missing = kind.requires_channel && !active.contains(&p);
        let generates = if id == "ae2:crystal_resonance_generator" { crate::config::config().crystal_resonance_generator_rate } else { 0.0 };
        match groups.iter_mut().find(|m| m.0 == id && m.1 == missing) {
            Some(m) => {
                m.2 += 1;
                m.3 += kind.idle_power;
                m.4 += generates;
            }
            None => groups.push((id, missing, 1, kind.idle_power, generates)),
        }
    }
    // MachineGroup.COMPARATOR: those missing a channel first, then the most.
    groups.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    let mut out = vec![
        format!("t gui.ae2.StoredPower|{stored}"),
        format!("t gui.ae2.MaxPower|{cap}"),
        format!("c {channels}"),
        format!("t gui.ae2.ChannelEnergyDrain|{channel_drain}|r"),
        format!("t gui.ae2.PowerUsageRate|{usage}|r"),
        format!("t gui.ae2.PowerInputRate|{input}|r"),
    ];
    out.extend(groups.into_iter().map(|(id, missing, n, idle, generates)| format!("n {id} {n} {} {idle} {generates}", u8::from(missing))));
    Some(out)
}

// ---- Client

#[derive(Default)]
pub(crate) struct View {
    lines: Vec<String>,
    scroll: usize,
}

static VIEW: Mutex<Option<View>> = Mutex::new(None);

/// The status from the server: the screen opens (or follows it).
pub(crate) fn from_server(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != NET_STATUS {
        return false;
    }
    let lines = String::from_utf8_lossy(data).lines().map(str::to_owned).collect();
    if host.mod_screen() == Some(SCREEN) || host.mod_screen_open(SCREEN) {
        let mut v = VIEW.lock().unwrap_or_else(|e| e.into_inner());
        let scroll = v.as_ref().map_or(0, |v| v.scroll);
        *v = Some(View { lines, scroll });
    }
    true
}

pub(crate) fn is_open(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(SCREEN)
}

/// The power unit button (SettingToggleButton on the client's PowerUnits), the left toolbar's first place.
const UNIT_BUTTON: [f32; 4] = [-20.0, 8.0, 16.0, 16.0];

/// Platform.formatPower in the client's unit (AEConfig.getSelectedPowerUnit): AE, or the fork's FE at
/// `PowerRatios.TechReborn` AE each, with k/M/G steps, `/t` for a rate.
fn power(ae: f64, rate: bool) -> String {
    let cfg = crate::config::config();
    let (value, unit) = if cfg.power_tr { (ae / cfg.power_ratio_tech_reborn.max(1e-9), "FE") } else { (ae, "AE") };
    power_in(value, unit, rate)
}

fn power_in(value: f64, unit: &str, rate: bool) -> String {
    let units = ["", "k", "M", "G", "T", "P"];
    let mut v = value;
    let mut i = 0;
    while v >= 1000.0 && i + 1 < units.len() {
        v /= 1000.0;
        i += 1;
    }
    let s = if i == 0 { format!("{}", (v * 10.0).round() / 10.0) } else { format!("{v:.1}") };
    format!("{s} {}{unit}{}", units[i], if rate { "/t" } else { "" })
}

/// A press on the status screen: the power unit button toggles AE and FE (saved in client.json).
pub(crate) fn pressed(host: &dyn ClientHost, screen: &ScreenView<'_>, button: i32) -> bool {
    let o = origin(crate::screens::gui());
    let at = [screen.mouse[0] - o[0], screen.mouse[1] - o[1]];
    let [x, y, w, h] = UNIT_BUTTON;
    if (button == 0 || button == 1) && at[0] >= x && at[0] < x + w && at[1] >= y && at[1] < y + h {
        crate::config::update_client(host, |c| c.power_tr = !c.power_tr);
        return true;
    }
    false
}

fn devices(v: &View) -> Vec<(String, i64, bool, f64, f64)> {
    v.lines
        .iter()
        .filter_map(|l| l.strip_prefix("n "))
        .filter_map(|r| {
            let f: Vec<&str> = r.split(' ').collect();
            let [id, n, missing, idle, generates] = f.as_slice() else { return None };
            Some(((*id).to_owned(), n.parse().unwrap_or(0), *missing == "1", idle.parse().unwrap_or(0.0), generates.parse().unwrap_or(0.0)))
        })
        .collect()
}

fn origin(size: [f32; 2]) -> [f32; 2] {
    [((size[0] - 195.0) / 2.0).floor(), ((size[1] - 157.0) / 2.0).floor()]
}

/// NetworkStatusScreen: the background, the title with the channels, power at the top, five devices a row (four rows,
/// scrolled), the rates under them; a device's tooltip.
pub(crate) fn draw(host: &dyn ClientHost, d: &mut dyn DrawContext, screen: &ScreenView<'_>) {
    let v = VIEW.lock().unwrap_or_else(|e| e.into_inner());
    let Some(v) = v.as_ref() else { return };
    let o = origin(d.size());
    let m = screen.mouse;
    spr(d, "ae2:gui/networkstatus", o, [0.0, 0.0], [0.0, 0.0, 195.0, 157.0]);
    let channels = v.lines.iter().find_map(|l| l.strip_prefix("c ")).unwrap_or("0");
    let title = tr(host, "gui.ae2.NetworkDetails", "Network Details (%s Channels)").replace("%s", channels).replace("%d", channels);
    d.text(&title, [o[0] + 8.0, o[1] + 6.0], TEXT, false);
    let texts: Vec<String> = v
        .lines
        .iter()
        .filter_map(|l| l.strip_prefix("t "))
        .map(|t| {
            let f: Vec<&str> = t.split('|').collect();
            let label = tr(host, f[0], f[0]);
            let ae = f.get(1).and_then(|n| n.parse::<f64>().ok()).unwrap_or(0.0);
            let value = power(ae, f.get(2).is_some());
            if label.contains("%s") { label.replace("%s", &value) } else { format!("{label}: {value}") }
        })
        .collect();
    // The power unit button: Icon.POWER_UNIT_AE or the other unit's.
    let fe = crate::config::config().power_tr;
    crate::screens::state_button(d, o, [UNIT_BUTTON[0], UNIT_BUTTON[1]], if fe { [80.0, 160.0] } else { [0.0, 160.0] });
    for (i, t) in texts.iter().take(2).enumerate() {
        d.text(t, [o[0] + 13.0, o[1] + 16.0 + 10.0 * i as f32], TEXT, false);
    }
    for (i, t) in texts.iter().skip(2).take(3).enumerate() {
        d.text(t, [o[0] + 13.0, o[1] + 123.0 + 10.0 * i as f32], TEXT, false);
    }
    let list = devices(v);
    let most = list.len().div_ceil(5).saturating_sub(4);
    let top = v.scroll.min(most);
    // The scrollbar's handle (AE2's Scrollbar: 12x15 on a 78 high track at 175, 39).
    let y = if most == 0 { 0.0 } else { 63.0 * top as f32 / most as f32 };
    spr(d, "ae2:gui/networkstatus", o, [175.0, 39.0 + y], [if most == 0 { 207.0 } else { 195.0 }, 0.0, 12.0, 15.0]);
    let mut tip: Option<Vec<String>> = None;
    if m[0] >= o[0] + UNIT_BUTTON[0] && m[0] < o[0] + UNIT_BUTTON[0] + 16.0 && m[1] >= o[1] + UNIT_BUTTON[1] && m[1] < o[1] + UNIT_BUTTON[1] + 16.0 {
        tip = Some(vec![tr(host, "gui.tooltips.ae2.PowerUnits", "Power Units"), if fe { "FE".to_owned() } else { tr(host, "gui.ae2.units.appliedenergistics", "AE") }]);
    }
    for (i, (id, n, missing, idle, generates)) in list.iter().enumerate().skip(top * 5).take(20) {
        let cell = i - top * 5;
        let (x, y) = (o[0] + 14.0 + 30.0 * (cell % 5) as f32, o[1] + 41.0 + 18.0 * (cell / 5) as f32);
        if *missing {
            d.fill([x, y, 30.0, 18.0], 0xFFFF_5555);
        }
        let label = if *n >= 10_000 { format!("{}k", n / 1000) } else { n.to_string() };
        // AE2's count: half size (a scaled pose, as the crafting screens draw it), left of the item.
        let w = d.text_width(&label) * 0.5;
        crate::craft_screens::half_text(d, &label, [x + 12.0 - w, y + 6.5], TEXT, false);
        if let Some(stack) = crate::cells::new_stack(host, id, 1) {
            d.gui_draw_item_with(&stack, [x + 13.0, y + 1.0], None);
        }
        if m[0] >= x && m[0] < x + 30.0 && m[1] >= y && m[1] < y + 18.0 {
            let item = id.split(':').nth(1).unwrap_or(id);
            let name = host.translate(&format!("block.ae2.{item}")).or_else(|| host.translate(&format!("item.ae2.{item}"))).unwrap_or_else(|| id.clone());
            let mut lines = vec![name];
            if *missing {
                lines.push(format!("§c{}", tr(host, "gui.ae2.MissingChannel", "Missing Channel")).replace("§c", ""));
            }
            lines.push(format!("{}: {n}", tr(host, "gui.ae2.Installed", "Installed")));
            if *idle > 0.0 {
                lines.push(format!("{}: {}", tr(host, "gui.ae2.EnergyDrain", "Passive Drain"), power(*idle, true)));
            }
            if *generates > 0.0 {
                lines.push(format!("{}: {}", tr(host, "gui.ae2.GenerationCapacity", "Generation Capacity"), power(*generates, true)));
            }
            tip = Some(lines);
        }
    }
    if let Some(lines) = tip {
        let lines: Vec<Text> = lines.iter().map(|l| Text::literal(l)).collect();
        let _ = d.gui_tooltip(&lines, m);
    }
}

pub(crate) fn scrolled(dy: f64) {
    if let Some(v) = VIEW.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let most = devices(v).len().div_ceil(5).saturating_sub(4);
        v.scroll = if dy > 0.0 { v.scroll.saturating_sub(1) } else { (v.scroll + 1).min(most) };
    }
}
