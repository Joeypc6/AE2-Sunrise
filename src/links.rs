//! The parts whose outer node joins the block in front of them (gap-list item 8):
//! - the toggle bus and the inverted toggle bus (ToggleBusPart, InvertedToggleBusPart): the part's main node (on its
//!   cable) and its outer node (on the block in front) are connected while the cell has redstone (the inverted one:
//!   while it has none). Each part's tick reads the cell's redstone (`CableBusContainer.hasRedstone`); a change makes
//!   the level's grids again;
//! - the quartz fiber (QuartzFiberPart): its outer node joins the block in front, but its nodes carry nothing
//!   (`CANNOT_CARRY`): the two grids stay apart and share their power only (`IEnergyOverlayGridConnection`), which
//!   `net` does with a power group per set of grids so joined (`power_groups`).

use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) fn is_toggle(id: &str) -> bool {
    matches!(id, "ae2:toggle_bus" | "ae2:inverted_toggle_bus")
}

pub(crate) fn is_fiber(id: &str) -> bool {
    id == "ae2:quartz_fiber"
}

/// Parts with an outer node on the block in front of them (`getExternalFacingNode`).
pub(crate) fn has_outer(id: &str) -> bool {
    is_toggle(id) || is_fiber(id)
}

/// ToggleBusPart.isEnabled: the host's redstone, inverted for the inverted bus.
pub(crate) fn toggle_on(id: &str, redstone: bool) -> bool {
    if id == "ae2:inverted_toggle_bus" {
        !redstone
    } else {
        redstone
    }
}

/// Each toggle bus's state as its last tick found it, by level and node.
static TOGGLES: Mutex<Option<HashMap<(&'static str, Pos), bool>>> = Mutex::new(None);

fn with_toggles<R>(f: impl FnOnce(&mut HashMap<(&'static str, Pos), bool>) -> R) -> R {
    let mut g = TOGGLES.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

pub(crate) fn forget_all() {
    *TOGGLES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Whether the toggle bus at `pos` (its node) connects (as its last tick found it).
pub(crate) fn toggle_enabled(dim: &'static str, pos: Pos) -> bool {
    with_toggles(|t| t.get(&(dim, pos)).copied().unwrap_or(false))
}

/// A toggle bus's tick (onNeighborChanged's check, each tick): its cell's redstone; a change remakes the grids.
pub(crate) fn toggle_tick(host: &mut dyn ServerHost, pos: Pos, block: &str) {
    let Some(level) = here(&*host) else { return };
    let dim = host.dimension();
    let on = toggle_on(block, level.has_neighbor_signal(&*host, real(pos)));
    let was = with_toggles(|t| t.insert((dim, pos), on));
    if was != Some(on) {
        crate::net::mark_dirty(dim);
    }
}

/// The node a part's outer node at `p` (hosted, facing out of its side) meets in front: the neighbour cell's cable
/// (its facing side free), that side's part's own outer node (a toggle bus that is on, a quartz fiber), or a network
/// block whose side is open there. `block_of` reads the network's view of a position.
pub(crate) fn outer_target(
    host: &dyn Host,
    level: LevelRef,
    p: Pos,
    block_of: &mut dyn FnMut(Pos) -> String,
) -> Option<Pos> {
    let (cell, side) = host_cell(p);
    let d = SIDES[side];
    let q = add(cell, d);
    let back = side ^ 1;
    if let Some(c) = crate::bus::cell(host, level, q) {
        if c.facades[back].is_some() {
            return None;
        }
        return match c.parts[back].as_deref() {
            None => Some(q),
            Some(id) if is_fiber(id) => Some(hosted(q, back)),
            Some(id) if is_toggle(id) && toggle_enabled(host.dimension(), hosted(q, back)) => {
                Some(hosted(q, back))
            }
            Some(_) => None,
        };
    }
    let other = block_of(q);
    if !crate::net::is_network_block(&other) {
        return None;
    }
    crate::net::open_toward(host, level, q, &other, [-d[0], -d[1], -d[2]]).then_some(q)
}

/// Whether the cell at `p` has a part with an outer node on its side toward `q` (a neighbouring cable draws its arm
/// into it, as it does into AE2's part).
pub(crate) fn faces_outer(host: &dyn Host, level: LevelRef, p: Pos, q: Pos) -> bool {
    let Some(side) = SIDES
        .iter()
        .position(|s| *s == [q[0] - p[0], q[1] - p[1], q[2] - p[2]])
    else {
        return false;
    };
    crate::bus::cell(host, level, p)
        .is_some_and(|c| c.parts[side].as_deref().is_some_and(has_outer))
}

/// The power group of each grid: grids joined by quartz fibers (`pairs` of grid indices) share one, named by the
/// lowest root among them. `roots[i]` is grid `i`'s root.
pub(crate) fn power_groups(roots: &[Option<Pos>], pairs: &[(usize, usize)]) -> Vec<Option<Pos>> {
    let n = roots.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for &(a, b) in pairs {
        if a >= n || b >= n {
            continue;
        }
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[ra] = rb;
        }
    }
    let mut best: HashMap<usize, Pos> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        if let Some(root) = roots[i] {
            best.entry(r)
                .and_modify(|b| *b = (*b).min(root))
                .or_insert(root);
        }
    }
    (0..n)
        .map(|i| best.get(&find(&mut parent, i)).copied())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles() {
        assert!(toggle_on("ae2:toggle_bus", true));
        assert!(!toggle_on("ae2:toggle_bus", false));
        assert!(!toggle_on("ae2:inverted_toggle_bus", true));
        assert!(toggle_on("ae2:inverted_toggle_bus", false));
    }

    #[test]
    fn groups() {
        let roots = vec![
            Some([5, 0, 0]),
            Some([1, 0, 0]),
            Some([9, 0, 0]),
            Some([3, 0, 0]),
        ];
        // No fibers: each its own.
        assert_eq!(power_groups(&roots, &[]), roots);
        // 0-2 and 2-3 joined: all three named by the lowest of them.
        let g = power_groups(&roots, &[(0, 2), (2, 3)]);
        assert_eq!(
            g,
            vec![
                Some([3, 0, 0]),
                Some([1, 0, 0]),
                Some([3, 0, 0]),
                Some([3, 0, 0])
            ]
        );
        // A fiber within one grid, or out of range, changes nothing.
        assert_eq!(power_groups(&roots, &[(1, 1), (0, 7)]), roots);
    }
}
