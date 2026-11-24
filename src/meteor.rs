//! AE2's meteorites (appeng.worldgen.meteorite, AE2 15.4.12) as the fork's own structure type (514ecf9's
//! `register_structure_type`): `data/ae2/worldgen/structure/meteorite.json` (`"type": "ae2:meteorite"`, biomes
//! `#ae2:has_meteorites`, step top_layer_modification) and the `ae2:meteorite` structure set (random_spread, spacing
//! 32, separation 8, salt 124895654) are the game's: it picks the start chunks by the seed, checks the biome and calls
//! the piece once in each chunk its box covers.
//!
//! A structure's worker contexts have no biome names or climates, and MeteoriteStructure's choices need them (the
//! ocean tag picks the heights the meteor sits on; the climate picks the crater; the biome's tags pick the fallout). So
//! the worker works out what needs no biome (centre, radius, both heights, both lake tests, the random's state where
//! the crater's draws start), and its piece notes each chunk it covers. On the server's next ticks (the Host has
//! `biome_at`, `biome_in_tag`, `biome_properties`) each noted chunk gets the rest worked out as Java does and its part
//! of the meteorite placed (MeteoritePlacer, clipped to the chunk, as upstream's `ae2-horizon/mixins/ae2/meteor.rs`).

use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// `ChunkGenerator.getSeaLevel()` in the overworld presets.
const SEA_LEVEL: i32 = 63;
/// MeteoriteStructurePiece.createBoundingBox: the meteor's chunk +-64 blocks.
const RANGE: i32 = 64;
/// Chunks placed a server tick (a chunk near the crater writes a few thousand blocks).
const CHUNKS_PER_TICK: usize = 2;

/// Java's `java.util.Random` LCG, as `WorldgenRandom` over a `LegacyRandomSource` draws from it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Random(u64);
impl Random {
    const MULTIPLIER: u64 = 0x5_DEEC_E66D;
    const MASK: u64 = (1 << 48) - 1;
    fn new(seed: i64) -> Self {
        Random((seed as u64 ^ Self::MULTIPLIER) & Self::MASK)
    }
    fn next(&mut self, bits: u32) -> i32 {
        self.0 = self.0.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB) & Self::MASK;
        (self.0 >> (48 - bits)) as i32
    }
    fn next_int(&mut self, bound: i32) -> i32 {
        if bound & (bound - 1) == 0 {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) >= 0 {
                return value;
            }
        }
    }
    fn next_long(&mut self) -> i64 {
        let high = i64::from(self.next(32)) << 32;
        high.wrapping_add(i64::from(self.next(32)))
    }
    fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * (1.0 / 16_777_216.0)
    }
    fn next_double(&mut self) -> f64 {
        let bits = (i64::from(self.next(26)) << 27) + i64::from(self.next(27));
        bits as f64 * (1.0 / 9_007_199_254_740_992.0)
    }
    /// `WorldgenRandom.setLargeFeatureSeed`.
    fn large_feature(seed: i64, x: i32, z: i32) -> Self {
        let mut r = Random::new(seed);
        let a = r.next_long();
        let b = r.next_long();
        Random::new(i64::from(x).wrapping_mul(a) ^ i64::from(z).wrapping_mul(b) ^ seed)
    }
}

/// CraterType.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Crater {
    None,
    Normal,
    Lava,
    Obsidian,
    Water,
    Snow,
    Ice,
}
impl Crater {
    fn filler(self) -> &'static str {
        match self {
            Crater::None | Crater::Normal => "minecraft:air",
            Crater::Lava => "minecraft:lava",
            Crater::Obsidian => "minecraft:obsidian",
            Crater::Water => "minecraft:water",
            Crater::Snow => "minecraft:snow_block",
            Crater::Ice => "minecraft:ice",
        }
    }
}

/// FalloutMode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fallout {
    Default,
    Sand,
    Terracotta,
    IceSnow,
}

/// Guava's StatsAccumulator, as MeteoriteStructure.generatePieces uses it (the heights are finite).
#[derive(Default)]
pub(crate) struct Stats {
    count: u64,
    mean: f64,
    squares: f64,
    min: f64,
}
impl Stats {
    fn add(&mut self, value: f64) {
        if self.count == 0 {
            (self.count, self.mean, self.min) = (1, value, value);
        } else {
            self.count += 1;
            let delta = value - self.mean;
            self.mean += delta / self.count as f64;
            self.squares += delta * (value - self.mean);
            self.min = self.min.min(value);
        }
    }
    fn population_variance(&self) -> f64 {
        if self.count <= 1 { 0.0 } else { self.squares.max(0.0) / self.count as f64 }
    }
}

/// What the worker works out of a start: everything that needs no biome. `[land, ocean]` pairs: the centre's y and
/// the lake test for each reading of the spawn biome's ocean tag.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Start {
    center: [i32; 2],
    radius: f32,
    y: [i32; 2],
    lake: [bool; 2],
    /// The random as the crater's draws find it (determineCraterType, then the pure float).
    random: u64,
}

/// MeteoriteStructure (`ae2:meteorite`).
pub(crate) struct Meteorite;

/// MeteoriteStructurePiece: the start, noted for each chunk it covers.
#[derive(Debug)]
pub(crate) struct Piece(Start);

/// The chunks waiting for their part (`[x, z]`), and each start's settings once worked out on the server.
#[derive(Default)]
pub(crate) struct Pending {
    chunks: Vec<([i32; 2], Start)>,
    settings: HashMap<[i32; 2], Settings>,
    /// The world whose saved list was read (`data/ae2_meteor_pending.txt`), and the list as last written.
    loaded: Option<std::path::PathBuf>,
    written: String,
}
static PENDING: LazyLock<Mutex<Pending>> = LazyLock::new(Default::default);

fn pending<R>(f: impl FnOnce(&mut Pending) -> R) -> R {
    f(&mut PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

pub(crate) fn register(r: &mut dyn Registrar) {
    r.register_structure_type("meteorite", Meteorite);
}

/// getBaseHeight: (OCEAN_FLOOR_WG, WORLD_SURFACE_WG) of a column, each column asked once.
fn height(heights: &mut HashMap<(i32, i32), (i32, i32)>, context: &mut dyn GenerationContext, x: i32, z: i32) -> (i32, i32) {
    *heights.entry((x, z)).or_insert_with(|| {
        (context.base_height(x, z, HeightmapType::OceanFloorWg), context.base_height(x, z, HeightmapType::WorldSurfaceWg))
    })
}

impl StructureType for Meteorite {
    /// MeteoriteStructure.findGenerationPoint and generatePieces (up to the biome's choices).
    fn find_generation_point(&self, context: &mut dyn GenerationContext) -> Option<FoundStructure> {
        if !context.random().next_boolean() {
            return None;
        }
        let chunk = context.chunk();
        let (seed, min_y) = (context.seed(), context.min_y());
        // onTopOfChunkCenter(OCEAN_FLOOR_WG): the stub, whose biome must be one of the structure's.
        let (mx, mz) = (chunk.x * 16 + 7, chunk.z * 16 + 7);
        let stub_y = context.base_height(mx, mz, HeightmapType::OceanFloorWg) - 1;
        if !context.column_has_valid_biome(mx, mz, stub_y, stub_y) {
            return None;
        }
        // generatePieces, with a random seeded as the boolean's was.
        // fixA: the centre's draws shared with the compass's scan of old worlds (`centre_of`).
        let (mut random, [center_x, center_z]) = centre_of(seed, [chunk.x, chunk.z]);
        let radius = random.next_float() * 6.0 + 2.0;
        let y_offset = f64::from(radius).ceil() as i32 + 1;
        let mut heights: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
        let scan = 1.0f32.max(radius * 2.0) as i32;
        let mut y = [0; 2];
        let mut lake = [false; 2];
        for (i, ocean) in [false, true].into_iter().enumerate() {
            let mut stats = Stats::default();
            for x in -scan..=scan {
                for z in -scan..=scan {
                    let (floor, surface) = height(&mut heights, context, center_x + x, center_z + z);
                    stats.add(f64::from(if ocean { floor } else { surface }));
                }
            }
            let mut center_y = stats.mean as i32;
            if stats.population_variance() > 5.0 {
                center_y = (f64::from(center_y) - (stats.mean - stats.min) * 0.75) as i32;
            }
            center_y -= y_offset;
            y[i] = (min_y + y_offset).max(center_y);
            // locateWaterAroundTheCrater.
            'ring: for x in center_x - 32..=center_x + 32 {
                for z in center_z - 32..=center_z + 32 {
                    let (dx, dz) = (f64::from(x - center_x), f64::from(z - center_z));
                    let h = f64::from(y[i] as f32 - radius + 1.0);
                    let distance = dx * dx + dz * dz;
                    let max_y = f64::from(SEA_LEVEL - 1);
                    if max_y > h + distance * 0.0175 && max_y < h + distance * 0.02 && height(&mut heights, context, x, z).0 < SEA_LEVEL {
                        lake[i] = true;
                        break 'ring;
                    }
                }
            }
        }
        let start = Start { center: [center_x, center_z], radius, y, lake, random: random.0 };
        Some(FoundStructure { position: [mx, stub_y, mz], pieces: vec![Arc::new(Piece(start))], pool_pieces: Vec::new() })
    }

    fn load_piece(&self, _metadata: &dyn BlockMetadata, tag: &Compound) -> Result<Arc<dyn StructurePiece>, StructureError> {
        let ints = match tag.get(&"meteor".into()) {
            Some(Tag::IntArray(v)) if v.len() == 9 => v.clone(),
            _ => return Err(StructureError("meteorite piece: no saved meteor".into())),
        };
        Ok(Arc::new(Piece(Start {
            center: [ints[0], ints[1]],
            radius: f32::from_bits(ints[2] as u32),
            y: [ints[3], ints[4]],
            lake: [ints[5] != 0, ints[6] != 0],
            random: (ints[7] as u32 as u64) | ((ints[8] as u32 as u64) << 32),
        })))
    }
}

impl StructurePiece for Piece {
    fn bounding_box(&self) -> BoundingBox {
        let s = &self.0;
        let (cx, cz) = (s.center[0] >> 4 << 4, s.center[1] >> 4 << 4);
        let y = s.y[0].min(s.y[1]);
        BoundingBox::new([cx - RANGE, y, cz - RANGE], [cx + 15 + RANGE, s.y[0].max(s.y[1]), cz + 15 + RANGE])
    }

    /// The chunk is noted; its part is placed on the server's next ticks.
    fn post_process(&self, context: &mut dyn PieceContext) -> Result<(), StructureError> {
        let c = context.chunk();
        pending(|p| {
            if p.chunks.len() < 8192 {
                p.chunks.push(([c.x, c.z], self.0));
            }
        });
        Ok(())
    }

    fn add_additional_save_data(&self, tag: &mut Compound) {
        let s = &self.0;
        let ints = vec![
            s.center[0],
            s.center[1],
            s.radius.to_bits() as i32,
            s.y[0],
            s.y[1],
            i32::from(s.lake[0]),
            i32::from(s.lake[1]),
            s.random as u32 as i32,
            (s.random >> 32) as u32 as i32,
        ];
        tag.insert("meteor".into(), Tag::IntArray(ints));
    }
}

// ---- The server's half

/// PlacedMeteoriteSettings, with FalloutCopy's block.
#[derive(Clone, Debug)]
pub(crate) struct Settings {
    pos: [i32; 3],
    radius: f32,
    crater: Crater,
    fallout: Fallout,
    pure: bool,
    lake: bool,
    copied: String,
}

fn in_tag(host: &dyn Host, level: LevelRef, biome: &str, tag: &str) -> bool {
    level.biome_in_tag(host, biome, tag).unwrap_or(false)
}

/// The rest of MeteoriteStructure.generatePieces, now the biomes can be read; None while the centre's column isn't
/// loaded.
fn settings(host: &dyn Host, level: LevelRef, s: &Start) -> Option<Settings> {
    let [cx, cz] = s.center;
    // getBiomesWithin(x, seaLevel, z, 0): the spawn biome.
    let spawn = level.biome_at(host, [cx, SEA_LEVEL, cz])?;
    let ocean = in_tag(host, level, &spawn, "c:ocean");
    let i = usize::from(ocean);
    let pos = [cx, s.y[i], cz];
    let mut random = Random(s.random);
    let crater = crater_type(host, level, &spawn, pos, ocean, &mut random);
    let pure = random.next_float() > 0.9;
    // FalloutMode.fromBiome: SAND (desert, beach), then TERRACOTTA (mesa), then ICE_SNOW (snowy, icy).
    let t = |tag: &str| in_tag(host, level, &spawn, tag);
    let fallout = if t("c:desert") || t("c:beach") {
        Fallout::Sand
    } else if t("c:mesa") {
        Fallout::Terracotta
    } else if t("c:snowy") || t("c:icy") {
        Fallout::IceSnow
    } else {
        Fallout::Default
    };
    // FalloutCopy's block, from `level.getBiome(meteorPos)`.
    let here = level.biome_at(host, pos).unwrap_or_else(|| spawn.clone());
    let h = |tag: &str| in_tag(host, level, &here, tag);
    let copied = if h("c:mesa") {
        "minecraft:terracotta"
    } else if h("c:icy") {
        "minecraft:snow_block"
    } else if h("c:beach") || h("c:desert") {
        "minecraft:sand"
    } else if h("c:plains") || h("c:forest") {
        "minecraft:dirt"
    } else {
        "minecraft:cobblestone"
    };
    Some(Settings { pos, radius: s.radius, crater, fallout, pure, lake: s.lake[i], copied: copied.to_owned() })
}

/// determineCraterType: 0 to 4 floats, by the spawn biome's climate.
fn crater_type(host: &dyn Host, level: LevelRef, biome: &str, pos: [i32; 3], ocean: bool, random: &mut Random) -> Crater {
    let climate = level.biome_properties(host, biome);
    let temp = climate.as_ref().map_or(0.8, |c| c.temperature);
    let precipitation = climate.as_ref().is_none_or(|c| c.has_precipitation);
    if ocean {
        return Crater::None;
    }
    if random.next_float() <= 0.5 {
        return Crater::Normal;
    }
    // Biome.coldEnoughToSnow: the height-adjusted temperature (above sea level + 17) below 0.15.
    let adjusted = if pos[1] > SEA_LEVEL + 17 { temp - (pos[1] - (SEA_LEVEL + 17)) as f32 * 0.05 / 40.0 } else { temp };
    let cold = adjusted < 0.15;
    if temp >= 1.0 {
        let lava = random.next_float() > 0.5;
        if !precipitation {
            return if lava { Crater::Lava } else { Crater::Normal };
        }
        if !cold {
            let obsidian = random.next_float() > 0.75;
            let alternative = if obsidian { Crater::Obsidian } else { Crater::Lava };
            return if lava { alternative } else { Crater::Normal };
        }
    }
    if temp < 1.0 && f64::from(temp) >= 0.2 {
        let water = random.next_float() > 0.25;
        let lava = random.next_float() > 0.8;
        if !precipitation {
            return if lava { Crater::Lava } else { Crater::Normal };
        }
        if !cold {
            let obsidian = random.next_float() > 0.75;
            let alternative = if obsidian { Crater::Obsidian } else { Crater::Lava };
            let lake = if water { Crater::Water } else { Crater::Normal };
            return if lava { alternative } else { lake };
        }
        let snow = random.next_float() > 0.75;
        let water = if water { Crater::Water } else { Crater::Normal };
        return if snow { Crater::Snow } else { water };
    }
    if f64::from(temp) < 0.2 {
        let snow = random.next_float() > 0.25;
        let lava = random.next_float() > 0.95;
        let ice = random.next_float() > 0.25;
        if !precipitation {
            return if lava { Crater::Lava } else { Crater::Normal };
        }
        if !cold {
            let alternative = if ice { Crater::Ice } else { Crater::Water };
            let lake = if snow { alternative } else { Crater::Normal };
            return if lava { Crater::Lava } else { lake };
        }
        let snow = if snow { Crater::Snow } else { Crater::Normal };
        return if lava { Crater::Lava } else { snow };
    }
    Crater::Normal
}

/// One noted chunk as a line of the saved list.
fn line_of(chunk: [i32; 2], s: &Start) -> String {
    format!(
        "{} {} {} {} {} {} {} {} {} {}",
        chunk[0], chunk[1], s.center[0], s.center[1], s.radius.to_bits(), s.y[0], s.y[1], u8::from(s.lake[0]), u8::from(s.lake[1]), s.random
    )
}

fn parse_line(l: &str) -> Option<([i32; 2], Start)> {
    let v: Vec<&str> = l.split_whitespace().collect();
    if v.len() != 10 {
        return None;
    }
    let i = |n: usize| v[n].parse::<i32>().ok();
    Some((
        [i(0)?, i(1)?],
        Start {
            center: [i(2)?, i(3)?],
            radius: f32::from_bits(v[4].parse::<u32>().ok()?),
            y: [i(5)?, i(6)?],
            lake: [v[7] == "1", v[8] == "1"],
            random: v[9].parse::<u64>().ok()?,
        },
    ))
}

/// polish: the meteorites worked out but not yet placed within `radius` blocks of `at` (x, z), nearest first: centre,
/// radius and the land reading's centre y (`/ae2 testmeteorites`).
pub(crate) fn known_near(at: [i32; 2], radius: i32) -> Vec<([i32; 2], f32, i32)> {
    let mut out: Vec<([i32; 2], f32, i32)> = Vec::new();
    pending(|p| {
        for (_, s) in &p.chunks {
            let d = f64::from(s.center[0] - at[0]).hypot(f64::from(s.center[1] - at[1]));
            if d <= f64::from(radius) && !out.iter().any(|(c, ..)| *c == s.center) {
                out.push((s.center, s.radius, s.y[0]));
            }
        }
    });
    out.sort_by_key(|(c, ..)| i64::from(c[0] - at[0]).pow(2) + i64::from(c[1] - at[1]).pow(2));
    out
}

/// The noted chunks are kept with the world (`data/ae2_meteor_pending.txt`), so a stop before they're placed
/// finishes on the next load: read once a world, written when the list changed.
fn persist(host: &dyn ServerHost) {
    let Some(dir) = host.world_dir() else {
        debug("meteor: no world folder");
        return;
    };
    let file = dir.join("data").join("ae2_meteor_pending.txt");
    pending(|p| {
        if p.loaded.as_ref() != Some(&dir) {
            p.loaded = Some(dir.clone());
            p.settings.clear();
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let mut saved: Vec<([i32; 2], Start)> = text.lines().filter_map(parse_line).collect();
            if !saved.is_empty() {
                debug(&format!("meteor: {} noted chunks read from the save", saved.len()));
            }
            saved.retain(|(c, s)| !p.chunks.iter().any(|(c2, s2)| c2 == c && s2.center == s.center));
            p.chunks.extend(saved);
            p.written = text;
        }
        let text: String = p.chunks.iter().map(|(c, s)| line_of(*c, s) + "\n").collect();
        if text != p.written {
            let _ = std::fs::create_dir_all(dir.join("data"));
            if std::fs::write(&file, &text).is_ok() {
                p.written = text;
            }
        }
    });
}

/// The overworld's tick: up to two noted chunks get their part of their meteorite (a chunk not loaded again, or whose
/// meteor's centre column isn't loaded yet, waits).
pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    scan_tick(host, level); // fixA
    if host.dimension() != "minecraft:overworld" {
        return;
    }
    persist(&*host);
    let work: Vec<([i32; 2], Start)> = pending(|p| std::mem::take(&mut p.chunks));
    if work.is_empty() {
        return;
    }
    let mut done = 0;
    let mut wait = Vec::new();
    for (chunk, start) in work {
        if done >= CHUNKS_PER_TICK || level.block_state(&*host, [chunk[0] * 16, 0, chunk[1] * 16]).is_none() {
            wait.push((chunk, start));
            continue;
        }
        let known = pending(|p| p.settings.get(&start.center).cloned());
        let Some(m) = known.or_else(|| settings(&*host, level, &start)) else {
            wait.push((chunk, start));
            continue;
        };
        pending(|p| {
            if p.settings.len() > 4096 {
                p.settings.clear();
            }
            p.settings.insert(start.center, m.clone());
        });
        if chunk == [m.pos[0] >> 4, m.pos[2] >> 4] {
            debug(&format!("meteor: start at {:?}, radius {:.2}, crater {:?}, fallout {:?}, pure {}, lake {}", m.pos, m.radius, m.crater, m.fallout, m.pure, m.lake));
        }
        place(host, level, chunk, &m);
        done += 1;
    }
    pending(|p| {
        wait.extend(std::mem::take(&mut p.chunks));
        p.chunks = wait;
    });
    persist(&*host);
}

/// The blocks the placer writes.
struct States {
    air: BlockState,
    water: BlockState,
    stone: BlockState,
    cobblestone: BlockState,
    dirt: BlockState,
    gravel: BlockState,
    grass_block: BlockState,
    glass: BlockState,
    snow: BlockState,
    ice: BlockState,
    sky_stone: BlockState,
    mysterious_cube: BlockState,
    filler: BlockState,
    copied: BlockState,
    quartz: [BlockState; 5],
    buds: [BlockState; 3],
}

/// MeteoritePlacer for one chunk.
fn place(host: &mut dyn ServerHost, level: LevelRef, chunk: [i32; 2], m: &Settings) {
    let s = |host: &dyn Host, id: &str| host.block_state_id(id);
    let h: &dyn Host = &*host;
    let air = match s(h, "minecraft:air") {
        Some(a) => a,
        None => return,
    };
    let get = |id: &str| s(h, id).unwrap_or(air);
    let up = |id: &str| s(h, &format!("{id}[facing=up]")).unwrap_or_else(|| get(id));
    let blocks = States {
        air,
        water: get("minecraft:water"),
        stone: get("minecraft:stone"),
        cobblestone: get("minecraft:cobblestone"),
        dirt: get("minecraft:dirt"),
        gravel: get("minecraft:gravel"),
        grass_block: get("minecraft:grass_block"),
        glass: get("minecraft:glass"),
        snow: get("minecraft:snow"),
        ice: get("minecraft:ice"),
        sky_stone: get("ae2:sky_stone_block"),
        mysterious_cube: get("ae2:mysterious_cube"),
        filler: get(m.crater.filler()),
        copied: get(&m.copied),
        quartz: ["ae2:quartz_block", "ae2:damaged_budding_quartz", "ae2:chipped_budding_quartz", "ae2:flawed_budding_quartz", "ae2:flawless_budding_quartz"].map(get),
        buds: ["ae2:small_quartz_bud", "ae2:medium_quartz_bud", "ae2:large_quartz_bud"].map(up),
    };
    // The chunk's own random, from the meteor and the chunk (deterministic for the seed, as the start is).
    let random = Random::new((i64::from(m.pos[0]) << 32 ^ i64::from(m.pos[2])) ^ (i64::from(chunk[0]) * 341_873_128_712 + i64::from(chunk[1]) * 132_897_987_541));
    let size = f64::from(m.radius);
    let real_crater = size * 2.0 + 5.0;
    let mut placer = Placer {
        host,
        level,
        random,
        blocks,
        m,
        bounds: [chunk[0] * 16, -64, chunk[1] * 16, chunk[0] * 16 + 15, 319, chunk[1] * 16 + 15],
        size,
        squared: size * size,
        crater: real_crater * real_crater,
        names: HashMap::new(),
    };
    placer.place();
}

/// Blocks `canBeReplaced` answers true for (the `replaceable` tag's blocks and fluids).
fn replaceable_name(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:water" | "minecraft:lava"
            | "minecraft:short_grass" | "minecraft:grass" | "minecraft:tall_grass" | "minecraft:fern"
            | "minecraft:large_fern" | "minecraft:dead_bush" | "minecraft:snow" | "minecraft:vine"
            | "minecraft:glow_lichen" | "minecraft:seagrass" | "minecraft:tall_seagrass" | "minecraft:fire"
            | "minecraft:soul_fire" | "minecraft:structure_void" | "minecraft:light" | "minecraft:crimson_roots"
            | "minecraft:warped_roots" | "minecraft:nether_sprouts" | "minecraft:hanging_roots" | "minecraft:bubble_column"
            | "minecraft:short_dry_grass" | "minecraft:tall_dry_grass" | "minecraft:bush" | "minecraft:leaf_litter"
            | "minecraft:resin_clump" | "minecraft:sculk_vein"
    )
}

pub(crate) struct Placer<'c> {
    host: &'c mut dyn ServerHost,
    level: LevelRef,
    random: Random,
    blocks: States,
    m: &'c Settings,
    bounds: [i32; 6],
    size: f64,
    squared: f64,
    crater: f64,
    names: HashMap<BlockState, String>,
}
impl Placer<'_> {
    fn state(&mut self, p: [i32; 3]) -> Option<BlockState> {
        if p[1] < self.bounds[1] || p[1] > self.bounds[4] {
            return None;
        }
        self.level.block_state(&*self.host, p)
    }
    fn name(&mut self, state: Option<BlockState>) -> String {
        let Some(s) = state else { return "minecraft:void_air".to_owned() };
        if let Some(n) = self.names.get(&s) {
            return n.clone();
        }
        let n = s.block().id(&*self.host).unwrap_or_default().to_owned();
        self.names.insert(s, n.clone());
        n
    }
    fn is_air(&mut self, state: Option<BlockState>) -> bool {
        self.name(state).ends_with(":air")
    }
    fn replaceable(&mut self, state: Option<BlockState>) -> bool {
        replaceable_name(&self.name(state))
    }
    /// BlockState.isSolid() (legacy solidity): blocks that aren't air, fluids or replaceable plants.
    fn solid(&mut self, state: Option<BlockState>) -> bool {
        state.is_some() && !self.replaceable(state)
    }
    /// `WorldGenRegion.setBlock`: no neighbour updates.
    fn set(&mut self, p: [i32; 3], state: BlockState) {
        if p[1] < self.bounds[1] || p[1] > self.bounds[4] {
            return;
        }
        let _ = self.level.set_block(self.host, p, state, BlockUpdateFlags::CLIENTS | BlockUpdateFlags::KNOWN_SHAPE);
    }
    /// MeteoriteBlockPutter.put: never over bedrock, nor the same state.
    fn put(&mut self, p: [i32; 3], state: BlockState) {
        let original = self.state(p);
        if original.is_none() || original == Some(state) || self.name(original) == "minecraft:bedrock" {
            return;
        }
        self.set(p, state);
    }
    fn clamp_x(&self, x: i32) -> i32 {
        x.clamp(self.bounds[0], self.bounds[3])
    }
    fn clamp_z(&self, z: i32) -> i32 {
        z.clamp(self.bounds[2], self.bounds[5])
    }
    /// Fallout.adjustCrater: 2 for FalloutSand and FalloutSnow.
    fn adjust_crater(&self) -> f64 {
        if matches!(self.m.fallout, Fallout::Sand | Fallout::IceSnow) { 2.0 } else { 0.0 }
    }

    fn place(&mut self) {
        let crater = self.m.crater != Crater::None;
        if crater {
            self.place_crater();
        }
        self.place_meteorite();
        if crater {
            self.decay();
        }
        if self.m.lake {
            self.place_crater_lake();
        }
    }

    fn place_crater(&mut self) {
        let [x, y, z] = self.m.pos;
        let filler = self.blocks.filler;
        let adjust = self.adjust_crater();
        for j in y - 5..=255 {
            for i in self.bounds[0]..=self.bounds[3] {
                for k in self.bounds[2]..=self.bounds[5] {
                    let (dx, dz) = (f64::from(i - x), f64::from(k - z));
                    let h = f64::from(y) - self.size + 1.0 + adjust;
                    if f64::from(j) > h + (dx * dx + dz * dz) * 0.02 {
                        let current = self.state([i, j, k]);
                        if self.m.crater != Crater::Normal && j < y && self.solid(current) {
                            self.put([i, j, k], filler);
                        } else {
                            self.put([i, j, k], self.blocks.air);
                        }
                    }
                }
            }
        }
    }

    fn place_meteorite(&mut self) {
        self.place_sky_stone();
        let [x, _, z] = self.m.pos;
        let b = self.bounds;
        if crate::config::config().spawn_presses_in_meteorites && x >= b[0] && x <= b[3] && z >= b[2] && z <= b[5] {
            let cube = self.blocks.mysterious_cube;
            self.put(self.m.pos, cube);
            // [tools] The meteorite compass looks for it.
            crate::handtools::block_changed("minecraft:overworld", self.m.pos, "", "ae2:mysterious_cube");
        }
    }

    fn place_sky_stone(&mut self) {
        let [x, y, z] = self.m.pos;
        let flawless_only = crate::config::config().spawn_flawless_only;
        let (x0, x1, z0, z1) = (self.clamp_x(x - 8), self.clamp_x(x + 8), self.clamp_z(z - 8), self.clamp_z(z + 8));
        for i in x0..=x1 {
            for j in y - 8..y + 8 {
                for k in z0..=z1 {
                    let (dx, dy, dz) = (i - x, j - y, k - z);
                    if f64::from(dx * dx) * 0.7 + f64::from(dy * dy) * (if j > y { 1.4 } else { 0.8 }) + f64::from(dz * dz) * 0.7 < self.squared {
                        if dx.abs() <= 1 && dy.abs() <= 1 && dz.abs() <= 1 {
                            if dy == -1 {
                                let index = if flawless_only { 4 } else { self.random.next_int(5) as usize };
                                let q = self.blocks.quartz[index];
                                self.put([i, j, k], q);
                                if index != 0 && (dx != 0 || dz != 0) && self.random.next_float() <= 0.7 {
                                    let bud = self.blocks.buds[self.random.next_int(3) as usize];
                                    self.put([i, j + 1, k], bud);
                                }
                            }
                        } else {
                            let sky = self.blocks.sky_stone;
                            self.put([i, j, k], sky);
                        }
                    }
                }
            }
        }
    }

    fn decay(&mut self) {
        let [x, y, z] = self.m.pos;
        let mut fallen = 0.0f64;
        let (x0, x1, z0, z1) = (self.clamp_x(x - 30), self.clamp_x(x + 30), self.clamp_z(z - 30), self.clamp_z(z + 30));
        let filler = self.m.crater.filler();
        for i in x0..=x1 {
            for k in z0..=z1 {
                for j in y - 9..y + 30 {
                    let state = self.state([i, j, k]);
                    if self.m.pure && self.name(state) == filler {
                        continue;
                    }
                    if self.replaceable(state) {
                        let up = self.state([i, j + 1, k]);
                        if !self.is_air(up) {
                            if let Some(up) = up {
                                self.set([i, j, k], up);
                            }
                        } else if fallen < 100.0 * self.crater {
                            let (dx, dy, dz) = (f64::from(i - x), f64::from(j - y), f64::from(k - z));
                            let distance = dx * dx + dy * dy + dz * dz;
                            let below = self.state([i, j - 1, k]);
                            if !self.replaceable(below) {
                                let extra = self.random.next_double() * 0.6;
                                let height = self.crater * (extra + 0.2) - (distance - self.crater * 1.7).abs();
                                if !self.is_air(below) && height > 0.0 && self.random.next_double() > 0.6 {
                                    fallen += 1.0;
                                    self.random_fall([i, j, k]);
                                }
                            }
                        }
                    } else {
                        let up = self.state([i, j + 1, k]);
                        if self.is_air(up) && self.random.next_double() > 0.4 {
                            let (dx, dy, dz) = (f64::from(i - x), f64::from(j - y), f64::from(k - z));
                            let distance = dx * dx + dy * dy + dz * dz;
                            if (dx.abs() > 1.0 || dy.abs() > 1.0 || dz.abs() > 1.0) && distance < self.crater * 1.6 {
                                self.random_inset([i, j, k]);
                            }
                        }
                    }
                }
            }
        }
    }

    fn place_crater_lake(&mut self) {
        let [x, y, z] = self.m.pos;
        let adjust = self.adjust_crater();
        for j in y - 5..=SEA_LEVEL - 1 {
            for i in self.bounds[0]..=self.bounds[3] {
                for k in self.bounds[2]..=self.bounds[5] {
                    let (dx, dz) = (f64::from(i - x), f64::from(k - z));
                    let h = f64::from(y) - self.size + 1.0 + adjust;
                    if f64::from(j) > h + (dx * dx + dz * dz) * 0.02 {
                        let current = self.state([i, j, k]);
                        if self.name(current) == "minecraft:air" {
                            let water = self.blocks.water;
                            self.put([i, j, k], water);
                        }
                    }
                }
            }
        }
    }

    /// Fallout.getRandomFall (FalloutCopy's for SAND, TERRACOTTA and ICE_SNOW).
    fn random_fall(&mut self, p: [i32; 3]) {
        let a = self.random.next_float();
        let b = &self.blocks;
        let state = if self.m.fallout == Fallout::Default {
            Some(if a > 0.9 {
                b.stone
            } else if a > 0.8 {
                b.cobblestone
            } else if a > 0.7 {
                b.dirt
            } else {
                b.gravel
            })
        } else if a > 0.9 {
            Some(b.copied)
        } else {
            self.other(a)
        };
        if let Some(s) = state {
            self.put(p, s);
        }
    }

    /// Fallout.getRandomInset (FalloutCopy's for SAND, TERRACOTTA and ICE_SNOW).
    fn random_inset(&mut self, p: [i32; 3]) {
        let a = self.random.next_float();
        let b = &self.blocks;
        let state = if self.m.fallout == Fallout::Default {
            Some(if a > 0.9 {
                b.cobblestone
            } else if a > 0.8 {
                b.stone
            } else if a > 0.7 {
                b.grass_block
            } else if a > 0.6 {
                b.sky_stone
            } else if a > 0.5 {
                b.gravel
            } else {
                b.air
            })
        } else if a > 0.9 {
            Some(b.copied)
        } else if a > 0.8 {
            Some(b.air)
        } else {
            self.other(a - 0.1)
        };
        if let Some(s) = state {
            self.put(p, s);
        }
    }

    /// getOther: nothing for FalloutCopy, glass for FalloutSand, snow or ice for FalloutSnow.
    fn other(&self, a: f32) -> Option<BlockState> {
        match self.m.fallout {
            Fallout::Sand if a > 0.66 => Some(self.blocks.glass),
            Fallout::IceSnow if a > 0.7 => Some(self.blocks.snow),
            Fallout::IceSnow if a > 0.5 => Some(self.blocks.ice),
            _ => None,
        }
    }
}

// ---- fixA: meteorites placed before the compass kept its list (CompassService.updateArea on chunk load)

/// The `ae2:meteorite` structure set's placement (`random_spread`, linear).
const SPACING: i32 = 32;
const SEPARATION: i32 = 8;
const SALT: i64 = 124_895_654;
/// Columns checked a level tick (each is one column of block reads).
const SCANS_PER_TICK: usize = 4;

/// The centre (x, z) of a meteorite started in `chunk`, and the random as the rest of generatePieces finds it (the
/// worker and the compass's scan share these draws).
fn centre_of(seed: i64, chunk: [i32; 2]) -> (Random, [i32; 2]) {
    let mut random = Random::large_feature(seed, chunk[0], chunk[1]);
    let x = chunk[0] * 16 + random.next_int(16);
    let z = chunk[1] * 16 + random.next_int(16);
    (random, [x, z])
}

/// RandomSpreadStructurePlacement.getPotentialStructureChunk: the one chunk of `chunk`'s 32x32 region where the set
/// may start (WorldgenRandom.setLargeFeatureWithSalt, then two nextInt(spacing - separation)).
pub(crate) fn spread_chunk(seed: i64, chunk: [i32; 2]) -> [i32; 2] {
    let (rx, rz) = (chunk[0].div_euclid(SPACING), chunk[1].div_euclid(SPACING));
    let mut r = Random::new(
        i64::from(rx)
            .wrapping_mul(341_873_128_712)
            .wrapping_add(i64::from(rz).wrapping_mul(132_897_987_541))
            .wrapping_add(seed)
            .wrapping_add(SALT),
    );
    let x = rx * SPACING + r.next_int(SPACING - SEPARATION);
    let z = rz * SPACING + r.next_int(SPACING - SEPARATION);
    [x, z]
}

/// Where a meteorite's mysterious cube would be (x, z) if `chunk` holds one: the region's start chunk whose
/// MeteoriteStructure.findGenerationPoint coin came up (the biome check is left to the block itself).
pub(crate) fn candidate(seed: i64, chunk: [i32; 2]) -> Option<[i32; 2]> {
    if spread_chunk(seed, chunk) != chunk {
        return None;
    }
    if Random::large_feature(seed, chunk[0], chunk[1]).next(1) == 0 {
        return None;
    }
    Some(centre_of(seed, chunk).1)
}

#[derive(Default)]
struct Scan {
    world: Option<std::path::PathBuf>,
    /// Columns to look down: level and centre.
    queue: Vec<(String, [i32; 2])>,
    /// Start chunks already looked at this session.
    checked: std::collections::HashSet<(String, [i32; 2])>,
}
static SCAN: LazyLock<Mutex<Scan>> = LazyLock::new(Default::default);

/// `chunk_load`: a meteorite's start chunk queues its centre column for the cube (cheap: one chunk in 1024 at most).
pub(crate) fn chunk_loaded(host: &dyn ServerHost, dim: &str, chunk: [i32; 2]) {
    let Some(seed) = host.level(dim).and_then(|l| l.seed(host)) else { return };
    let Some(centre) = candidate(seed, chunk) else { return };
    let world = host.world_dir();
    let mut s = SCAN.lock().unwrap_or_else(|e| e.into_inner());
    if s.world != world {
        *s = Scan { world, ..Scan::default() };
    }
    let key = (dim.to_owned(), chunk);
    if s.queue.len() < 4096 && !s.checked.contains(&key) {
        s.queue.push((dim.to_owned(), centre));
    }
}

/// The level's tick: up to four queued columns looked down for an `ae2:mysterious_cube`; one found goes on the
/// compass's list (`handtools::note_cube`).
pub(crate) fn scan_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let work: Vec<[i32; 2]> = {
        let mut s = SCAN.lock().unwrap_or_else(|e| e.into_inner());
        if s.queue.is_empty() {
            return;
        }
        let mut mine = Vec::new();
        s.queue.retain(|(d, c)| {
            if d == dim && mine.len() < SCANS_PER_TICK {
                mine.push(*c);
                false
            } else {
                true
            }
        });
        mine
    };
    let h: &dyn Host = &*host;
    let Some(cube) = h.block_state_id("ae2:mysterious_cube") else { return };
    let Some((min_y, height)) = level.height(h) else { return };
    for [x, z] in work {
        if !level.is_loaded(h, [x, min_y, z]) {
            continue; // Gone again: looked at on its next load.
        }
        SCAN.lock().unwrap_or_else(|e| e.into_inner()).checked.insert((dim.to_owned(), [x >> 4, z >> 4]));
        if let Some(y) = (min_y..min_y + height).rev().find(|y| level.block_state(h, [x, *y, z]) == Some(cube)) {
            debug(&format!("compass: an older meteorite's cube at {x} {y} {z} in {dim}"));
            crate::handtools::note_cube(&*host, dim, [x, y, z], true);
        }
    }
}

#[cfg(test)]
mod fixa_tests {
    use super::*;

    /// Java's own numbers (java.util.Random through setLargeFeatureWithSalt, nextInt(24) twice; then
    /// setLargeFeatureSeed's nextBoolean and the centre's two nextInt(16)): seed, region, start chunk, coin, centre.
    const JAVA: [(i64, [i32; 2], [i32; 2], bool, [i32; 2]); 12] = [
        (12345, [0, 0], [18, 12], false, [295, 192]),
        (12345, [-1, 2], [-27, 74], true, [-424, 1189]),
        (12345, [7, -3], [230, -85], true, [3690, -1349]),
        (12345, [-20, -20], [-631, -617], true, [-10081, -9869]),
        (-4172144997902289642, [0, 0], [13, 6], false, [213, 96]),
        (-4172144997902289642, [-1, 2], [-30, 79], false, [-480, 1264]),
        (-4172144997902289642, [7, -3], [243, -73], true, [3903, -1153]),
        (-4172144997902289642, [-20, -20], [-624, -640], true, [-9971, -10233]),
        (0, [0, 0], [23, 0], true, [378, 3]),
        (0, [-1, 2], [-19, 73], false, [-301, 1168]),
        (0, [7, -3], [224, -94], false, [3591, -1497]),
        (0, [-20, -20], [-632, -637], true, [-10098, -10188]),
    ];

    #[test]
    fn candidate_chunks_match_java_and_the_worker() {
        for (seed, region, start, coin, centre) in JAVA {
            // Every chunk of the region names the same start chunk.
            for (dx, dz) in [(0, 0), (31, 0), (0, 31), (17, 9)] {
                assert_eq!(spread_chunk(seed, [region[0] * 32 + dx, region[1] * 32 + dz]), start, "{seed} {region:?}");
            }
            // The worker's centre (find_generation_point's draws) is the scan's.
            assert_eq!(centre_of(seed, start).1, centre);
            assert_eq!(candidate(seed, start), coin.then_some(centre), "{seed} {region:?}");
            // Its neighbours are no candidates; the centre lies in the start chunk.
            assert_eq!(candidate(seed, [start[0] + 1, start[1]]), None);
            assert_eq!([centre[0] >> 4, centre[1] >> 4], start);
        }
    }

    #[test]
    fn about_half_the_regions_have_a_start() {
        let mut n = 0;
        for rx in -16..16 {
            for rz in -16..16 {
                let s = spread_chunk(99, [rx * 32, rz * 32]);
                assert!((0..24).contains(&(s[0] - rx * 32)) && (0..24).contains(&(s[1] - rz * 32)));
                n += usize::from(candidate(99, s).is_some());
            }
        }
        assert!((400..624).contains(&n), "{n}");
    }
}
