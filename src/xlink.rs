//! fixA: quantum network bridges across levels (QuantumCluster / QuantumBridgeBlockEntity: the two chambers of a
//! pair of entangled singularities form one grid connection wherever they are, while both are loaded and formed).
//!
//! Each level still works out its own networks (`net::rebuild`), and joins the other levels' parts in:
//! - every rebuild reports its formed chambers and their frequencies (`report`), and keeps its graph (`Snapshot`);
//! - a chamber whose frequency has exactly one other formed chamber, in another level, seen in the last 60 server
//!   ticks, is linked to it; the rebuild then works the channels and the controllers out on one graph holding its
//!   own nodes and the linked levels' parts of the grid (their last graphs, the linked component only, further
//!   links followed), every node renumbered in one order of (level, position) so each level's rebuild gets the same
//!   answer (`join`); each level keeps the results for its own nodes and, per grid, where the grid goes on in the
//!   other levels (`GridInfo::xlinks`);
//! - power: a grid's energy is the sum of its parts' (each level keeps its own stores' share); draws are paid from the
//!   level's own part first, then from the linked parts (`net`);
//! - storage: a grid's cells and storage buses are those of all its parts (`storage::with_storage`, `parts`);
//! - an unloaded level (it stops reporting) or chunk (its chamber stops ticking), a broken or emptied chamber: the
//!   link drops on the next rebuild of either side (AE2: QuantumCluster.onUnload, updateStatus).

use crate::grid::{Grid, Network, NodeId};
use crate::util::*;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Mutex;

/// Server ticks a report stays good: a level that stopped ticking (unloaded) drops its links after this.
pub(crate) const STALE: u64 = 60;

/// A formed chamber as its level last reported it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct End {
    pub(crate) dim: &'static str,
    pub(crate) pos: Pos,
    pub(crate) freq: i64,
    pub(crate) round: u64,
}

/// A level's graph as its last rebuild made it (its own nodes only).
#[derive(Clone, Default)]
pub(crate) struct Snapshot {
    pub(crate) graph: Network,
    pub(crate) round: u64,
    /// A cheap fingerprint of the graph: a change tells linked levels to rebuild.
    pub(crate) print: u64,
}

#[derive(Default)]
pub(crate) struct Links {
    /// Server ticks (each level ticks once a server tick: a level seen again starts a new round).
    pub(crate) round: u64,
    ticked: HashSet<&'static str>,
    pub(crate) ends: Vec<End>,
    pub(crate) snaps: HashMap<&'static str, Snapshot>,
}

static LINKS: Mutex<Option<Links>> = Mutex::new(None);

pub(crate) fn with_links<R>(f: impl FnOnce(&mut Links) -> R) -> R {
    let mut g = LINKS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Links::default))
}

pub(crate) fn forget_all() {
    *LINKS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// A level's tick: counts server ticks.
pub(crate) fn level_ticked(dim: &'static str) {
    with_links(|l| l.tick(dim));
}

impl Links {
    pub(crate) fn tick(&mut self, dim: &'static str) {
        if !self.ticked.insert(dim) {
            self.round += 1;
            self.ticked.clear();
            self.ticked.insert(dim);
        }
    }

    fn fresh(&self, round: u64) -> bool {
        self.round.saturating_sub(round) <= STALE
    }

    /// A level's formed chambers now (all of them): the other levels with a chamber on one of the frequencies that
    /// came or went (their links may have changed: their grids are worked out again).
    pub(crate) fn report(&mut self, dim: &'static str, ends: &[(Pos, i64)]) -> Vec<&'static str> {
        let mut old: Vec<(Pos, i64)> = self
            .ends
            .iter()
            .filter(|e| e.dim == dim)
            .map(|e| (e.pos, e.freq))
            .collect();
        let mut new = ends.to_vec();
        old.sort();
        new.sort();
        let round = self.round;
        let horizon = STALE * 4;
        let now = self.round;
        self.ends
            .retain(|e| e.dim != dim && now.saturating_sub(e.round) <= horizon);
        self.ends.extend(new.iter().map(|(pos, freq)| End {
            dim,
            pos: *pos,
            freq: *freq,
            round,
        }));
        if old == new {
            return Vec::new();
        }
        let freqs: HashSet<i64> = old.iter().chain(&new).map(|e| e.1).collect();
        let mut out: Vec<&'static str> = self
            .ends
            .iter()
            .filter(|e| e.dim != dim && freqs.contains(&e.freq))
            .map(|e| e.dim)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The other end of the chamber at `pos` in `dim`: the one other formed chamber of its frequency, in another level,
    /// both seen lately (QuantumCluster: one other side for each frequency).
    pub(crate) fn partner(&self, dim: &'static str, pos: Pos) -> Option<(&'static str, Pos)> {
        let me = self
            .ends
            .iter()
            .find(|e| e.dim == dim && e.pos == pos && self.fresh(e.round))?;
        let same: Vec<&End> = self
            .ends
            .iter()
            .filter(|e| e.freq == me.freq && self.fresh(e.round))
            .collect();
        if same.len() != 2 {
            return None;
        }
        let other = same.into_iter().find(|e| !(e.dim == dim && e.pos == pos))?;
        (other.dim != dim).then_some((other.dim, other.pos))
    }

    /// A level's graph after its rebuild: whether it changed (its linked levels work theirs out again).
    pub(crate) fn keep(&mut self, dim: &'static str, graph: &Network) -> bool {
        let print = fingerprint(graph);
        let round = self.round;
        let old = self.snaps.insert(
            dim,
            Snapshot {
                graph: graph.clone(),
                round,
                print,
            },
        );
        old.is_none_or(|o| o.print != print)
    }
}

fn fingerprint(g: &Network) -> u64 {
    let mut ids: Vec<NodeId> = g.node_ids().collect();
    ids.sort_unstable();
    let mut h: u64 = ids.len() as u64;
    for id in ids {
        h = h.rotate_left(7) ^ id.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    h
}

/// The grids of a level whose bridges link other levels, worked out on the joint graph.
pub(crate) struct Joint {
    /// The grids holding this level's nodes, in this level's ids (pack), others' nodes left out.
    pub(crate) grids: Vec<Grid>,
    /// For each grid (same order): where it goes on in other levels (a node there), and the channels its devices want
    /// in all (the ad hoc limit counts them all).
    pub(crate) remote: Vec<Vec<(&'static str, Pos)>>,
    pub(crate) wanted: Vec<usize>,
}

/// The joint graph of `dim`'s `graph` and the parts of other levels its chambers `ends` (unpaired here) reach, or
/// None when no chamber links another level (the level's grids are its own, as before).
pub(crate) fn join(l: &Links, dim: &'static str, graph: &Network, ends: &[Pos]) -> Option<Joint> {
    // Levels' components reached: (level, a start node) and the cross edges ((level, node), (level, node)).
    let mut parts: Vec<(&'static str, BTreeSet<NodeId>)> = Vec::new();
    let mut edges: Vec<((&'static str, NodeId), (&'static str, NodeId))> = Vec::new();
    let mut queue: Vec<(&'static str, Pos, &'static str, Pos)> = ends
        .iter()
        .filter_map(|p| l.partner(dim, *p).map(|(d, q)| (dim, *p, d, q)))
        .collect();
    if queue.is_empty() {
        return None;
    }
    let mut seen_edges: HashSet<((&'static str, Pos), (&'static str, Pos))> = HashSet::new();
    while let Some((da, pa, db, pb)) = queue.pop() {
        let key = if (da, pa) < (db, pb) {
            ((da, pa), (db, pb))
        } else {
            ((db, pb), (da, pa))
        };
        if !seen_edges.insert(key) {
            continue;
        }
        let graph_of = |d: &'static str| -> Option<&Network> {
            if d == dim {
                Some(graph)
            } else {
                l.snaps
                    .get(d)
                    .filter(|s| l.fresh(s.round))
                    .map(|s| &s.graph)
            }
        };
        let (Some(ga), Some(gb)) = (graph_of(da), graph_of(db)) else {
            continue;
        };
        if ga.kind(pack(pa)).is_none() || gb.kind(pack(pb)).is_none() {
            continue;
        }
        edges.push(((da, pack(pa)), (db, pack(pb))));
        for (d, p, g) in [(da, pa, ga), (db, pb, gb)] {
            if d == dim
                || parts
                    .iter()
                    .any(|(pd, set)| *pd == d && set.contains(&pack(p)))
            {
                continue;
            }
            let comp = g.component(pack(p));
            // Further bridges in that part: their links too.
            for e in l
                .ends
                .iter()
                .filter(|e| e.dim == d && comp.contains(&pack(e.pos)))
            {
                if let Some((d2, p2)) = l.partner(d, e.pos) {
                    queue.push((d, e.pos, d2, p2));
                }
            }
            parts.push((d, comp));
        }
    }
    if edges.is_empty() {
        return None;
    }
    // This level's own part: what the links touch (its whole graph would do too; the rest is unlinked).
    let mut own: BTreeSet<NodeId> = BTreeSet::new();
    for ((d, n), (d2, n2)) in &edges {
        for (dd, nn) in [(d, n), (d2, n2)] {
            if *dd == dim && !own.contains(nn) {
                own.extend(graph.component(*nn));
            }
        }
    }
    parts.push((dim, own));
    // One numbering for all: (level, position, outer node, own id) sorted. Every level's rebuild makes the same.
    let mut keys: Vec<(&'static str, Pos, bool, NodeId)> = Vec::new();
    for (d, set) in &parts {
        let g = if *d == dim { graph } else { &l.snaps[d].graph };
        for &n in set {
            keys.push((*d, g.position(n).unwrap_or([0; 3]), g.is_outer(n), n));
        }
    }
    keys.sort();
    keys.dedup();
    let canon: HashMap<(&'static str, NodeId), NodeId> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| ((k.0, k.3), i as NodeId + 1))
        .collect();
    let back: HashMap<NodeId, (&'static str, NodeId, Pos)> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| (i as NodeId + 1, (k.0, k.3, k.1)))
        .collect();
    let mut joint = Network::new();
    for (d, set) in &parts {
        let g = if *d == dim { graph } else { &l.snaps[d].graph };
        let d = *d;
        let map = |n: NodeId| {
            canon
                .get(&(d, n))
                .copied()
                .unwrap_or(u64::MAX / 2 + (n % (1 << 40)))
        };
        joint.absorb(g, set, &map);
    }
    for ((da, a), (db, b)) in &edges {
        if let (Some(x), Some(y)) = (canon.get(&(*da, *a)), canon.get(&(*db, *b))) {
            joint.connect(*x, *y);
        }
    }
    let mut out = Joint {
        grids: Vec::new(),
        remote: Vec::new(),
        wanted: Vec::new(),
    };
    let to_local = |n: &NodeId| back.get(n).filter(|b| b.0 == dim).map(|b| b.1);
    for g in joint.grids() {
        if !g.nodes.iter().any(|n| to_local(n).is_some()) {
            continue;
        }
        let wanted = joint.channels_wanted(&g.nodes);
        let mut remote: Vec<(&'static str, Pos)> = Vec::new();
        for n in &g.nodes {
            if let Some((d, _, p)) = back.get(n).filter(|b| b.0 != dim)
                && !remote.iter().any(|r| r.0 == *d)
            {
                remote.push((*d, *p));
            }
        }
        // The joint grid of the other level's own nodes; mapped to this level's ids, its nodes only.
        let local = Grid {
            nodes: g.nodes.iter().filter_map(to_local).collect(),
            controller_state: g.controller_state,
            with_channel: g.with_channel.iter().filter_map(to_local).collect(),
            used_channels: g
                .used_channels
                .iter()
                .filter_map(|(n, c)| Some((to_local(n)?, *c)))
                .collect(),
            edge_channels: g
                .edge_channels
                .iter()
                .filter_map(|((a, b), c)| Some((crate::grid::edge(to_local(a)?, to_local(b)?), *c)))
                .collect(),
            channels_in_use: g.channels_in_use,
            nested_p2p: g.nested_p2p,
        };
        out.grids.push(local);
        out.remote.push(remote);
        out.wanted.push(wanted);
    }
    // This level's nodes no link reaches: their grids as before.
    let reached: HashSet<NodeId> = out
        .grids
        .iter()
        .flat_map(|g| g.nodes.iter().copied())
        .collect();
    if reached.len() < graph.kinds().len() {
        let mut rest = graph.clone();
        for n in &reached {
            rest.remove_node(*n);
        }
        for g in rest.grids() {
            out.wanted.push(rest.channels_wanted(&g.nodes));
            out.remote.push(Vec::new());
            out.grids.push(g);
        }
    }
    Some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::grid::{ControllerState, NodeKind};

    /// A level: a chamber at x 0 (dense, needs no channel), `n` devices on a line of cable behind it, a controller
    /// at the end when asked.
    pub(crate) fn level(n: usize, controller: bool) -> Network {
        // fixC: pub(crate) for the power test in `net` (an unpowered side powered over the bridge).
        let mut g = Network::new();
        let link = NodeKind {
            controller: false,
            requires_channel: false,
            max_channels: 32,
            dense: true,
            idle_power: 22.0,
        };
        g.add_node(pack([0, 64, 0]), link, [0, 64, 0]);
        let mut prev = pack([0, 64, 0]);
        for i in 1..=n as i32 {
            g.add_node(pack([i, 64, 0]), NodeKind::DENSE_CABLE, [i, 64, 0]);
            g.connect(prev, pack([i, 64, 0]));
            g.add_node(pack([i, 65, 0]), NodeKind::device(1.0), [i, 65, 0]);
            g.connect(pack([i, 64, 0]), pack([i, 65, 0]));
            prev = pack([i, 64, 0]);
        }
        if controller {
            let c = [n as i32 + 1, 64, 0];
            g.add_node(pack(c), NodeKind::CONTROLLER, c);
            g.connect(prev, pack(c));
        }
        g
    }

    fn linked() -> Links {
        let mut l = Links::default();
        l.tick("minecraft:overworld");
        l.tick("minecraft:the_nether");
        l.report("minecraft:overworld", &[([0, 64, 0], 77)]);
        l.report("minecraft:the_nether", &[([0, 64, 0], 77)]);
        l.keep("minecraft:overworld", &level(3, true));
        l.keep("minecraft:the_nether", &level(12, false));
        l
    }

    #[test]
    fn pairs_across_levels_and_drops_when_one_side_goes() {
        let mut l = linked();
        assert_eq!(
            l.partner("minecraft:overworld", [0, 64, 0]),
            Some(("minecraft:the_nether", [0, 64, 0]))
        );
        assert_eq!(
            l.partner("minecraft:the_nether", [0, 64, 0]),
            Some(("minecraft:overworld", [0, 64, 0]))
        );
        // A third chamber on the frequency: nobody links (one other side for each frequency).
        l.report("minecraft:the_end", &[([5, 70, 5], 77)]);
        assert_eq!(l.partner("minecraft:overworld", [0, 64, 0]), None);
        l.report("minecraft:the_end", &[]);
        assert!(l.partner("minecraft:overworld", [0, 64, 0]).is_some());
        // Broken (or its singularity taken out): the next report has no chamber, and the link drops.
        let changed = l.report("minecraft:the_nether", &[]);
        assert!(changed.contains(&"minecraft:overworld"), "{changed:?}");
        assert_eq!(l.partner("minecraft:overworld", [0, 64, 0]), None);
        // Back, then the nether unloads: it stops reporting, and after STALE server ticks its end is stale.
        l.report("minecraft:the_nether", &[([0, 64, 0], 77)]);
        assert!(l.partner("minecraft:overworld", [0, 64, 0]).is_some());
        for _ in 0..=STALE + 1 {
            l.tick("minecraft:overworld");
            l.report("minecraft:overworld", &[([0, 64, 0], 77)]);
        }
        assert_eq!(l.partner("minecraft:overworld", [0, 64, 0]), None);
        // Two in the same level are paired there, not across.
        let mut l = Links::default();
        l.report("a", &[([0, 0, 0], 5), ([9, 0, 0], 5)]);
        assert_eq!(l.partner("a", [0, 0, 0]), None);
    }

    #[test]
    fn one_grid_over_two_levels() {
        let l = linked();
        let over = level(3, true);
        let nether = level(12, false);
        let a = join(&l, "minecraft:overworld", &over, &[[0, 64, 0]]).expect("linked");
        let b = join(&l, "minecraft:the_nether", &nether, &[[0, 64, 0]]).expect("linked");
        assert_eq!(a.grids.len(), 1);
        assert_eq!(b.grids.len(), 1);
        // The nether's twelve devices have no controller of their own, yet get channels from the overworld's (no
        // ad hoc limit of 8): one controller for the joint grid, fifteen devices with channels in all.
        assert_eq!(
            b.grids[0].controller_state,
            ControllerState::ControllerOnline
        );
        assert_eq!(
            a.grids[0].controller_state,
            ControllerState::ControllerOnline
        );
        assert_eq!(b.grids[0].with_channel.len(), 12);
        assert_eq!(a.grids[0].with_channel.len(), 3);
        assert_eq!(a.grids[0].channels_in_use, 15);
        assert_eq!(b.grids[0].channels_in_use, 15);
        // Each side keeps its own nodes, by its own ids, and knows where the grid goes on.
        assert!(b.grids[0].nodes.contains(&pack([12, 65, 0])));
        assert!(!a.grids[0].nodes.contains(&pack([12, 65, 0])));
        assert_eq!(a.remote[0], vec![("minecraft:the_nether", [0, 64, 0])]);
        assert_eq!(b.remote[0], vec![("minecraft:overworld", [0, 64, 0])]);
    }

    #[test]
    fn two_controllers_across_levels_conflict() {
        let mut l = linked();
        l.keep("minecraft:the_nether", &level(2, true));
        let a = join(&l, "minecraft:overworld", &level(3, true), &[[0, 64, 0]]).expect("linked");
        assert_eq!(
            a.grids[0].controller_state,
            ControllerState::ControllerConflict
        );
    }

    #[test]
    fn unlinked_levels_keep_their_own_grids() {
        let mut l = linked();
        l.report("minecraft:the_nether", &[]);
        assert!(join(&l, "minecraft:overworld", &level(3, true), &[[0, 64, 0]]).is_none());
    }
}
