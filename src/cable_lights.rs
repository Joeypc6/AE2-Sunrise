//! fixB: a plain smart cable block's channel lights, side by side (AE2's CableBuilder.addSmartConnection and
//! addStraightSmartConnection: each arm lights the channels carried on its own side, a smart cable one light a
//! channel and a dense smart one a light for each 4, up to 8, tinted with the colour's dark and light variants and
//! drawn full bright).
//!
//! A count for each side can't be block state (9^6 counts times the arms, past Sunrise's 2^20 states for the whole
//! game), so:
//! - the block's own model (chunk terrain) is the cable with every light off (`hz_cable/*_ch0`): body, item, breaking
//!   and particles as before, and what shows past the block entities' view distance;
//! - each smart cable block has a block entity (`ae2:smart_cable`, no ticks) holding the lights on each side
//!   (`lights`, 4 bits a side in `SIDES` order), written by the network's looks pass (`net::apply_looks`) only when
//!   they change;
//! - its `block_entity_submit` draws, over the dark lines, only the lit ones: per arm (or the straight run) the odd
//!   lights (`channels_0N`, tinted dark) and from 5 on the even ones (`channels_1N`, tinted light), as standalone
//!   cut-out terrain models (`hz_cable/lit/*`, tools/make_cables.py) a hair outside the dark lines, then `keep`s them:
//!   the game redraws the kept geometry with no call into the mod until the state or the lights change.

use crate::util::*;
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// The block entity type of the plain smart cables (`ae2:smart_cable`).
pub(crate) const TYPE: &str = "smart_cable";

/// AE2's colours as the cables name them, with AEColor's dark and light variants (blackVariant, whiteVariant): the
/// odd and the even channel lights (tools/make_cables.py TINTS).
const COLORS: [(&str, u32, u32); 17] = [
    ("white", 12500670, 16448250),
    ("orange", 16357177, 16047811),
    ("magenta", 8527490, 12949704),
    ("light_blue", 6458827, 14219007),
    ("yellow", 16775082, 16777192),
    ("lime", 8191818, 15202263),
    ("pink", 14454197, 16244459),
    ("gray", 8158332, 13224393),
    ("light_gray", 10329501, 15724527),
    ("cyan", 3120037, 11460084),
    ("purple", 8532146, 13083596),
    ("blue", 2959776, 14542591),
    ("brown", 7491125, 14734024),
    ("green", 4562977, 14938851),
    ("red", 10813481, 16770797),
    ("black", 2829099, 8684676),
    ("fluix", 1778500, 14138348),
];
/// The two smart kinds as the `hz_cable` models name them, by the block's name suffix.
const KINDS: [(&str, &str); 2] = [
    ("_smart_dense_cable", "dense_smart"),
    ("_smart_cable", "smart"),
];
/// Each axis's straight run: its model's axis name, by the lower of its two sides (`SIDES` order).
const STRAIGHT: [(usize, &str); 3] = [(0, "y"), (2, "z"), (4, "x")];
/// The light textures a lit overlay can use: the odd lights 1..4 and the even ones 1..4 (`channels_<tex>`).
const TEXTURES: [&str; 8] = ["01", "02", "03", "04", "11", "12", "13", "14"];

/// Every plain smart cable block (`ae2:red_smart_cable`, `ae2:fluix_smart_dense_cable`).
pub(crate) fn blocks() -> Vec<String> {
    KINDS
        .iter()
        .flat_map(|(suffix, _)| COLORS.iter().map(move |(c, ..)| format!("ae2:{c}{suffix}")))
        .collect()
}

/// A plain smart cable block as its models' kind (`smart`, `dense_smart`) and its colour's index in `COLORS`.
pub(crate) fn smart(block: &str) -> Option<(&'static str, usize)> {
    let name = block.strip_prefix("ae2:")?;
    KINDS.iter().find_map(|(suffix, kind)| {
        let color = name.strip_suffix(suffix)?;
        Some((*kind, COLORS.iter().position(|(c, ..)| *c == color)?))
    })
}

/// The lights a side shows for the channels it carries (SmartCableTextures): one a channel, a dense cable's one for
/// each 4, up to 8.
pub(crate) fn lights_of(kind: &str, channels: u32) -> u32 {
    if kind == "dense_smart" {
        channels.div_ceil(4).min(8)
    } else {
        channels.min(8)
    }
}

/// Each side's lights as the block entity keeps them: 4 bits a side, `SIDES` order.
pub(crate) fn pack(lights: [u32; 6]) -> i32 {
    (0..6).fold(0i32, |m, s| m | (lights[s].min(8) as i32) << (4 * s))
}

pub(crate) fn unpack(v: i64) -> [u32; 6] {
    std::array::from_fn(|s| ((v >> (4 * s)) & 15).min(8) as u32)
}

/// Every lit overlay's model id (registered as standalone models).
pub(crate) fn model_ids() -> Vec<String> {
    let mut out = Vec::new();
    for (_, kind) in KINDS {
        let pieces = STRAIGHT
            .iter()
            .map(|(_, a)| format!("straight_{a}"))
            .chain(SIDE_NAMES.iter().map(|s| format!("arm_{s}")));
        for piece in pieces {
            for tex in TEXTURES {
                out.push(format!("ae2:block/hz_cable/lit/{kind}_{piece}_{tex}"));
            }
        }
    }
    out
}

/// What a smart cable with arms on the `on` sides (`core`: its `x_core`) and `lights` on each side draws over its
/// dark lines: each lit arm's (or the straight run's) overlays and their tints. The pieces are the block model's own
/// (make_cables.py `multipart`): one straight run for exactly two opposite arms and no core, else an arm a side.
pub(crate) fn light_pieces(
    kind: &str,
    color: usize,
    on: u32,
    core: bool,
    lights: [u32; 6],
) -> Vec<(String, u32)> {
    let (_, dark, light) = COLORS[color.min(COLORS.len() - 1)];
    let pieces: Vec<(String, u32)> =
        match STRAIGHT.iter().find(|(lo, _)| !core && on & 63 == 3 << lo) {
            Some((lo, axis)) => vec![(format!("straight_{axis}"), lights[*lo])],
            None => (0..6)
                .filter(|s| on >> s & 1 == 1)
                .map(|s| (format!("arm_{}", SIDE_NAMES[s]), lights[s]))
                .collect(),
        };
    let mut out = Vec::new();
    for (piece, n) in pieces {
        let n = n.min(8);
        if n == 0 {
            continue;
        }
        // light_textures: the odd lights `channels_0<min(n,4)>`, the even ones `channels_1<n-4>` (none lit below 5:
        // the block's own dark lines show).
        out.push((
            format!("ae2:block/hz_cable/lit/{kind}_{piece}_0{}", n.min(4)),
            dark,
        ));
        if n >= 5 {
            out.push((
                format!("ae2:block/hz_cable/lit/{kind}_{piece}_1{}", n - 4),
                light,
            ));
        }
    }
    out
}

// ---- The client: the overlays, kept

static MODELS: Mutex<Option<HashMap<String, ModelRef>>> = Mutex::new(None);

/// Looks the overlays up once a model load is done (`model_baking_completed`).
pub(crate) fn resolve_models(host: &mut dyn ClientHost) {
    let assets = host.render_assets();
    let ids = model_ids();
    let table: HashMap<String, ModelRef> = ids
        .iter()
        .filter_map(|id| {
            let model = assets.model(&Identifier::parse(id).ok()?).ok()?;
            Some((id.clone(), model))
        })
        .collect();
    // fixE: which overlays the game didn't bake (the in-game test reads it).
    let missing: Vec<&String> = ids.iter().filter(|id| !table.contains_key(*id)).collect();
    debug(&format!("cable lights: {} of {} overlays baked; missing {:?}", table.len(), ids.len(), missing.iter().take(8).collect::<Vec<_>>()));
    *MODELS.lock().unwrap_or_else(|e| e.into_inner()) = Some(table);
}

/// The block entity's lights as the server sent them (`lights`; a guide scene's SNBT gives an int).
fn lights_in(nbt: &Compound) -> [u32; 6] {
    unpack(nbt_i64(nbt, "lights"))
}

fn submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let Some((kind, color)) = smart(event.block) else {
        return;
    };
    let states = host.block_states();
    let is = |key: &str| event.state.property(&states, key).ok().flatten() == Some("true");
    let on = (0..6)
        .filter(|s| is(SIDE_NAMES[*s]))
        .fold(0u32, |m, s| m | 1 << s);
    let pieces = light_pieces(kind, color, on, is("x_core"), lights_in(&event.entity.nbt));
    // Drawn from the state and the block entity alone: the game keeps it until either changes.
    if draw_lights(&mut *event.draw, &pieces) {
        event.draw.keep();
    }
}

/// fixE: draws `pieces` (`light_pieces`) over a smart cable's dark lines, for a plain cable's block entity and a cable
/// bus cell's cable alike: cut out, tinted, unshaded (AE2's lights are emissive quads, no diffuse shading). False when
/// the overlays aren't baked yet or one was refused (then nothing is kept and the next frame asks again).
pub(crate) fn draw_lights(draw: &mut dyn BlockEntityDrawContext, pieces: &[(String, u32)]) -> bool {
    let models: Vec<(ModelRef, u32, &str)> = {
        let table = MODELS.lock().unwrap_or_else(|e| e.into_inner());
        // Not baked yet: nothing kept, so the next frame asks again.
        let Some(table) = table.as_ref() else { return false };
        pieces
            .iter()
            .filter_map(|(id, tint)| match table.get(id) {
                Some(m) => Some((*m, *tint, id.as_str())),
                None => {
                    note_failure(id, "not baked");
                    None
                }
            })
            .collect()
    };
    let mut ok = models.len() == pieces.len();
    for (m, tint, id) in models {
        let options = TerrainModelOptions::default()
            .with_material(TerrainMaterial::Cutout)
            .with_shade(TerrainShade::Off)
            .with_tint(tint);
        if let Err(e) = draw.terrain_model(m, options) {
            note_failure(id, &format!("{e:?}"));
            ok = false;
        }
    }
    ok
}

/// fixE: an overlay that didn't draw, logged once each (what the in-game test reads).
fn note_failure(id: &str, why: &str) {
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    if SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(id.to_owned()) {
        debug(&format!("cable lights: {id} not drawn: {why}"));
    }
}

/// The smart cables' block entity: its update tag (the lights) and its draw. It never ticks.
pub(crate) struct SmartCable;

/// No block hooks of its own (the cables' are the network's and the bus's).
#[mod_api::sunrise_part]
impl BlockPart for SmartCable {}

#[mod_api::sunrise_part]
impl BlockEntityPart for SmartCable {
    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        let mut out = Compound::new();
        out.insert(java("lights"), Tag::Int(pack(lights_in(&event.entity.nbt))));
        Some(out)
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        submit(host, event);
    }
}

pub(crate) fn register(r: &mut dyn Registrar) {
    let blocks = blocks();
    let names: Vec<&str> = blocks.iter().map(String::as_str).collect();
    let ty = r.block_entity_type(TYPE).blocks(&names).build();
    r.block_entity_part(&ty, SmartCable)
        .expect("the smart cables' block entity type is native");
}

// ---- The server: the lights follow the network

/// Smart cables found without their block entity (placed before this version): their state was set again once to
/// make one (`LevelChunk.setBlockState` adds a missing block entity); never twice, so a refusal costs no re-mesh.
static REMADE: Mutex<Option<HashSet<(&'static str, Pos)>>> = Mutex::new(None);

/// After a rebuild: the smart cable at `p` (block `name`) shows `channels` on each side. Writes its block entity
/// only when its lights change (each write sends the block entity to the clients that see it).
pub(crate) fn apply(
    host: &mut dyn ServerHost,
    level: LevelRef,
    dim: &'static str,
    p: Pos,
    name: &str,
    channels: [u32; 6],
) {
    let Some((kind, _)) = smart(name) else { return };
    let want = pack(std::array::from_fn(|s| lights_of(kind, channels[s])));
    let Some(be) = host.block_entity(level, p) else {
        let first = REMADE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashSet::new)
            .insert((dim, p));
        if first && let Some(core) = level.block_property(&*host, p, "x_core") {
            // Clients only, shapes known: the arms stay as they are.
            let other = if core == "true" { "false" } else { "true" };
            host.set_block_property_with(p, "x_core", other, 2 | 16);
            host.set_block_property_with(p, "x_core", &core, 2 | 16);
            debug(&format!(
                "smart cable lights: {p:?} had no block entity; state set again"
            ));
        }
        return;
    };
    let have = be.data(&*host).map(|d| nbt_i64(&d, "lights"));
    if have == Some(i64::from(want)) {
        return;
    }
    let mut data = Compound::new();
    data.insert(java("lights"), Tag::Int(want));
    let _ = be.merge_data(host, data);
    debug(&format!(
        "smart cable lights: {p:?} {:?}",
        unpack(i64::from(want))
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_smart_cables_and_their_colours() {
        let all = blocks();
        assert_eq!(all.len(), 34);
        assert_eq!(smart("ae2:fluix_smart_cable"), Some(("smart", 16)));
        assert_eq!(
            smart("ae2:light_blue_smart_dense_cable"),
            Some(("dense_smart", 3))
        );
        assert_eq!(smart("ae2:red_covered_cable"), None);
        assert_eq!(smart("ae2:red_glass_cable"), None);
        assert_eq!(smart("ae2:hz_fcable_red_smart_cable"), None);
        assert!(
            all.iter()
                .all(|b| smart(b).is_some() && crate::net::cable(b).is_some())
        );
    }

    #[test]
    fn a_light_a_channel_and_a_dense_light_each_four() {
        assert_eq!(
            [0, 1, 5, 8, 9, 32].map(|c| lights_of("smart", c)),
            [0, 1, 5, 8, 8, 8]
        );
        assert_eq!(
            [0, 1, 4, 5, 13, 32, 40].map(|c| lights_of("dense_smart", c)),
            [0, 1, 1, 2, 4, 8, 8]
        );
        let l = [0, 8, 3, 1, 7, 2];
        assert_eq!(unpack(i64::from(pack(l))), l);
        assert_eq!(unpack(i64::from(pack([9; 6]))), [8; 6]);
        assert!(pack([8; 6]) > 0);
    }

    #[test]
    fn each_arm_lights_its_own_side() {
        let red = smart("ae2:red_smart_cable").unwrap();
        // Arms north and east (a corner): each with its own count; a side with no lights adds nothing.
        let on = 1 << 2 | 1 << 5;
        assert_eq!(
            light_pieces(red.0, red.1, on, false, [0, 0, 3, 0, 0, 6]),
            [
                (
                    "ae2:block/hz_cable/lit/smart_arm_north_03".to_owned(),
                    10813481
                ),
                (
                    "ae2:block/hz_cable/lit/smart_arm_east_04".to_owned(),
                    10813481
                ),
                (
                    "ae2:block/hz_cable/lit/smart_arm_east_12".to_owned(),
                    16770797
                ),
            ]
        );
        // Lights on a side without an arm draw nothing; no lights, no overlay.
        assert!(light_pieces(red.0, red.1, on, false, [5, 5, 0, 5, 5, 0]).is_empty());
        assert!(light_pieces(red.0, red.1, 63, false, [0; 6]).is_empty());
    }

    #[test]
    fn a_straight_run_is_one_overlay_by_its_lower_side() {
        let fluix = smart("ae2:fluix_smart_dense_cable").unwrap();
        // West and east only: the x run, lit as its west side; with `x_core` it is two arms.
        let on = 1 << 4 | 1 << 5;
        assert_eq!(
            light_pieces(fluix.0, fluix.1, on, false, [0, 0, 0, 0, 2, 2]),
            [(
                "ae2:block/hz_cable/lit/dense_smart_straight_x_02".to_owned(),
                1778500
            )]
        );
        assert_eq!(
            light_pieces(fluix.0, fluix.1, on, true, [0, 0, 0, 0, 2, 0]),
            [(
                "ae2:block/hz_cable/lit/dense_smart_arm_west_02".to_owned(),
                1778500
            )]
        );
        // Down and up with all eight lit: odd and even overlays.
        assert_eq!(
            light_pieces(fluix.0, fluix.1, 0b11, false, [8, 8, 0, 0, 0, 0]),
            [
                (
                    "ae2:block/hz_cable/lit/dense_smart_straight_y_04".to_owned(),
                    1778500
                ),
                (
                    "ae2:block/hz_cable/lit/dense_smart_straight_y_14".to_owned(),
                    14138348
                ),
            ]
        );
    }

    #[test]
    fn the_dense_cables_seen_in_game_light_their_lines() {
        // fixE: a plain dense cable, `lights` 4352: north and south one light each, a straight z run (its one overlay).
        let fluix = smart("ae2:fluix_smart_dense_cable").unwrap();
        let l = unpack(4352);
        assert_eq!(l, [0, 0, 1, 1, 0, 0]);
        assert_eq!(
            light_pieces(fluix.0, fluix.1, 0b001100, false, l),
            [("ae2:block/hz_cable/lit/dense_smart_straight_z_01".to_owned(), 1778500)]
        );
        // A dense cable with a part (`lights` 1118464): an arm each on north, south, west and east.
        let l = unpack(1118464);
        assert_eq!(l, [0, 0, 1, 1, 1, 1]);
        assert_eq!(light_pieces(fluix.0, fluix.1, 0b111100, true, l).len(), 4);
        // The smart cable bus's `lights` 2293776: up 1, west 3, east 2.
        assert_eq!(unpack(2293776), [0, 1, 0, 0, 3, 2]);
    }

    #[test]
    fn every_overlay_is_a_generated_model() {
        let ids = model_ids();
        assert_eq!(ids.len(), 2 * 9 * 8);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ae2/models");
        for id in &ids {
            let rel = id.strip_prefix("ae2:").unwrap();
            assert!(root.join(format!("{rel}.json")).exists(), "{id}");
        }
        // Every piece any count can ask for is among them.
        for (kind, color) in [
            smart("ae2:white_smart_cable").unwrap(),
            smart("ae2:black_smart_dense_cable").unwrap(),
        ] {
            for on in 0..64u32 {
                for n in 0..=8 {
                    for core in [false, true] {
                        for (id, _) in light_pieces(kind, color, on, core, [n; 6]) {
                            assert!(ids.contains(&id), "{id}");
                        }
                    }
                }
            }
        }
    }
}
