//! AE2's Jade plugin (appeng.integration.modules.jade) through the Jade fork's provider API (`../jade-fork`):
//! GridNodeStateProvider, ChannelDataProvider (smart cables) and PowerStorageDataProvider, from the server's network
//! state. Texts are `waila.ae2.*` keys the client fills in its language (`jade::tr`).
//! Energy, fluids and items need no provider: Jade reads the fork's capabilities.

use crate::net::{self, ae2_name, cable, is_network_block, is_part};
use mod_api::*;

/// TooltipIds, as `config.jade.plugin_ae2.*` names them.
const OPTIONS: [(&str, &str); 7] = [
    ("plugin_ae2.grid_node_state", "AE2 Network State"),
    ("plugin_ae2.power_storage", "AE2 Power State"),
    ("plugin_ae2.crafting_monitor", "AE2 Crafting Monitor"),
    ("plugin_ae2.pattern_provider", "AE2 Pattern Provider"),
    ("plugin_ae2.charger", "AE2 Charger"),
    ("plugin_ae2.part_tooltip", "AE2 Part Tooltip"),
    ("plugin_ae2.debug", "AE2 Debug Info"),
];

pub(crate) fn register() {
    for (key, label) in OPTIONS {
        jade::register_option(key, label, true);
    }
    jade::register_server(lines);
    // polish: a cable bus is named after what the crosshair is on (PartHostTooltips).
    jade::register_name(part_host_name);
    // AE2 draws its own stored power line (as Java's PowerStorageDataProvider), not Jade's FE bar of the buffer.
    jade::hide_storage("ae2:", false, true, true);
}

/// Platform.formatPower: AE with k/M/G steps.
fn format_power(ae: f64) -> String {
    let units = ["", "k", "M", "G", "T", "P"];
    let mut v = ae;
    let mut i = 0;
    while v >= 1000.0 && i + 1 < units.len() {
        v /= 1000.0;
        i += 1;
    }
    let s = if i == 0 { format!("{}", v.floor()) } else { format!("{:.1}", v) };
    format!("{s} {}AE", units[i])
}

fn lines(host: &mut dyn ServerHost, req: &jade::Request, out: &mut Vec<jade::Line>) {
    let (block, pos) = crate::bus::jade_target(&*host, req.pos, &req.block, req.hit).unwrap_or((req.block.to_string(), req.pos));
    if !block.starts_with("ae2:") {
        return;
    }
    let dim = host.dimension();
    let name = ae2_name(&block).unwrap_or("").to_owned();
    let info = net::with_net(dim, |n| {
        let g = n.grid_of.get(&pos).and_then(|g| n.grids.get(*g))?;
        let root = g.power_root()?;
        Some((g.online, g.conflict, g.too_many, g.nested_p2p, g.active.contains(&pos), g.cable_channels.get(&pos).copied().unwrap_or(0), n.energy.get(&root).copied().unwrap_or(0.0)))
    });
    let (online, conflict, too_many, nested, active, channels, _stored) = info.unwrap_or((false, false, false, false, false, 0, 0.0));
    crate::util::debug(&format!("jade: {block} at {pos:?} online {online} active {active}"));
    let part = is_part(&block) || cable(&block).is_some();
    let red = |t: String| jade::Line::text(format!("§c{t}"));
    if part && jade::plugin_on(host, "plugin_ae2.part_tooltip")
        && let Some((_, dense)) = cable(&block)
        && name.contains("_smart_")
    {
        if conflict {
            out.push(red(jade::tr("waila.ae2.ErrorControllerConflict", "Error: Controller Conflict", &[])));
        } else if nested {
            out.push(red(jade::tr("waila.ae2.ErrorNestedP2PTunnel", "Error: Nested P2P Tunnel", &[])));
        } else if too_many {
            out.push(red(jade::tr("waila.ae2.ErrorTooManyChannels", "Error: Too Many Channels", &[])));
        } else {
            let used = if online { channels } else { 0 };
            match crate::grid::channel_factor() {
                0 => out.push(jade::Line::text(jade::tr("waila.ae2.Channels", "%1$d Channels", &[&used.to_string()]))),
                f => out.push(jade::Line::text(jade::tr(
                    "waila.ae2.ChannelsOf",
                    "%1$d of %2$d Channels",
                    &[&used.to_string(), &(if dense { 32 } else { 8 } * f).to_string()],
                ))),
            }
        }
    }
    let grid_block = part || (is_network_block(&block) && !matches!(name.as_str(), "cell_workbench" | "condenser"));
    if grid_block && jade::plugin_on(host, "plugin_ae2.grid_node_state") && cable(&block).is_none() {
        let needs_channel = net::node_kind(&block).requires_channel;
        // GridNodeState.fromNode's order: not powered, booting, no channel.
        let state = if info.is_some() && net::booting(dim, pos) {
            jade::tr("waila.ae2.NetworkBooting", "Network Booting", &[])
        } else if info.is_none() || !online {
            jade::tr("waila.ae2.DeviceOffline", "Device Offline", &[])
        } else if needs_channel && !active {
            jade::tr("waila.ae2.DeviceMissingChannel", "Device Missing Channel", &[])
        } else {
            jade::tr("waila.ae2.DeviceOnline", "Device Online", &[])
        };
        out.push(jade::Line::text(state));
    }
    // CrystalResonanceGeneratorBlockEntity: a second generator on a grid makes nothing.
    if name == "crystal_resonance_generator"
        && let Some(level) = crate::util::here(&*host)
        && crate::bus::nbt_at(&*host, level, pos).is_some_and(|n| crate::util::nbt_i64(&n, "suppressed") != 0)
    {
        out.push(red(jade::tr("waila.ae2.Suppressed", "Suppressed", &[])));
    }
    // polish: the other providers (P2P, charger, crafting monitor, pattern provider, annihilation plane).
    polish_lines(host, &block, &name, pos, online, active, channels, part, out);
    if jade::plugin_on(host, "plugin_ae2.power_storage") {
        // PowerStorageDataProvider: an energy cell's or controller's share of its grid (what it saves).
        let most = net::store_capacity(&block);
        if most > 0.0 {
            let current = net::stored_share(dim, pos).unwrap_or(0.0);
            out.push(jade::Line::text(jade::tr(
                "waila.ae2.Stored",
                "Stored: %s / %s",
                &[&format_power(current.min(most)), &format_power(most)],
            )));
        }
    }
}

// ---- polish: the rest of AE2's Jade providers

/// AEColor.mediumVariant, white to black (the P2P frequency's nibbles).
const AE_COLOR_MEDIUM: [u32; 16] = [
    14408667, 16428612, 12069560, 8563943, 16318282, 12320593, 16299479, 10526880, 13487565, 5352134, 10769358, 5327615, 12031615, 6349614, 16711740,
    5658198,
];

/// P2PHelper's frequency as four hex digits, each in its colour (the high nibble first).
pub(crate) fn p2p_colored_hex(freq: u16) -> String {
    (0..4)
        .map(|i| {
            let nibble = ((freq >> (4 * (3 - i))) & 0xF) as usize;
            format!("§#{:06X}§l{:X}", AE_COLOR_MEDIUM[nibble], nibble)
        })
        .collect::<Vec<_>>()
        .concat()
}

/// P2PStateDataProvider's link line: (key, fallback, count argument).
pub(crate) fn p2p_state(output: bool, peers: usize) -> (&'static str, &'static str, Option<usize>) {
    match (output, peers) {
        (true, n) if n > 0 => ("waila.ae2.P2POutput", "Linked (Output Side)", None),
        (false, 1) => ("waila.ae2.P2PInputOneOutput", "Linked (Input Side)", None),
        (false, n) if n > 1 => ("waila.ae2.P2PInputManyOutputs", "Linked (Input Side) - %d Outputs", Some(n)),
        _ => ("waila.ae2.P2PUnlinked", "Unlinked", None),
    }
}

/// PatternProviderLogic.getCraftingLockedReason, as Jade words it: (key, fallback), from the lock mode (`lock`: 1
/// while high, 2 while low, 3 until a pulse, 4 until a result), the redstone signal and a pending unlock.
pub(crate) fn lock_line(lock: i64, signal: bool, pending: Option<u8>) -> Option<(&'static str, &'static str)> {
    match lock {
        1 if signal => Some(("waila.ae2.CraftingLockedByRedstoneSignal", "Locked by redstone signal")),
        2 if !signal => Some(("waila.ae2.CraftingLockedByLackOfRedstoneSignal", "Locked by lack of redstone signal")),
        3 if pending.is_some() => Some(("waila.ae2.CraftingLockedUntilPulse", "Waiting for redstone pulse to unlock")),
        _ => None,
    }
}

/// An enchantment's name and level, as the game words it ("Efficiency III"; one-level ones alone).
fn enchantment_text(id: &str, level: i32) -> String {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    let name = jade::tr(&format!("enchantment.{ns}.{path}"), path, &[]);
    if level == 1 && matches!(path, "silk_touch" | "infinity" | "mending" | "aqua_affinity" | "channeling" | "flame" | "multishot") {
        return name;
    }
    format!("{name} {}", jade::tr(&format!("enchantment.level.{level}"), &level.to_string(), &[]))
}

#[allow(clippy::too_many_arguments)]
fn polish_lines(host: &mut dyn ServerHost, block: &str, name: &str, pos: crate::util::Pos, online: bool, active: bool, channels: u32, part: bool, out: &mut Vec<jade::Line>) {
    use crate::util::*;
    let dim = host.dimension();
    let Some(level) = here(&*host) else { return };
    let nbt = crate::bus::nbt_at(&*host, level, pos).unwrap_or_default();
    let red = |t: String| jade::Line::text(format!("§c{t}"));
    // ChargerDataProvider: what it holds, and a powered item's charge.
    if name == "charger"
        && jade::plugin_on(host, "plugin_ae2.charger")
        && let Some(stack) = host.block_entity(level, pos).and_then(|be| be.item_slot(&*host, 0).ok().flatten())
    {
        let id = crate::cells::sid(&stack);
        out.push(jade::Line::text(jade::tr("waila.ae2.Contains", "Contains: %s", &[&format!("§f{}", jade::item_name(id))])));
        if let Some(most) = crate::powered::capacity(&stack).filter(|m| *m > 0.0) {
            let pct = (crate::world::item_power(&stack) * 100.0 / most).floor() as i64;
            out.push(jade::Line::text(jade::tr("waila.ae2.Charged", "%d%% charged", &[&pct.to_string()])));
        }
    }
    // CraftingMonitorDataProvider: what its CPU makes.
    if name == "crafting_monitor"
        && jade::plugin_on(host, "plugin_ae2.crafting_monitor")
        && let Some(Tag::String(shown)) = nbt.get(&java("shown"))
        && let Some(key) = crate::cells::key_from_string(&shown.to_string_lossy())
    {
        out.push(jade::Line::text(jade::tr("waila.ae2.Crafting", "Crafting: %s", &[&jade::item_name(key.id())])));
    }
    // PatternProviderDataProvider: why it doesn't push.
    if matches!(name, "pattern_provider" | "cable_pattern_provider") && jade::plugin_on(host, "plugin_ae2.pattern_provider") {
        let signal = level.has_neighbor_signal(&*host, real(pos));
        if let Some((key, fallback)) = lock_line(nbt_i64(&nbt, "lock"), signal, crate::autocraft::lock_pending(dim, pos)) {
            out.push(red(jade::tr(key, fallback, &[])));
        }
    }
    // AnnihilationPlaneDataProvider: its enchantments.
    if part && name == "annihilation_plane" && jade::plugin_on(host, "plugin_ae2.part_tooltip") {
        let enchants = crate::planes::enchantments_of(&nbt);
        if !enchants.is_empty() {
            out.push(jade::Line::text(jade::tr("waila.ae2.EnchantedWith", "Enchanted with:", &[])));
            for (id, lvl) in enchants {
                out.push(jade::Line::text(format!("§7{}", enchantment_text(&id, lvl))));
            }
        }
    }
    // P2PStateDataProvider (powered tunnels): the link, the frequency in its colours, an ME tunnel's channels.
    if block.ends_with("_p2p_tunnel")
        && online
        && let Some(info) = net::p2p_info(dim, pos)
    {
        let peers = if active { net::p2p_peers(dim, pos).len() } else { 0 };
        let (key, fallback, n) = p2p_state(info.output, peers);
        let n = n.map(|n| n.to_string()).unwrap_or_default();
        out.push(jade::Line::text(jade::tr(key, fallback, &[&n])));
        let hex = p2p_colored_hex(info.freq as u16);
        let custom = match nbt.get(&java("name")) {
            Some(Tag::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        };
        let freq = if custom.is_empty() { hex } else { format!("§f{custom} ({hex}§r)") };
        out.push(jade::Line::text(jade::tr("waila.ae2.P2PFrequency", "Frequency: %s", &[&freq])));
        if name == "me_p2p_tunnel" && peers > 0 {
            out.push(jade::Line::text(jade::tr("waila.ae2.P2PMECarriedChannels", "Carried Channels: %d", &[&channels.to_string()])));
        }
    }
}

/// PartHostTooltips.getName / getIcon: a cable bus looks like what the crosshair is on (a part, a facade, the cable).
fn part_host_name(host: &dyn ClientHost, target: &jade::Target) -> Option<(String, String, String)> {
    let jade::Target::Block { pos, block, .. } = target else { return None };
    if !crate::bus::is_cell_block(block) {
        return None;
    }
    let (at, id, facade) = crate::bus::aimed_thing(host)?;
    if at != *pos {
        return None;
    }
    let name_of = |id: &str| {
        let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
        host.translate(&format!("block.{ns}.{path}")).or_else(|| host.translate(&format!("item.{ns}.{path}"))).unwrap_or_else(|| path.replace('_', " "))
    };
    if facade {
        let facade_name = host.translate("item.ae2.facade").unwrap_or_else(|| "Cable Facade".to_owned());
        return Some((id.clone(), format!("{facade_name} - {}", name_of(&id)), "ae2".to_owned()));
    }
    Some((id.clone(), name_of(&id), "ae2".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p2p_link_words() {
        assert_eq!(p2p_state(true, 1).0, "waila.ae2.P2POutput");
        assert_eq!(p2p_state(true, 0).0, "waila.ae2.P2PUnlinked");
        assert_eq!(p2p_state(false, 0).0, "waila.ae2.P2PUnlinked");
        assert_eq!(p2p_state(false, 1).0, "waila.ae2.P2PInputOneOutput");
        assert_eq!(p2p_state(false, 3), ("waila.ae2.P2PInputManyOutputs", "Linked (Input Side) - %d Outputs", Some(3)));
    }

    #[test]
    fn p2p_frequency_digits() {
        let hex = p2p_colored_hex(0x1A2F);
        let digits: String = hex.split("§l").skip(1).map(|p| p.chars().next().unwrap_or(' ')).collect();
        assert_eq!(digits, "1A2F");
        assert!(hex.starts_with("§#FAAE44"));
    }

    #[test]
    fn provider_lock_reasons() {
        assert_eq!(lock_line(1, true, None).map(|l| l.0), Some("waila.ae2.CraftingLockedByRedstoneSignal"));
        assert_eq!(lock_line(1, false, None), None);
        assert_eq!(lock_line(2, false, None).map(|l| l.0), Some("waila.ae2.CraftingLockedByLackOfRedstoneSignal"));
        assert_eq!(lock_line(3, false, Some(1)).map(|l| l.0), Some("waila.ae2.CraftingLockedUntilPulse"));
        assert_eq!(lock_line(3, false, None), None);
        assert_eq!(lock_line(0, true, Some(1)), None);
    }
}
