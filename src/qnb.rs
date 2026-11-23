//! AE2's quantum network bridge (QuantumBridgeBlockEntity, QuantumCluster, QuantumCalculator; gap-list item 8):
//! - a link chamber in the middle of a 3x3 of quantum rings, in any plane, forms a bridge; formed, the edges join
//!   cables on every side, the corners and the chamber only their neighbours in the bridge; unformed, no side joins;
//! - the chamber's one slot takes a quantum entangled singularity with a frequency (`ae2:freq` in its custom data);
//!   two formed bridges whose singularities share a frequency (a pair made by one blast) join their networks;
//! - the formed look (`qnb`: link, corner, edge; `power`: lit while its network is on; `side_*`: the stubs joining
//!   the chamber and corners to the edges), its screen (screens/qnb.json: the slot at 80, 37 over chest.png).
//!
//! fixA: a bridge joins its pair in any level: same-level pairs join in the level's own walk, a pair across levels
//! through `xlink` (one grid over both levels).

use crate::cells::sid;
use crate::screens::{spr, tr};
use crate::util::*;
use mod_api::*;

pub(crate) const LINK: &str = "ae2:quantum_link";
pub(crate) const RING: &str = "ae2:quantum_ring";
pub(crate) const ENTANGLED: &str = "ae2:quantum_entangled_singularity";
const FREQ: &str = "ae2:freq";
const TEXT: u32 = 0xFF40_4040;

pub(crate) fn is_qnb(block: &str) -> bool {
    block == LINK || block == RING
}

/// The planes a bridge can lie in, by the two axes it spans.
const PLANES: [(usize, usize); 3] = [(1, 2), (0, 2), (0, 1)];

/// QuantumCalculator: the plane of the bridge formed round the chamber at `link`, if any: its eight neighbours in
/// that plane are rings, and nothing of the bridge's kind sticks out of the 3x3 (MBCalculator's bounds from the
/// chamber: two along the plane's axes, one along its normal).
pub(crate) fn formed_plane(
    link: Pos,
    block_at: &mut dyn FnMut(Pos) -> String,
) -> Option<(usize, usize)> {
    if block_at(link) != LINK {
        return None;
    }
    PLANES.into_iter().find(|&(i, j)| {
        let k = 3 - i - j;
        let ring = (-1..=1).all(|a: i32| {
            (-1..=1).all(|b: i32| {
                if a == 0 && b == 0 {
                    return true;
                }
                let mut q = link;
                q[i] += a;
                q[j] += b;
                block_at(q) == RING
            })
        });
        if !ring {
            return false;
        }
        let mut beyond = Vec::new();
        for (axis, reach) in [(i, 2), (j, 2), (k, 1)] {
            for s in [-1, 1] {
                let mut q = link;
                q[axis] += s * reach;
                beyond.push(q);
            }
        }
        beyond.into_iter().all(|q| !is_qnb(&block_at(q)))
    })
}

/// A block's role in a formed bridge: `link`, `corner`, `edge` (QnbFormedState), or `none`; and the bridge's plane.
pub(crate) fn role(
    pos: Pos,
    block_at: &mut dyn FnMut(Pos) -> String,
) -> (&'static str, Option<(usize, usize)>) {
    let block = block_at(pos);
    if block == LINK {
        return match formed_plane(pos, block_at) {
            Some(p) => ("link", Some(p)),
            None => ("none", None),
        };
    }
    if block != RING {
        return ("none", None);
    }
    for (i, j) in PLANES {
        for a in -1..=1i32 {
            for b in -1..=1i32 {
                if a == 0 && b == 0 {
                    continue;
                }
                let mut l = pos;
                l[i] -= a;
                l[j] -= b;
                if formed_plane(l, block_at) == Some((i, j)) {
                    return (
                        if a != 0 && b != 0 { "corner" } else { "edge" },
                        Some((i, j)),
                    );
                }
            }
        }
    }
    ("none", None)
}

/// Which sides of a formed chamber or corner join the bridge's look (the sides in its plane toward its members).
pub(crate) fn joined_sides(
    pos: Pos,
    role: &str,
    plane: Option<(usize, usize)>,
    block_at: &mut dyn FnMut(Pos) -> String,
) -> [bool; 6] {
    let mut joined = [false; 6];
    if let Some((i, j)) = plane.filter(|_| role == "link" || role == "corner") {
        for (k, d) in SIDES.iter().enumerate() {
            let in_plane = (0..3).all(|a| a == i || a == j || d[a] == 0);
            joined[k] = in_plane && is_qnb(&block_at(add(pos, *d)));
        }
    }
    joined
}

/// getGridConnectableSides: unformed none; an edge every side; a corner or the chamber only toward other bridge
/// blocks. From the block's `qnb` look, which the bridge's tick keeps.
pub(crate) fn side_closed(role: Option<&str>, neighbour: &str) -> bool {
    match role {
        Some("edge") => false,
        Some("link" | "corner") => !is_qnb(neighbour),
        _ => true,
    }
}

// ---- Frequencies

/// An entangled singularity's frequency (both of a pair share it): QuantumBridgeBlockEntity.isValidEntangledSingularity.
pub(crate) fn frequency(stack: &ItemStack) -> Option<i64> {
    if sid(stack) != ENTANGLED {
        return None;
    }
    match crate::cells::custom_data(stack)?.get(&java(FREQ)) {
        Some(Tag::Long(f)) if *f != 0 => Some(*f),
        _ => None,
    }
}

static SEED: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// QuantumBridgeBlockEntity.assignFrequency: the time in milliseconds times 100, plus a seed.
pub(crate) fn new_frequency() -> i64 {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_millis() as i64);
    ms * 100
        + SEED
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .rem_euclid(100)
}

/// A stack of entangled singularities with a new frequency (a pair made by a blast).
pub(crate) fn with_frequency(mut stack: ItemStack) -> ItemStack {
    if sid(&stack) != ENTANGLED {
        return stack;
    }
    crate::cells::edit_custom_data(&mut stack, |d| {
        d.insert(java(FREQ), Tag::Long(new_frequency()));
    });
    stack
}

// ---- The tick, the network's view

/// The bridge's tick (a ring's or the chamber's, every second): its look from its structure and network.
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, block: &str, fields: &mut Fields) {
    let Some(level) = here(&*host) else { return };
    let dim = host.dimension();
    if block == LINK {
        fields.data.resize(2, 0);
        fields.data[1] = i32::from(
            fields
                .items
                .first()
                .cloned()
                .flatten()
                .as_ref()
                .and_then(frequency)
                .is_some(),
        );
    }
    if crate::net::with_net(dim, |n| n.tick) % 20 != (pos[0] + pos[2]).rem_euclid(20) as u64 {
        return;
    }
    let h: &dyn Host = &*host;
    let mut at = |q: Pos| crate::net::block_id(h, level, q);
    let (role, plane) = role(pos, &mut at);
    let joined = joined_sides(pos, role, plane, &mut at);
    if block == LINK {
        fields.data[0] = i32::from(role == "link");
    }
    let powered = crate::net::device_state(dim, pos).1;
    let lit = if powered && role != "none" {
        "on"
    } else {
        "off"
    };
    let mut want: Vec<(&'static str, String)> =
        vec![("qnb", role.to_owned()), ("power", lit.to_owned())];
    for (k, key) in [
        "side_down",
        "side_up",
        "side_north",
        "side_south",
        "side_west",
        "side_east",
    ]
    .into_iter()
    .enumerate()
    {
        want.push((key, joined[k].to_string()));
    }
    let changes: Vec<(Pos, &'static str, String)> = want
        .into_iter()
        .filter(|(k, v)| {
            level
                .block_property(&*host, pos, k)
                .is_some_and(|now| &now != v)
        })
        .map(|(k, v)| (pos, k, v))
        .collect();
    if !changes.is_empty() {
        crate::net::with_net(dim, |n| n.prop_sets.extend(changes));
    }
}

/// The formed chambers among `blocks` and their singularities' frequencies, for the network's walk: each pair on one
/// frequency joins.
#[allow(dead_code)]
pub(crate) fn links(
    host: &dyn Host,
    level: LevelRef,
    blocks: &std::collections::HashMap<Pos, String>,
) -> Vec<(Pos, Pos)> {
    pairs(formed_ends(host, level, blocks))
}

/// fixA: the formed chambers among `blocks` holding a singularity with a frequency (pairs here join in the walk; the
/// rest may pair with a chamber in another level, `xlink`).
pub(crate) fn formed_ends(
    host: &dyn Host,
    level: LevelRef,
    blocks: &std::collections::HashMap<Pos, String>,
) -> Vec<(Pos, i64)> {
    let mut found: Vec<(Pos, i64)> = Vec::new();
    for (p, b) in blocks {
        if b != LINK
            || is_hosted(*p)
            || level.block_property(host, *p, "qnb").as_deref() != Some("link")
        {
            continue;
        }
        let stack = host
            .block_entity(level, *p)
            .and_then(|be| be.item_slot(host, 0).ok().flatten());
        if let Some(f) = stack.as_ref().and_then(frequency) {
            found.push((*p, f));
        }
    }
    found
}

/// The chambers on one frequency, paired (QuantumCluster: one other side for each frequency).
pub(crate) fn pairs(mut found: Vec<(Pos, i64)>) -> Vec<(Pos, Pos)> {
    found.sort();
    let mut out = Vec::new();
    let mut used = vec![false; found.len()];
    for a in 0..found.len() {
        if used[a] {
            continue;
        }
        if let Some(b) = (a + 1..found.len()).find(|b| !used[*b] && found[*b].1 == found[a].1) {
            used[a] = true;
            used[b] = true;
            out.push((found[a].0, found[b].0));
        }
    }
    out
}

// ---- The chamber's slot and screen

pub(crate) fn has_screen(block: &str) -> bool {
    block == LINK
}

/// Its one slot (players only: the screen).
pub(crate) fn container_of(block: &str) -> Option<ModContainer> {
    (block == LINK).then(|| ModContainer {
        slots: 1,
        faces: std::array::from_fn(|_| Vec::new()),
        // Main's Container.canPlaceItem / canTakeItem read these: the menu's clicks need every slot; with no
        // faces, hoppers and pipes still reach none.
        inputs: (0..1).collect(),
        outputs: (0..1).collect(),
        insert_rules: Vec::new(),
        menu: Some(LINK.to_owned()),
    })
}

/// EntangledSingularityFilter: one entangled singularity with a frequency.
pub(crate) fn slot_rules(menu: &str, stack: &ItemStack, may_place: &mut bool, max_stack: &mut u32) {
    if menu == LINK {
        *may_place = frequency(stack).is_some();
        *max_stack = 1;
    }
}

/// A use of the chamber opens its screen (claimed before its beacon template's own use); not sneaking.
pub(crate) fn link_use(
    host: &mut dyn ServerHost,
    click: &BlockClick<'_>,
) -> Option<InteractionResult> {
    let level = here(&*host)?;
    if crate::net::block_id(&*host, level, click.pos) != LINK {
        return None;
    }
    let player = host.player(click.player)?;
    let e = player.entity();
    if e.is_shift_key_down(&*host).unwrap_or(false) || e.is_crouching(&*host).unwrap_or(false) {
        return None;
    }
    let entity = host.block_entity(level, click.pos)?;
    crate::devices::pad_slots(host, entity, LINK);
    let _ = player.open_menu(host, entity);
    Some(InteractionResult::SuccessServer)
}

pub(crate) fn screen(block: &str) -> Option<MenuScreenLayout> {
    (block == LINK).then(|| MenuScreenLayout {
        width: 176.0,
        height: 166.0,
        slots: vec![Some(slot_at([80.0, 37.0]))],
        inventory_y: 84.0,
        inventory_x: 8.0,
        own_labels: true,
        own_background: true,
        ..MenuScreenLayout::default()
    })
}

pub(crate) fn screen_draw(
    host: &dyn ClientHost,
    block: &str,
    d: &mut dyn DrawContext,
    o: [f32; 2],
) -> bool {
    if block != LINK {
        return false;
    }
    spr(d, "ae2:gui/chest", o, [0.0, 0.0], [0.0, 0.0, 176.0, 166.0]);
    d.text(
        &tr(host, "gui.ae2.QuantumLinkChamber", "Quantum Link Chamber"),
        [o[0] + 8.0, o[1] + 6.0],
        TEXT,
        false,
    );
    d.text(
        &tr(host, "container.inventory", "Inventory"),
        [o[0] + 8.0, o[1] + 166.0 - 93.0],
        TEXT,
        false,
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn world(blocks: &[(Pos, &str)]) -> HashMap<Pos, String> {
        blocks.iter().map(|(p, b)| (*p, (*b).to_owned())).collect()
    }

    fn ring_round(link: Pos, i: usize, j: usize) -> Vec<(Pos, &'static str)> {
        let mut out = vec![(link, LINK)];
        for a in -1..=1 {
            for b in -1..=1 {
                if a == 0 && b == 0 {
                    continue;
                }
                let mut q = link;
                q[i] += a;
                q[j] += b;
                out.push((q, RING));
            }
        }
        out
    }

    #[test]
    fn ring_check() {
        for (i, j) in PLANES {
            let w = world(&ring_round([0, 64, 0], i, j));
            let mut at = |q: Pos| w.get(&q).cloned().unwrap_or_default();
            assert_eq!(formed_plane([0, 64, 0], &mut at), Some((i, j)));
        }
        // A ring missing: not formed.
        let mut blocks = ring_round([0, 64, 0], 0, 2);
        blocks.pop();
        let w = world(&blocks);
        let mut at = |q: Pos| w.get(&q).cloned().unwrap_or_default();
        assert_eq!(formed_plane([0, 64, 0], &mut at), None);
        // A ring sticking out of the plane, or past its edge: not formed.
        for extra in [[0, 65, 0], [2, 64, 0]] {
            let mut blocks = ring_round([0, 64, 0], 0, 2);
            blocks.push((extra, RING));
            let w = world(&blocks);
            let mut at = |q: Pos| w.get(&q).cloned().unwrap_or_default();
            assert_eq!(formed_plane([0, 64, 0], &mut at), None, "{extra:?}");
        }
    }

    #[test]
    fn roles_and_sides() {
        let w = world(&ring_round([0, 64, 0], 0, 2));
        let mut at = |q: Pos| w.get(&q).cloned().unwrap_or_default();
        assert_eq!(role([0, 64, 0], &mut at).0, "link");
        assert_eq!(role([1, 64, 1], &mut at).0, "corner");
        assert_eq!(role([1, 64, 0], &mut at).0, "edge");
        assert_eq!(role([5, 64, 0], &mut at).0, "none");
        // The chamber joins its four edges (north, south, west, east), not up or down.
        let (r, plane) = role([0, 64, 0], &mut at);
        assert_eq!(
            joined_sides([0, 64, 0], r, plane, &mut at),
            [false, false, true, true, true, true]
        );
        // The corner at +x +z: toward its edges (north and west).
        let (r, plane) = role([1, 64, 1], &mut at);
        assert_eq!(
            joined_sides([1, 64, 1], r, plane, &mut at),
            [false, false, true, false, true, false]
        );
        assert!(side_closed(Some("none"), "ae2:fluix_glass_cable"));
        assert!(!side_closed(Some("edge"), "ae2:fluix_glass_cable"));
        assert!(side_closed(Some("corner"), "ae2:fluix_glass_cable"));
        assert!(!side_closed(Some("link"), RING));
    }

    #[test]
    fn frequencies_pair() {
        let a = new_frequency();
        let b = new_frequency();
        assert_ne!(a, b);
        assert!(a > 0);
        let found = vec![
            ([0, 0, 0], 5),
            ([9, 0, 0], 7),
            ([4, 0, 0], 5),
            ([8, 0, 0], 5),
        ];
        // Two of frequency 5 pair (the lowest positions first); the third waits; 7 has no other side.
        assert_eq!(pairs(found), vec![([0, 0, 0], [4, 0, 0])]);
    }
}
