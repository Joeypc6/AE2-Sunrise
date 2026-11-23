//! The cable bus (CableBusContainer, CableBusBlock): one cable's cell holding the cable, up to six parts and facades.
//! Upstream's design (`../ae2-horizon/PLAN.md`, Rounds 21 and 22) on the fork's APIs:
//! - the cell is a facade twin (`ae2:hz_fcb<core>_<n>[rotation]`, block entity type `cable_bus`), whose state's shape
//!   holds the cable's arms (bits 0 to 5) and plates (bits 6 to 11): its collision and what a click can hit;
//! - each part is a network node of its own at a position past the world's top (`util::hosted`); its block entity
//!   fields are the cell's: its nbt under `p<side>`, its slots `side * SLOTS ..`, its data `side * DATA ..`. Its
//!   logic is the part's own (`devices::run_device`), and `util::add` steps from the cell, so its target is the
//!   cell's real neighbour;
//! - the cell's block entity is each part's menu holder (`ae2:cable_bus`, six parts' slots, the open part's shown);
//! - aiming, the outline and breaking are each part's own shape (AE2's part boxes), drawn and picked by the mod.

use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

/// Each side's slots in the cell's container (the pattern provider's 18 is the most a hosted part has).
pub(crate) const SLOTS: usize = 18;
/// Each side's menu data (the buses' 9 is the most).
pub(crate) const DATA: usize = 9;
/// Each side's tanks (a hosted pattern provider's nine return tanks).
pub(crate) const TANKS: usize = 9;
pub(crate) const MENU: &str = "ae2:cable_bus";
pub(crate) const NET_BUS: &str = "ae2:bus";

/// A cell: its cable (`ae2:fluix_glass_cable`), the part on each side and each side's facade (a block id).
#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) struct Cell {
    pub(crate) cable: String,
    pub(crate) parts: [Option<String>; 6],
    pub(crate) facades: [Option<String>; 6],
}

pub(crate) fn is_cell_block(id: &str) -> bool {
    id.starts_with("ae2:hz_fcb") || id.starts_with("ae2:hz_fcl")
}

fn str_of(t: Option<&Tag>) -> Option<String> {
    match t {
        Some(Tag::String(s)) => Some(s.to_string_lossy().to_string()).filter(|s| !s.is_empty()),
        _ => None,
    }
}

fn part_key(side: usize) -> String {
    format!("p{side}")
}

pub(crate) fn read_cell(nbt: &Compound) -> Cell {
    let mut c = Cell { cable: str_of(nbt.get(&java("cable"))).unwrap_or_default(), ..Cell::default() };
    for s in 0..6 {
        if let Some(Tag::Compound(p)) = nbt.get(&java(&part_key(s))) {
            c.parts[s] = str_of(p.get(&java("id")));
        }
        c.facades[s] = str_of(nbt.get(&java(&format!("f{s}"))));
    }
    c
}

fn write_cell(nbt: &mut Compound, c: &Cell) {
    nbt.insert(java("cable"), Tag::String(java(&c.cable)));
    for s in 0..6 {
        match &c.parts[s] {
            Some(id) => {
                let mut p = match nbt.get(&java(&part_key(s))) {
                    Some(Tag::Compound(p)) => p.clone(),
                    _ => Compound::new(),
                };
                p.insert(java("id"), Tag::String(java(id)));
                nbt.insert(java(&part_key(s)), Tag::Compound(p));
            }
            None => {
                nbt.remove(&java(&part_key(s)));
            }
        }
        match &c.facades[s] {
            Some(f) => {
                nbt.insert(java(&format!("f{s}")), Tag::String(java(f)));
            }
            None => {
                nbt.remove(&java(&format!("f{s}")));
            }
        }
    }
}

static CELLS: Mutex<Option<HashMap<(&'static str, Pos), Cell>>> = Mutex::new(None);

fn with_cells<R>(f: impl FnOnce(&mut HashMap<(&'static str, Pos), Cell>) -> R) -> R {
    let mut g = CELLS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

pub(crate) fn forget_all() {
    *CELLS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The cell at `pos` (a real position), if one is there.
pub(crate) fn cell(host: &dyn Host, level: LevelRef, pos: Pos) -> Option<Cell> {
    let id = level.block(host, pos).and_then(|b| b.id(host).map(str::to_owned))?;
    if !is_cell_block(&id) {
        return None;
    }
    let dim = host.dimension();
    if let Some(c) = with_cells(|t| t.get(&(dim, pos)).cloned()) {
        return Some(c);
    }
    let c = read_cell(&host.block_entity(level, pos)?.data(host)?);
    with_cells(|t| t.insert((dim, pos), c.clone()));
    Some(c)
}

/// The part a hosted node is.
pub(crate) fn part_at(host: &dyn Host, level: LevelRef, p: Pos) -> Option<String> {
    let (c, side) = host_cell(p);
    cell(host, level, c)?.parts[side].clone()
}

pub(crate) fn cable_at(host: &dyn Host, level: LevelRef, pos: Pos) -> Option<String> {
    Some(cell(host, level, pos)?.cable).filter(|c| !c.is_empty())
}

/// A block property as the network reads it: a hosted part faces out of its side.
pub(crate) fn prop(host: &dyn Host, level: LevelRef, pos: Pos, key: &str) -> Option<String> {
    if is_hosted(pos) {
        return (key == "facing").then(|| SIDE_NAMES[host_cell(pos).1].to_owned());
    }
    level.block_property(host, pos, key)
}

fn side_of(d: Pos) -> Option<usize> {
    SIDES.iter().position(|s| *s == d)
}

/// Whether the cell at `p` (if it is one) is closed toward its neighbour `q`: a part or a facade on that side.
pub(crate) fn blocked(host: &dyn Host, level: LevelRef, p: Pos, q: Pos) -> bool {
    let Some(side) = side_of([q[0] - p[0], q[1] - p[1], q[2] - p[2]]) else { return false };
    cell(host, level, p).is_some_and(|c| c.parts[side].is_some() || c.facades[side].is_some())
}

/// What the network walk tries from `p`: a hosted part only its cell; a cell its neighbours and its parts.
pub(crate) fn edges(host: &dyn Host, level: LevelRef, p: Pos) -> Vec<Pos> {
    if is_hosted(p) {
        return vec![host_cell(p).0];
    }
    let mut out: Vec<Pos> = SIDES.iter().map(|d| add(p, *d)).collect();
    if let Some(c) = cell(host, level, p) {
        out.extend((0..6).filter(|s| c.parts[*s].is_some()).map(|s| hosted(p, s)));
    }
    out
}

/// A hosted part's own nbt (else the block entity's).
pub(crate) fn nbt_at(host: &dyn Host, level: LevelRef, pos: Pos) -> Option<Compound> {
    if !is_hosted(pos) {
        return host.block_entity(level, pos)?.data(host);
    }
    let (c, side) = host_cell(pos);
    match host.block_entity(level, c)?.data(host)?.get(&java(&part_key(side))) {
        Some(Tag::Compound(p)) => match p.get(&java("nbt")) {
            Some(Tag::Compound(n)) => Some(n.clone()),
            _ => Some(Compound::new()),
        },
        _ => None,
    }
}

/// Writes a hosted part's own nbt (else the block entity's), as `nbt_at` reads it.
pub(crate) fn set_nbt_at(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, own: Compound) -> bool {
    if !is_hosted(pos) {
        let Some(be) = host.block_entity(level, pos) else { return false };
        return be.replace_data(host, own).is_ok();
    }
    let (c, side) = host_cell(pos);
    let Some(be) = host.block_entity(level, c) else { return false };
    let mut nbt = be.data(&*host).unwrap_or_default();
    let Some(Tag::Compound(mut p)) = nbt.get(&java(&part_key(side))).cloned() else { return false };
    p.insert(java("nbt"), Tag::Compound(own));
    nbt.insert(java(&part_key(side)), Tag::Compound(p));
    be.replace_data(host, nbt).is_ok()
}

/// A hosted part's slot (else the block entity's).
pub(crate) fn item_slot(host: &dyn Host, level: LevelRef, pos: Pos, slot: usize) -> Option<ItemStack> {
    if !is_hosted(pos) {
        return host.block_entity(level, pos)?.item_slot(host, slot as u32).ok().flatten();
    }
    if slot >= SLOTS {
        return None;
    }
    let (c, side) = host_cell(pos);
    host.block_entity(level, c)?.item_slot(host, (side * SLOTS + slot) as u32).ok().flatten()
}

// ---- A side's fields: the part's block entity, as its logic sees it

pub(crate) fn side_fields(cell: &Fields, side: usize) -> Fields {
    let nbt = match cell.nbt.get(&java(&part_key(side))) {
        Some(Tag::Compound(p)) => match p.get(&java("nbt")) {
            Some(Tag::Compound(n)) => n.clone(),
            _ => Compound::new(),
        },
        _ => Compound::new(),
    };
    let mut nbt = nbt;
    if let Some(Tag::List(t)) = cell.nbt.get(&java("fluids")) {
        nbt.insert(java("fluids"), Tag::List(t.iter().skip(side * TANKS).take(TANKS).cloned().collect()));
    }
    if let Some(e) = cell.nbt.get(&java("energy")) {
        nbt.insert(java("energy"), e.clone());
    }
    let items = (0..SLOTS).map(|i| cell.items.get(side * SLOTS + i).cloned().flatten()).collect();
    let data = (0..DATA).map(|i| cell.data.get(side * DATA + i).copied().unwrap_or(0)).collect();
    Fields { nbt, items, data }
}

/// Writes a side's fields back where they changed.
pub(crate) fn put_side(cell: &mut Fields, side: usize, mut f: Fields) {
    if let Some(Tag::List(mine)) = f.nbt.remove(&java("fluids")) {
        // A side's tanks written before the cell had any saved (an interface's fluid stock): the cell's list first.
        let mut all = match cell.nbt.get(&java("fluids")) {
            Some(Tag::List(all)) => all.clone(),
            _ => Vec::new(),
        };
        while all.len() < 6 * TANKS {
            let mut empty = Compound::new();
            empty.insert(java("Fluid"), Tag::String(java("minecraft:empty")));
            empty.insert(java("Amount"), Tag::Int(0));
            all.push(Tag::Compound(empty));
        }
        for (i, t) in mine.into_iter().enumerate().take(TANKS) {
            if let Some(slot) = all.get_mut(side * TANKS + i) {
                *slot = t;
            }
        }
        if cell.nbt.get(&java("fluids")) != Some(&Tag::List(all.clone())) {
            cell.nbt.insert(java("fluids"), Tag::List(all));
        }
    }
    if let Some(e) = f.nbt.remove(&java("energy"))
        && cell.nbt.get(&java("energy")) != Some(&e)
    {
        cell.nbt.insert(java("energy"), e);
    }
    let key = java(&part_key(side));
    if let Some(Tag::Compound(p)) = cell.nbt.get(&key)
        && p.get(&java("nbt")) != Some(&Tag::Compound(f.nbt.clone()))
    {
        let mut p = p.clone();
        p.insert(java("nbt"), Tag::Compound(f.nbt));
        cell.nbt.insert(key, Tag::Compound(p));
    }
    if cell.items.len() < 6 * SLOTS {
        cell.items.resize(6 * SLOTS, None);
    }
    for (i, it) in f.items.into_iter().take(SLOTS).enumerate() {
        if cell.items[side * SLOTS + i] != it {
            cell.items[side * SLOTS + i] = it;
        }
    }
    if cell.data.len() < 6 * DATA {
        cell.data.resize(6 * DATA, 0);
    }
    for (i, v) in f.data.into_iter().take(DATA).enumerate() {
        if cell.data[side * DATA + i] != v {
            cell.data[side * DATA + i] = v;
        }
    }
}

// ---- What a cell may host

/// The parts a cable bus hosts (all of AE2's parts but the pattern provider's, whose return tanks are a block's).
pub(crate) fn hostable(id: &str) -> bool {
    crate::net::is_part(id) && !id.starts_with("ae2:hz_")
}

/// The cable's core: glass 0, covered and smart 1, dense 2 (the twins' three families).
fn core_of(cable: &str) -> usize {
    match crate::net::cable(cable) {
        Some((_, true)) => 2,
        _ if cable.contains("glass") => 0,
        _ => 1,
    }
}

const CORE: [[f64; 2]; 3] = [[6.0, 10.0], [5.0, 11.0], [3.0, 13.0]];

/// AE2's part boxes (each part's `getBoxes`), facing south (z 16 is the outer face), in pixels.
fn part_boxes_local(id: &str) -> &'static [[f64; 6]] {
    match crate::net::ae2_name(id).unwrap_or("") {
        "import_bus" => &[[6.0, 6.0, 11.0, 10.0, 10.0, 13.0], [5.0, 5.0, 13.0, 11.0, 11.0, 14.0], [4.0, 4.0, 14.0, 12.0, 12.0, 16.0]],
        "export_bus" => &[[4.0, 4.0, 12.0, 12.0, 12.0, 14.0], [5.0, 5.0, 14.0, 11.0, 11.0, 15.0], [6.0, 6.0, 15.0, 10.0, 10.0, 16.0], [6.0, 6.0, 11.0, 10.0, 10.0, 12.0]],
        "storage_bus" => &[[3.0, 3.0, 15.0, 13.0, 13.0, 16.0], [2.0, 2.0, 14.0, 14.0, 14.0, 15.0], [5.0, 5.0, 12.0, 11.0, 11.0, 14.0]],
        "level_emitter" | "energy_level_emitter" => &[[7.0, 7.0, 11.0, 9.0, 9.0, 16.0]],
        "quartz_fiber" => &[[6.0, 6.0, 10.0, 10.0, 10.0, 16.0]],
        "toggle_bus" | "inverted_toggle_bus" => &[[6.0, 6.0, 11.0, 10.0, 10.0, 16.0]],
        "cable_anchor" => &[[7.0, 7.0, 10.0, 9.0, 9.0, 16.0]],
        "annihilation_plane" | "formation_plane" => &[[1.0, 1.0, 15.0, 15.0, 15.0, 16.0], [5.0, 5.0, 14.0, 11.0, 11.0, 15.0]],
        "interface" | "energy_acceptor" => &[[2.0, 2.0, 14.0, 14.0, 14.0, 16.0], [5.0, 5.0, 12.0, 11.0, 11.0, 14.0]],
        n if n.ends_with("_p2p_tunnel") => &[[5.0, 5.0, 12.0, 11.0, 11.0, 13.0], [3.0, 3.0, 13.0, 13.0, 13.0, 14.0], [2.0, 2.0, 14.0, 14.0, 14.0, 16.0]],
        // Terminals, panels and monitors (AbstractDisplayPart).
        _ => &[[2.0, 2.0, 14.0, 14.0, 14.0, 16.0], [4.0, 4.0, 13.0, 12.0, 12.0, 14.0]],
    }
}

/// A south-facing local box on `side`, in blocks.
fn on_side(b: &[f64; 6], side: usize) -> [f64; 6] {
    let (a0, b0, u0, a1, b1, u1) = (b[0], b[1], b[2], b[3], b[4], b[5]);
    let (u0, u1) = if side % 2 == 1 { (u0, u1) } else { (16.0 - u1, 16.0 - u0) };
    let r = match side {
        0 | 1 => [a0, u0, b0, a1, u1, b1],
        2 | 3 => [a0, b0, u0, a1, b1, u1],
        _ => [u0, b0, a0, u1, b1, a1],
    };
    r.map(|v| v / 16.0)
}

pub(crate) fn part_boxes(id: &str, side: usize) -> Vec<[f64; 6]> {
    part_boxes_local(id).iter().map(|b| on_side(b, side)).collect()
}

pub(crate) fn facade_box(side: usize) -> [f64; 6] {
    on_side(&[0.0, 0.0, 14.0, 16.0, 16.0, 16.0], side)
}

/// The cable's core and its arms, in blocks.
pub(crate) fn cable_boxes(core: usize, arms: u32) -> Vec<[f64; 6]> {
    let [lo, hi] = CORE[core.min(2)];
    let mut out = vec![[lo, lo, lo, hi, hi, hi].map(|v| v / 16.0)];
    for s in 0..6 {
        if arms >> s & 1 == 1 {
            out.push(on_side(&[lo, lo, hi, hi, hi, 16.0], s));
        }
    }
    out
}

/// What a point (block-relative) is on: a part's side, else a facade's (`6 + side`), else the cable (12).
fn pick_point(c: &Cell, at: [f64; 3]) -> Option<usize> {
    const E: f64 = 1.0 / 512.0;
    let inside = |b: &[f64; 6]| (0..3).all(|i| at[i] >= b[i] - E && at[i] <= b[i + 3] + E);
    for s in 0..6 {
        if let Some(id) = &c.parts[s]
            && part_boxes(id, s).iter().any(inside)
        {
            return Some(s);
        }
    }
    // fixE: a thin rod (the quartz fiber, a level emitter, the anchor) has no cable arm under it: the cell's shape
    // there is its side's arm (`look_of`), so a hit on that arm, past the core, is the rod's.
    let [lo, hi] = CORE[core_of(&c.cable).min(2)];
    for s in 0..6 {
        if let Some(id) = &c.parts[s]
            && is_rod_without_arm(id)
            && inside(&on_side(&[lo, lo, hi, hi, hi, 16.0], s))
        {
            return Some(s);
        }
    }
    for s in 0..6 {
        if c.facades[s].is_some() && inside(&facade_box(s)) {
            return Some(6 + s);
        }
    }
    // A plate bit stands for a part too: its side's whole face.
    for s in 0..6 {
        if c.parts[s].is_some() && inside(&facade_box(s)) {
            return Some(s);
        }
    }
    Some(12)
}

/// fixE: the parts drawn with no cable arm to them (`submit`'s `core_parts` only): a hit on their side's arm is theirs.
fn is_rod_without_arm(id: &str) -> bool {
    matches!(crate::net::ae2_name(id), Some("level_emitter" | "energy_level_emitter" | "quartz_fiber" | "cable_anchor"))
}

/// A click's point relative to its cell (`BlockHit::point` is in world coordinates); a point that is already
/// relative (inside the unit block where the world one isn't) is taken as it is.
fn rel_point(pos: Pos, point: [f64; 3]) -> [f64; 3] {
    let rel = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    let near = |p: &[f64; 3]| p.iter().all(|v| (-0.1..=1.1).contains(v));
    if !near(&rel) && near(&point) {
        point
    } else {
        rel
    }
}

/// The axis (0 x, 1 y, 2 z) side `side` (down, up, north, south, west, east) is out along.
fn side_axis(side: usize) -> usize {
    [1, 1, 2, 2, 0, 0][side % 6]
}

/// What a click on a cell is on (AE2's SelectedPart): the hit point's pick when it is a part; else the part a ray
/// from the eye (block-relative) through the point meets first; else, for a point on the clicked face's outer
/// plane (`face`, its normal), that side's part; else the point's pick (a facade's `6 + side`, the cable 12).
fn pick_click(c: &Cell, at: [f64; 3], face: Pos, eye: Option<[f64; 3]>) -> Option<usize> {
    let first = pick_point(c, at);
    if first.is_some_and(|s| s < 6) {
        return first;
    }
    if let Some(o) = eye {
        let d = [at[0] - o[0], at[1] - o[1], at[2] - o[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if len > 1e-6
            && let Some(s) = pick_ray(c, 0, o, d.map(|v| v / len)).filter(|s| *s < 6)
        {
            return Some(s);
        }
    }
    if let Some(side) = side_of(face)
        && c.parts[side].is_some()
    {
        // The outer plane of that side: 0 or 1 on its axis.
        let axis = side_axis(side);
        let edge = if side % 2 == 1 { 1.0 } else { 0.0 };
        if (at[axis] - edge).abs() <= 1.0 / 64.0 {
            return Some(side);
        }
    }
    first
}

/// The nearest box a ray (block-relative) meets: the same answers as `pick_point`.
fn pick_ray(c: &Cell, arms: u32, o: [f64; 3], d: [f64; 3]) -> Option<usize> {
    let hit = |b: &[f64; 6]| -> Option<f64> {
        let (mut t0, mut t1) = (0.0f64, 8.0f64);
        for i in 0..3 {
            if d[i].abs() < 1e-9 {
                if o[i] < b[i] || o[i] > b[i + 3] {
                    return None;
                }
                continue;
            }
            let (mut a, mut z) = ((b[i] - o[i]) / d[i], (b[i + 3] - o[i]) / d[i]);
            if a > z {
                std::mem::swap(&mut a, &mut z);
            }
            t0 = t0.max(a);
            t1 = t1.min(z);
            if t0 > t1 {
                return None;
            }
        }
        Some(t0)
    };
    let mut best: Option<(f64, usize)> = None;
    let mut try_box = |b: &[f64; 6], what: usize| {
        if let Some(t) = hit(b)
            && best.is_none_or(|(bt, _)| t < bt - 1e-9)
        {
            best = Some((t, what));
        }
    };
    for s in 0..6 {
        if let Some(id) = &c.parts[s] {
            for b in part_boxes(id, s) {
                try_box(&b, s);
            }
        }
    }
    for s in 0..6 {
        if c.facades[s].is_some() {
            try_box(&facade_box(s), 6 + s);
        }
    }
    for b in cable_boxes(core_of(&c.cable), arms) {
        try_box(&b, 12);
    }
    best.map(|(_, w)| w)
}

// ---- The twin's state: arms and plates

/// The twin block and rotation for a look (`n * 16 + rotation`).
fn twin_state(core: usize, look: u32) -> String {
    format!("ae2:hz_fcb{core}_{}[rotation={}]", look / 16, look % 16)
}

fn look_of(arms: u32, c: &Cell) -> u32 {
    // Parts are plates (their face) unless thin enough to be a rod; the rod is the side's arm.
    let mut arms = arms;
    let mut plates = 0;
    for s in 0..6 {
        if c.facades[s].is_some() {
            plates |= 1 << s;
        }
        if let Some(id) = &c.parts[s] {
            arms |= 1 << s;
            if !matches!(crate::net::ae2_name(id), Some("level_emitter" | "energy_level_emitter" | "quartz_fiber" | "toggle_bus" | "inverted_toggle_bus" | "cable_anchor")) {
                plates |= 1 << s;
            }
        }
    }
    (arms & 63) | (plates & 63) << 6
}

/// Looks waiting for the level's tick end (a block entity's tick can't replace its own block): block, rotation.
static LOOKS: Mutex<Vec<(&'static str, Pos, String, u32)>> = Mutex::new(Vec::new());

/// The cell's twin: its arms and plates (`hz_fcb`), or, with a lit panel, its plates on a light twin (`hz_fcl15`,
/// light 15 as the panels' lit blocks; those twins hold plates only).
fn want_state(c: &Cell, arms: u32, lit: bool) -> (String, u32) {
    let look = look_of(arms, c);
    if lit {
        let plates = look >> 6;
        return (format!("ae2:hz_fcl15_{}", plates / 16), plates % 16);
    }
    (format!("ae2:hz_fcb{}_{}", core_of(&c.cable), look / 16), look % 16)
}

/// Puts the cell's twin `block[rotation=rot]`, its block entity's data kept.
fn set_look(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, block: &str, rot: u32) {
    let want = format!("{block}[rotation={rot}]");
    let look = rot;
    let cur = level.block(&*host, pos).and_then(|b| b.id(&*host).map(str::to_owned)).unwrap_or_default();
    if !is_cell_block(&cur) {
        return;
    }
    if cur == block {
        let r = look.to_string();
        if level.block_property(&*host, pos, "rotation").as_deref() != Some(r.as_str()) {
            host.set_block_property_with(pos, "rotation", &r, 3);
        }
        return;
    }
    let Some(be) = host.block_entity(level, pos) else { return };
    let nbt = be.data(&*host).unwrap_or_default();
    let items: Vec<Option<ItemStack>> = (0..6 * SLOTS).map(|i| be.item_slot(&*host, i as u32).ok().flatten()).collect();
    let data = be.data_slots(&*host).unwrap_or_default();
    let Some(state) = host.block_state_id(&want) else { return };
    let flags = BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS | BlockUpdateFlags::SUPPRESS_DROPS;
    if !matches!(level.set_block(host, pos, state, flags), Ok(true)) {
        return;
    }
    restore(host, level, pos, nbt, items, data);
}

fn restore(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, nbt: Compound, items: Vec<Option<ItemStack>>, data: Vec<i32>) {
    let Some(be) = host.block_entity(level, pos) else { return };
    let _ = be.replace_data(host, nbt);
    for (i, it) in items.into_iter().enumerate() {
        if it.is_some() {
            let _ = host.transaction(|host, tx| be.set_item_slot(host, tx, i as u32, it.clone()).map_err(|_| ()));
        }
    }
    if !data.is_empty() {
        let _ = be.set_data_slots(host, data);
    }
}

/// Menus to open at the tick's end: dimension, player, cell, side, part.
static PENDING: Mutex<Vec<(&'static str, u128, Pos, usize, String)>> = Mutex::new(Vec::new());

fn open_pending(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str) {
    let todo: Vec<_> = {
        let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *g).into_iter().partition(|p| p.0 == dim);
        *g = rest;
        here
    };
    for (_, player, pos, side, id) in todo {
        // Still that part there.
        if cell(&*host, level, pos).and_then(|c| c.parts[side].clone()).as_deref() != Some(id.as_str()) {
            continue;
        }
        let (Some(p), Some(entity)) = (host.player(player), host.block_entity(level, pos)) else { continue };
        // The twin's own id (`net::block_id` names the cable it stands for, which declares no slots).
        let block = level.block(&*host, pos).and_then(|b| b.id(&*host).map(str::to_owned)).unwrap_or_default();
        crate::devices::pad_slots(host, entity, &block);
        if matches!(p.open_menu(host, entity), Ok(true)) {
            let window = p.container_menu(&*host).and_then(|m| m.container_id(&*host));
            crate::storage::opened(player, window, dim, hosted(pos, side), &id, false);
            crate::parts::opened(player);
            debug(&format!("bus: {id} screen on {} of {pos:?} (window {window:?})", SIDE_NAMES[side]));
        }
    }
}

pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    open_pending(host, level, dim);
    let todo: Vec<(Pos, String, u32)> = {
        let mut g = LOOKS.lock().unwrap_or_else(|e| e.into_inner());
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *g).into_iter().partition(|l| l.0 == dim);
        *g = rest;
        here.into_iter().map(|l| (l.1, l.2, l.3)).collect()
    };
    for (pos, block, rot) in todo {
        set_look(host, level, pos, &block, rot);
    }
    removed_drops(host, level, dim);
}

// ---- The cell's tick: each part's logic on its own fields

pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let Some(level) = here(&*host) else { return };
    let dim = host.dimension();
    let c = read_cell(&fields.nbt);
    if c.cable.is_empty() {
        return;
    }
    with_cells(|t| t.insert((dim, pos), c.clone()));
    // [blocks] A cable anchor on a side makes the cell climbable (`misc_blocks::is_ladder`).
    crate::misc_blocks::note_cell(pos, &c.parts);
    let mut looks = Compound::new();
    for s in 0..6 {
        let Some(id) = c.parts[s].clone() else { continue };
        let vpos = hosted(pos, s);
        let mut f = side_fields(fields, s);
        crate::devices::run_device(host, vpos, &id, &mut f);
        put_side(fields, s, f);
        let (_, mut online, mut active) = crate::net::device_state(dim, vpos);
        // [blocks] A toggle bus that is off shows off (ToggleBusPart.getStaticModels).
        if crate::links::is_toggle(&id) && !crate::links::toggle_enabled(dim, vpos) {
            (online, active) = (false, false);
        }
        looks.insert(java(&format!("l{s}")), Tag::String(java(&format!("{}{}", u8::from(online), u8::from(active)))));
    }
    // The arms: a neighbour it joins (as a cable block's), or a part's back.
    let mut arms = 0u32;
    for (s, d) in SIDES.iter().enumerate() {
        if c.parts[s].is_some() || c.facades[s].is_some() {
            continue;
        }
        let q = add(pos, *d);
        let other = crate::net::block_id(&*host, level, q);
        // [blocks] Or a neighbour cell's toggle bus or quartz fiber facing this side (`links`).
        if crate::net::is_network_block(&other) && crate::net::links(&*host, level, pos, &c.cable, q, &other) || crate::links::faces_outer(&*host, level, q, pos) {
            arms |= 1 << s;
        }
    }
    looks.insert(java("arms"), Tag::Long(i64::from(arms)));
    // Each side's channels (CablePart.getChannelsOnSide, 6 bits a side): the smart cable's lights on that arm.
    let ch = crate::net::side_channels_at(dim, pos);
    looks.insert(java("ch"), Tag::Long((0..6).fold(0i64, |m, s| m | i64::from(ch[s].min(63)) << (6 * s))));
    // fixF: what a block entity's tick writes reaches clients only with its block's next change or a sync
    // (`block_entity_tick`: "when its chunk is sent or its block changes"). The looks changed here a tick after the
    // network's rebuild, whose block changes had sent the old ones: each change showed the one before. Sent now.
    if set_looks(&mut fields.nbt, looks) {
        crate::storage::with_store(|s| s.syncs.insert((dim, pos)));
    }
    // A powered illuminated panel (PanelPart.getLightLevel) lights its cell.
    let lit = (0..6).any(|s| {
        c.parts[s].as_deref().is_some_and(|id| matches!(crate::net::ae2_name(id), Some("monitor" | "semi_dark_monitor" | "dark_monitor")))
            && crate::net::device_state(dim, hosted(pos, s)).1
    });
    let (block, rot) = want_state(&c, arms, lit);
    let cur = level.block(&*host, pos).and_then(|b| b.id(&*host).map(str::to_owned)).unwrap_or_default();
    let now: u32 = level.block_property(&*host, pos, "rotation").and_then(|r| r.parse().ok()).unwrap_or(0);
    if cur != block || now != rot {
        LOOKS.lock().unwrap_or_else(|e| e.into_inner()).push((dim, pos, block, rot));
    }
    // What its slots hold, for drops if it is removed some other way (an explosion, a command).
    let slots: Vec<Option<ItemStack>> = fields.items.clone();
    with_kept(|k| {
        if k.get(&(dim, pos)) != Some(&slots) {
            k.insert((dim, pos), slots);
        }
    });
}

/// fixF: the cell's looks (each part's lights, the arms, each side's channels) into its saved compound: whether they
/// changed (and clients need them: `tick` syncs the block entity then).
fn set_looks(nbt: &mut Compound, looks: Compound) -> bool {
    let looks = Tag::Compound(looks);
    if nbt.get(&java("looks")) == Some(&looks) {
        return false;
    }
    nbt.insert(java("looks"), looks);
    true
}

static KEPT: Mutex<Option<HashMap<(&'static str, Pos), Vec<Option<ItemStack>>>>> = Mutex::new(None);

fn with_kept<R>(f: impl FnOnce(&mut HashMap<(&'static str, Pos), Vec<Option<ItemStack>>>) -> R) -> R {
    let mut g = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// Cells gone other than by a player (`block_changed` from a cell to anything else).
static REMOVED: Mutex<Vec<(&'static str, Pos)>> = Mutex::new(Vec::new());

pub(crate) fn note_changed(dim: &'static str, pos: Pos, old: &str, new: &str) {
    if is_cell_block(old) && !is_cell_block(new) {
        REMOVED.lock().unwrap_or_else(|e| e.into_inner()).push((dim, pos));
    }
}

/// A cell removed by an explosion or a command drops its cable, parts (with their slots) and facades, as
/// CableBusBlock's drops; a player's break has taken its cell out of the table already.
fn removed_drops(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str) {
    let todo: Vec<Pos> = {
        let mut g = REMOVED.lock().unwrap_or_else(|e| e.into_inner());
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *g).into_iter().partition(|r| r.0 == dim);
        *g = rest;
        here.into_iter().map(|r| r.1).collect()
    };
    for pos in todo {
        if level.block(&*host, pos).is_some_and(|b| is_cell_block(b.id(&*host).unwrap_or_default())) {
            continue;
        }
        let Some(c) = with_cells(|t| t.remove(&(dim, pos))) else { continue };
        let slots = with_kept(|k| k.remove(&(dim, pos))).unwrap_or_default();
        let mut drops: Vec<ItemStack> = crate::cells::new_stack(&*host, &c.cable, 1).into_iter().collect();
        for s in 0..6 {
            if let Some(id) = &c.parts[s] {
                drops.extend(crate::cells::new_stack(&*host, id, 1));
                drops.extend(slots.iter().skip(s * SLOTS).take(SLOTS).flatten().cloned());
            }
            if let Some(f) = &c.facades[s] {
                drops.extend(facade_item(&*host, f));
            }
        }
        debug(&format!("bus: {pos:?} removed, {} drops", drops.len()));
        for d in drops {
            drop_at(host, level, pos, d);
        }
        crate::net::mark_dirty(dim);
    }
}

/// What clients draw: the cable, its arms, each part with its look, the facades.
pub(crate) fn update_tag(nbt: &Compound) -> Compound {
    let mut out = Compound::new();
    for k in ["cable", "looks"] {
        if let Some(t) = nbt.get(&java(k)) {
            out.insert(java(k), t.clone());
        }
    }
    let c = read_cell(nbt);
    for s in 0..6 {
        if let Some(id) = &c.parts[s] {
            out.insert(java(&part_key(s)), Tag::String(java(id)));
            // [blocks] A monitor's screen (`monitors::update_tag`).
            if crate::monitors::is_monitor(id)
                && let Some(Tag::Compound(p)) = nbt.get(&java(&part_key(s)))
                && let Some(m) = crate::monitors::update_tag(p)
            {
                out.insert(java(&format!("m{s}")), Tag::Compound(m));
            }
        }
        if let Some(f) = &c.facades[s] {
            out.insert(java(&format!("f{s}")), Tag::String(java(f)));
        }
    }
    out
}

// ---- Placing parts and facades (PartPlacement, FacadeItem)

/// A part or a facade used on a cable or a cell goes onto the clicked side, if that side is free. Otherwise the old
/// way (a block of its own beside).
pub(crate) fn place(host: &mut dyn ServerHost, stack: &ItemStack, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let level = here(&*host)?;
    let player = host.player(click.player)?;
    let e = player.entity();
    if e.is_shift_key_down(&*host).unwrap_or(false) || e.is_crouching(&*host).unwrap_or(false) {
        return None;
    }
    let item = crate::cells::sid(stack).to_owned();
    let facade = if item == "ae2:facade" {
        let block = crate::cells::custom_data(stack).and_then(|d| str_of(d.get(&java("item"))))?;
        host.block_state_id(&block)?;
        Some(block)
    } else {
        None
    };
    if facade.is_none() && !hostable(&item) {
        return None;
    }
    let pos = click.pos;
    let side = side_of(click.hit.face)?;
    let id = level.block(&*host, pos).and_then(|b| b.id(&*host).map(str::to_owned)).unwrap_or_default();
    let mut c = if is_cell_block(&id) {
        cell(&*host, level, pos)?
    } else if crate::net::cable(&id).is_some() {
        Cell { cable: id.clone(), ..Cell::default() }
    } else {
        return None;
    };
    // A click on a part's own face or a facade: that side is taken (a facade only needs no facade there).
    match &facade {
        Some(_) if c.facades[side].is_some() => return None,
        None if c.parts[side].is_some() => return None,
        _ => {}
    }
    match &facade {
        Some(f) => c.facades[side] = Some(f.clone()),
        None => c.parts[side] = Some(item.clone()),
    }
    let core = core_of(&c.cable);
    if !is_cell_block(&id) {
        // The cable becomes a cell (its arms worked out at its first tick).
        let state = host.block_state_id(&twin_state(core, look_of(0, &c)))?;
        let flags = BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS | BlockUpdateFlags::SUPPRESS_DROPS;
        if !matches!(level.set_block(host, pos, state, flags), Ok(true)) {
            return None;
        }
    }
    let be = host.block_entity(level, pos)?;
    let mut nbt = be.data(&*host).unwrap_or_default();
    write_cell(&mut nbt, &c);
    // A hosted annihilation plane keeps its item's enchantments on its own part's nbt.
    if item == "ae2:annihilation_plane"
        && let Some(comps) = crate::planes::components_of(&*host, stack)
        && let Some(Tag::Compound(mut p)) = nbt.get(&java(&part_key(side))).cloned()
    {
        let mut own = match p.get(&java("nbt")) {
            Some(Tag::Compound(n)) => n.clone(),
            _ => Compound::new(),
        };
        own.insert(java("hz_item_components"), Tag::Compound(comps));
        p.insert(java("nbt"), Tag::Compound(own));
        nbt.insert(java(&part_key(side)), Tag::Compound(p));
    }
    let _ = be.replace_data(host, nbt);
    let dim = host.dimension();
    with_cells(|t| t.insert((dim, pos), c.clone()));
    if !player.is_creative(&*host).unwrap_or(false) {
        let left = stack.count().saturating_sub(1);
        let _ = LivingRef::new(e).set_hand_item(host, click.hand, if left == 0 { None } else { crate::cells::with_count(stack, left) });
    }
    crate::net::mark_dirty(dim);
    // polish: PartPlacement's place sound.
    crate::effects::part_placed(host, pos);
    debug(&format!("bus: {item} on {} of {pos:?}{}", SIDE_NAMES[side], facade.map(|f| format!(" ({f})")).unwrap_or_default()));
    Some(InteractionResult::SuccessServer)
}

/// fixC: a part's own block placed against a block (PartPlacement.getPartPlacement: the part goes on the side of the
/// new spot toward the clicked block, so it faces it). The block's template (the shulker box) faces it out of the
/// clicked face instead: a storage bus on a barrel's top had its back on the barrel and its front in the air.
/// - Buses, planes, tunnels, toggle buses and the quartz fiber lie on their front (`model_near` 0): they now face the
///   clicked block, their model against it, their back (where a cable joins) away from it.
/// - Displays (terminals, monitors, level emitters, the cable interface, pattern provider and energy acceptor, the
///   cable anchor) lie on their back. fixG: they face the clicked block too (a pattern provider pushes into it, as in
///   AE2), with `x_front` set: their blockstate then draws the model against that block, its screen to the player
///   (the back-lying model turned as for the opposite facing). Blocks placed before keep `x_front` false and the old
///   meaning (`facing` the screen's way, the back on the block), so they look and work as they did.
/// - Against a cable (sneaking: a part used on a cable otherwise goes onto its side, `place`): the template's facing,
///   its back on the cable, as before.
///
/// None leaves the block's own placement.
pub(crate) fn part_placement(level: &dyn LevelHost, event: &StateForPlacementEvent<'_>) -> Option<Option<BlockState>> {
    let block = level.state_block(event.state)?.to_string();
    if !hostable(&block) || block == "ae2:cable_bus" {
        return None;
    }
    // fixG: a back-lying part faces the block too, front-mounted (`x_front`, make_fork_content.py FRONT_MOUNTED).
    let front_mounted = model_near(&block) != 0.0;
    let cx = event.context;
    let (out, toward) = match cx.face {
        Direction::Down => ([0, -1, 0], "up"),
        Direction::Up => ([0, 1, 0], "down"),
        Direction::North => ([0, 0, -1], "south"),
        Direction::South => ([0, 0, 1], "north"),
        Direction::West => ([-1, 0, 0], "east"),
        _ => ([1, 0, 0], "west"),
    };
    let clicked = [cx.pos.x - out[0], cx.pos.y - out[1], cx.pos.z - out[2]];
    if let Some(on) = level.block_state(clicked).and_then(|s| level.state_block(s)).map(|b| b.to_string())
        && (crate::net::cable(&on).is_some() || is_cell_block(&on))
    {
        return None;
    }
    let state = level.state_with_property(event.state, "facing", toward)?;
    if front_mounted {
        return level.state_with_property(state, "x_front", "true").map(Some);
    }
    Some(Some(state))
}

/// A hosted part tunnel turned into another kind (its settings kept).
pub(crate) fn retype(host: &mut dyn ServerHost, level: LevelRef, vpos: Pos, to: &str) {
    let (pos, side) = host_cell(vpos);
    let Some(mut c) = cell(&*host, level, pos) else { return };
    c.parts[side] = Some(to.to_owned());
    let Some(be) = host.block_entity(level, pos) else { return };
    let mut nbt = be.data(&*host).unwrap_or_default();
    write_cell(&mut nbt, &c);
    let _ = be.replace_data(host, nbt);
    let dim = host.dimension();
    with_cells(|t| t.insert((dim, pos), c));
}

/// [tools] The colour applicator on a cell (CableBusBlock.recolorBlock): its cable becomes `to` (another colour of the
/// same kind). True when it changed.
pub(crate) fn recolor_cable(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, to: &str) -> bool {
    let Some(mut c) = cell(&*host, level, pos) else { return false };
    if c.cable == to || crate::net::cable(to).is_none() {
        return false;
    }
    c.cable = to.to_owned();
    let Some(be) = host.block_entity(level, pos) else { return false };
    let mut nbt = be.data(&*host).unwrap_or_default();
    write_cell(&mut nbt, &c);
    if be.replace_data(host, nbt).is_err() {
        return false;
    }
    let dim = host.dimension();
    with_cells(|t| t.insert((dim, pos), c));
    crate::net::mark_dirty(dim);
    true
}

// ---- Breaking (CableBusBlock.onDestroyedByPlayer): what the crosshair is on

fn drop_at(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, stack: ItemStack) {
    let _ = level.spawn_item(host, f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5, stack, 10);
}

/// The item a facade is.
fn facade_item(host: &dyn Host, block: &str) -> Option<ItemStack> {
    let mut s = crate::cells::new_stack(host, "ae2:facade", 1)?;
    crate::cells::edit_custom_data(&mut s, |d| {
        d.insert(java("item"), Tag::String(java(block)));
    });
    Some(s)
}

/// What a part leaves: itself and what its slots hold (its cards, view cells, stored items).
fn part_drops(host: &dyn Host, level: LevelRef, pos: Pos, side: usize, id: &str) -> Vec<ItemStack> {
    let mut out: Vec<ItemStack> = crate::cells::new_stack(host, id, 1).into_iter().collect();
    if let Some(be) = host.block_entity(level, pos) {
        out.extend((0..SLOTS).filter_map(|i| be.item_slot(host, (side * SLOTS + i) as u32).ok().flatten()));
    }
    out
}

/// `break_block` on a cell: the part or facade aimed at goes (into drops), the cell stays; the cable takes the rest.
/// True when the break is the mod's (the block stays).
pub(crate) fn break_cell(host: &mut dyn ServerHost, pos: Pos) -> bool {
    let Some(level) = here(&*host) else { return false };
    let Some(c) = cell(&*host, level, pos) else { return false };
    let Some(player) = host.players().acting_player() else { return false };
    let e = player.entity();
    let (Some(eye), Some(look)) = (e.eye_position(&*host), e.view_vector(&*host)) else { return false };
    let creative = player.is_creative(&*host).unwrap_or(false);
    let arms = host.block_entity(level, pos).and_then(|b| b.data(&*host)).and_then(|n| match n.get(&java("looks")) {
        Some(Tag::Compound(l)) => Some(nbt_i64(l, "arms") as u32),
        _ => None,
    });
    let o = [eye[0] - f64::from(pos[0]), eye[1] - f64::from(pos[1]), eye[2] - f64::from(pos[2])];
    let ray = pick_ray(&c, arms.unwrap_or(0), o, look).unwrap_or(12);
    // The player's own pick (their outline), when it names something on this cell; else the server's ray.
    let told = AIMS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|a| a.get(&player.uuid()).copied());
    let what = match told {
        Some((p, w)) if p == pos && (w == 12 || (w < 6 && c.parts[w].is_some()) || (6..12).contains(&w) && c.facades[w - 6].is_some()) => w,
        _ => ray,
    };
    take(host, level, pos, &c, what, None, creative)
}

/// [blocks] The part a punch on `pos` (through `face`) is on: the acting player and the side, by the player's told aim
/// (their outline), else the server's ray, else the punched face.
pub(crate) fn punched_part(host: &dyn ServerHost, level: LevelRef, pos: Pos, face: Pos) -> Option<(u128, usize)> {
    let c = cell(host, level, pos)?;
    let player = host.players().acting_player()?;
    let told = AIMS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|a| a.get(&player.uuid()).copied());
    if let Some((p, w)) = told
        && p == pos
        && w < 6
        && c.parts[w].is_some()
    {
        return Some((player.uuid(), w));
    }
    let e = player.entity();
    if let (Some(eye), Some(look)) = (e.eye_position(host), e.view_vector(host)) {
        let o = [eye[0] - f64::from(pos[0]), eye[1] - f64::from(pos[1]), eye[2] - f64::from(pos[2])];
        if let Some(w) = pick_ray(&c, 0, o, look).filter(|w| *w < 6) {
            return Some((player.uuid(), w));
        }
    }
    let side = side_of(face)?;
    c.parts[side].as_ref()?;
    Some((player.uuid(), side))
}

/// Takes `what` off a cell (a part's side, a facade's `6 + side`, the cable 12): its drops at the cell or, for a
/// wrench, at the player's feet. True when the cell stays (the break is the mod's).
fn take(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, c: &Cell, what: usize, to: Option<[f64; 3]>, creative: bool) -> bool {
    let dim = host.dimension();
    let drop = |host: &mut dyn ServerHost, d: ItemStack| match to {
        Some(p) => {
            let _ = level.spawn_item(host, p[0], p[1], p[2], d, 0);
        }
        None => drop_at(host, level, pos, d),
    };
    let mut drops = Vec::new();
    let mut c2 = c.clone();
    if what < 6 {
        let id = c.parts[what].clone().unwrap_or_default();
        drops = part_drops(&*host, level, pos, what, &id);
        c2.parts[what] = None;
        // Its slots are emptied with it.
        if let Some(be) = host.block_entity(level, pos) {
            for i in 0..SLOTS {
                let _ = host.transaction(|host, tx| be.set_item_slot(host, tx, (what * SLOTS + i) as u32, None).map_err(|_| ()));
            }
        }
        debug(&format!("bus: broke {id} on {} of {pos:?}", SIDE_NAMES[what]));
    } else if what < 12 {
        let f = c.facades[what - 6].clone().unwrap_or_default();
        drops.extend(facade_item(&*host, &f));
        c2.facades[what - 6] = None;
        debug(&format!("bus: broke facade {f} on {} of {pos:?}", SIDE_NAMES[what - 6]));
    } else {
        // The cable: everything goes.
        drops.extend(crate::cells::new_stack(&*host, &c.cable, 1));
        for s in 0..6 {
            if let Some(id) = &c.parts[s] {
                drops.extend(part_drops(&*host, level, pos, s, id));
            }
            if let Some(f) = &c.facades[s] {
                drops.extend(facade_item(&*host, f));
            }
        }
        with_cells(|t| t.remove(&(dim, pos)));
        debug(&format!("bus: broke the cable of {pos:?}"));
        if !creative {
            for d in drops {
                drop(host, d);
            }
        }
        crate::net::mark_dirty(dim);
        if to.is_some() {
            // A wrench takes the cable too: the block goes.
            let flags = BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS | BlockUpdateFlags::SUPPRESS_DROPS;
            if let Some(air) = host.block_state_id("minecraft:air") {
                let _ = level.set_block(host, pos, air, flags);
            }
        }
        return false;
    }
    if !creative {
        for d in drops {
            drop(host, d);
        }
    }
    let empty = c2.parts.iter().all(Option::is_none) && c2.facades.iter().all(Option::is_none);
    if empty {
        // Nothing left on it: a plain cable again.
        let Some(state) = host.block_state_id(&c2.cable) else { return true };
        with_cells(|t| t.remove(&(dim, pos)));
        let flags = BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS | BlockUpdateFlags::SUPPRESS_DROPS;
        let _ = level.set_block(host, pos, state, flags);
    } else if let Some(be) = host.block_entity(level, pos) {
        let mut nbt = be.data(&*host).unwrap_or_default();
        write_cell(&mut nbt, &c2);
        let _ = be.replace_data(host, nbt);
        with_cells(|t| t.insert((dim, pos), c2));
    }
    crate::net::mark_dirty(dim);
    true
}

// ---- Screens: the cell is each part's menu holder

/// Each player's open part: its cell and side (the server's), and the last opened cell's parts (slot rules, which
/// are asked without a player).
#[derive(Default)]
pub(crate) struct Open {
    players: HashMap<u128, (Pos, usize)>,
    last: [Option<String>; 6],
    last_side: Option<usize>,
    /// The client's open part: side and id.
    client: Option<(usize, String)>,
}

static OPEN: Mutex<Option<Open>> = Mutex::new(None);

fn with_open<R>(f: impl FnOnce(&mut Open) -> R) -> R {
    let mut g = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Open::default))
}

/// A use of a cell: the aimed part's screen. None to fall through (no part there, or none with a screen).
pub(crate) fn use_cell(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let level = here(&*host)?;
    let pos = click.pos;
    let Some(c) = cell(&*host, level, pos) else {
        debug(&format!("bus: use_cell {pos:?}: no cell"));
        return None;
    };
    let at = rel_point(pos, click.hit.point);
    let eye = host
        .player(click.player)
        .and_then(|p| p.entity().eye_position(&*host))
        .map(|e| [e[0] - f64::from(pos[0]), e[1] - f64::from(pos[1]), e[2] - f64::from(pos[2])]);
    let picked = pick_click(&c, at, click.hit.face, eye);
    debug(&format!(
        "bus: use_cell {pos:?} point {:?} rel {at:?} face {:?} eye {eye:?} stack {:?} -> side {picked:?} part {:?}",
        click.hit.point,
        click.hit.face,
        click.stack.map(|s| (crate::cells::sid(s).to_owned(), s.count())),
        picked.filter(|s| *s < 6).and_then(|s| c.parts[s].clone()),
    ));
    let side = picked.filter(|s| *s < 6)?;
    let id = c.parts[side].clone()?;
    // [blocks] Storage and conversion monitors take the click themselves (`monitors`).
    if crate::monitors::is_monitor(&id) {
        let r = crate::monitors::use_part(host, click, hosted(pos, side), &id);
        debug(&format!("bus: use_cell {pos:?} monitor {id} on {side} -> {r:?}"));
        return r;
    }
    let screen = crate::devices::MENU_BLOCKS.contains(&id.as_str()) || crate::parts::has_screen(&id) || crate::autocraft::has_screen(&id);
    if !screen {
        return Some(InteractionResult::Pass);
    }
    let player = host.player(click.player)?;
    let e = player.entity();
    if e.is_shift_key_down(&*host).unwrap_or(false) || e.is_crouching(&*host).unwrap_or(false) {
        return None;
    }
    crate::cells::ids(&*host);
    let entity = host.block_entity(level, pos)?;
    with_open(|o| {
        o.players.insert(click.player, (pos, side));
        o.last = c.parts.clone();
        o.last_side = Some(side);
    });
    // The client learns which part first; the menu opens at the tick's end (a LAN guest gets them in that order).
    let _ = entity;
    let _ = crate::util::send_to_player(host, click.player, NET_BUS, format!("open {side} {id}").as_bytes());
    let dim = host.dimension();
    {
        // One open per click: a second use in the same tick (both hands, or the same click heard twice in one
        // process) would open the menu again and leave the client a window behind.
        let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        if !g.iter().any(|p| p.0 == dim && p.1 == click.player && p.2 == pos) {
            g.push((dim, click.player, pos, side, id));
        }
    }
    Some(InteractionResult::SuccessServer)
}

/// The part a player's cable bus menu shows: its node, id and side.
pub(crate) fn opened_part(host: &dyn Host, player: u128, pos: Pos) -> Option<(Pos, String, usize)> {
    let (cpos, side) = with_open(|o| o.players.get(&player).copied())?;
    if cpos != pos {
        return None;
    }
    let level = here(host)?;
    let id = cell(host, level, pos)?.parts[side].clone()?;
    Some((hosted(pos, side), id, side))
}

/// The menu a hosted part's slots follow (`menu_slot_rules` on `ae2:cable_bus`): its own, at its own index. The
/// part is the one on that side of the menu's cell (`MenuSlotContext::pos`, the block entity read through the host,
/// on both sides); without a position, the part this side last opened.
pub(crate) fn slot_rules(host: &dyn Host, pos: Option<Pos>, slot: usize, f: impl FnOnce(&str, usize)) -> bool {
    let side = slot / SLOTS;
    let at_cell = pos.and_then(|pos| cell(host, here(host)?, pos)).map(|c| c.parts.get(side).cloned().flatten());
    let part = at_cell.unwrap_or_else(|| {
        with_open(|o| match &o.client {
            Some((s, id)) if *s == side => Some(id.clone()),
            Some(_) => None,
            None if o.last_side == Some(side) => o.last.get(side).cloned().flatten(),
            None => None,
        })
    });
    // The side's slots past its part's own are hidden on its screen (`screen`): none takes anything.
    let own = part.as_deref().and_then(crate::Mod::container_of).map_or(0, |c| c.slots);
    match part.as_deref().and_then(menu_name) {
        Some(menu) if slot % SLOTS < own => {
            f(menu, slot % SLOTS);
            true
        }
        _ => false,
    }
}

/// Whether `slot` of a cell's menu is on the side `player` opened (`Open::players`, the server's). The screen hides
/// the other sides' slots as it opens; the server refuses them (`devices::Menus::menu_slot_rules`) as its menu had
/// none of them while the layout hid them on both sides. Unknown (no player, no record): no refusal of its own.
pub(crate) fn on_open_side(player: Option<u128>, pos: Option<Pos>, slot: usize) -> bool {
    let (Some(player), Some(pos)) = (player, pos) else { return true };
    match with_open(|o| o.players.get(&player).copied()) {
        Some((cpos, side)) if cpos == pos => slot / SLOTS == side,
        _ => true,
    }
}

/// The content menu a part's own block opens.
fn menu_name(id: &str) -> Option<&'static str> {
    if let Some((m, _)) = crate::parts::menu_of(id) {
        return Some(m);
    }
    if crate::autocraft::is_provider(id) {
        return Some("ae2:pattern_provider");
    }
    crate::devices::MENU_BLOCKS.iter().chain(["ae2:pattern_access_terminal"].iter()).find(|m| **m == id).copied()
}

// ---- Client

pub(crate) fn from_server(channel: &str, data: &[u8]) -> bool {
    if channel != NET_BUS {
        return false;
    }
    let text = String::from_utf8_lossy(data);
    if let Some(rest) = text.strip_prefix("open ")
        && let Some((s, id)) = rest.split_once(' ')
        && let Ok(s) = s.parse::<usize>()
        && s < 6
    {
        with_open(|o| o.client = Some((s, id.to_owned())));
    }
    true
}

/// The open screen's menu as its part's own (`ae2:cable_bus` is the open part's).
pub(crate) fn menu<'a>(menu: Option<&'a str>) -> Option<&'a str> {
    if menu == Some(MENU) {
        return with_open(|o| o.client.as_ref().and_then(|(_, id)| menu_name(id)));
    }
    menu
}

/// A screen hook's block and data as the open part's.
pub(crate) fn client_part(block: &str, data: &[i32]) -> Option<(String, Vec<i32>)> {
    if !is_cell_block(block) {
        return None;
    }
    let (side, id) = with_open(|o| o.client.clone())?;
    Some((id, (0..DATA).map(|i| data.get(side * DATA + i).copied().unwrap_or(0)).collect()))
}

/// The cell's screen: its open part's, that part's slots placed and the others hidden.
pub(crate) fn screen(inner: impl FnOnce(&str) -> Option<MenuScreenLayout>) -> Option<MenuScreenLayout> {
    let (side, id) = with_open(|o| o.client.clone())?;
    let mut s = inner(&id)?;
    let mut slots = vec![None; 6 * SLOTS];
    for (j, at) in s.slots.iter().enumerate().take(SLOTS) {
        slots[side * SLOTS + j] = at.clone();
    }
    s.slots = slots;
    Some(s)
}

/// What the client knows of each cell it draws (the outline reads it).
#[derive(Clone, Default)]
pub(crate) struct Drawn {
    cell: Cell,
    arms: u32,
}

static DRAWN: Mutex<Option<HashMap<Pos, Drawn>>> = Mutex::new(None);

fn drawn_of(nbt: &Compound) -> Drawn {
    let mut c = Cell { cable: str_of(nbt.get(&java("cable"))).unwrap_or_default(), ..Cell::default() };
    for s in 0..6 {
        c.parts[s] = str_of(nbt.get(&java(&part_key(s))));
        c.facades[s] = str_of(nbt.get(&java(&format!("f{s}"))));
    }
    let arms = match nbt.get(&java("looks")) {
        Some(Tag::Compound(l)) => nbt_i64(l, "arms") as u32,
        _ => 0,
    };
    Drawn { cell: c, arms }
}

fn block_key(host: &dyn ClientHost, id: &str) -> Option<BlockKey> {
    thread_local! {
        static KEYS: std::cell::RefCell<HashMap<String, Option<BlockKey>>> = std::cell::RefCell::new(HashMap::new());
    }
    KEYS.with_borrow_mut(|k| *k.entry(id.to_owned()).or_insert_with(|| host.block_state_id(id).map(|s| s.block())))
}

/// fixE: whether a part's block has `with_channel` (the buses, terminals and tunnels; not the level emitters, the
/// quartz fiber or the anchor).
fn has_channel_property(host: &dyn ClientHost, id: &str) -> bool {
    thread_local! {
        static HAS: std::cell::RefCell<HashMap<String, bool>> = std::cell::RefCell::new(HashMap::new());
    }
    HAS.with_borrow_mut(|h| {
        *h.entry(id.to_owned()).or_insert_with(|| {
            let states = host.block_states();
            host.block_state_id(id).is_some_and(|s| s.property(&states, "with_channel").ok().flatten().is_some())
        })
    })
}

/// fixE: a cell's piece that didn't draw, logged once each (what the in-game test reads).
fn note_draw_failure(id: &str, why: &str) {
    static SEEN: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);
    if SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(id.to_owned()) {
        debug(&format!("bus draw: {id} not drawn: {why}"));
    }
}

/// CableBusRenderer: the cable with its arms, each part's own model on its side, the facades as plates.
pub(crate) fn submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let d = drawn_of(&event.entity.nbt);
    DRAWN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(event.pos, d.clone());
    // [blocks] The client's climbing reads it too.
    crate::misc_blocks::note_cell(event.pos, &d.cell.parts);
    let draw = &mut *event.draw;
    let looks = match event.entity.nbt.get(&java("looks")) {
        Some(Tag::Compound(l)) => l.clone(),
        _ => Compound::new(),
    };
    // CableBusBakedModel: a part keeps the cable's core (no straight run), and its side gets an arm unless the
    // part has none (CableBusRenderState.attachmentConnections: a cable connection length of 1 to 8; the anchor
    // wants no cable at all).
    let (arm_parts, core_parts) = (0..6).fold((0u32, false), |(m, core), s| match d.cell.parts[s].as_deref().map(|id| crate::net::ae2_name(id).unwrap_or("")) {
        Some("cable_anchor") => (m, core),
        Some("level_emitter" | "energy_level_emitter" | "quartz_fiber") => (m, true),
        Some(_) => (m | 1 << s, true),
        None => (m, core),
    });
    // A dense cable draws no arm to a part (buildCableModel's attachment switch has no dense case): its core reaches it.
    let arm_parts = if cable_kind(&d.cell.cable).is_some_and(|(k, _)| k.starts_with("dense")) { 0 } else { arm_parts };
    // A side that holds a part has no cable arm to a neighbour (its part decides; the quartz fiber's rod is thinner).
    let part_sides = (0..6).fold(0u32, |m, s| if d.cell.parts[s].is_some() { m | 1 << s } else { m });
    let on_mask = ((d.arms & !part_sides) | arm_parts) & 63;
    // fixE: read as any int tag (a long from the server, an int from a guide scene's SNBT).
    let ch = unpack_ch(nbt_i64(&looks, "ch"));
    // The cable's pieces drawn as terrain (cut out, back faces culled), as the cable blocks' own models draw: the
    // lights-off pieces, as a plain cable block's model.
    if !draw_cable(draw, &d.cell.cable, on_mask, core_parts)
        && let Some(k) = block_key(host, &d.cell.cable)
    {
        let mut on: Vec<(&str, &str)> = (0..6).map(|s| (SIDE_NAMES[s], if on_mask >> s & 1 == 1 { "true" } else { "false" })).collect();
        on.push(("x_core", if core_parts { "true" } else { "false" }));
        draw.block(k, &on);
    }
    // fixE: a smart cable's lit lines over them, side by side, from the same tinted overlays and rules as a plain
    // smart cable's block entity (cable_lights: a light a channel, a dense cable's a light each 4, up to 8).
    if let Some(pieces) = bus_light_pieces(&d.cell.cable, on_mask, core_parts, ch) {
        crate::cable_lights::draw_lights(draw, &pieces);
    }
    for s in 0..6 {
        let Some(id) = &d.cell.parts[s] else { continue };
        let l = str_of(looks.get(&java(&format!("l{s}")))).unwrap_or_default();
        let (powered, channel) = (if l.starts_with('1') { "true" } else { "false" }, if l.ends_with("11") { "true" } else { "false" });
        // fixE: the quartz fiber is AE2's rod from the core to this side's face (its own block's model is a rod through
        // a whole block, made for a block of its own).
        if crate::net::ae2_name(id) == Some("quartz_fiber") && draw_piece(draw, &fiber_model_id(s)) {
            continue;
        }
        let Some(k) = block_key(host, id) else { continue };
        // A part's own block is made for a block beside the cable (its back on that block's far face): moved one
        // block less its depth, onto this cell's face.
        let out = model_near(id) / 16.0;
        let mut t = [0.0f32; 3];
        let axis = [1, 1, 2, 2, 0, 0][s];
        t[axis] = (if s % 2 == 1 { out } else { -out }) as f32;
        // fixE: only the properties the part's block has (a level emitter, the quartz fiber and the anchor have no
        // `with_channel`): the game draws nothing for a state naming a property its block lacks.
        let mut props = vec![("facing", SIDE_NAMES[s]), ("powered", powered)];
        if has_channel_property(host, id) {
            props.push(("with_channel", channel));
        }
        draw.push();
        draw.translate(t);
        if !draw.block(k, &props) {
            note_draw_failure(id, "block refused");
        }
        draw.pop();
    }
    // [blocks] The monitors' screens.
    crate::monitors::submit(host, draw, &event.entity.nbt, &d.cell.parts, &looks);
    for s in 0..6 {
        let Some(f) = &d.cell.facades[s] else { continue };
        let Some(k) = block_key(host, f) else { continue };
        let b = facade_box(s);
        draw.push();
        draw.translate([b[0] as f32, b[1] as f32, b[2] as f32]);
        draw.scale([(b[3] - b[0]) as f32, (b[4] - b[1]) as f32, (b[5] - b[2]) as f32]);
        draw.block(k, &[]);
        draw.pop();
    }
}

// ---- The cell's cable, piece by piece (CableBusBakedModel.buildCableModel on the `hz_cable` models)

/// AE2's cable types as the `hz_cable` models name them (tools/make_cables.py), by the block's name part.
const CABLE_KINDS: [(&str, &str); 5] =
    [("_smart_dense", "dense_smart"), ("_covered_dense", "dense_covered"), ("_glass", "glass"), ("_covered", "covered"), ("_smart", "smart")];
const CABLE_COLORS: [&str; 17] = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple", "blue",
    "brown", "green", "red", "black", "fluix",
];
/// Each axis's straight run: its model's axis name, by the lower of its two sides (`SIDES` order).
const STRAIGHT: [(usize, &str); 3] = [(0, "y"), (2, "z"), (4, "x")];

static CABLE_MODELS: Mutex<Option<HashMap<String, ModelRef>>> = Mutex::new(None);

/// A cable block (`ae2:fluix_smart_dense_cable`) as its models' kind and colour.
fn cable_kind(cable: &str) -> Option<(&'static str, &str)> {
    let name = cable.strip_prefix("ae2:")?.strip_suffix("_cable")?;
    CABLE_KINDS.iter().find_map(|(suffix, kind)| Some((*kind, name.strip_suffix(suffix)?))).filter(|(_, c)| CABLE_COLORS.contains(c))
}

/// Every cable piece's model id (the cable bus draws them as standalone models).
pub(crate) fn cable_model_ids() -> Vec<String> {
    let mut out = Vec::new();
    for (_, kind) in CABLE_KINDS {
        let smart = kind.ends_with("smart");
        for color in CABLE_COLORS {
            let pre = format!("ae2:block/hz_cable/{kind}_{color}");
            out.push(format!("{pre}_core"));
            // fixE: the lights-off pieces only; their lit lines are cable_lights' overlays.
            let ch = if smart { "_ch0" } else { "" };
            for (_, axis) in STRAIGHT {
                out.push(format!("{pre}_straight_{axis}{ch}"));
            }
            for side in SIDE_NAMES {
                out.push(format!("{pre}_arm_{side}{ch}"));
            }
        }
    }
    out
}

/// Looks the cable pieces up once a model load is done (`model_baking_completed`).
pub(crate) fn resolve_cable_models(host: &mut dyn ClientHost) {
    let assets = host.render_assets();
    // fixE: the cell's own part pieces (the quartz fiber's rods) too.
    let ids: Vec<String> = cable_model_ids().into_iter().chain(part_model_ids()).collect();
    let table: HashMap<String, ModelRef> = ids
        .iter()
        .filter_map(|id| {
            let model = assets.model(&Identifier::parse(id).ok()?).ok()?;
            Some((id.clone(), model))
        })
        .collect();
    // fixE: which pieces the game didn't bake (the in-game test reads it).
    let missing: Vec<&String> = ids.iter().filter(|id| !table.contains_key(*id)).take(8).collect();
    debug(&format!("bus pieces: {} of {} baked; missing {missing:?}", table.len(), ids.len()));
    *CABLE_MODELS.lock().unwrap_or_else(|e| e.into_inner()) = Some(table);
}

/// The pieces of `cable` with arms on the `on` sides: one straight run (two opposite arms of its own, no part), or
/// the core and an arm each. fixE: a smart cable's with its lights off (`_ch0`, its block model's): the lit lines are
/// `bus_light_pieces`, drawn over them.
fn cable_pieces(cable: &str, on: u32, core: bool) -> Option<Vec<String>> {
    let (kind, color) = cable_kind(cable)?;
    let pre = format!("ae2:block/hz_cable/{kind}_{color}");
    let ch = if kind.ends_with("smart") { "_ch0" } else { "" };
    if !core
        && let Some((_, axis)) = STRAIGHT.iter().find(|(lo, _)| on == 3 << lo)
    {
        return Some(vec![format!("{pre}_straight_{axis}{ch}")]);
    }
    let mut out = vec![format!("{pre}_core")];
    out.extend((0..6).filter(|s| on >> s & 1 == 1).map(|s| format!("{pre}_arm_{}{ch}", SIDE_NAMES[s])));
    Some(out)
}

/// fixE: each side's channels (`looks.ch`, 6 bits a side).
fn unpack_ch(v: i64) -> [u32; 6] {
    std::array::from_fn(|s| (v >> (6 * s) & 63) as u32)
}

/// fixE: a smart cable's lit overlays in a cell, as a plain smart cable block's (`cable_lights::light_pieces` on the
/// same arms and straight run as `cable_pieces`); None for a cable that isn't smart.
fn bus_light_pieces(cable: &str, on: u32, core: bool, ch: [u32; 6]) -> Option<Vec<(String, u32)>> {
    let (kind, color) = crate::cable_lights::smart(cable)?;
    let lights = ch.map(|c| crate::cable_lights::lights_of(kind, c));
    Some(crate::cable_lights::light_pieces(kind, color, on, core, lights))
}

/// Draws the cell's cable from its pieces; false when they aren't loaded (the caller draws the block's model).
fn draw_cable(draw: &mut dyn BlockEntityDrawContext, cable: &str, on: u32, core: bool) -> bool {
    let Some(pieces) = cable_pieces(cable, on, core) else { return false };
    let models: Vec<ModelRef> = {
        let table = CABLE_MODELS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(table) = table.as_ref() else { return false };
        match pieces.iter().map(|p| table.get(p).copied()).collect::<Option<Vec<_>>>() {
            Some(m) => m,
            None => {
                note_draw_failure(cable, "a piece isn't baked");
                return false;
            }
        }
    };
    let options = TerrainModelOptions::default().with_material(TerrainMaterial::Cutout);
    for m in models {
        if let Err(e) = draw.terrain_model(m, options) {
            note_draw_failure(cable, &format!("{e:?}"));
            return false;
        }
    }
    true
}

/// fixE: the cell's own part pieces drawn as standalone models: the quartz fiber's rod on each side
/// (`hz_part/quartz_fiber_bus_<side>`, tools/make_cables.py `fiber_models`).
pub(crate) fn part_model_ids() -> Vec<String> {
    (0..6).map(fiber_model_id).collect()
}

fn fiber_model_id(side: usize) -> String {
    format!("ae2:block/hz_part/quartz_fiber_bus_{}", SIDE_NAMES[side % 6])
}

/// fixE: one standalone piece (`part_model_ids`) drawn cut out; false when it isn't baked or was refused.
fn draw_piece(draw: &mut dyn BlockEntityDrawContext, id: &str) -> bool {
    let model = CABLE_MODELS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|t| t.get(id).copied());
    let Some(m) = model else {
        note_draw_failure(id, "not baked");
        return false;
    };
    match draw.terrain_model(m, TerrainModelOptions::default().with_material(TerrainMaterial::Cutout)) {
        Ok(()) => true,
        Err(e) => {
            note_draw_failure(id, &format!("{e:?}"));
            false
        }
    }
}

#[cfg(test)]
mod cable_tests {
    use super::*;

    #[test]
    fn a_straight_run_is_one_piece_and_a_part_keeps_the_core() {
        let none = [0; 6];
        assert_eq!(unpack_ch(0), none);
        assert_eq!(cable_pieces("ae2:fluix_glass_cable", 0b110000, false).unwrap(), ["ae2:block/hz_cable/glass_fluix_straight_x"]);
        assert_eq!(
            cable_pieces("ae2:fluix_glass_cable", 0b111000, true).unwrap(),
            ["ae2:block/hz_cable/glass_fluix_core", "ae2:block/hz_cable/glass_fluix_arm_south", "ae2:block/hz_cable/glass_fluix_arm_west", "ae2:block/hz_cable/glass_fluix_arm_east"]
        );
    }

    #[test]
    fn new_looks_are_synced_and_drawn_from_the_update_tag() {
        // fixF: the cell of the bug: a lime smart cable with an export bus on top, ch [0,1,0,0,3,2].
        let looks = |ch: [i64; 6]| {
            let mut l = Compound::new();
            l.insert(java("l1"), Tag::String(java("11")));
            l.insert(java("arms"), Tag::Long(0b111100));
            l.insert(java("ch"), Tag::Long((0..6).fold(0i64, |m, s| m | ch[s] << (6 * s))));
            l
        };
        let mut nbt = Compound::new();
        nbt.insert(java("cable"), Tag::String(java("ae2:lime_smart_cable")));
        // A world loaded before the network: no channels yet. Then the rebuild's counts: a change, synced.
        assert!(set_looks(&mut nbt, looks([0; 6])));
        assert!(set_looks(&mut nbt, looks([0, 1, 0, 0, 3, 2])));
        // The same again (every tick after): nothing to send.
        assert!(!set_looks(&mut nbt, looks([0, 1, 0, 0, 3, 2])));
        // What the client gets and draws from is the newest.
        let tag = update_tag(&nbt);
        let Some(Tag::Compound(sent)) = tag.get(&java("looks")) else { panic!("no looks sent") };
        assert_eq!(unpack_ch(nbt_i64(sent, "ch")), [0, 1, 0, 0, 3, 2]);
        // One channel more on the east arm: a change again.
        assert!(set_looks(&mut nbt, looks([0, 1, 0, 0, 3, 3])));
    }

    #[test]
    fn smart_arms_light_their_own_side() {
        // fixE: the pieces have their lights off; each side's lit lines are cable_lights' tinted overlays, as a plain
        // smart cable's.
        let ch = [0, 0, 12, 3, 0, 0];
        assert_eq!(
            cable_pieces("ae2:red_smart_cable", 0b001100, true).unwrap(),
            ["ae2:block/hz_cable/smart_red_core", "ae2:block/hz_cable/smart_red_arm_north_ch0", "ae2:block/hz_cable/smart_red_arm_south_ch0"]
        );
        let (red_dark, red_light) = (10813481, 16770797);
        assert_eq!(
            bus_light_pieces("ae2:red_smart_cable", 0b001100, true, ch).unwrap(),
            [
                ("ae2:block/hz_cable/lit/smart_arm_north_04".to_owned(), red_dark),
                ("ae2:block/hz_cable/lit/smart_arm_north_14".to_owned(), red_light),
                ("ae2:block/hz_cable/lit/smart_arm_south_03".to_owned(), red_dark),
            ]
        );
        assert_eq!(cable_pieces("ae2:red_smart_dense_cable", 0b001100, false).unwrap(), ["ae2:block/hz_cable/dense_smart_red_straight_z_ch0"]);
        // A dense cable: a light for each 4 channels (13 is 4 lights), up to 8.
        assert_eq!(
            bus_light_pieces("ae2:red_smart_dense_cable", 0b001100, false, [0, 0, 13, 13, 0, 0]).unwrap(),
            [("ae2:block/hz_cable/lit/dense_smart_straight_z_04".to_owned(), red_dark)]
        );
        assert_eq!(
            bus_light_pieces("ae2:red_smart_dense_cable", 0b001100, true, [0, 0, 32, 1, 0, 0]).unwrap(),
            [
                ("ae2:block/hz_cable/lit/dense_smart_arm_north_04".to_owned(), red_dark),
                ("ae2:block/hz_cable/lit/dense_smart_arm_north_14".to_owned(), red_light),
                ("ae2:block/hz_cable/lit/dense_smart_arm_south_01".to_owned(), red_dark),
            ]
        );
        // The in-game cell: a smart cable with an export bus, ch [0,1,0,0,3,2] (looks.ch packs 6 bits a side).
        let packed = (0..6).fold(0i64, |m, s| m | i64::from([0u32, 1, 0, 0, 3, 2][s]) << (6 * s));
        assert_eq!(unpack_ch(packed), [0, 1, 0, 0, 3, 2]);
        assert_eq!(bus_light_pieces("ae2:red_smart_cable", 0b110010, true, unpack_ch(packed)).unwrap().len(), 3);
        assert!(bus_light_pieces("ae2:red_covered_cable", 0b001100, false, ch).is_none());
        assert_eq!(cable_kind("ae2:light_blue_covered_dense_cable"), Some(("dense_covered", "light_blue")));
    }

    #[test]
    fn every_piece_id_is_a_generated_model() {
        let ids: Vec<String> = cable_model_ids().into_iter().chain(part_model_ids()).collect();
        // fixE: 5 kinds x 17 colours, a core and 9 pieces each (a smart one's lights off), and 6 fiber rods.
        assert_eq!(ids.len(), 5 * 17 * 10 + 6);
        let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ae2/models");
        for id in ids {
            let path = models.join(format!("{}.json", id.strip_prefix("ae2:").unwrap()));
            assert!(path.exists(), "{id}");
        }
    }
}

/// How far in from its front a part's own block model starts (the `hz_part` models, facing north: their least z).
/// Displays lie against their block's back (beside the cable), buses, planes and tunnels on its front; on a cell the
/// model moves out by that much, onto the cell's face.
fn model_near(id: &str) -> f64 {
    match crate::net::ae2_name(id).unwrap_or("") {
        "terminal" | "crafting_terminal" | "pattern_encoding_terminal" | "pattern_access_terminal" | "storage_monitor"
        | "conversion_monitor" | "monitor" | "semi_dark_monitor" | "dark_monitor" => 13.0,
        "level_emitter" | "energy_level_emitter" => 11.0,
        "interface" | "energy_acceptor" | "pattern_provider" => 12.0,
        "cable_anchor" => 10.0,
        _ => 0.0,
    }
}

fn part_mask(c: &Cell) -> u32 {
    (0..6).filter(|s| c.parts[*s].is_some()).fold(0, |m, s| m | 1 << s)
}

/// The block outline: none of the game's on a cell (`block_highlight`); the mod draws the aimed part's own.
pub(crate) fn is_drawn_cell(pos: Pos) -> bool {
    DRAWN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|t| t.contains_key(&pos))
}

/// The aimed part's (or facade's, or the cable's) boxes, in world space.
pub(crate) fn aimed_boxes(host: &dyn ClientHost) -> Option<(Pos, usize, Vec<[f64; 6]>)> {
    let CrosshairTarget::Block { pos, hit, .. } = host.crosshair_target()? else { return None };
    let pos = [pos.x, pos.y, pos.z];
    let d = DRAWN.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&pos)?.clone();
    let level = host.level(host.dimension())?;
    if !level.block(host, pos).is_some_and(|b| is_cell_block(b.id(host).unwrap_or_default())) {
        DRAWN.lock().unwrap_or_else(|e| e.into_inner()).as_mut().map(|t| t.remove(&pos));
        return None;
    }
    let at = [hit.point[0] - f64::from(pos[0]), hit.point[1] - f64::from(pos[1]), hit.point[2] - f64::from(pos[2])];
    let what = pick_point(&d.cell, at)?;
    let boxes = match what {
        s if s < 6 => part_boxes(d.cell.parts[s].as_deref().unwrap_or(""), s),
        s if s < 12 => vec![facade_box(s - 6)],
        _ => cable_boxes(core_of(&d.cell.cable), d.arms | part_mask(&d.cell)),
    };
    let o = pos.map(f64::from);
    Some((pos, what, boxes.into_iter().map(|b| [b[0] + o[0], b[1] + o[1], b[2] + o[2], b[3] + o[0], b[4] + o[1], b[5] + o[2]]).collect()))
}

// polish: what the crosshair is on in a drawn cell (Jade's PartHostTooltips.getName): the cell, then the part's id, a
// facade's block (`true`) or the cable's id.
pub(crate) fn aimed_thing(host: &dyn ClientHost) -> Option<(Pos, String, bool)> {
    let CrosshairTarget::Block { pos, hit, .. } = host.crosshair_target()? else { return None };
    let pos = [pos.x, pos.y, pos.z];
    let d = DRAWN.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&pos)?.clone();
    let at = [hit.point[0] - f64::from(pos[0]), hit.point[1] - f64::from(pos[1]), hit.point[2] - f64::from(pos[2])];
    match pick_point(&d.cell, at)? {
        s if s < 6 => Some((pos, d.cell.parts[s].clone()?, false)),
        s if s < 12 => Some((pos, d.cell.facades[s - 6].clone()?, true)),
        _ => Some((pos, d.cell.cable.clone(), false)),
    }
}

/// The aim to tell the server (sent from the client's tick).
static AIM_OUT: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    if let Some(m) = AIM_OUT.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = crate::util::send_to_server(host, NET_BUS, m.as_bytes());
    }
}

/// Each player's aim on a cell (what their outline is on): which of the cell's things a break takes. Vanilla's break
/// carries no hit point; the client's pick is checked against the cell (it must be there) and the player's reach.
static AIMS: Mutex<Option<HashMap<u128, (Pos, usize)>>> = Mutex::new(None);

pub(crate) fn from_client(host: &mut dyn ServerHost, player: u128, data: &[u8]) {
    if data.len() > 64 {
        return;
    }
    let text = String::from_utf8_lossy(data);
    let mut aims = AIMS.lock().unwrap_or_else(|e| e.into_inner());
    let aims = aims.get_or_insert_with(HashMap::new);
    let f: Vec<i64> = text.strip_prefix("aim ").unwrap_or("").split(' ').filter_map(|x| x.parse().ok()).collect();
    if f.len() != 4 || !(0..13).contains(&f[3]) {
        aims.remove(&player);
        return;
    }
    let pos = [f[0] as i32, f[1] as i32, f[2] as i32];
    let near = host.player(player).and_then(|p| p.entity().eye_position(&*host)).is_some_and(|e| {
        (0..3).map(|i| (e[i] - f64::from(pos[i]) - 0.5).powi(2)).sum::<f64>() < 8.0 * 8.0
    });
    if near {
        aims.insert(player, (pos, f[3] as usize));
    }
}

pub(crate) fn draw_outline(host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
    if event.stage != RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let aim = aimed_boxes(host);
    // What the outline is on, for the tests (a line each time it changes).
    thread_local! {
        static LAST: std::cell::Cell<Option<(Pos, usize)>> = const { std::cell::Cell::new(None) };
    }
    let now = aim.as_ref().map(|a| (a.0, a.1));
    if LAST.get() != now {
        LAST.set(now);
        *AIM_OUT.lock().unwrap_or_else(|e| e.into_inner()) = Some(now.map_or_else(|| "aim -".to_owned(), |(p, w)| format!("aim {} {} {} {w}", p[0], p[1], p[2])));
        if let Some((p, w)) = now {
            debug(&format!("bus aim {p:?} {}", if w < 6 { format!("part {}", SIDE_NAMES[w]) } else if w < 12 { format!("facade {}", SIDE_NAMES[w - 6]) } else { "cable".to_owned() }));
        }
    }
    let Some((_, _, boxes)) = aim else { return };
    let mut lines = Vec::new();
    for b in boxes {
        let (x, y, z, x1, y1, z1) = (b[0] - 0.002, b[1] - 0.002, b[2] - 0.002, b[3] + 0.002, b[4] + 0.002, b[5] + 0.002);
        let c = [[x, y, z], [x1, y, z], [x1, y, z1], [x, y, z1], [x, y1, z], [x1, y1, z], [x1, y1, z1], [x, y1, z1]];
        for (i, j) in [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
            lines.push(WorldLine::new(c[i], c[j], 0x6600_0000, 2.0, WorldDepth::Tested));
        }
    }
    let _ = event.draw.lines(&lines);
}

/// A quick move in a cable bus menu: the acting player's open part (node, side, its own slot count).
pub(crate) fn quick_move_part(host: &dyn ServerHost, pos: Pos, entity: &Fields) -> Option<(Pos, usize, usize)> {
    let player = host.players().acting_player()?.uuid();
    let (cpos, side) = with_open(|o| o.players.get(&player).copied())?;
    if cpos != pos {
        debug(&format!("bus: quick move at {pos:?}: the player's open part is at {cpos:?}"));
        return None;
    }
    // The block entity is out of the world while `menu_quick_move` runs (the hook has it as `entity`): its parts are
    // read from it, not through `cell` (which can't find it then).
    let id = read_cell(&entity.nbt).parts[side].clone()?;
    Some((hosted(pos, side), side, own_slots(&id)))
}

/// How many slots a part's own menu has.
pub(crate) fn own_slots(id: &str) -> usize {
    if let Some((_, n)) = crate::parts::menu_of(id) {
        return n;
    }
    match id {
        "ae2:terminal" => 1 + crate::storage::VIEW_CELLS,
        "ae2:crafting_terminal" => 11 + crate::storage::VIEW_CELLS,
        "ae2:pattern_encoding_terminal" => 3 + crate::storage::VIEW_CELLS,
        "ae2:pattern_access_terminal" => 1,
        "ae2:cable_pattern_provider" => 18,
        _ => 0,
    }
}

/// The node a click on a cell is on (the part under the hit point), for items used on parts (memory cards).
pub(crate) fn aimed_node(host: &dyn Host, level: LevelRef, click: &BlockClick<'_>) -> Option<Pos> {
    let pos = click.pos;
    let c = cell(host, level, pos)?;
    let at = rel_point(pos, click.hit.point);
    let eye = host
        .player(click.player)
        .and_then(|p| p.entity().eye_position(host))
        .map(|e| [e[0] - f64::from(pos[0]), e[1] - f64::from(pos[1]), e[2] - f64::from(pos[2])]);
    let side = pick_click(&c, at, click.hit.face, eye).filter(|s| *s < 6)?;
    c.parts[side].as_ref()?;
    Some(hosted(pos, side))
}

/// Jade on a cell: the aimed part (its id and node), else the cable.
pub(crate) fn jade_target(host: &dyn Host, pos: Pos, block: &str, hit: Option<[f64; 3]>) -> Option<(String, Pos)> {
    if !is_cell_block(block) {
        return None;
    }
    let level = here(host)?;
    let c = cell(host, level, pos)?;
    let at = hit.map(|h| [h[0] - f64::from(pos[0]), h[1] - f64::from(pos[1]), h[2] - f64::from(pos[2])]);
    match at.and_then(|a| pick_point(&c, a)) {
        Some(s) if s < 6 => Some((c.parts[s].clone()?, hosted(pos, s))),
        _ => Some((c.cable, pos)),
    }
}

/// Puts a stack into a hosted part's slot (else the block entity's).
pub(crate) fn set_item_slot(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, slot: usize, stack: Option<ItemStack>) -> bool {
    let (at, i) = if is_hosted(pos) {
        if slot >= SLOTS {
            return false;
        }
        let (c, side) = host_cell(pos);
        (c, side * SLOTS + slot)
    } else {
        (pos, slot)
    };
    let Some(be) = host.block_entity(level, at) else { return false };
    host.transaction(|host, tx| be.set_item_slot(host, tx, i as u32, stack.clone()).map_err(|_| ())).is_ok()
}

// ---- Capabilities by side: each side answers as its own part would

/// A side's run of the cell's slots or tanks.
pub(crate) struct Slice<R: 'static> {
    pub(crate) inner: Capability<dyn ResourceHandler<R>>,
    pub(crate) base: usize,
    pub(crate) len: usize,
}

impl<R: 'static> ResourceHandler<R> for Slice<R> {
    fn size(&self, host: &mut dyn ServerHost) -> usize {
        self.inner.size(host).saturating_sub(self.base).min(self.len)
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<R> {
        (index < self.len).then(|| self.inner.resource(host, self.base + index)).flatten()
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        if index < self.len { self.inner.amount(host, self.base + index) } else { 0 }
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, resource: Option<&R>) -> u32 {
        if index < self.len { self.inner.capacity(host, self.base + index, resource) } else { 0 }
    }
    fn is_valid(&self, host: &mut dyn ServerHost, index: usize, resource: &R) -> bool {
        index < self.len && self.inner.is_valid(host, self.base + index, resource)
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &Transaction, index: usize, resource: &R, amount: u32) -> u32 {
        if index < self.len { self.inner.insert_at(host, tx, self.base + index, resource, amount).min(amount) } else { 0 }
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &Transaction, index: usize, resource: &R, amount: u32) -> u32 {
        if index < self.len { self.inner.extract_at(host, tx, self.base + index, resource, amount).min(amount) } else { 0 }
    }
}

/// Nothing on that side (an energy buffer the cell keeps for its acceptors answers no other side).
pub(crate) struct NoEnergy;

impl EnergyHandler for NoEnergy {
    fn amount(&self, _host: &mut dyn ServerHost) -> u32 {
        0
    }
    fn capacity(&self, _host: &mut dyn ServerHost) -> u32 {
        0
    }
    fn insert(&self, _host: &mut dyn ServerHost, _tx: &Transaction, _amount: u32) -> u32 {
        0
    }
    fn extract(&self, _host: &mut dyn ServerHost, _tx: &Transaction, _amount: u32) -> u32 {
        0
    }
}

/// The part on the asked side, its node and the side's index.
fn side_part(host: &dyn Host, at: &BlockSide) -> Option<(String, Pos, usize)> {
    let d = match at.side? {
        Direction::Down => [0, -1, 0],
        Direction::Up => [0, 1, 0],
        Direction::North => [0, 0, -1],
        Direction::South => [0, 0, 1],
        Direction::West => [-1, 0, 0],
        _ => [1, 0, 0],
    };
    let side = side_of(d)?;
    let id = cell(host, at.level, at.pos)?.parts[side].clone()?;
    Some((id, hosted(at.pos, side), side))
}

/// The part on the asked side, from the cell's saved data (what `cell` caches): for the providers' presence.
fn side_part_view(level: &dyn LevelHost, at: &BlockSide, be: BlockEntityRef) -> Option<String> {
    let d = match at.side? {
        Direction::Down => [0, -1, 0],
        Direction::Up => [0, 1, 0],
        Direction::North => [0, 0, -1],
        Direction::South => [0, 0, 1],
        Direction::West => [-1, 0, 0],
        _ => [1, 0, 0],
    };
    let side = side_of(d)?;
    read_cell(&level.block_entity_data(be)?).parts[side].clone()
}

/// Whether `items` answers on this side (a hosted interface, provider or item tunnel there).
pub(crate) fn items_present(level: &dyn LevelHost, at: &BlockSide, _: BlockState, be: BlockEntityRef) -> bool {
    let id = side_part_view(level, at, be);
    matches!(id.as_deref().and_then(crate::net::ae2_name), Some("interface" | "pattern_provider" | "item_p2p_tunnel"))
}

/// Whether `fluids` answers on this side (a hosted provider, interface or fluid tunnel there).
pub(crate) fn fluids_present(level: &dyn LevelHost, at: &BlockSide, _: BlockState, be: BlockEntityRef) -> bool {
    let id = side_part_view(level, at, be);
    matches!(id.as_deref().and_then(crate::net::ae2_name), Some("pattern_provider" | "interface" | "fluid_p2p_tunnel"))
}

/// Whether `energy` answers: everywhere but a hosted energy acceptor's side (the game's own answers there).
pub(crate) fn energy_present(level: &dyn LevelHost, at: &BlockSide, _: BlockState, be: BlockEntityRef) -> bool {
    let id = side_part_view(level, at, be);
    id.as_deref().and_then(crate::net::ae2_name) != Some("energy_acceptor")
}

/// ITEMS on a cell's side: a hosted interface's nine stored stacks, a provider's nine return slots, an item tunnel.
pub(crate) fn items(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    let (id, node, side) = side_part(&*host, &at)?;
    let at_node = BlockSide { level: at.level, pos: node, side: at.side };
    match crate::net::ae2_name(&id)? {
        "interface" => {
            let inner = at.level.capability(host, &ITEMS, at.pos, None)?;
            crate::parts::interface_over(host, at.level, node, Box::new(Slice { inner, base: side * SLOTS, len: 9 }))
        }
        "pattern_provider" => {
            let inner = at.level.capability(host, &ITEMS, at.pos, None)?;
            Some(Box::new(Slice { inner, base: side * SLOTS + 9, len: 9 }))
        }
        "item_p2p_tunnel" => crate::parts::p2p_items(host, at_node),
        _ => None,
    }
}

/// FLUIDS on a cell's side: a provider's return tanks, a fluid tunnel.
pub(crate) fn fluids(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<FluidResource>>> {
    let (id, node, side) = side_part(&*host, &at)?;
    match crate::net::ae2_name(&id)? {
        "pattern_provider" => {
            let inner = at.level.capability(host, &FLUIDS, at.pos, None)?;
            Some(Box::new(Slice { inner, base: side * TANKS, len: TANKS }))
        }
        "fluid_p2p_tunnel" => crate::parts::p2p_fluids(host, BlockSide { level: at.level, pos: node, side: at.side }),
        "interface" => {
            let inner = at.level.capability(host, &FLUIDS, at.pos, None)?;
            crate::iface_fluids::fluids_over(host, at.level, node, Box::new(Slice { inner, base: side * TANKS, len: crate::iface_fluids::TANKS }))
        }
        _ => None,
    }
}

/// ENERGY on a cell's side: a hosted energy acceptor (the cell's FE buffer, into the network at its tick), an FE
/// tunnel; nothing on the other sides.
pub(crate) fn energy(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn EnergyHandler>> {
    let Some((id, node, _)) = side_part(&*host, &at) else { return Some(Box::new(NoEnergy)) };
    match crate::net::ae2_name(&id) {
        Some("energy_acceptor") => None,
        Some("fe_p2p_tunnel") => crate::parts::p2p_energy(host, BlockSide { level: at.level, pos: node, side: at.side }).or(Some(Box::new(NoEnergy))),
        _ => Some(Box::new(NoEnergy)),
    }
}

// ---- The wrench (IAEWrench on a part: sneak-use takes it off into the inventory)

pub(crate) fn wrench(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let level = here(&*host)?;
    let pos = click.pos;
    let c = cell(&*host, level, pos)?;
    let player = host.player(click.player)?;
    let e = player.entity();
    if !(e.is_shift_key_down(&*host).unwrap_or(false) || e.is_crouching(&*host).unwrap_or(false)) {
        return None;
    }
    let at = [click.hit.point[0] - f64::from(pos[0]), click.hit.point[1] - f64::from(pos[1]), click.hit.point[2] - f64::from(pos[2])];
    let what = pick_point(&c, at)?;
    let feet = e.position(&*host)?;
    take(host, level, pos, &c, what, Some(feet), false);
    Some(InteractionResult::SuccessServer)
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    fn bus(parts: &[(usize, &str)]) -> Cell {
        let mut c = Cell { cable: "ae2:fluix_glass_cable".to_owned(), ..Cell::default() };
        for (s, id) in parts {
            c.parts[*s] = Some((*id).to_owned());
        }
        c
    }

    /// The centre of side `s`'s outer face, block-relative.
    fn face_centre(s: usize) -> [f64; 3] {
        let mut p = [0.5; 3];
        p[side_axis(s)] = if s % 2 == 1 { 1.0 } else { 0.0 };
        p
    }

    #[test]
    fn face_centres_pick_their_part() {
        for s in 0..6 {
            let c = bus(&[(s, "ae2:storage_monitor")]);
            assert_eq!(pick_point(&c, face_centre(s)), Some(s), "side {s}");
            assert_eq!(pick_click(&c, face_centre(s), SIDES[s], None), Some(s), "side {s}");
            // A monitor's frame edge (outside its box, on the plate) is still the part.
            let mut edge = face_centre(s);
            edge[(side_axis(s) + 1) % 3] = 0.03;
            assert_eq!(pick_click(&c, edge, SIDES[s], None), Some(s), "edge of {s}");
        }
        // The cable's own face centre where no part is: the cable.
        let c = bus(&[(3, "ae2:conversion_monitor")]);
        assert_eq!(pick_click(&c, face_centre(2), SIDES[2], None), Some(12));
        assert_eq!(pick_point(&c, [0.5, 0.5, 0.5]), Some(12));
    }

    #[test]
    fn a_ray_finds_the_part_behind_a_centre_point() {
        // A point at the cell's centre (a tool that reports the block's centre): the eye's ray through it meets the
        // south monitor first.
        let c = bus(&[(3, "ae2:storage_monitor")]);
        let eye = Some([0.5, 0.6, 3.0]);
        assert_eq!(pick_click(&c, [0.5, 0.5, 0.5], SIDES[3], eye), Some(3));
        // Seen from the north (no part there), the cable.
        assert_eq!(pick_click(&c, [0.5, 0.5, 0.5], SIDES[2], Some([0.5, 0.5, -3.0])), Some(12));
    }

    #[test]
    fn a_quartz_fiber_is_aimed_on_its_arm() {
        // fixE: the in-game cell: a smart cable, the fiber on its east face. The cell's shape there is the east arm
        // (5..11 across); a hit on that arm's top, past the core, is the fiber, not the cable.
        let mut c = bus(&[(5, "ae2:quartz_fiber")]);
        c.cable = "ae2:fluix_smart_cable".to_owned();
        assert_eq!(pick_point(&c, [13.0 / 16.0, 11.0 / 16.0, 0.5]), Some(5));
        assert_eq!(pick_point(&c, [1.0, 0.5, 0.5]), Some(5));
        // The core and the far side are still the cable.
        assert_eq!(pick_point(&c, [0.5, 11.0 / 16.0, 0.5]), Some(12));
        assert_eq!(pick_point(&c, [3.0 / 16.0, 11.0 / 16.0, 0.5]), Some(12));
        // Its rod is AE2's part box, from the core out to the face, and its piece is the east rod.
        assert_eq!(part_boxes("ae2:quartz_fiber", 5), vec![[10.0 / 16.0, 6.0 / 16.0, 6.0 / 16.0, 1.0, 10.0 / 16.0, 10.0 / 16.0]]);
        assert_eq!(fiber_model_id(5), "ae2:block/hz_part/quartz_fiber_bus_east");
        // A bus keeps its cable arm: a hit on it is the cable.
        let mut b = bus(&[(5, "ae2:export_bus")]);
        b.cable = "ae2:fluix_smart_cable".to_owned();
        assert_eq!(pick_point(&b, [11.5 / 16.0, 11.0 / 16.0, 0.5]), Some(12));
    }

    #[test]
    fn every_fiber_rod_is_its_part_box() {
        // fixE: the generated rods (tools/make_cables.py `fiber_models`) are the quartz fiber's boxes on each side.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ae2/models");
        for s in 0..6 {
            let id = fiber_model_id(s);
            let path = root.join(format!("{}.json", id.strip_prefix("ae2:").unwrap()));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{id}: {e}"));
            let v: mod_api::serde_json::Value = mod_api::serde_json::from_str(&text).unwrap();
            let e = &v["elements"][0];
            let px: Vec<f64> = e["from"].as_array().unwrap().iter().chain(e["to"].as_array().unwrap()).map(|n| n.as_f64().unwrap()).collect();
            let want: Vec<f64> = part_boxes("ae2:quartz_fiber", s)[0].iter().map(|v| v * 16.0).collect();
            assert_eq!(px, want, "{id}");
            assert_eq!(e["faces"].as_object().unwrap().len(), 6, "{id}");
        }
    }

    #[test]
    fn points_relative_or_world() {
        assert_eq!(rel_point([5, 101, 72], [5.5, 101.5, 73.0]), [0.5, 0.5, 1.0]);
        // Already relative.
        assert_eq!(rel_point([5, 101, 72], [0.5, 0.5, 1.0]), [0.5, 0.5, 1.0]);
        // Negative coordinates.
        assert_eq!(rel_point([-3, 60, -9], [-2.5, 61.0, -8.5]), [0.5, 1.0, 0.5]);
    }
}
