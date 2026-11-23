//! AE2's spatial storage dimension (SpatialStorageDimensionIds, SpatialStorageChunkGenerator, SpatialStoragePlot,
//! SpatialStorageHelper): a real level `ae2:spatial_storage`, solid matrix frame from y 0 to 255 as Java's generator
//! makes it, with one plot carved out for each spatial cell. A transition swaps the pylons' volume with the cell's
//! plot, blocks, block entities and entities (players included), both ways, as Java does.
//!
//! This replaces the upstream port's walled plots in a far band of the End (no mod levels on upstream Horizon).

use crate::util::*;
use mod_api::*;

pub(crate) const DIMENSION: &str = "ae2:spatial_storage";
/// Plots sit on a grid of this step (Java: a plot is at most 128 on a side; the rest is solid frame between them).
const PLOT_STEP: i32 = 512;
const PLOT_ROW: i32 = 128;
/// Plots start at this height in the 0..256 level.
const PLOT_Y: i32 = 64;

/// SpatialStorageChunkGenerator.fillChunk: every block from y 0 to 255 is matrix frame.
pub(crate) struct MatrixFill {
    frame: BlockKey,
}

impl ChunkGenerator for MatrixFill {
    fn build_terrain(&self, chunk: &mut dyn ChunkAccess) {
        let frame = chunk.state(self.frame);
        let [x, z] = [chunk.pos().x * 16, chunk.pos().z * 16];
        for y in 0..256 {
            for dx in 0..16 {
                for dz in 0..16 {
                    chunk.set_block([x + dx, y, z + dz], frame);
                }
            }
        }
    }
}

/// `data/ae2/dimension_type/spatial_storage.json` and the biome `ae2:spatial_storage`, as settings.
pub(crate) fn register(r: &mut dyn Registrar) {
    let frame = r.block_key("ae2:matrix_frame");
    r.register_chunk_generator("spatial_storage", MatrixFill { frame });
    r.register_dimension(
        "spatial_storage",
        DimensionSettings {
            min_y: 0,
            height: 256,
            logical_height: 256,
            has_skylight: false,
            has_ceiling: false,
            generator: "ae2:spatial_storage".into(),
            biome: "ae2:spatial_storage".into(),
            has_fixed_time: true,
            ambient_light: 1.0,
            monster_spawn_light_level: (0, 0),
            monster_spawn_block_light_limit: 0,
            respawn_anchor_works: false,
            can_start_raid: false,
            nether_portal_spawns_piglin: false,
            piglins_zombify: false,
            bed_can_sleep: "never".into(),
            bed_can_set_spawn: "never".into(),
            sky_color: Some("#111111".into()),
            fog_color: Some("#000000".into()),
            day_cycle: "none".into(),
            ..Default::default()
        },
    );
}

/// SpatialStoragePlot.getOrigin: the plot's lowest corner in the spatial level.
pub(crate) fn plot_origin(plot: i32) -> Pos {
    let i = plot.max(1) - 1;
    [(i % PLOT_ROW) * PLOT_STEP + 8, PLOT_Y, (i / PLOT_ROW) * PLOT_STEP + 8]
}

/// SpatialStoragePlotManager.allocatePlot: the next plot id, kept with the world (`data/ae2_spatial_plots.txt`).
pub(crate) fn next_plot(host: &dyn ServerHost) -> i32 {
    let Some(dir) = host.world_dir() else { return 1 };
    let file = dir.join("data").join("ae2_spatial_plots.txt");
    let last = std::fs::read_to_string(&file).ok().and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(0);
    let _ = std::fs::create_dir_all(dir.join("data"));
    let _ = std::fs::write(&file, (last + 1).to_string());
    last + 1
}

/// polish: the last plot id given out (0 for none), for `/ae2 spatial`.
pub(crate) fn last_plot(host: &dyn ServerHost) -> i32 {
    let Some(dir) = host.world_dir() else { return 0 };
    std::fs::read_to_string(dir.join("data").join("ae2_spatial_plots.txt")).ok().and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(0)
}

/// One block of a volume, as read before the swap.
#[derive(Clone)]
pub(crate) struct Taken {
    state: BlockState,
    data: Option<Compound>,
}

fn read_volume(host: &dyn Host, level: LevelRef, lo: Pos, size: [i32; 3]) -> Vec<Taken> {
    let mut out = Vec::with_capacity((size[0] * size[1] * size[2]).max(0) as usize);
    for x in 0..size[0] {
        for y in 0..size[1] {
            for z in 0..size[2] {
                let at = [lo[0] + x, lo[1] + y, lo[2] + z];
                let Some(state) = level.block_state(host, at) else { continue };
                let data = host.block_entity(level, at).and_then(|be| be.data(host));
                out.push(Taken { state, data });
            }
        }
    }
    out
}

/// Writes `blocks` (or air) into the volume of the level **in place** (the fork writes only there): no drops, no
/// container spill, as Java's TransitionInfo writes chunk sections. False if any write was refused.
fn write_volume(host: &mut dyn ServerHost, level: LevelRef, lo: Pos, size: [i32; 3], blocks: Option<&[Taken]>) -> bool {
    let flags = BlockUpdateFlags::CLIENTS | BlockUpdateFlags::SKIP_BLOCK_ENTITY_SIDE_EFFECTS | BlockUpdateFlags::SUPPRESS_DROPS | BlockUpdateFlags::KNOWN_SHAPE;
    let Some(air) = host.block_state_id("minecraft:air") else { return false };
    let mut ok = true;
    let mut i = 0;
    for x in 0..size[0] {
        for y in 0..size[1] {
            for z in 0..size[2] {
                let at = [lo[0] + x, lo[1] + y, lo[2] + z];
                let t = blocks.and_then(|b| b.get(i));
                i += 1;
                ok &= level.set_block(host, at, t.map_or(air, |t| t.state), flags).is_ok();
                if let Some(data) = t.and_then(|t| t.data.clone())
                    && let Some(be) = host.block_entity(level, at)
                {
                    let _ = be.replace_data(host, data);
                }
            }
        }
    }
    ok
}

pub(crate) type Riders = Vec<(EntityRef, [f64; 3])>;

fn entities_inside(host: &dyn Host, level: LevelRef, lo: Pos, size: [i32; 3]) -> Riders {
    let min = [f64::from(lo[0]), f64::from(lo[1]), f64::from(lo[2])];
    let max = [min[0] + f64::from(size[0]), min[1] + f64::from(size[1]), min[2] + f64::from(size[2])];
    let inside = |p: [f64; 3]| (0..3).all(|i| p[i] >= min[i] && p[i] < max[i]);
    // Players by the level's own list too (a player is the one to go for sure).
    let mut all = level.entities_in_owned(host, min, max).unwrap_or_default();
    for p in level.online_players_owned(host).unwrap_or_default() {
        let e = p.entity();
        if !all.contains(&e) {
            all.push(e);
        }
    }
    all.into_iter()
        .filter_map(|e| Some((e, e.position(host)?)))
        .filter(|(_, p)| inside(*p))
        .map(|(e, p)| (e, [p[0] - min[0], p[1] - min[1], p[2] - min[2]]))
        .collect()
}

fn send(host: &mut dyn ServerHost, ents: Riders, dim: &str, to: Pos) {
    for (e, rel) in ents {
        let ok = e.teleport_to_level(host, dim, [f64::from(to[0]) + rel[0], f64::from(to[1]) + rel[1], f64::from(to[2]) + rel[2]], None);
        crate::util::debug(&format!("spatial: entity {:?} to {dim}: {ok}", e.kind(&*host)));
    }
}

// ---- The spatial IO port (SpatialIOPortBlockEntity): a redstone pulse swaps the pylons' volume with the cell's plot.

/// A spatial cell's largest side.
pub(crate) fn cell_limit(item: &str) -> Option<i32> {
    match item {
        "ae2:spatial_storage_cell_2" => Some(2),
        "ae2:spatial_storage_cell_16" => Some(16),
        "ae2:spatial_storage_cell_128" => Some(128),
        _ => None,
    }
}

/// The volume the grid's pylons span, contracted by one on each side (SpatialPylonCluster's bounds, inside).
fn region(dim: &'static str, pos: Pos) -> Option<(Pos, Pos, usize)> {
    let pylons: Vec<Pos> = crate::net::grid_devices(dim, pos, true)?.into_iter().filter(|(_, b)| b == "ae2:spatial_pylon").map(|(p, _)| p).collect();
    if pylons.len() < 2 {
        return None;
    }
    let mut lo = pylons[0];
    let mut hi = pylons[0];
    for p in &pylons {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let inner = ([lo[0] + 1, lo[1] + 1, lo[2] + 1], [hi[0] - 1, hi[1] - 1, hi[2] - 1]);
    (0..3).all(|i| inner.1[i] >= inner.0[i]).then_some((inner.0, inner.1, pylons.len()))
}

/// SpatialPylonService.updateStatus: a transition's AE, from the volume and how well the pylons cover it.
fn power_needed(lo: Pos, hi: Pos, pylons: usize) -> f64 {
    let (x, y, z) = (f64::from(hi[0] - lo[0] + 1), f64::from(hi[1] - lo[1] + 1), f64::from(hi[2] - lo[2] + 1));
    let wanted = ((x * z + x * y + y * z) * 3.0 / 8.0).floor().max(6.0);
    let efficiency = (pylons as f64 / wanted).clamp(0.0, 1.0);
    // AEConfig spatialPowerMultiplier (1250), spatialPowerExponent (1.35).
    let cfg = crate::config::config();
    (x * y * z * cfg.spatial_power_multiplier).powf(1.0 + (cfg.spatial_power_exponent - 1.0) * (1.0 - efficiency)).floor()
}

/// A transition in steps, each in the tick of the level it writes (SpatialStorageHelper.swapRegions does it in one go
/// in Java; the fork writes only in the level in place): the volume is taken up in its level, put into the plot in
/// the spatial level, and the plot's old contents put into the volume back in its level.
pub(crate) struct Job {
    src: &'static str,
    lo: Pos,
    size: [i32; 3],
    plot: i32,
    fresh: bool,
    step: u8,
    a: Vec<Taken>,
    b: Vec<Taken>,
    b_entities: Riders,
    /// What went from the volume to the plot this transition: never sent back with the plot's own.
    came: Vec<EntityRef>,
}

/// Ports with a rising redstone edge (level, port, ticks waited for the plot), and transitions on their way.
static PENDING: std::sync::Mutex<Vec<(&'static str, Pos, u32)>> = std::sync::Mutex::new(Vec::new());
static JOBS: std::sync::Mutex<Vec<Job>> = std::sync::Mutex::new(Vec::new());

/// The port's tick: a rising redstone edge (it remembers the last signal as `rs`) queues a transition for the end of
/// the level's tick (a block entity is out of the world during its own tick).
pub(crate) fn port_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let Some(level) = here(&*host) else { return };
    let signal = level.has_neighbor_signal(&*host, pos);
    let was = nbt_i64(&fields.nbt, "rs") != 0;
    if signal != was {
        set_nbt_i64(&mut fields.nbt, "rs", i64::from(signal));
    }
    if signal && !was {
        PENDING.lock().unwrap_or_else(|e| e.into_inner()).push((host.dimension(), pos, 0));
    }
}

pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let ports: Vec<(Pos, u32)> = {
        let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        let mine = p.iter().filter(|(d, _, _)| *d == dim).map(|(_, q, w)| (*q, *w)).collect();
        p.retain(|(d, _, _)| *d != dim);
        mine
    };
    for (pos, waited) in ports {
        // The plot's level and chunks first: the fork opens a mod level when it is first named, so a transition waits
        // (at most 5 s) for them, where Java's spatial level is always there.
        if start(host, level, dim, pos) && waited < 100 {
            PENDING.lock().unwrap_or_else(|e| e.into_inner()).push((dim, pos, waited + 1));
        }
    }
    let mine: Vec<Job> = {
        let mut j = JOBS.lock().unwrap_or_else(|e| e.into_inner());
        let (here, rest): (Vec<Job>, Vec<Job>) = j.drain(..).partition(|j| (j.step == 1 && dim == DIMENSION) || (j.step == 2 && dim == j.src));
        *j = rest;
        here
    };
    for mut job in mine {
        if job.step == 1 {
            // In the spatial level: the plot's contents taken (a new plot is empty inside its frame), the volume put in.
            let o = plot_origin(job.plot);
            job.b = if job.fresh { Vec::new() } else { read_volume(&*host, level, o, job.size) };
            job.b_entities = entities_inside(&*host, level, o, job.size).into_iter().filter(|(e, _)| !job.came.contains(e)).collect();
            write_volume(host, level, o, job.size, Some(&job.a));
            job.a.clear();
            job.step = 2;
            JOBS.lock().unwrap_or_else(|e| e.into_inner()).push(job);
        } else {
            let b = (!job.fresh).then_some(job.b.as_slice());
            write_volume(host, level, job.lo, job.size, b);
            let ents = std::mem::take(&mut job.b_entities);
            send(host, ents, job.src, job.lo);
            // polish: the Spatial Explorer advancement for the players near.
            crate::advancements::due(job.src, job.lo, crate::advancements::SPATIAL);
        }
    }
}

/// A port's pulse: the checks of SpatialIOPortBlockEntity.transition, then the volume is taken up here. True: wait.
fn start(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str, pos: Pos) -> bool {
    let (_, _, active) = crate::net::device_state(dim, pos);
    if !active || dim == DIMENSION {
        return false;
    }
    let Some(be) = host.block_entity(level, pos) else { return false };
    let Ok(Some(mut cell)) = be.item_slot(&*host, 0) else { return false };
    if !matches!(be.item_slot(&*host, 1), Ok(None)) {
        return false;
    }
    let Some(limit) = cell_limit(crate::cells::sid(&cell)) else { return false };
    let Some((lo, hi, pylons)) = region(dim, pos) else { return false };
    let size = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
    if size.iter().any(|s| *s > limit) {
        return false;
    }
    // A used cell keeps its size (SpatialStorageCellItem: it can't be reformatted).
    let data = crate::cells::custom_data(&cell).cloned().unwrap_or_default();
    let fresh = data.get(&java("ae2:spatial_size")).is_none();
    if let Some(Tag::IntArray(s)) = data.get(&java("ae2:spatial_size"))
        && s.as_slice() != size.as_slice()
    {
        return false;
    }
    // It runs only with all the power it needs at hand.
    let need = power_needed(lo, hi, pylons);
    if crate::net::grid_power(dim, pos).is_none_or(|(e, _)| e < need) {
        return false;
    }
    let plot = match data.get(&java("ae2:plot_id")) {
        Some(Tag::Int(p)) => *p,
        _ => {
            let p = next_plot(&*host);
            // Allocated at once, so a waiting transition keeps its plot.
            crate::cells::edit_custom_data(&mut cell, |d| {
                d.insert(java("ae2:plot_id"), Tag::Int(p));
            });
            let c = cell.clone();
            let _ = host.transaction(|host, tx| be.set_item_slot(host, tx, 0, Some(c)).map_err(|_| ()));
            p
        }
    };
    if !plot_ready(host, plot_origin(plot), size) {
        return true;
    }
    // Taken up: the blocks and their data in memory, the volume emptied here, its entities sent to the plot.
    let a = read_volume(&*host, level, lo, size);
    let ents = entities_inside(&*host, level, lo, size);
    if !write_volume(host, level, lo, size, None) {
        // Nothing may be lost: what was taken goes back.
        write_volume(host, level, lo, size, Some(&a));
        return false;
    }
    let came: Vec<EntityRef> = ents.iter().map(|(e, _)| *e).collect();
    send(host, ents, DIMENSION, plot_origin(plot));
    crate::net::extract_power(dim, pos, need);
    crate::cells::edit_custom_data(&mut cell, |d| {
        d.insert(java("ae2:spatial_size"), Tag::IntArray(size.to_vec()));
    });
    let _ = host.transaction(|host, tx| {
        be.set_item_slot(host, tx, 0, None).map_err(|_| ())?;
        be.set_item_slot(host, tx, 1, Some(cell)).map_err(|_| ())?;
        Ok::<(), ()>(())
    });
    JOBS.lock().unwrap_or_else(|e| e.into_inner()).push(Job { src: dim, lo, size, plot, fresh, step: 1, a, b: Vec::new(), b_entities: Vec::new(), came });
    false
}

/// The plot's chunks forced in the spatial level (which opens with the command), and whether they are loaded.
fn plot_ready(host: &mut dyn ServerHost, o: Pos, size: [i32; 3]) -> bool {
    let (x2, z2) = (o[0] + size[0], o[2] + size[2]);
    host.commands().run_command(&format!("execute in {DIMENSION} run forceload add {} {} {} {}", o[0] - 1, o[2] - 1, x2, z2));
    let Some(store) = host.level(DIMENSION) else { return false };
    [[o[0], o[1], o[2]], [x2 - 1, o[1], z2 - 1]].iter().all(|p| store.is_loaded(&*host, *p))
}
