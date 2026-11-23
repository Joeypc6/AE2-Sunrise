//! AE2's spatial anchor (SpatialAnchorBlockEntity, SpatialAnchorMenu, SpatialAnchorScreen; gap-list item 8):
//! - while its network is online it keeps every chunk of this level that holds a node of its network loaded
//!   (StatisticsService.getChunks), through this mod's chunk ticket controller (`ae2:spatial_anchor`, a ticket per
//!   anchor and chunk); with natural spawning (random ticks) when `spatialAnchorEnableRandomTicks` is on;
//! - it draws 80 + n(n+1)/2 AE a tick for its n chunks (updatePowerConsumption);
//! - offline for more than 200 ticks, it lets them go; broken, at once;
//! - its screen: energy usage, chunks loaded, the network's statistics, and the overlay setting, which shows the
//!   loaded chunks' borders as translucent walls (`getOverlayColor`: half-transparent fluix).

use crate::screens::{spr, tr};
use crate::util::*;
use mod_api::*;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

pub(crate) const ANCHOR: &str = "ae2:spatial_anchor";
pub(crate) const BTN_OVERLAY: i32 = 1;
const TEXT: u32 = 0xFF40_4040;

static CONTROLLER: OnceLock<TicketController> = OnceLock::new();

pub(crate) fn register(r: &mut dyn Registrar) {
    let _ = CONTROLLER.set(r.register_ticket_controller("spatial_anchor", true));
}

/// SpatialAnchorBlockEntity.updatePowerConsumption: AE a tick for `n` chunks.
pub(crate) fn power_for(n: usize) -> f64 {
    80.0 + (n * (n + 1) / 2) as f64
}

/// The chunk a block is in.
pub(crate) fn chunk_of(p: Pos) -> [i32; 2] {
    [p[0] >> 4, p[2] >> 4]
}

/// StatisticsService.getChunks for one level: the chunks holding the network's nodes.
pub(crate) fn required_chunks(nodes: impl IntoIterator<Item = Pos>) -> BTreeSet<[i32; 2]> {
    nodes.into_iter().map(|p| chunk_of(real(p))).collect()
}

#[derive(Default)]
struct State {
    /// Each anchor's forced chunks (by level and position).
    held: HashMap<(&'static str, Pos), BTreeSet<[i32; 2]>>,
    /// Anchors broken: their tickets go at the level's tick end.
    gone: Vec<(&'static str, Pos)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

pub(crate) fn forget_all() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The extra idle draw of an anchor (its chunks), for its grid's.
pub(crate) fn idle(dim: &'static str, pos: Pos) -> f64 {
    with_state(|s| s.held.get(&(dim, pos)).map_or(0, BTreeSet::len)).pipe(power_for)
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}

fn set_forced(
    host: &mut dyn ServerHost,
    level: LevelRef,
    owner: Pos,
    chunk: [i32; 2],
    on: bool,
) -> bool {
    let Some(ctl) = CONTROLLER.get().copied() else {
        return false;
    };
    let Some(c) = host.chunk(level, chunk) else {
        return false;
    };
    let spawning = crate::config::config().spatial_anchor_random_ticks;
    c.set_mod_forced(host, ctl, TicketOwner::Block(owner), on, spawning)
}

/// The anchor's tick (its block entity's, each second: TickRates.SpatialAnchor 20): its chunks follow its network.
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let Some(level) = here(&*host) else { return };
    let dim = host.dimension();
    let tick = crate::net::with_net(dim, |n| n.tick);
    fields.data.resize(8, 0);
    let (_, online, active) = crate::net::device_state(dim, pos);
    if tick % 20 != 0 {
        return;
    }
    // What its tickets held before a reload.
    if !with_state(|s| s.held.contains_key(&(dim, pos))) {
        let saved: BTreeSet<[i32; 2]> = match fields.nbt.get(&java("chunks")) {
            Some(Tag::LongArray(v)) => v.iter().map(|c| [(*c >> 32) as i32, *c as i32]).collect(),
            _ => BTreeSet::new(),
        };
        with_state(|s| s.held.insert((dim, pos), saved));
    }
    let mut held = with_state(|s| s.held.get(&(dim, pos)).cloned()).unwrap_or_default();
    if online && active {
        set_nbt_i64(&mut fields.nbt, "powerless", 0);
        let nodes = crate::net::with_net(dim, |n| {
            n.grid_of
                .get(&pos)
                .and_then(|g| n.grids.get(*g))
                .map(|g| g.nodes.iter().copied().collect::<Vec<_>>())
        })
        .unwrap_or_default();
        let want = required_chunks(nodes);
        for c in held.clone().difference(&want) {
            set_forced(host, level, pos, *c, false);
            held.remove(c);
        }
        for c in want.difference(&held.clone()) {
            if set_forced(host, level, pos, *c, true) || host.chunk(level, *c).is_some() {
                held.insert(*c);
            }
        }
    } else {
        // Offline: after 200 ticks the chunks go.
        let off = nbt_i64(&fields.nbt, "powerless") + 20;
        set_nbt_i64(&mut fields.nbt, "powerless", off);
        if off > 200 {
            for c in std::mem::take(&mut held) {
                set_forced(host, level, pos, c, false);
            }
        }
    }
    let changed = with_state(|s| s.held.insert((dim, pos), held.clone())).as_ref() != Some(&held);
    let saved = Tag::LongArray(
        held.iter()
            .map(|c| (i64::from(c[0]) << 32) | i64::from(c[1] as u32))
            .collect(),
    );
    if fields.nbt.get(&java("chunks")) != Some(&saved) {
        fields.nbt.insert(java("chunks"), saved);
    }
    if changed {
        // Its draw changed: the grid's idle is worked out again.
        crate::net::mark_dirty(dim);
    }
    // The menu (SpatialAnchorMenu): power, chunks, overlay, the network's statistics (one level: its own).
    let n = held.len();
    let grid_chunks = crate::net::with_net(dim, |n| {
        n.grid_of
            .get(&pos)
            .and_then(|g| n.grids.get(*g))
            .map(|g| required_chunks(g.nodes.iter().copied()).len())
    })
    .unwrap_or(0);
    let anchors_best = crate::net::grid_devices(dim, pos, false)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, b)| b == ANCHOR)
        .map(|(p, _)| with_state(|s| s.held.get(&(dim, p)).map_or(0, BTreeSet::len)))
        .max()
        .unwrap_or(0);
    fields.data[0] = if active { power_for(n) as i32 } else { 0 };
    fields.data[1] = n as i32;
    fields.data[2] = i32::from(nbt_i64(&fields.nbt, "overlay") != 0);
    fields.data[3] = anchors_best as i32;
    fields.data[4] = i32::from(anchors_best > 0);
    fields.data[5] = grid_chunks as i32;
    fields.data[6] = i32::from(grid_chunks > 0);
    // Lit while online (SpatialAnchorBlock's POWERED look).
    let lit = (online && active).to_string();
    if level
        .block_property(&*host, pos, "powered")
        .is_some_and(|v| v != lit)
    {
        crate::net::with_net(dim, |n| n.prop_sets.push((pos, "powered", lit)));
    }
}

/// The overlay setting's button (ServerSettingToggleButton OVERLAY_MODE), from the anchor's menu.
pub(crate) fn menu_button(block: &str, id: i32, fields: &mut Fields) -> bool {
    if block != ANCHOR || id != BTN_OVERLAY {
        return false;
    }
    let on = nbt_i64(&fields.nbt, "overlay") == 0;
    set_nbt_i64(&mut fields.nbt, "overlay", i64::from(on));
    fields.data.resize(8, 0);
    fields.data[2] = i32::from(on);
    true
}

/// A block left the world: an anchor's tickets go (SpatialAnchorBlockEntity.setRemoved → releaseAll).
pub(crate) fn note_changed(dim: &'static str, pos: Pos, old: &str, new: &str) {
    if old == ANCHOR && new != ANCHOR {
        with_state(|s| s.gone.push((dim, pos)));
    }
}

pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let gone: Vec<Pos> = with_state(|s| {
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut s.gone)
            .into_iter()
            .partition(|g| g.0 == dim);
        s.gone = rest;
        here.into_iter().map(|g| g.1).collect()
    });
    for pos in gone {
        let mut chunks: BTreeSet<[i32; 2]> =
            with_state(|s| s.held.remove(&(dim, pos))).unwrap_or_default();
        // Tickets a reload left without the table: the level's own list.
        if let Some(ctl) = CONTROLLER.get().copied() {
            for (owner, c, _) in level.mod_forced_chunks(&*host, ctl).unwrap_or_default() {
                if owner == TicketOwner::Block(pos) {
                    chunks.insert(c);
                }
            }
        }
        for c in chunks {
            set_forced(host, level, pos, c, false);
        }
    }
}

/// ChunkLoadingService's validation as a level loads its tickets: those of a block that is no anchor any more go.
pub(crate) fn validate(host: &mut dyn ServerHost, event: &mut ValidateTicketsEvent<'_>) {
    if CONTROLLER.get().copied() != Some(event.controller) {
        return;
    }
    let level = host.level(host.dimension());
    for (pos, _) in event.validation.block_tickets() {
        let block = level
            .and_then(|l| l.block(&*host, pos))
            .and_then(|b| b.id(&*host).map(str::to_owned));
        // Unknown (not loaded): kept; the anchor's own tick lets go of what it no longer needs.
        if block.is_some_and(|b| b != ANCHOR) {
            event.validation.remove_block_tickets(pos);
        }
    }
}

// ---- Its screen

pub(crate) fn has_screen(block: &str) -> bool {
    block == ANCHOR
}

pub(crate) fn container_of(block: &str) -> Option<ModContainer> {
    (block == ANCHOR).then(|| ModContainer {
        slots: 1,
        faces: std::array::from_fn(|_| Vec::new()),
        // Main's Container.canPlaceItem / canTakeItem read these: the menu's clicks need every slot; with no
        // faces, hoppers and pipes still reach none.
        inputs: (0..1).collect(),
        outputs: (0..1).collect(),
        insert_rules: Vec::new(),
        menu: Some(ANCHOR.to_owned()),
    })
}

/// screens/spatial_anchor.json: 195 x 100, no inventory.
pub(crate) fn screen(block: &str) -> Option<MenuScreenLayout> {
    (block == ANCHOR).then(|| MenuScreenLayout {
        width: 195.0,
        height: 100.0,
        slots: vec![Some(slot_at([-10_000.0, -10_000.0]))],
        inventory_y: -10_000.0,
        inventory_x: -10_000.0,
        own_labels: true,
        own_background: true,
        ..MenuScreenLayout::default()
    })
}

/// Platform.formatPowerLong(n * 100, true): `80 AE/t`, `1.5 kAE/t`.
pub(crate) fn format_power_rate(ae: f64) -> String {
    let mut p = ae;
    let mut level = "";
    for prefix in ["k", "M", "G", "T", "P", "E", "Z", "Y"] {
        if p <= 1000.0 {
            break;
        }
        p /= 1000.0;
        level = prefix;
    }
    let s = format!("{p:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s} {level}AE/t")
}

/// The overlay button on the left toolbar (AE2's states.png: YesNo icons).
const BUTTON: [f32; 4] = [-20.0, 8.0, 16.0, 16.0];

pub(crate) fn screen_draw(
    host: &dyn ClientHost,
    block: &str,
    data: &[i32],
    d: &mut dyn DrawContext,
    o: [f32; 2],
) -> bool {
    if block != ANCHOR {
        return false;
    }
    let get = |i: usize| data.get(i).copied().unwrap_or(0);
    spr(
        d,
        "ae2:gui/spatialanchor",
        o,
        [0.0, 0.0],
        [0.0, 0.0, 195.0, 100.0],
    );
    d.text(
        &tr(host, "gui.ae2.SpatialAnchor", "Spatial Anchor"),
        [o[0] + 8.0, o[1] + 6.0],
        TEXT,
        false,
    );
    let power = format_power_rate(f64::from(get(0)));
    d.text(
        &tr(host, "gui.ae2.SpatialAnchorUsedPower", "Energy Usage: %s").replace("%s", &power),
        [o[0] + 13.0, o[1] + 21.0],
        TEXT,
        false,
    );
    d.text(
        &tr(
            host,
            "gui.ae2.SpatialAnchorLoadedChunks",
            "Chunks loaded: %s",
        )
        .replace("%s", &get(1).to_string()),
        [o[0] + 13.0, o[1] + 31.0],
        TEXT,
        false,
    );
    d.text(
        &tr(
            host,
            "gui.ae2.SpatialAnchorStatistics",
            "Network Statistics",
        ),
        [o[0] + 8.0, o[1] + 56.0],
        TEXT,
        false,
    );
    let two = |t: String, a: i32, b: i32| {
        t.replacen("%d", &a.to_string(), 1)
            .replacen("%d", &b.to_string(), 1)
    };
    d.text(
        &two(
            tr(
                host,
                "gui.ae2.SpatialAnchorAllLoaded",
                "Loading: %d chunks in %d worlds",
            ),
            get(3),
            get(4),
        ),
        [o[0] + 13.0, o[1] + 71.0],
        TEXT,
        false,
    );
    d.text(
        &two(
            tr(
                host,
                "gui.ae2.SpatialAnchorAll",
                "Spanning: %d chunks in %d worlds",
            ),
            get(5),
            get(6),
        ),
        [o[0] + 13.0, o[1] + 81.0],
        TEXT,
        false,
    );
    // The overlay toggle: a button frame and the setting's icon.
    let b = [o[0] + BUTTON[0], o[1] + BUTTON[1]];
    d.sprite(
        "ae2:gui/states",
        [240.0, 240.0, 16.0, 16.0],
        [b[0], b[1], 16.0, 16.0],
        None,
    );
    let icon = if get(2) != 0 {
        [64.0, 240.0]
    } else {
        [48.0, 240.0]
    };
    d.sprite(
        "ae2:gui/states",
        [icon[0], icon[1], 16.0, 16.0],
        [b[0], b[1], 16.0, 16.0],
        None,
    );
    true
}

pub(crate) fn screen_click(block: &str, at: [f32; 2]) -> Option<i32> {
    if block != ANCHOR {
        return None;
    }
    let inside = at[0] >= BUTTON[0]
        && at[0] < BUTTON[0] + BUTTON[2]
        && at[1] >= BUTTON[1]
        && at[1] < BUTTON[1] + BUTTON[3];
    inside.then_some(BTN_OVERLAY)
}

pub(crate) fn screen_tooltip(
    host: &dyn ClientHost,
    block: &str,
    data: &[i32],
    at: [f32; 2],
    lines: &mut Vec<String>,
) -> bool {
    if screen_click(block, at).is_none() {
        return false;
    }
    let on = data.get(2).copied().unwrap_or(0) != 0;
    lines.push(tr(host, "gui.tooltips.ae2.OverlayMode", "Overlay Mode"));
    lines.push(if on {
        tr(
            host,
            "gui.tooltips.ae2.OverlayModeYes",
            "Shows the loaded area within the world.",
        )
    } else {
        tr(
            host,
            "gui.tooltips.ae2.OverlayModeNo",
            "Loaded area is hidden.",
        )
    });
    true
}

// ---- The overlay (client)

/// For clients: whether the overlay shows, and the chunks.
pub(crate) fn update_tag(nbt: &Compound) -> Compound {
    let mut out = Compound::new();
    let on = nbt_i64(nbt, "overlay") != 0;
    out.insert(java("overlay"), Tag::Long(i64::from(on)));
    if on && let Some(c) = nbt.get(&java("chunks")) {
        out.insert(java("chunks"), c.clone());
    }
    out
}

/// The anchors the client last drew with their overlay on: their chunks, and the frame (game time) seen.
static SHOWN: Mutex<Option<HashMap<Pos, (Vec<[i32; 2]>, i64)>>> = Mutex::new(None);

pub(crate) fn submit(event: &mut BlockEntitySubmitEvent<'_>) {
    let mut g = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
    let shown = g.get_or_insert_with(HashMap::new);
    if nbt_i64(&event.entity.nbt, "overlay") == 0 {
        shown.remove(&event.pos);
        return;
    }
    let chunks: Vec<[i32; 2]> = match event.entity.nbt.get(&java("chunks")) {
        Some(Tag::LongArray(v)) => v
            .iter()
            .take(1024)
            .map(|c| [(*c >> 32) as i32, *c as i32])
            .collect(),
        _ => Vec::new(),
    };
    shown.insert(event.pos, (chunks, event.draw.game_time()));
}

/// The walls round each set of chunks an anchor holds: a quad on each chunk side with no held chunk beyond, the
/// level's height, both faces.
pub(crate) fn walls(chunks: &[[i32; 2]], y0: f64, y1: f64) -> Vec<[[f64; 3]; 4]> {
    let set: HashSet<[i32; 2]> = chunks.iter().copied().collect();
    let mut out = Vec::new();
    for c in chunks {
        let (x0, z0) = (f64::from(c[0] * 16), f64::from(c[1] * 16));
        let (x1, z1) = (x0 + 16.0, z0 + 16.0);
        let sides = [
            (
                [c[0], c[1] - 1],
                [[x0, y1, z0], [x0, y0, z0], [x1, y0, z0], [x1, y1, z0]],
            ),
            (
                [c[0], c[1] + 1],
                [[x1, y1, z1], [x1, y0, z1], [x0, y0, z1], [x0, y1, z1]],
            ),
            (
                [c[0] - 1, c[1]],
                [[x0, y1, z1], [x0, y0, z1], [x0, y0, z0], [x0, y1, z0]],
            ),
            (
                [c[0] + 1, c[1]],
                [[x1, y1, z0], [x1, y0, z0], [x1, y0, z1], [x1, y1, z1]],
            ),
        ];
        for (n, q) in sides {
            if !set.contains(&n) {
                out.push(q);
                out.push([q[3], q[2], q[1], q[0]]);
            }
        }
    }
    out
}

pub(crate) fn draw_overlay(host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
    if event.stage != RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let now = host
        .level(host.dimension())
        .and_then(|l| l.game_time(host))
        .unwrap_or(0);
    let list: Vec<Vec<[i32; 2]>> = {
        let mut g = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
        let shown = g.get_or_insert_with(HashMap::new);
        // An anchor not drawn for two seconds (out of sight, or gone) shows nothing.
        shown.retain(|_, (_, seen)| (now - *seen).abs() <= 40);
        shown.values().map(|(c, _)| c.clone()).collect()
    };
    if list.is_empty() {
        return;
    }
    // getOverlayColor: AEColor.TRANSPARENT's medium variant at half alpha.
    let color = 0x8000_0000 | 9_002_152;
    let mut quads = Vec::new();
    for chunks in list {
        for q in walls(&chunks, -64.0, 320.0) {
            quads.push(WorldQuad::new(q, color, WorldDepth::Tested));
        }
    }
    quads.truncate(2048);
    let _ = event.draw.quads(&quads);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_and_chunks() {
        assert_eq!(power_for(0), 80.0);
        assert_eq!(power_for(1), 81.0);
        assert_eq!(power_for(4), 90.0);
        assert_eq!(chunk_of([-1, 64, 17]), [-1, 1]);
        let c = required_chunks([[0, 64, 0], [15, 64, 15], [16, 64, 0], [-1, 0, -1]]);
        assert_eq!(
            c.into_iter().collect::<Vec<_>>(),
            vec![[-1, -1], [0, 0], [1, 0]]
        );
    }

    #[test]
    fn power_text() {
        assert_eq!(format_power_rate(80.0), "80 AE/t");
        assert_eq!(format_power_rate(1500.0), "1.5 kAE/t");
        assert_eq!(format_power_rate(90.25), "90.25 AE/t");
    }

    #[test]
    fn overlay_walls() {
        // One chunk: four sides, each both ways.
        assert_eq!(walls(&[[0, 0]], 0.0, 16.0).len(), 8);
        // Two side by side: the shared side has no wall.
        assert_eq!(walls(&[[0, 0], [1, 0]], 0.0, 16.0).len(), 12);
    }
}
