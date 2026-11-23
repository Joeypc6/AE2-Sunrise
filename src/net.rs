//! AE2's ME network on the server: devices report from their block entity's tick, cables are walked from them into
//! grids (`grid::Network`: channels, controllers, ad hoc limits), and each grid keeps one energy buffer (AE2's
//! EnergyService: stores' capacity, idle and channel draw, power lost and back after 30 ticks).
//!
//! The logic is the upstream port's `rebuild` and power pass (`ae2-horizon/mixins/ae2/me.rs`), on the fork's API:
//! one state per level (`NETS`, keyed by the level's id) instead of the swapped thread locals, block reads through
//! `LevelRef`, block states through `set_block_property_with`, and FE through the game's energy buffer on the
//! acceptor and the controller (`block_entity_energy`), which the fork's `ENERGY` capability answers for every mod.

use crate::config::config;
use crate::grid::{ControllerState, Network, NodeKind};
use crate::util::*;
use mod_api::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

// ---- AE2's blocks by name

/// `ae2:name` → `name`, with AE2's flat parts and lit twins named as their blocks.
pub(crate) fn ae2_name(id: &str) -> Option<&str> {
    id.strip_prefix("ae2:").map(|n| match n {
        "cable_interface" => "interface",
        "cable_pattern_provider" => "pattern_provider",
        "cable_energy_acceptor" => "energy_acceptor",
        "hz_monitor_lit" => "monitor",
        "hz_semi_dark_monitor_lit" => "semi_dark_monitor",
        "hz_dark_monitor_lit" => "dark_monitor",
        n => n,
    })
}

/// Devices: everything that joins a network or holds items (they get a block entity that reports in).
pub(crate) const DEVICES: &[&str] = &[
    "controller", "drive", "chest", "interface", "pattern_provider", "io_port", "molecular_assembler",
    "energy_acceptor", "energy_cell", "dense_energy_cell", "creative_energy_cell", "vibration_chamber",
    "crafting_unit", "crafting_accelerator", "crafting_monitor", "1k_crafting_storage", "4k_crafting_storage",
    "16k_crafting_storage", "64k_crafting_storage", "256k_crafting_storage", "wireless_access_point",
    "quantum_ring", "quantum_link", "spatial_io_port", "spatial_pylon", "spatial_anchor", "cell_workbench",
    "inscriber", "charger", "condenser", "crystal_resonance_generator", "terminal", "crafting_terminal",
    "pattern_encoding_terminal", "pattern_access_terminal", "level_emitter", "import_bus", "export_bus", "storage_bus",
    "growth_accelerator", "me_p2p_tunnel", "item_p2p_tunnel", "fluid_p2p_tunnel", "redstone_p2p_tunnel",
    "light_p2p_tunnel", "fe_p2p_tunnel", "annihilation_plane", "formation_plane", "energy_level_emitter",
    "storage_monitor", "conversion_monitor", "monitor", "semi_dark_monitor", "dark_monitor", "quartz_fiber",
    "toggle_bus", "inverted_toggle_bus",
];

pub(crate) fn is_device(block: &str) -> bool {
    ae2_name(block).is_some_and(|n| DEVICES.contains(&n))
}

/// A cable's colour (`fluix` for AE2's transparent one) and whether it is dense.
pub(crate) fn cable(block: &str) -> Option<(&str, bool)> {
    let name = ae2_name(block)?;
    for suffix in ["_glass_cable", "_covered_cable", "_smart_cable", "_covered_dense_cable", "_smart_dense_cable"] {
        if let Some(color) = name.strip_suffix(suffix) {
            return Some((color, suffix.contains("dense")));
        }
    }
    None
}

pub(crate) fn is_network_block(block: &str) -> bool {
    is_device(block) || cable(block).is_some()
}

/// Parts: they sit on a cable and join it through their back.
pub(crate) fn is_part(block: &str) -> bool {
    block.strip_prefix("ae2:").is_some_and(|n| {
        n.ends_with("_bus")
            || n.ends_with("_p2p_tunnel")
            || n.ends_with("_plane")
            || matches!(
                n,
                "terminal" | "crafting_terminal" | "pattern_encoding_terminal" | "pattern_access_terminal" | "level_emitter"
                    | "energy_level_emitter" | "storage_monitor" | "conversion_monitor" | "cable_interface"
                    | "cable_pattern_provider" | "cable_energy_acceptor" | "monitor" | "semi_dark_monitor" | "dark_monitor"
                    | "hz_monitor_lit" | "hz_semi_dark_monitor_lit" | "hz_dark_monitor_lit" | "quartz_fiber" | "cable_anchor"
            )
    })
}

/// Parts and the crystal resonance generator join on their back only.
fn back_only(block: &str) -> bool {
    is_part(block) || ae2_name(block) == Some("crystal_resonance_generator")
}

/// Whether two neighbouring network blocks join (`AEColor.matches`: same colour, or either fluix).
pub(crate) fn joins(a: &str, b: &str) -> bool {
    match (cable(a), cable(b)) {
        (Some((ca, _)), Some((cb, _))) => ca == cb || ca == "fluix" || cb == "fluix",
        _ => true,
    }
}

pub(crate) fn is_crafting(block: &str) -> bool {
    matches!(ae2_name(block), Some(n) if n.starts_with("crafting_") && n != "crafting_terminal" || n.ends_with("_crafting_storage"))
}

/// The grid node each block is (AE2's IGridNode flags and idle draw, AE a tick).
pub(crate) fn node_kind(block: &str) -> NodeKind {
    if let Some((_, dense)) = cable(block) {
        return if dense { NodeKind::DENSE_CABLE } else { NodeKind::CABLE };
    }
    match ae2_name(block).unwrap_or("") {
        "controller" => NodeKind::CONTROLLER,
        "drive" => NodeKind::device(2.0),
        "chest" => NodeKind::device(1.0),
        "terminal" | "crafting_terminal" | "pattern_encoding_terminal" | "pattern_access_terminal" => NodeKind::device(0.5),
        "level_emitter" | "energy_level_emitter" => NodeKind::passive(1.0),
        "wireless_access_point" | "spatial_anchor" => NodeKind::device(0.0),
        n if is_crafting(&format!("ae2:{n}")) => NodeKind::device(1.0),
        "quantum_ring" | "quantum_link" => NodeKind { max_channels: 32, dense: true, ..NodeKind::passive(22.0) },
        "storage_monitor" | "conversion_monitor" => NodeKind::device(0.5),
        "monitor" | "semi_dark_monitor" | "dark_monitor" => NodeKind::passive(1.0 / 16.0),
        "quartz_fiber" | "toggle_bus" | "inverted_toggle_bus" | "light_detector" => NodeKind::passive(0.0),
        "import_bus" | "export_bus" | "storage_bus" | "interface" | "pattern_provider" | "io_port" => NodeKind::device(1.0),
        "annihilation_plane" | "formation_plane" | "spatial_io_port" => NodeKind::device(1.0),
        "redstone_p2p_tunnel" | "light_p2p_tunnel" => NodeKind::device(0.5),
        "me_p2p_tunnel" => NodeKind::device(2.0),
        "item_p2p_tunnel" | "fluid_p2p_tunnel" | "fe_p2p_tunnel" => NodeKind::device(2.0),
        n if n.ends_with("_p2p_tunnel") => NodeKind::device(1.0),
        "spatial_pylon" => NodeKind::device(0.5),
        "inscriber" => NodeKind::passive(0.0),
        "growth_accelerator" => NodeKind::passive(8.0),
        _ => NodeKind::passive(0.0),
    }
}

/// CreativeEnergyCellBlockEntity.getAEMaxPower.
pub(crate) const CREATIVE_POWER: f64 = 9.22337203685477E14;

/// What a block holds of its grid's power (AEBasePoweredBlockEntity.internalMaxPower of the stores).
pub(crate) fn store_capacity(block: &str) -> f64 {
    match ae2_name(block).unwrap_or("") {
        "controller" => 8_000.0,
        "energy_cell" => 200_000.0,
        "dense_energy_cell" => 1_600_000.0,
        _ => 0.0,
    }
}

/// AEBasePoweredBlockEntity.getEnergyStorage: what takes FE from outside into the network.
pub(crate) fn takes_fe(block: &str) -> bool {
    matches!(ae2_name(block), Some("energy_acceptor" | "controller"))
}

/// The game's FE buffer on those (insert only): `block_entity_energy`. What comes in goes into the grid each tick.
pub(crate) const FE_BUFFER: i32 = 100_000;

/// Blocks with a `powered` look (terminals, parts): lit on a working network.
fn shows_power(block: &str) -> bool {
    (is_part(block) && !block.ends_with("level_emitter"))
        || matches!(
            ae2_name(block),
            Some("drive" | "chest" | "interface" | "pattern_provider" | "io_port" | "molecular_assembler" | "spatial_io_port"
                | "crafting_monitor" | "crafting_unit" | "crafting_accelerator" | "1k_crafting_storage" | "4k_crafting_storage"
                | "16k_crafting_storage" | "64k_crafting_storage" | "256k_crafting_storage" | "spatial_anchor")
        )
}

// ---- State

/// The side of `p` a connection to `q` leaves by: `q` beside it, or the part on that side of `p`'s cell.
fn side_toward(p: Pos, q: Pos) -> Option<usize> {
    if is_hosted(p) {
        return None;
    }
    if is_hosted(q) {
        let (cell, side) = host_cell(q);
        if cell == p {
            return Some(side);
        }
    }
    let r = real(q);
    SIDES.iter().position(|d| add(p, *d) == r)
}

/// Each node's channels per side from its grid's connections (GridConnection.getUsedChannels).
fn side_channels(edges: &HashMap<(u64, u64), u32>, by_id: &HashMap<u64, Pos>) -> HashMap<Pos, [u32; 6]> {
    let mut out: HashMap<Pos, [u32; 6]> = HashMap::new();
    for ((a, b), c) in edges {
        let (Some(pa), Some(pb)) = (by_id.get(a), by_id.get(b)) else { continue };
        for (p, q) in [(*pa, *pb), (*pb, *pa)] {
            if let Some(s) = side_toward(p, q) {
                out.entry(p).or_default()[s] = *c;
            }
        }
    }
    out
}

/// The channels the cable at `pos` carries on each side (all 0 off a grid).
pub(crate) fn side_channels_at(dim: &'static str, pos: Pos) -> [u32; 6] {
    with_net(dim, |n| n.grid_of.get(&pos).and_then(|g| n.grids.get(*g)).and_then(|g| g.side_channels.get(&pos).copied()).unwrap_or_default())
}

/// One grid as the server last worked it out.
#[derive(Default, Clone)]
pub(crate) struct GridInfo {
    pub(crate) nodes: HashSet<Pos>,
    pub(crate) online: bool,
    pub(crate) conflict: bool,
    pub(crate) too_many: bool,
    pub(crate) nested_p2p: bool,
    pub(crate) channel_drain: f64,
    pub(crate) cable_channels: HashMap<Pos, u32>,
    /// The channels each cable carries on each side (`SIDES` order): CablePart.getChannelsOnSide, from the connection
    /// to that side's neighbour or to the part on that side of its cell.
    pub(crate) side_channels: HashMap<Pos, [u32; 6]>,
    pub(crate) active: HashSet<Pos>,
    pub(crate) idle: f64,
    pub(crate) channels: u32,
    /// [blocks] The power group's root, when quartz fibers join this grid's power with others' (`links`).
    pub(crate) power_group: Option<Pos>,
    /// fixA: where this grid goes on in other levels through quantum bridges (`xlink`): a node there, by level.
    pub(crate) xlinks: Vec<(&'static str, Pos)>,
}

impl GridInfo {
    pub(crate) fn root(&self) -> Option<Pos> {
        self.nodes.iter().min().copied()
    }

    /// [blocks] Where this grid's energy is kept: its power group's root (quartz fibers), else its own.
    pub(crate) fn power_root(&self) -> Option<Pos> {
        self.power_group.or_else(|| self.root())
    }
}

/// [blocks] The grids sharing `g`'s power (itself included).
fn group<'a>(grids: &'a [GridInfo], g: &GridInfo) -> Vec<&'a GridInfo> {
    let root = g.power_root();
    grids.iter().filter(|o| o.power_root() == root).collect()
}

/// [blocks] A power group's capacity and whether a creative energy cell is on it.
fn group_capacity(grids: &[GridInfo], g: &GridInfo, devices: &HashMap<Pos, (String, u64)>) -> (f64, bool) {
    let members = group(grids, g);
    let cap = members.iter().map(|m| grid_capacity(m, devices)).sum();
    let creative = members.iter().any(|m| m.nodes.iter().any(|p| devices.get(p).is_some_and(|(id, _)| id == "ae2:creative_energy_cell")));
    (cap, creative)
}

/// One level's networks.
#[derive(Default)]
pub(crate) struct LevelNet {
    pub(crate) tick: u64,
    /// Device block entities seen ticking: block and tick.
    pub(crate) devices: HashMap<Pos, (String, u64)>,
    pub(crate) grids: Vec<GridInfo>,
    pub(crate) grid_of: HashMap<Pos, usize>,
    /// AE each grid holds, by its root (lowest position).
    pub(crate) energy: HashMap<Pos, f64>,
    /// What each store held at the last rebuild (a split, a lost block or a reload goes by it).
    node_power: HashMap<Pos, f64>,
    store_caps: HashMap<Pos, f64>,
    /// Each store's share, as last written on its block entity, and shares waiting for its tick to write them.
    power_saved: HashMap<Pos, f64>,
    pub(crate) power_writes: HashMap<Pos, f64>,
    /// Stores' saved power read from their block entities, waiting for the next rebuild.
    power_read: HashMap<Pos, f64>,
    /// Grids (by root) that lost power, and when: back after 30 ticks with power.
    power_lost: HashMap<Pos, u64>,
    power_look: u64,
    power_pools: Vec<(Pos, f64, bool)>,
    /// fixC: each power group's parts in other levels (quantum bridges), by root, as the last rebuild found them: the
    /// boot check between rebuilds (`power_due`) counts their energy and draw too.
    power_remote: HashMap<Pos, Vec<(&'static str, Pos)>>,
    /// EnergyService's averages by root: this second's injection and drain, then the averages a tick.
    pub(crate) energy_flow: HashMap<Pos, [f64; 4]>,
    /// Lit state last set.
    pub(crate) powered: HashMap<Pos, bool>,
    /// Block states to set at the end of the tick (a block entity is out of the world during its own tick).
    pub(crate) prop_sets: Vec<(Pos, &'static str, String)>,
    /// A block joined or left since the last rebuild.
    dirty: bool,
    /// P2P tunnels as their block entities last reported (stage 4): frequency, output, kind, tick seen.
    pub(crate) p2p: HashMap<Pos, P2pInfo>,
}

/// A P2P tunnel's settings (P2PTunnelPart: frequency, output), its kind (`ae2:item_p2p_tunnel`) and when it last ticked.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct P2pInfo {
    pub(crate) freq: i64,
    pub(crate) output: bool,
    pub(crate) kind: String,
    pub(crate) seen: u64,
}

/// A tunnel's tick: its settings into the level's table (an ME tunnel's change makes the grids again).
pub(crate) fn report_p2p(dim: &'static str, pos: Pos, kind: &str, freq: i64, output: bool) {
    let changed = with_net(dim, |n| {
        let seen = n.tick;
        let was = n.p2p.insert(pos, P2pInfo { freq, output, kind: kind.to_owned(), seen });
        was.is_none_or(|w| w.freq != freq || w.output != output || w.kind != kind)
    });
    if changed && kind == "ae2:me_p2p_tunnel" {
        mark_dirty(dim);
    }
}

/// The other ends of the tunnel at `pos` (an input's outputs, an output's input) of its kind, on its grid, online and
/// with their channels (P2PTunnelPart.getOutputs / getInput).
pub(crate) fn p2p_peers(dim: &'static str, pos: Pos) -> Vec<Pos> {
    with_net(dim, |n| {
        let Some(me) = n.p2p.get(&pos).cloned() else { return Vec::new() };
        if me.freq == 0 {
            return Vec::new();
        }
        let Some(g) = n.grid_of.get(&pos).and_then(|g| n.grids.get(*g)) else { return Vec::new() };
        if !g.online || !g.active.contains(&pos) {
            return Vec::new();
        }
        let tick = n.tick;
        let mut out: Vec<Pos> = n
            .p2p
            .iter()
            .filter(|(q, i)| {
                **q != pos && i.freq == me.freq && i.kind == me.kind && i.output != me.output && tick.saturating_sub(i.seen) <= 40 && g.active.contains(*q)
            })
            .map(|(q, _)| *q)
            .collect();
        out.sort();
        out
    })
}

// polish: GridNodeState's NETWORK_BOOTING (Jade): the block's grid has power back but is still in its boot after a
// loss (`power_lost`, 32 ticks).
pub(crate) fn booting(dim: &'static str, pos: Pos) -> bool {
    with_net(dim, |n| {
        let Some(root) = n.grid_of.get(&pos).and_then(|g| n.grids.get(*g)).and_then(GridInfo::root) else { return false };
        n.power_lost.get(&root).is_some_and(|t| n.tick.saturating_sub(*t) < 32) && n.energy.get(&root).copied().unwrap_or(0.0) > 0.0
    })
}

pub(crate) fn p2p_info(dim: &'static str, pos: Pos) -> Option<P2pInfo> {
    with_net(dim, |n| n.p2p.get(&pos).cloned())
}

/// Every level's networks, by the level's id.
static NETS: Mutex<Option<HashMap<&'static str, LevelNet>>> = Mutex::new(None);
/// Levels whose blocks changed (set from hooks that may run while a level's state is out for its rebuild).
static DIRTY: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// Runs `f` on the level's networks.
pub(crate) fn with_net<R>(dim: &'static str, f: impl FnOnce(&mut LevelNet) -> R) -> R {
    let mut g = NETS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new).entry(dim).or_default())
}

pub(crate) fn mark_dirty(dim: &'static str) {
    DIRTY.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(dim);
}

// polish: every level's networks worked out again (`/ae2 channelmode`, a reloaded config); how many grids there were.
pub(crate) fn mark_all_dirty() -> usize {
    let (dims, grids): (Vec<&'static str>, usize) = {
        let g = NETS.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().map_or((Vec::new(), 0), |m| (m.keys().copied().collect(), m.values().map(|n| n.grids.len()).sum()))
    };
    for d in dims {
        mark_dirty(d);
    }
    grids
}

pub(crate) fn forget_all() {
    crate::xlink::forget_all(); // fixA
    *NETS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *DIRTY.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The grid a block is on, and whether it is online and the block has its channel.
pub(crate) fn device_state(dim: &'static str, pos: Pos) -> (Option<usize>, bool, bool) {
    with_net(dim, |n| {
        let g = n.grid_of.get(&pos).copied();
        let online = g.and_then(|g| n.grids.get(g)).is_some_and(|g| g.online);
        let active = g.and_then(|g| n.grids.get(g)).is_some_and(|g| g.online && g.active.contains(&pos));
        (g, online, active)
    })
}

/// The devices on the grid `pos` is on (position and block), if it is online (or any grid, `online` false).
pub(crate) fn grid_devices(dim: &'static str, pos: Pos, online: bool) -> Option<Vec<(Pos, String)>> {
    with_net(dim, |n| {
        let g = n.grids.get(*n.grid_of.get(&pos)?)?;
        if online && !g.online {
            return None;
        }
        Some(g.nodes.iter().filter_map(|p| n.devices.get(p).map(|(b, _)| (*p, b.clone()))).collect())
    })
}

/// Grid capacity: its stores plus GridEnergyStoragePerNode for each node.
fn grid_capacity(grid: &GridInfo, devices: &HashMap<Pos, (String, u64)>) -> f64 {
    grid.nodes
        .iter()
        .filter_map(|p| devices.get(p))
        .map(|(id, _)| if ae2_name(id) == Some("creative_energy_cell") { CREATIVE_POWER } else { store_capacity(id) })
        .sum::<f64>()
        + config().grid_energy_storage_per_node * grid.nodes.len() as f64
}

/// The stored AE and capacity of the grid at `pos`.
pub(crate) fn grid_power(dim: &'static str, pos: Pos) -> Option<(f64, f64)> {
    let (e, cap, links) = with_net(dim, |n| {
        let g = n.grids.get(*n.grid_of.get(&pos)?)?;
        let root = g.power_root()?;
        let links: Vec<(&'static str, Pos)> = group(&n.grids, g).iter().flat_map(|m| m.xlinks.iter().copied()).collect();
        Some((n.energy.get(&root).copied().unwrap_or(0.0), group_capacity(&n.grids, g, &n.devices).0, links))
    })?;
    // fixA: with the grid's parts in other levels.
    if links.is_empty() {
        return Some((e, cap));
    }
    let (re, rcap, _, _) = remote_power(&remote_roots(&links));
    Some((e + re, cap + rcap))
}

/// AE a generator or acceptor at `pos` adds to its grid, as far as there is room: how much went in.
pub(crate) fn inject_power(dim: &'static str, pos: Pos, ae: f64) -> f64 {
    let (went, links) = with_net(dim, |n| {
        let Some(g) = n.grid_of.get(&pos).and_then(|g| n.grids.get(*g)) else { return (0.0, Vec::new()) };
        let cap = group_capacity(&n.grids, g, &n.devices).0;
        let links: Vec<(&'static str, Pos)> = group(&n.grids, g).iter().flat_map(|m| m.xlinks.iter().copied()).collect();
        let Some(root) = g.power_root() else { return (0.0, Vec::new()) };
        let e = n.energy.entry(root).or_insert(0.0);
        let went = ae.min((cap - *e).max(0.0));
        *e += went;
        n.energy_flow.entry(root).or_default()[0] += went;
        (went, links)
    });
    // fixA: what this level's part has no room for goes to the grid's parts in other levels.
    if went < ae && !links.is_empty() {
        return went + give_remote(&remote_roots(&links), ae - went);
    }
    went
}

/// EnergyService.extractAEPower: up to `ae` from the grid at `pos` (on an online grid).
pub(crate) fn extract_power(dim: &'static str, pos: Pos, ae: f64) -> f64 {
    let (came, links) = with_net(dim, |n| {
        let Some(g) = n.grid_of.get(&pos).and_then(|g| n.grids.get(*g)) else { return (0.0, Vec::new()) };
        if !g.online {
            return (0.0, Vec::new());
        }
        let Some(root) = g.power_root() else { return (0.0, Vec::new()) };
        let creative = group_capacity(&n.grids, g, &n.devices).1;
        if creative {
            return (ae, Vec::new());
        }
        let links: Vec<(&'static str, Pos)> = group(&n.grids, g).iter().flat_map(|m| m.xlinks.iter().copied()).collect();
        let e = n.energy.entry(root).or_insert(0.0);
        let came = ae.min(*e);
        *e -= came;
        n.energy_flow.entry(root).or_default()[1] += came;
        (came, links)
    });
    // fixA: the rest from the grid's parts in other levels (a creative energy cell there gives it all).
    if came < ae && !links.is_empty() {
        let roots = remote_roots(&links);
        if remote_power(&roots).3 {
            return ae;
        }
        return came + take_remote(&roots, ae - came);
    }
    came
}

// ---- fixA: a grid's parts in other levels (quantum bridges across levels, `xlink`)

/// The linked levels' parts of a grid, by (level, a node there): each part's power root, once.
pub(crate) fn remote_roots(links: &[(&'static str, Pos)]) -> Vec<(&'static str, Pos)> {
    let mut out: Vec<(&'static str, Pos)> = Vec::new();
    for (d, p) in links {
        if let Some(root) = with_net(d, |n| n.grid_of.get(p).and_then(|g| n.grids.get(*g)).and_then(GridInfo::power_root))
            && !out.contains(&(*d, root))
        {
            out.push((*d, root));
        }
    }
    out
}

/// The linked parts' stored AE, capacity, draw a tick, and whether a creative energy cell is on one.
pub(crate) fn remote_power(roots: &[(&'static str, Pos)]) -> (f64, f64, f64, bool) {
    let mut out = (0.0, 0.0, 0.0, false);
    for (d, root) in roots {
        with_net(d, |n| {
            out.0 += n.energy.get(root).copied().unwrap_or(0.0);
            if let Some(g) = n.grids.iter().find(|g| g.power_root() == Some(*root)) {
                let (cap, creative) = group_capacity(&n.grids, g, &n.devices);
                out.1 += cap;
                out.3 |= creative;
            }
            if let Some((_, draw, creative)) = n.power_pools.iter().find(|p| p.0 == *root) {
                out.2 += draw;
                out.3 |= *creative;
            }
        });
    }
    out
}

/// fixC: whether a power group has a tick's draw (`draw`) stored, its parts in other levels (`remote_power`: their
/// stored AE, capacity, draw a tick, creative) counted with its own (`had`): one grid's energy over all its levels.
/// None stored is off, even drawing none; a creative energy cell on any part powers all.
pub(crate) fn has_power_with(had: f64, draw: f64, remote: (f64, f64, f64, bool)) -> bool {
    remote.3 || (had + remote.0 > 0.0 && had + remote.0 >= draw + remote.2)
}

/// fixC (from fixA's rebuild): `cost` paid out of a group's own buffer (`had`) first; what it can't pay comes out of
/// its parts in other levels (a creative one there pays it all). What the group's own buffer has left.
pub(crate) fn pay_draw(had: f64, cost: f64, creative: bool, remote: &[(&'static str, Pos)], remote_creative: bool) -> f64 {
    let left = had - cost;
    if left < 0.0 && !creative && !remote.is_empty() {
        if !remote_creative {
            take_remote(remote, -left);
        }
        return 0.0;
    }
    left
}

/// Up to `ae` out of the linked parts' buffers: how much came.
fn take_remote(roots: &[(&'static str, Pos)], ae: f64) -> f64 {
    let mut got = 0.0;
    for (d, root) in roots {
        if got >= ae {
            break;
        }
        got += with_net(d, |n| {
            let e = n.energy.entry(*root).or_insert(0.0);
            let came = (ae - got).min(*e).max(0.0);
            *e -= came;
            n.energy_flow.entry(*root).or_default()[1] += came;
            came
        });
    }
    got
}

/// Up to `ae` into the linked parts' buffers, as far as they have room: how much went.
fn give_remote(roots: &[(&'static str, Pos)], ae: f64) -> f64 {
    let mut went = 0.0;
    for (d, root) in roots {
        if went >= ae {
            break;
        }
        went += with_net(d, |n| {
            let Some(g) = n.grids.iter().find(|g| g.power_root() == Some(*root)) else { return 0.0 };
            let cap = group_capacity(&n.grids, g, &n.devices).0;
            let e = n.energy.entry(*root).or_insert(0.0);
            let put = (ae - went).min((cap - *e).max(0.0));
            *e += put;
            n.energy_flow.entry(*root).or_default()[0] += put;
            put
        });
    }
    went
}

/// The grid at `pos` and its parts in other levels (quantum bridges), by (level, a node there); itself first.
pub(crate) fn grid_parts(dim: &'static str, pos: Pos) -> Vec<(&'static str, Pos)> {
    let links = with_net(dim, |n| n.grid_of.get(&pos).and_then(|g| n.grids.get(*g)).map(|g| g.xlinks.clone()));
    let Some(links) = links else { return Vec::new() };
    let mut out = vec![(dim, pos)];
    for (d, p) in links {
        if with_net(d, |n| n.grid_of.contains_key(&p)) && !out.iter().any(|o| o.0 == d) {
            out.push((d, p));
        }
    }
    out
}

/// Every device on the grid at `pos` in every level it reaches, with its level (`grid_devices` over `grid_parts`).
pub(crate) fn grid_devices_all(dim: &'static str, pos: Pos, online: bool) -> Option<Vec<(&'static str, Pos, String)>> {
    let mut out: Vec<(&'static str, Pos, String)> = grid_devices(dim, pos, online)?.into_iter().map(|(q, b)| (dim, q, b)).collect();
    for (d, p) in grid_parts(dim, pos).into_iter().skip(1) {
        if let Some(list) = grid_devices(d, p, online) {
            out.extend(list.into_iter().map(|(q, b)| (d, q, b)));
        }
    }
    Some(out)
}

/// One name for the grid at `pos` whichever level it is asked from: its lowest (level, root) over its parts.
pub(crate) fn grid_key(dim: &'static str, pos: Pos) -> Option<(&'static str, Pos)> {
    grid_parts(dim, pos)
        .into_iter()
        .filter_map(|(d, p)| with_net(d, |n| n.grid_of.get(&p).and_then(|g| n.grids.get(*g)).and_then(GridInfo::root)).map(|r| (d, r)))
        .min()
}

// ---- Device ticks

/// A device's block entity ticked: it reports in, its stored power is read once and written when it changes, and
/// FE that came into its buffer goes into the grid.
pub(crate) fn device_tick(host: &mut dyn ServerHost, pos: Pos, block: &str, fields: &mut Fields) {
    let dim = host.dimension();
    let cap = store_capacity(block);
    let (grid, newly) = with_net(dim, |n| {
        let newly = n.devices.insert(pos, (block.to_owned(), n.tick)).is_none_or(|(b, _)| b != block);
        if cap > 0.0 {
            if !n.node_power.contains_key(&pos) && !n.power_read.contains_key(&pos) {
                n.power_read.insert(pos, internal_power(&fields.nbt));
            }
            if let Some(share) = n.power_writes.remove(&pos) {
                set_internal_power(&mut fields.nbt, share);
            }
        }
        (n.grid_of.get(&pos).copied(), newly)
    });
    if newly {
        mark_dirty(dim);
    }
    if takes_fe(block) {
        fe_into_grid(dim, pos, grid, fields);
    }
}

pub(crate) fn internal_power(nbt: &Compound) -> f64 {
    match nbt.get(&java("internalCurrentPower")) {
        Some(Tag::Double(v)) => f64::from_bits(*v),
        Some(Tag::Float(v)) => f64::from(f32::from_bits(*v)),
        _ => 0.0,
    }
}

pub(crate) fn set_internal_power(nbt: &mut Compound, power: f64) {
    nbt.insert(java("internalCurrentPower"), Tag::Double(power.to_bits()));
}

/// 1 FE (Tech Reborn's E on Fabric) is this many AE (AEConfig `PowerRatios.TechReborn`).
pub(crate) fn ae_per_fe() -> f64 {
    config().power_ratio_tech_reborn
}

/// `funnelPowerIntoStorage`: the FE buffer into the network, as far as it has room.
fn fe_into_grid(dim: &'static str, pos: Pos, grid: Option<usize>, fields: &mut Fields) {
    let held = match fields.nbt.get(&java("energy")) {
        Some(Tag::Int(n)) if *n > 0 => *n,
        _ => return,
    };
    if grid.is_none() {
        return;
    }
    let room_fe = grid_power(dim, pos).map_or(0.0, |(e, cap)| ((cap - e).max(0.0) / ae_per_fe()).floor());
    let fe = f64::from(held).min(room_fe);
    if fe <= 0.0 {
        return;
    }
    let went = inject_power(dim, pos, fe * ae_per_fe());
    let moved = (went / ae_per_fe()).floor() as i32;
    if moved > 0 {
        fields.nbt.insert(java("energy"), Tag::Int(held - moved));
    }
}

// ---- The level's tick

/// The end of a level's tick: blocks wanted by device ticks, then, each second or when something joined or left, the
/// grids worked out again and power paid.
pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    crate::xlink::level_ticked(dim); // fixA
    let mut net = {
        let mut g = NETS.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(HashMap::new).remove(dim).unwrap_or_default()
    };
    net.tick += 1;
    if DIRTY.lock().unwrap_or_else(|e| e.into_inner()).as_mut().is_some_and(|d| d.remove(dim)) {
        net.dirty = true;
    }
    for (pos, key, value) in std::mem::take(&mut net.prop_sets) {
        if level.block_property(host, pos, key).is_some_and(|v| v != value) {
            host.set_block_property_with(pos, key, &value, 3);
        }
    }
    let tick = net.tick;
    if tick % 20 == 0 || net.dirty || power_due(&mut net, tick) {
        net.dirty = false;
        // Put back first: the walk's block writes can run hooks that read this level's state.
        let changes = rebuild(host, level, &mut net, tick);
        NETS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(dim, net);
        apply_looks(host, level, dim, changes);
        return;
    }
    NETS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(dim, net);
}

/// Whether a grid's power ran out or came back since the last pass (EnergyService judges it every tick).
fn power_due(net: &mut LevelNet, tick: u64) -> bool {
    let spent = tick.saturating_sub(net.power_look) as f64;
    let mut due = false;
    for (root, draw, creative) in net.power_pools.clone() {
        if creative {
            continue;
        }
        let local = net.energy.get(&root).copied().unwrap_or(0.0) - draw * spent;
        // fixC: a grid over several levels has its linked parts' energy too. Its own buffer may stay empty while they
        // pay its draw; judged on that alone it lost power every tick, and its 32-tick boot never ended.
        let remote = net.power_remote.get(&root).map(|r| remote_power(r)).unwrap_or_default();
        if remote.3 {
            if net.power_lost.get(&root).is_some_and(|lost| tick.saturating_sub(*lost) >= 32) {
                due = true;
            }
            continue;
        }
        let left = local + remote.0 - remote.2 * spent;
        let powered = left > 0.0 && left >= draw + remote.2;
        match (powered, net.power_lost.get(&root).copied()) {
            (false, None) => due = true,
            (false, Some(_)) => {
                net.power_lost.insert(root, tick);
            }
            (true, Some(lost)) => due |= tick.saturating_sub(lost) >= 32,
            (true, None) => {}
        }
    }
    due
}

/// The block at `pos` as the network sees it: a hosted part is its part, a cable's cell is its cable.
pub(crate) fn block_id(host: &dyn Host, level: LevelRef, pos: Pos) -> String {
    if is_hosted(pos) {
        return crate::bus::part_at(host, level, pos).unwrap_or_default();
    }
    let id = level.block(host, pos).and_then(|b| b.id(host).map(str::to_owned)).unwrap_or_default();
    if crate::bus::is_cell_block(&id) {
        return crate::bus::cable_at(host, level, pos).unwrap_or(id);
    }
    id
}

fn part_back(host: &dyn Host, level: LevelRef, p: Pos) -> Pos {
    let f = facing_dir(level.block_property(host, p, "facing").as_deref());
    [-f[0], -f[1], -f[2]]
}

/// Sides a block never joins on (the drive's front, the access point's all but its back...).
fn side_closed(host: &dyn Host, level: LevelRef, p: Pos, block: &str, d: Pos) -> bool {
    // [blocks] A quantum bridge's sides by its formed role (`qnb::side_closed`).
    if crate::qnb::is_qnb(block) {
        let role = level.block_property(host, p, "qnb");
        return crate::qnb::side_closed(role.as_deref(), &block_id(host, level, add(p, d)));
    }
    let name = ae2_name(block).unwrap_or("");
    if matches!(name, "condenser" | "cell_workbench") {
        return true;
    }
    if !matches!(name, "drive" | "charger" | "inscriber" | "growth_accelerator" | "wireless_access_point") {
        return false;
    }
    let Some(facing) = level.block_property(host, p, "facing") else { return false };
    let front = facing_dir(Some(facing.as_str()));
    let back = [-front[0], -front[1], -front[2]];
    match name {
        "growth_accelerator" => d != front && d != back,
        "wireless_access_point" => d != back,
        _ => d == front,
    }
}

/// Whether neighbours `a` at `p` and `b` at `q` join.
pub(crate) fn links(host: &dyn Host, level: LevelRef, p: Pos, a: &str, q: Pos, b: &str) -> bool {
    // A hosted part joins its own cell's cable on its back, whatever the colour (CableBusContainer); a cell's side
    // with a part or a facade on it joins nothing beside.
    if is_hosted(p) || is_hosted(q) {
        return real(p) == real(q);
    }
    if crate::bus::blocked(host, level, p, q) || crate::bus::blocked(host, level, q, p) {
        return false;
    }
    if !joins(a, b) {
        return false;
    }
    let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
    let back = [-d[0], -d[1], -d[2]];
    if side_closed(host, level, p, a, d) || side_closed(host, level, q, b, back) {
        return false;
    }
    if (is_part(a) && cable(b).is_none()) || (is_part(b) && cable(a).is_none()) {
        return false;
    }
    (!back_only(a) || part_back(host, level, p) == d) && (!back_only(b) || part_back(host, level, q) == back)
}

/// [blocks] Whether network block `b` at `q` joins on its side toward `q + d` (its open sides, a part's back).
pub(crate) fn open_toward(host: &dyn Host, level: LevelRef, q: Pos, b: &str, d: Pos) -> bool {
    !side_closed(host, level, q, b, d) && (!back_only(b) || part_back(host, level, q) == d)
}

/// The crafting blocks forming a cuboid with a storage: each formed block and the sides its cluster goes on.
pub(crate) fn crafting_clusters(blocks: &HashMap<Pos, String>) -> (HashMap<Pos, [bool; 6]>, Vec<Vec<Pos>>) {
    let mut seen = HashSet::new();
    let mut out = HashMap::new();
    let mut clusters = Vec::new();
    for (start, name) in blocks {
        if !is_crafting(name) || !seen.insert(*start) {
            continue;
        }
        let mut cluster = vec![*start];
        let mut i = 0;
        while i < cluster.len() {
            for d in SIDES {
                let q = add(cluster[i], d);
                if blocks.get(&q).is_some_and(|b| is_crafting(b)) && seen.insert(q) {
                    cluster.push(q);
                }
            }
            i += 1;
        }
        let (lo, hi) = cluster.iter().fold(([i32::MAX; 3], [i32::MIN; 3]), |(lo, hi), p| {
            (std::array::from_fn(|k| lo[k].min(p[k])), std::array::from_fn(|k| hi[k].max(p[k])))
        });
        let volume: i64 = (0..3).map(|k| (hi[k] - lo[k] + 1) as i64).product();
        let storage = cluster.iter().any(|p| blocks[p].ends_with("_crafting_storage"));
        if volume != cluster.len() as i64 || !storage {
            continue;
        }
        let members: HashSet<Pos> = cluster.iter().copied().collect();
        for p in &cluster {
            out.insert(*p, SIDES.map(|d| members.contains(&add(*p, d))));
        }
        clusters.push(cluster);
    }
    (out, clusters)
}

/// What the looks pass sets after a rebuild.
pub(crate) struct Looks {
    blocks: HashMap<Pos, String>,
    lit: Vec<(Pos, bool, bool)>,
}

/// The grids worked out again from every device seen in the last two seconds, and a second's power paid.
fn rebuild(host: &mut dyn ServerHost, level: LevelRef, net: &mut LevelNet, tick: u64) -> Looks {
    net.devices.retain(|_, (_, seen)| tick.saturating_sub(*seen) <= 40);
    let starts: Vec<Pos> = net.devices.keys().copied().collect();
    let mut blocks: HashMap<Pos, String> = HashMap::new();
    let mut graph = Network::new();
    let mut me_fronts: Vec<(Pos, Pos)> = Vec::new();
    // [blocks] Toggle buses (on) and quartz fibers: each one's node, what its outer node meets, and whether a fiber.
    let mut outer_links: Vec<(Pos, Pos, bool)> = Vec::new();
    let dim = host.dimension();
    for start in starts {
        if blocks.contains_key(&start) {
            continue;
        }
        let name = block_id(host, level, start);
        if !is_device(&name) {
            continue;
        }
        let mut queue = VecDeque::from([(start, name)]);
        while let Some((p, name)) = queue.pop_front() {
            if blocks.contains_key(&p) {
                continue;
            }
            graph.add_node(pack(p), node_kind(&name), p);
            blocks.insert(p, name.clone());
            // An ME tunnel's front is a network of its own (MEP2PTunnelPart's outer node): walked too.
            if name == "ae2:me_p2p_tunnel" {
                let f = facing_dir(crate::bus::prop(host, level, p, "facing").as_deref());
                let q = add(p, f);
                let other = match blocks.get(&q) {
                    Some(n) => n.clone(),
                    None => block_id(host, level, q),
                };
                if is_network_block(&other) && (!back_only(&other) || part_back(host, level, q) == [-f[0], -f[1], -f[2]]) && !side_closed(host, level, q, &other, [-f[0], -f[1], -f[2]]) {
                    me_fronts.push((p, q));
                    if !blocks.contains_key(&q) {
                        queue.push_back((q, other));
                    }
                }
            }
            if is_hosted(p) && crate::links::has_outer(&name) {
                let h: &dyn Host = &*host;
                let t = crate::links::outer_target(h, level, p, &mut |q| blocks.get(&q).cloned().unwrap_or_else(|| block_id(h, level, q)));
                let fiber = crate::links::is_fiber(&name);
                if let Some(t) = t.filter(|_| fiber || crate::links::toggle_enabled(dim, p)) {
                    outer_links.push((p, t, fiber));
                    if !blocks.contains_key(&t) {
                        let other = block_id(h, level, t);
                        if is_network_block(&other) {
                            queue.push_back((t, other));
                        }
                    }
                }
            }
            for q in crate::bus::edges(host, level, p) {
                let other = match blocks.get(&q) {
                    Some(n) => n.clone(),
                    None => block_id(host, level, q),
                };
                if is_network_block(&other) && links(host, level, p, &name, q, &other) {
                    if blocks.contains_key(&q) {
                        graph.connect(pack(p), pack(q));
                    } else {
                        queue.push_back((q, other));
                    }
                }
            }
        }
    }
    let positions: Vec<Pos> = blocks.keys().copied().collect();
    for p in &positions {
        for q in crate::bus::edges(host, level, *p) {
            if let Some(other) = blocks.get(&q)
                && links(host, level, *p, &blocks[p], q, other)
            {
                graph.connect(pack(*p), pack(q));
            }
        }
    }
    // ME tunnels: each one's outer node joins what is in front of it, and an input's with its outputs' (by frequency).
    for (p, q) in &me_fronts {
        if blocks.contains_key(q) {
            let outer = graph.tunnel_outer(pack(*p));
            graph.connect(outer, pack(*q));
        }
    }
    {
        net.p2p.retain(|_, i| tick.saturating_sub(i.seen) <= 40);
        let me: Vec<(Pos, P2pInfo)> = net.p2p.iter().filter(|(p, i)| i.kind == "ae2:me_p2p_tunnel" && i.freq != 0 && blocks.get(*p).is_some_and(|b| b == &i.kind)).map(|(p, i)| (*p, i.clone())).collect();
        for (i, a) in me.iter().filter(|(_, a)| !a.output) {
            for (o, _) in me.iter().filter(|(_, b)| b.output && b.freq == a.freq) {
                graph.connect_tunnel(pack(*i), pack(*o));
            }
        }
    }
    // [blocks] Quantum bridges: formed chambers whose singularities share a frequency join (`qnb::links`).
    // fixA: a chamber left without a pair here may pair with one in another level (`xlink`).
    let qnb_ends = crate::qnb::formed_ends(&*host, level, &blocks);
    let qnb_pairs = crate::qnb::pairs(qnb_ends.clone());
    for (a, b) in &qnb_pairs {
        graph.connect(pack(*a), pack(*b));
    }
    let qnb_unpaired: Vec<Pos> = qnb_ends.iter().map(|e| e.0).filter(|p| !qnb_pairs.iter().any(|(a, b)| a == p || b == p)).collect();
    // [blocks] A toggle bus that is on joins its cable and what is in front of it as one grid.
    for (p, t, fiber) in &outer_links {
        if !fiber && blocks.contains_key(t) {
            graph.connect(pack(*p), pack(*t));
        }
    }
    let by_id: HashMap<u64, Pos> = positions.iter().map(|p| (pack(*p), *p)).collect();
    // A CPU's blocks, and a spatial structure's pylons: one channel between them.
    {
        let wanted = |name: &str| is_crafting(name) || ae2_name(name) == Some("spatial_pylon");
        let mut seen: HashSet<Pos> = HashSet::new();
        for (start, name) in &blocks {
            if !wanted(name) || !seen.insert(*start) {
                continue;
            }
            let pylon = ae2_name(name) == Some("spatial_pylon");
            let mut members = vec![*start];
            let mut queue = vec![*start];
            while let Some(p) = queue.pop() {
                for d in SIDES {
                    let q = add(p, d);
                    if blocks.get(&q).is_some_and(|n| wanted(n) && (ae2_name(n) == Some("spatial_pylon")) == pylon) && seen.insert(q) {
                        members.push(q);
                        queue.push(q);
                    }
                }
            }
            let group = members.iter().copied().map(pack).min().unwrap_or(0);
            for p in members {
                graph.set_multiblock(pack(p), group);
            }
        }
    }
    let mut grids = Vec::new();
    let mut grid_of = HashMap::new();
    // fixA: with a bridge to another level, the grids are worked out on the joint graph (`xlink::join`).
    let (joint, relink, graph_changed) = crate::xlink::with_links(|l| {
        let relink = l.report(dim, &qnb_ends);
        // Only a level with formed chambers keeps its graph for the others.
        let changed = if qnb_ends.is_empty() { l.snaps.remove(dim).is_some() } else { l.keep(dim, &graph) };
        (crate::xlink::join(l, dim, &graph, &qnb_unpaired), relink, changed)
    });
    for d in relink {
        mark_dirty(d);
    }
    let (raw_grids, mut x_remote, x_wanted): (Vec<crate::grid::Grid>, Vec<Vec<(&'static str, Pos)>>, Vec<usize>) = match joint {
        Some(j) => (j.grids, j.remote, j.wanted),
        None => {
            let gs = graph.grids();
            let wanted = gs.iter().map(|g| graph.channels_wanted(&g.nodes)).collect();
            let n = gs.len();
            (gs, vec![Vec::new(); n], wanted)
        }
    };
    if graph_changed {
        for d in x_remote.iter().flatten().map(|r| r.0).collect::<HashSet<_>>() {
            mark_dirty(d);
        }
    }
    for (gi, g) in raw_grids.into_iter().enumerate() {
        let nodes: HashSet<Pos> = g.nodes.iter().map(|n| by_id[n]).collect();
        let active: HashSet<Pos> = g.nodes.iter().filter(|n| g.is_active(**n, graph.kinds())).map(|n| by_id[n]).collect();
        // The nodes' draw, and each drive's and chest's cells' (StorageCell.getIdleDrain).
        let idle: f64 = g.nodes.iter().map(|n| graph.kinds()[n].idle_power).sum::<f64>()
            + g.nodes.iter().map(|n| by_id[n]).filter(|p| crate::storage::is_holder(&blocks[p])).map(|p| crate::storage::holder_idle(&*host, level, p, &blocks[&p])).sum::<f64>()
            + g.nodes.iter().map(|n| by_id[n]).filter(|p| blocks[p] == crate::wireless::WAP).map(|p| crate::wireless::wap_idle(&*host, level, p)).sum::<f64>()
            // [blocks] Each spatial anchor's draw for its chunks.
            + g.nodes.iter().map(|n| by_id[n]).filter(|p| blocks[p] == crate::anchor::ANCHOR).map(|p| crate::anchor::idle(dim, p)).sum::<f64>();
        let info = GridInfo {
            online: g.controller_state != ControllerState::ControllerConflict,
            conflict: g.controller_state == ControllerState::ControllerConflict,
            too_many: g.controller_state == ControllerState::NoController
                && !g.nested_p2p
                && x_wanted[gi] > crate::grid::ad_hoc_channels(),
            nested_p2p: g.nested_p2p,
            // PathingService.setChannelPowerUsage: channelsByBlocks / 128 (twice the nodes' with a controller).
            channel_drain: f64::from(g.used_channels.values().sum::<u32>())
                * if g.controller_state == ControllerState::ControllerOnline { 2.0 } else { 1.0 }
                / 128.0,
            cable_channels: g.used_channels.iter().filter_map(|(n, c)| Some((*by_id.get(n)?, *c))).collect(),
            side_channels: side_channels(&g.edge_channels, &by_id),
            nodes,
            active,
            idle,
            channels: g.channels_in_use,
            power_group: None,
            xlinks: std::mem::take(&mut x_remote[gi]),
        };
        for p in &info.nodes {
            grid_of.insert(*p, grids.len());
        }
        grids.push(info);
    }
    // [blocks] Quartz fibers: the grids on either side of each share one power group (`links::power_groups`).
    {
        let pairs: Vec<(usize, usize)> = outer_links.iter().filter(|l| l.2).filter_map(|(p, t, _)| Some((*grid_of.get(p)?, *grid_of.get(t)?))).collect();
        let roots: Vec<Option<Pos>> = grids.iter().map(GridInfo::root).collect();
        for (g, root) in grids.iter_mut().zip(crate::links::power_groups(&roots, &pairs)) {
            g.power_group = root.filter(|r| Some(*r) != g.root());
        }
    }
    // Each old grid's (power group's) power back into its stores (a split or a lost cable loses none of it).
    let mut node_power: HashMap<Pos, f64> = std::mem::take(&mut net.power_read);
    let mut seen_groups: HashSet<Pos> = HashSet::new();
    for g in &net.grids {
        let Some(root) = g.power_root() else { continue };
        if !seen_groups.insert(root) {
            continue;
        }
        let members = group(&net.grids, g);
        let stores: Vec<(Pos, f64)> = members.iter().flat_map(|m| m.nodes.iter()).filter_map(|p| net.store_caps.get(p).map(|c| (*p, *c))).collect();
        let creative = members.iter().any(|m| m.nodes.iter().any(|p| net.devices.get(p).is_some_and(|(id, _)| id == "ae2:creative_energy_cell")));
        if creative {
            node_power.extend(stores.iter().map(|(p, _)| (*p, net.node_power.get(p).copied().unwrap_or(0.0))));
            continue;
        }
        let e = net.energy.get(&root).copied().unwrap_or(0.0);
        let total: f64 = stores.iter().map(|(_, c)| *c).sum();
        let count: usize = members.iter().map(|m| m.nodes.len()).sum();
        if total > 0.0 {
            node_power.extend(stores.iter().map(|(p, c)| (*p, e * c / total)));
        } else if count > 0 {
            let each = e / count as f64;
            node_power.extend(members.iter().flat_map(|m| m.nodes.iter()).map(|p| (*p, each)));
        }
    }
    node_power.retain(|p, _| blocks.contains_key(p));
    net.node_power = node_power;
    net.store_caps = blocks.iter().filter_map(|(p, name)| Some((*p, store_capacity(name))).filter(|(_, c)| *c > 0.0)).collect();
    // A second's idle and channel draw paid from each power group's buffer (a grid alone is a group of one).
    let cfg = config();
    let paid = tick.saturating_sub(net.power_look).clamp(1, 20) as f64 / 20.0;
    net.power_look = tick;
    let mut energy = HashMap::new();
    let mut pools = Vec::new();
    let mut group_powered: HashMap<Pos, bool> = HashMap::new();
    let mut power_remote: HashMap<Pos, Vec<(&'static str, Pos)>> = HashMap::new(); // fixC
    for i in 0..grids.len() {
        let Some(root) = grids[i].power_root() else { continue };
        if group_powered.contains_key(&root) {
            continue;
        }
        let members = group(&grids, &grids[i]);
        let creative = members.iter().any(|m| m.nodes.iter().any(|p| blocks.get(p).is_some_and(|id| id == "ae2:creative_energy_cell")));
        let had: f64 = members.iter().flat_map(|m| m.nodes.iter()).filter_map(|p| net.node_power.get(p)).sum();
        let cap: f64 = members.iter().map(|m| grid_capacity(m, &net.devices)).sum();
        let need = members.iter().map(|m| m.idle).sum::<f64>() * 20.0 * cfg.power_usage_multiplier;
        let channels = members.iter().map(|m| m.channel_drain).sum::<f64>() * 20.0 * cfg.power_usage_multiplier;
        // fixA: a grid over several levels (quantum bridges): the other levels' parts' energy and draw count too.
        let remote = remote_roots(&members.iter().flat_map(|m| m.xlinks.iter().copied()).collect::<Vec<_>>());
        let r = remote_power(&remote);
        let r_creative = r.3;
        // A grid is powered while it has stored energy for a tick's draw: none stored is off, even drawing none.
        let has_power = creative || has_power_with(had, need / 20.0, r); // fixC
        let powered = if has_power {
            if net.power_lost.get(&root).is_some_and(|t| tick.saturating_sub(*t) < 32) {
                false
            } else {
                net.power_lost.remove(&root);
                true
            }
        } else {
            net.power_lost.insert(root, tick);
            false
        };
        group_powered.insert(root, powered);
        let left = pay_draw(had, (need + channels) * paid, creative, &remote, r_creative); // fixC
        energy.insert(root, if creative { cap } else { left.max(0.0).min(cap) });
        pools.push((root, need / 20.0, creative));
        if !remote.is_empty() {
            power_remote.insert(root, remote); // fixC
        }
    }
    for g in grids.iter_mut() {
        if let Some(root) = g.power_root() {
            g.online &= group_powered.get(&root).copied().unwrap_or(false);
        }
    }
    net.power_pools = pools;
    net.power_remote = power_remote; // fixC
    net.energy = energy;
    // EnergyService.tickDrain's averages over 40 ticks, a second's step at a time.
    let mut flow = HashMap::new();
    for g in &grids {
        let Some(root) = g.power_root() else { continue };
        if flow.contains_key(&root) {
            continue;
        }
        let members = group(&grids, g);
        let draw: f64 = members.iter().map(|m| m.idle + m.channel_drain).sum();
        let [input, output, avg_input, avg_output] = net.energy_flow.get(&root).copied().unwrap_or_default();
        let step = 0.3973;
        flow.insert(
            root,
            [0.0, 0.0, avg_input + (input / 20.0 - avg_input) * step, avg_output + (output / 20.0 + draw * cfg.power_usage_multiplier - avg_output) * step],
        );
    }
    net.energy_flow = flow;
    // Each store's share goes on its block entity (its own tick writes it), so a reload or an unloaded chunk finds it.
    let mut written: HashSet<Pos> = HashSet::new();
    for g in &grids {
        let Some(root) = g.power_root() else { continue };
        if !written.insert(root) {
            continue;
        }
        let members = group(&grids, g);
        if members.iter().any(|m| m.nodes.iter().any(|p| blocks.get(p).is_some_and(|id| id == "ae2:creative_energy_cell"))) {
            continue;
        }
        let e = net.energy.get(&root).copied().unwrap_or(0.0);
        let stores: Vec<(Pos, f64)> = members.iter().flat_map(|m| m.nodes.iter()).filter_map(|p| net.store_caps.get(p).map(|c| (*p, *c))).collect();
        let total: f64 = stores.iter().map(|(_, c)| *c).sum();
        for (p, c) in stores {
            let share = if total > 0.0 { e * c / total } else { 0.0 };
            if net.power_saved.get(&p).is_none_or(|was| (was - share).abs() >= 1.0 || (share == 0.0 && *was != 0.0)) {
                net.power_saved.insert(p, share);
                net.power_writes.insert(p, share);
            }
        }
    }
    net.power_saved.retain(|p, _| blocks.contains_key(p));
    let mut lit = Vec::new();
    for (p, name) in &blocks {
        if !shows_power(name) {
            continue;
        }
        let on = grid_of.get(p).is_some_and(|&g| grids[g].online && grids[g].active.contains(p));
        let powered = grid_of.get(p).is_some_and(|&g: &usize| grids[g].online);
        net.powered.insert(*p, on);
        let lit_now = on || (ae2_name(name) == Some("molecular_assembler") && powered);
        lit.push((*p, lit_now, powered));
    }
    net.powered.retain(|p, _| blocks.contains_key(p));
    net.grids = grids;
    net.grid_of = grid_of;
    Looks { blocks, lit }
}

/// After a rebuild: the blocks' looks follow their grids (lit parts, smart cables' channels, controllers, energy
/// cells' charge, formed crafting CPUs, cable arms on parts' backs).
fn apply_looks(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str, looks: Looks) {
    let set = |host: &mut dyn ServerHost, p: Pos, key: &str, value: &str, flags: i32| {
        if level.block_property(host, p, key).is_some_and(|v| v != value) {
            host.set_block_property_with(p, key, value, flags);
        }
    };
    for (p, on, powered) in looks.lit {
        let channel = level.block_property(host, p, "with_channel");
        let want = if on || (channel.is_some() && powered) { "true" } else { "false" };
        set(host, p, "powered", want, 3);
        set(host, p, "with_channel", if on { "true" } else { "false" }, 3);
    }
    // fixB: the smart cables' channels are read below, side by side (cable_lights).
    let (controllers, cells): (Vec<(Pos, &'static str)>, Vec<(Pos, i32)>) = with_net(dim, |n| {
        let mut controllers = Vec::new();
        let mut cells = Vec::new();
        for (p, name) in &looks.blocks {
            let g = n.grid_of.get(p).and_then(|g| n.grids.get(*g));
            match ae2_name(name) {
                Some("controller") => controllers.push((
                    *p,
                    match g {
                        Some(g) if g.conflict => "conflicted",
                        Some(g) if g.online => "online",
                        _ => "offline",
                    },
                )),
                Some("energy_cell" | "dense_energy_cell") => {
                    let ratio = g.and_then(|g| Some(n.energy.get(&g.power_root()?).copied().unwrap_or(0.0) / group_capacity(&n.grids, g, &n.devices).0.max(1.0)));
                    // EnergyCellBlock.getStorageLevelFromFillFactor.
                    cells.push((*p, (4.0 * (ratio.unwrap_or(0.0) + 0.01).clamp(0.0, 1.0)).floor() as i32));
                }
                _ => {}
            }
        }
        (controllers, cells)
    });
    // fixB: smart cables light each arm for the channels on its own side (up to 8, a dense one a line for each 4 of
    // its 32), on their block entity (cable_lights).
    let smart: Vec<(Pos, &String)> = looks.blocks.iter().filter(|(_, name)| crate::cable_lights::smart(name).is_some()).map(|(p, n)| (*p, n)).collect();
    let sides: Vec<[u32; 6]> = with_net(dim, |n| {
        smart.iter().map(|(p, _)| n.grid_of.get(p).and_then(|g| n.grids.get(*g)).and_then(|g| g.side_channels.get(p).copied()).unwrap_or_default()).collect()
    });
    for ((p, name), ch) in smart.into_iter().zip(sides) {
        crate::cable_lights::apply(host, level, dim, p, name, ch);
    }
    // AE2's spatial pylon look (SpatialPylonBlockEntity.recalculateDisplay): a column along one axis (start, middle,
    // end) or alone; lit while its grid is online.
    for (p, name) in &looks.blocks {
        if ae2_name(name) != Some("spatial_pylon") {
            continue;
        }
        let is_p = |d: Pos| looks.blocks.get(&add(*p, d)).is_some_and(|b| ae2_name(b) == Some("spatial_pylon"));
        let axes: Vec<(char, bool, bool)> = [('x', [1, 0, 0]), ('y', [0, 1, 0]), ('z', [0, 0, 1])]
            .iter()
            .map(|(a, d)| (*a, is_p([-d[0], -d[1], -d[2]]), is_p(*d)))
            .filter(|(_, lo, hi)| *lo || *hi)
            .collect();
        let span = match axes.as_slice() {
            [(axis, lo, hi)] => format!("{axis}_{}", if !lo { "start" } else if !hi { "end" } else { "middle" }),
            _ => "none".to_owned(),
        };
        let online = with_net(dim, |n| n.grid_of.get(p).and_then(|g| n.grids.get(*g)).is_some_and(|g| g.online));
        let state = if online && span != "none" { "online" } else { "offline" };
        set(host, *p, "span", &span, 2);
        set(host, *p, "state", state, 2);
    }
    // AE2's controller look (ControllerBlockEntity.updateState): a block, a column, or inside a bigger structure.
    for (p, state) in controllers {
        let is_c = |host: &dyn Host, d: Pos| looks.blocks.get(&add(p, d)).is_some_and(|b| b == "ae2:controller") || block_id(host, level, add(p, d)) == "ae2:controller";
        let xx = is_c(host, [-1, 0, 0]) && is_c(host, [1, 0, 0]);
        let yy = is_c(host, [0, -1, 0]) && is_c(host, [0, 1, 0]);
        let zz = is_c(host, [0, 0, -1]) && is_c(host, [0, 0, 1]);
        let kind = match (u8::from(xx) + u8::from(yy) + u8::from(zz), xx, yy, zz) {
            (n, ..) if n >= 2 => {
                if (p[0] + p[1] + p[2]) & 1 == 0 { "inside_a" } else { "inside_b" }
            }
            (_, true, _, _) => "column_x",
            (_, _, true, _) => "column_y",
            (_, _, _, true) => "column_z",
            _ => "block",
        };
        set(host, p, "type", kind, 2);
        set(host, p, "state", state, 2);
    }
    for (p, stage) in cells {
        set(host, p, "stage", &stage.to_string(), 2);
    }
    // Cable arms as their neighbours are (a part joins only through its back; blocks set by commands skip shapes).
    for (p, name) in &looks.blocks {
        if cable(name).is_none() {
            continue;
        }
        for (d, key) in SIDES.iter().zip(SIDE_NAMES) {
            let q = add(*p, *d);
            let other = looks.blocks.get(&q).cloned().unwrap_or_else(|| block_id(host, level, q));
            let joined = crate::links::faces_outer(host, level, q, *p)
                || is_network_block(&other)
                && (links(host, level, *p, name, q, &other)
                    || (other == "ae2:me_p2p_tunnel" && add(q, facing_dir(level.block_property(host, q, "facing").as_deref())) == *p));
            set(host, *p, key, if joined { "true" } else { "false" }, 2);
        }
    }
    // Crafting blocks show the cluster they form.
    let (formed, _) = crafting_clusters(&looks.blocks);
    for (p, name) in &looks.blocks {
        if !is_crafting(name) {
            continue;
        }
        match formed.get(p) {
            Some(joined) => {
                set(host, *p, "structure", "true", 3);
                for (key, on) in CUBE_SIDES.iter().zip(joined) {
                    set(host, *p, key, if *on { "true" } else { "false" }, 3);
                }
            }
            None => set(host, *p, "structure", "false", 3),
        }
    }
}

/// The state properties naming a formed crafting block's neighbours in its cluster, in `SIDES` order.
const CUBE_SIDES: [&str; 6] = ["side_down", "side_up", "side_north", "side_south", "side_west", "side_east"];

/// A store's share of its grid's power (what it saves as `internalCurrentPower`): PowerStorageDataProvider's line.
pub(crate) fn stored_share(dim: &'static str, pos: Pos) -> Option<f64> {
    with_net(dim, |n| {
        let g = n.grids.get(*n.grid_of.get(&pos)?)?;
        let e = n.energy.get(&g.power_root()?).copied().unwrap_or(0.0);
        let total: f64 = group(&n.grids, g).iter().flat_map(|m| m.nodes.iter()).filter_map(|p| n.devices.get(p)).map(|(b, _)| store_capacity(b)).sum();
        let mine = store_capacity(&n.devices.get(&pos)?.0);
        (total > 0.0).then(|| e * mine / total)
    })
}

// fixC: a grid over two levels (quantum bridges, `xlink`) powered from one side only.
#[cfg(test)]
mod xlink_power_tests {
    use super::*;
    use crate::xlink::{join, tests::level, Links};

    const OVER: &str = "fixc:overworld";
    const NETHER: &str = "fixc:the_nether";

    /// The overworld: a controller and three devices behind a chamber; the nether: twelve devices behind its chamber,
    /// no energy of its own. Both chambers on frequency 91. Each level's grids as `rebuild` keeps them.
    fn setup(over_creative: bool, over_energy: f64) -> (LevelNet, Vec<(&'static str, Pos)>) {
        let mut l = Links::default();
        l.tick(OVER);
        l.tick(NETHER);
        l.report(OVER, &[([0, 64, 0], 91)]);
        l.report(NETHER, &[([0, 64, 0], 91)]);
        let (over, nether) = (level(3, true), level(12, false));
        l.keep(OVER, &over);
        l.keep(NETHER, &nether);
        let infos = |graph: &crate::grid::Network, dim: &'static str| -> Vec<GridInfo> {
            let j = join(&l, dim, graph, &[[0, 64, 0]]).expect("linked");
            j.grids
                .iter()
                .zip(&j.remote)
                .map(|(g, r)| GridInfo {
                    nodes: g.nodes.iter().filter_map(|n| graph.position(*n)).collect(),
                    online: true,
                    xlinks: r.clone(),
                    ..Default::default()
                })
                .collect()
        };
        let over_grids = infos(&over, OVER);
        let nether_grids = infos(&nether, NETHER);
        assert_eq!((over_grids.len(), nether_grids.len()), (1, 1));
        // The overworld's part: its controller is the creative energy cell's stand-in when asked.
        with_net(OVER, |n| {
            *n = LevelNet::default();
            let root = over_grids[0].power_root().expect("root");
            n.grid_of = over_grids[0].nodes.iter().map(|p| (*p, 0)).collect();
            if over_creative {
                n.devices.insert([4, 64, 0], ("ae2:creative_energy_cell".to_owned(), 0));
            }
            n.energy.insert(root, over_energy);
            n.power_pools = vec![(root, 2.0, over_creative)];
            n.grids = over_grids;
        });
        let mut net = LevelNet::default();
        let root = nether_grids[0].power_root().expect("root");
        let remote = remote_roots(&nether_grids[0].xlinks);
        assert_eq!(remote.len(), 1, "the nether's grid goes on in the overworld");
        net.grid_of = nether_grids[0].nodes.iter().map(|p| (*p, 0)).collect();
        net.energy.insert(root, 0.0);
        net.power_pools = vec![(root, 5.0, false)];
        // It booted unpowered (before the link): lost at tick 10.
        net.power_lost.insert(root, 10);
        net.power_remote.insert(root, remote.clone());
        net.grids = nether_grids;
        (net, remote)
    }

    #[test]
    fn unpowered_side_is_powered_over_the_bridge() {
        for creative in [true, false] {
            let (mut nether, remote) = setup(creative, if creative { 0.0 } else { 10_000.0 });
            let root = nether.grids[0].power_root().expect("root");
            // The joint grid has power, though the nether's own buffer is empty.
            assert!(has_power_with(0.0, 5.0, remote_power(&remote)), "creative {creative}");
            assert!(!has_power_with(0.0, 5.0, (0.0, 0.0, 0.0, false)));
            // Between rebuilds the boot runs out instead of starting again each tick, then a rebuild brings it back.
            nether.power_look = 20;
            assert!(!power_due(&mut nether, 20), "creative {creative}");
            assert_eq!(nether.power_lost.get(&root), Some(&10), "creative {creative}: the boot kept its start");
            assert!(power_due(&mut nether, 42), "creative {creative}: booted, a rebuild powers it");
            // The nether's draw comes out of the overworld's buffer (a creative cell pays it all).
            let left = pay_draw(0.0, 100.0, false, &remote, remote_power(&remote).3);
            assert_eq!(left, 0.0);
            let over_root = remote[0].1;
            let over_left = with_net(OVER, |n| n.energy.get(&over_root).copied().unwrap_or(0.0));
            assert_eq!(over_left, if creative { 0.0 } else { 9_900.0 }, "creative {creative}");
        }
        with_net(OVER, |n| *n = LevelNet::default());
    }
}
