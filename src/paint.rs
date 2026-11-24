//! AE2's colours (AEColor) and its paint splotches (PaintSplotchesBlock, PaintSplotchesBlockEntity, Splotch,
//! PaintSplotchesBakedModel): a matter cannon's paint ball leaves a splotch on the face it hits, kept by a paint block
//! in front of that face (21 at most, the oldest going first); lumen paint lights the block (12 for one lumen splotch,
//! 15 for two or more: the content's `hz_paint_lit1`/`hz_paint_lit2` twins stand for AE2's `light_level` 1 and 2);
//! a face that stops being sturdy loses its splotches; the colour applicator's snowball cleans a face. Upstream's
//! `add_splotch`, `clean_splotches`, `paint_light` and `paint_submit` (`../ae2-horizon/mixins/ae2/me.rs`).
//!
//! The splotches are saved as AE2 saves them (`dots`, a byte array: the count, then two bytes a splotch), so the
//! block entity's data reads the same as Java's.

use crate::util::*;
use mod_api::*;
use std::sync::Mutex;

// ---- AEColor

/// AEColor's order: the sixteen dye colours, then TRANSPARENT ("fluix", the cables' uncoloured colour).
pub(crate) const COLORS: [&str; 17] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
    "fluix",
];
pub(crate) const TRANSPARENT: usize = 16;
/// AEColor.blackVariant, mediumVariant and whiteVariant.
pub(crate) const BLACK_VARIANT: [u32; 17] = [
    12500670, 16357177, 8527490, 6458827, 16775082, 8191818, 14454197, 8158332, 10329501, 3120037,
    8532146, 2959776, 7491125, 4562977, 10813481, 2829099, 1778500,
];
pub(crate) const MEDIUM_VARIANT: [u32; 17] = [
    14408667, 16428612, 12069560, 8563943, 16318282, 12320593, 16299479, 10526880, 13487565,
    5352134, 10769358, 5327615, 12031615, 6349614, 16711740, 5658198, 9002152,
];
pub(crate) const WHITE_VARIANT: [u32; 17] = [
    16448250, 16047811, 12949704, 14219007, 16777192, 15202263, 16244459, 13224393, 15724527,
    11460084, 13083596, 14542591, 14734024, 14938851, 16770797, 8684676, 14138348,
];
/// AEColor's translation keys (gui.ae2.<Name>).
pub(crate) const COLOR_KEYS: [&str; 17] = [
    "gui.ae2.White",
    "gui.ae2.Orange",
    "gui.ae2.Magenta",
    "gui.ae2.LightBlue",
    "gui.ae2.Yellow",
    "gui.ae2.Lime",
    "gui.ae2.Pink",
    "gui.ae2.Gray",
    "gui.ae2.LightGray",
    "gui.ae2.Cyan",
    "gui.ae2.Purple",
    "gui.ae2.Blue",
    "gui.ae2.Brown",
    "gui.ae2.Green",
    "gui.ae2.Red",
    "gui.ae2.Black",
    "gui.ae2.Fluix",
];

pub(crate) fn color_index(name: &str) -> Option<usize> {
    COLORS.iter().position(|c| *c == name)
}

/// PaintBallItem: an `ae2:<colour>_paint_ball` or `ae2:<colour>_lumen_paint_ball`, its colour and whether it is lumen.
pub(crate) fn paint_ball(item: &str) -> Option<(usize, bool)> {
    let name = item.strip_prefix("ae2:")?.strip_suffix("_paint_ball")?;
    let (name, lumen) = match name.strip_suffix("_lumen") {
        Some(n) => (n, true),
        None => (name, false),
    };
    let c = color_index(name).filter(|c| *c < TRANSPARENT)?;
    Some((c, lumen))
}

// ---- Splotches (pure)

/// Splotch: where on its face (`pos`: x in the low four bits, y in the high four, each 0..15), the paint block's side
/// toward the painted block (Direction's ordinal: down, up, north, south, west, east), the colour and lumen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Splotch {
    pub(crate) pos: u8,
    pub(crate) side: u8,
    pub(crate) color: u8,
    pub(crate) lumen: bool,
}

impl Splotch {
    /// Splotch's constructor: the hit point `local` (in the paint block, 0..1) across the face of `side`.
    pub(crate) fn new(color: usize, lumen: bool, side: usize, local: [f64; 3]) -> Splotch {
        let (x, y) = match side {
            2 | 3 => (local[0], local[1]),
            0 | 1 => (local[0], local[2]),
            _ => (local[1], local[2]),
        };
        // (int) casts: towards zero, and the four bits kept as Java's `a | b << 4` keeps them in a byte.
        let a = (x * 15.0) as i32;
        let b = (y * 15.0) as i32;
        Splotch {
            pos: (a | (b << 4)) as u8,
            side: side as u8 % 6,
            color: color.min(16) as u8,
            lumen,
        }
    }

    fn val(self) -> u8 {
        self.side | (self.color << 3) | if self.lumen { 128 } else { 0 }
    }

    pub(crate) fn x(self) -> f32 {
        f32::from(self.pos & 15) / 15.0
    }

    pub(crate) fn y(self) -> f32 {
        f32::from((self.pos >> 4) & 15) / 15.0
    }

    /// Splotch.getSeed: Java reads the position as a signed byte.
    pub(crate) fn seed(self) -> usize {
        (i32::from(self.pos as i8) + i32::from(self.val())).unsigned_abs() as usize
    }
}

/// PaintSplotchesBlockEntity.readBuffer.
pub(crate) fn decode(bytes: &[u8]) -> Vec<Splotch> {
    let Some((&n, rest)) = bytes.split_first() else {
        return Vec::new();
    };
    rest.chunks_exact(2)
        .take(usize::from(n))
        .map(|c| Splotch {
            pos: c[0],
            side: (c[1] & 7) % 6,
            color: ((c[1] >> 3) & 15).min(15),
            lumen: c[1] & 128 != 0,
        })
        .collect()
}

/// PaintSplotchesBlockEntity.writeBuffer.
pub(crate) fn encode(dots: &[Splotch]) -> Vec<u8> {
    let mut out = vec![dots.len().min(255) as u8];
    for d in dots.iter().take(255) {
        out.push(d.pos);
        out.push(d.val());
    }
    out
}

/// addBlot's list: more than 20 already, the oldest goes.
pub(crate) fn push_dot(dots: &mut Vec<Splotch>, dot: Splotch) {
    if dots.len() > 20 {
        dots.remove(0);
    }
    dots.push(dot);
}

/// updateData's light level: the lumen splotches, two at most.
pub(crate) fn light_level(dots: &[Splotch]) -> usize {
    dots.iter().filter(|d| d.lumen).count().min(2)
}

// ---- The paint block (server)

pub(crate) const PAINT: &str = "ae2:paint";
const PAINT_BLOCKS: [&str; 3] = [PAINT, "ae2:hz_paint_lit1", "ae2:hz_paint_lit2"];
const DOTS: &str = "dots";

pub(crate) fn is_paint(block: &str) -> bool {
    PAINT_BLOCKS.contains(&block)
}

fn dots_of(nbt: &Compound) -> Vec<Splotch> {
    match nbt.get(&java(DOTS)) {
        Some(Tag::ByteArray(b)) => decode(b),
        _ => Vec::new(),
    }
}

fn read_dots(host: &dyn Host, level: LevelRef, pos: Pos) -> (Compound, Vec<Splotch>) {
    let nbt = host
        .block_entity(level, pos)
        .and_then(|b| b.data(host))
        .unwrap_or_default();
    let dots = dots_of(&nbt);
    (nbt, dots)
}

/// The paint block's side toward the face it paints is sturdy (`isSideValid`: the neighbour's face is sturdy).
fn side_valid(host: &dyn Host, level: LevelRef, pos: Pos, side: usize) -> bool {
    let p = add(pos, SIDES[side]);
    let face = [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::South,
        BlockFace::North,
        BlockFace::East,
        BlockFace::West,
    ][side];
    level
        .block_face_sturdy(host, p, face, SupportType::Full)
        .unwrap_or(false)
}

/// The splotches written back and the block as they make it (updateData): none left removes it; else the light level's
/// block, its data kept.
fn write_dots(
    host: &mut dyn ServerHost,
    level: LevelRef,
    pos: Pos,
    mut nbt: Compound,
    dots: &[Splotch],
) {
    let flags = BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS;
    if dots.is_empty() {
        if let Some(air) = host.block_state_id("minecraft:air") {
            let _ = level.set_block(host, pos, air, flags);
        }
        return;
    }
    nbt.insert(java(DOTS), Tag::ByteArray(encode(dots)));
    let want = PAINT_BLOCKS[light_level(dots)];
    if crate::net::block_id(&*host, level, pos) != want
        && let Some(s) = host.block_state_id(want)
    {
        let _ = level.set_block(host, pos, s, flags);
    }
    if let Some(be) = host.block_entity(level, pos) {
        let _ = be.replace_data(host, nbt);
    }
}

/// MatterCannonItem.shootPaintBalls on a block: the paint block in front of the face hit (`face`, out of the block at
/// `hit_block`) takes a splotch where the ray met it. The player's permission is checked by the caller.
pub(crate) fn add_blot(
    host: &mut dyn ServerHost,
    level: LevelRef,
    hit_block: Pos,
    face: usize,
    at: [f64; 3],
    ball: &str,
) -> bool {
    let Some((color, lumen)) = paint_ball(ball) else {
        return false;
    };
    let pos = add(hit_block, SIDES[face]);
    let here = crate::net::block_id(&*host, level, pos);
    if !is_paint(&here) {
        // `canBeReplaced() && isEmptyBlock`: only into air.
        if !matches!(
            here.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        ) {
            return false;
        }
        let Some(s) = host.block_state_id(PAINT) else {
            return false;
        };
        if !matches!(
            level.set_block(
                host,
                pos,
                s,
                BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS
            ),
            Ok(true)
        ) {
            return false;
        }
    }
    // The paint block's side toward the painted block.
    let side = face ^ 1;
    let (nbt, mut dots) = read_dots(&*host, level, pos);
    if !side_valid(&*host, level, pos, side) {
        if dots.is_empty() {
            write_dots(host, level, pos, nbt, &dots);
        }
        return false;
    }
    let local = [
        at[0] - f64::from(pos[0]),
        at[1] - f64::from(pos[1]),
        at[2] - f64::from(pos[2]),
    ];
    push_dot(&mut dots, Splotch::new(color, lumen, side, local));
    write_dots(host, level, pos, nbt, &dots);
    true
}

/// PaintSplotchesBlockEntity.cleanSide: the splotches on the paint block at `pos` toward `side` come off. False when
/// there is no paint there.
pub(crate) fn clean_side(
    host: &mut dyn ServerHost,
    level: LevelRef,
    pos: Pos,
    side: usize,
) -> bool {
    if !is_paint(&crate::net::block_id(&*host, level, pos)) {
        return false;
    }
    let (nbt, mut dots) = read_dots(&*host, level, pos);
    dots.retain(|d| usize::from(d.side) != side);
    write_dots(host, level, pos, nbt, &dots);
    true
}

/// Paint blocks whose neighbours changed, checked at the level's tick end (a neighbour update can't write).
static CHANGED: Mutex<Vec<(&'static str, Pos)>> = Mutex::new(Vec::new());

/// The paint block's Part: neighborChanged (its splotches on faces gone soft go).
pub(crate) struct PaintBlock;

#[mod_api::sunrise_part]
impl BlockPart for PaintBlock {
    fn neighbor_changed(&self, _level: &mut dyn LevelWriteHost, event: &mut NeighborChangedEvent) {
        let mut g = CHANGED.lock().unwrap_or_else(|e| e.into_inner());
        if g.len() < 4096 {
            g.push((
                event.dimension.key().id(),
                [event.pos.x, event.pos.y, event.pos.z],
            ));
        }
    }
}

/// The level's tick end: the changed paint blocks' sides checked again.
pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let todo: Vec<Pos> = {
        let mut g = CHANGED.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_empty() {
            return;
        }
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *g)
            .into_iter()
            .partition(|(d, _)| *d == dim);
        *g = rest;
        here.into_iter().map(|(_, p)| p).collect()
    };
    for pos in todo {
        if !is_paint(&crate::net::block_id(&*host, level, pos)) {
            continue;
        }
        let (nbt, mut dots) = read_dots(&*host, level, pos);
        let before = dots.len();
        dots.retain(|d| side_valid(&*host, level, pos, usize::from(d.side)));
        if dots.len() != before || dots.is_empty() {
            write_dots(host, level, pos, nbt, &dots);
        }
    }
}

/// A block placed into a face covered by paint takes the paint's place (the paint block is replaceable in AE2): the
/// paint in front of the clicked face goes before the placement looks there.
pub(crate) fn clear_for_place(host: &mut dyn ServerHost, click: &BlockClick<'_>) {
    let Some(stack) = click.stack else { return };
    if host.block_state_id(crate::cells::sid(stack)).is_none() {
        return;
    }
    let Some(level) = here(&*host) else { return };
    let at = add(click.pos, click.hit.face);
    if is_paint(&crate::net::block_id(&*host, level, at))
        && let Some(air) = host.block_state_id("minecraft:air")
    {
        let _ = level.set_block(
            host,
            at,
            air,
            BlockUpdateFlags::NEIGHBORS | BlockUpdateFlags::CLIENTS,
        );
    }
}

// ---- Clients

/// What clients draw (PaintSplotchesBlockEntity.writeToStream).
pub(crate) fn update_tag(nbt: &Compound) -> Option<Compound> {
    let dots = nbt.get(&java(DOTS))?;
    let mut out = Compound::new();
    out.insert(java(DOTS), dots.clone());
    Some(out)
}

/// PaintSplotchesBakedModel: each splotch a fifth of a block square on its face, just off it (each a thousandth further
/// out than the one before), one of the three paint textures by its seed, tinted its colour's medium variant (lumen:
/// the white variant, drawn full bright).
pub(crate) fn submit(_host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let dots = dots_of(&event.entity.nbt);
    if dots.is_empty() {
        return;
    }
    const SPRITES: [&str; 3] = ["ae2:block/paint1", "ae2:block/paint2", "ae2:block/paint3"];
    let draw = &mut *event.draw;
    let base_light = draw.light();
    let mut offset_constant = 0.001f32;
    for d in dots {
        let color = usize::from(d.color).min(16);
        let tint = if d.lumen {
            WHITE_VARIANT[color]
        } else {
            MEDIUM_VARIANT[color]
        };
        draw.set_light(if d.lumen { 0xF0F0 } else { base_light });
        let offset = offset_constant;
        offset_constant += 0.001;
        let (px, py) = (d.x().clamp(0.1, 0.9), d.y().clamp(0.1, 0.9));
        let sprite = SPRITES[d.seed() % 3];
        let face = Some(CubeFace::new(sprite).with_uv([0.0, 0.0, 16.0, 16.0]));
        // The face drawn faces away from the painted block: the side's opposite.
        let (lo, hi, facing) = match d.side {
            1 => (
                [px - 0.1, 1.0 - offset, py - 0.1],
                [px + 0.1, 1.0 - offset, py + 0.1],
                0,
            ),
            0 => (
                [px - 0.1, offset, py - 0.1],
                [px + 0.1, offset, py + 0.1],
                1,
            ),
            5 => (
                [1.0 - offset, px - 0.1, py - 0.1],
                [1.0 - offset, px + 0.1, py + 0.1],
                4,
            ),
            4 => (
                [offset, px - 0.1, py - 0.1],
                [offset, px + 0.1, py + 0.1],
                5,
            ),
            3 => (
                [px - 0.1, py - 0.1, 1.0 - offset],
                [px + 0.1, py + 0.1, 1.0 - offset],
                2,
            ),
            _ => (
                [px - 0.1, py - 0.1, offset],
                [px + 0.1, py + 0.1, offset],
                3,
            ),
        };
        let mut faces: [Option<CubeFace<'_>>; 6] = [None, None, None, None, None, None];
        faces[facing] = face;
        draw.cube(
            lo.map(|v| v * 16.0),
            hi.map(|v| v * 16.0),
            faces,
            0xFF00_0000 | tint,
        );
    }
    draw.set_light(base_light);
}

pub(crate) fn register(r: &mut dyn Registrar) {
    for b in PAINT_BLOCKS {
        let k = r.block_key(b);
        r.block_part(k, PaintBlock);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_balls() {
        assert_eq!(paint_ball("ae2:red_paint_ball"), Some((14, false)));
        assert_eq!(
            paint_ball("ae2:light_blue_lumen_paint_ball"),
            Some((3, true))
        );
        assert_eq!(paint_ball("ae2:fluix_paint_ball"), None);
        assert_eq!(paint_ball("minecraft:red_dye"), None);
        assert_eq!(COLORS[TRANSPARENT], "fluix");
    }

    #[test]
    fn splotch_round_trip() {
        // A hit on a north face at x 0.5, y 0.25: a = 7, b = 3.
        let s = Splotch::new(14, true, 2, [0.5, 0.25, 0.0]);
        assert_eq!((s.pos & 15, s.pos >> 4), (7, 3));
        assert!((s.x() - 7.0 / 15.0).abs() < 1e-6 && (s.y() - 3.0 / 15.0).abs() < 1e-6);
        // Up and down faces read x and z; east and west y and z.
        assert_eq!(Splotch::new(0, false, 1, [0.0, 0.9, 1.0]).pos, 15 << 4);
        assert_eq!(Splotch::new(0, false, 5, [0.9, 1.0, 0.0]).pos, 15);
        let dots = vec![s, Splotch::new(3, false, 0, [0.99, 0.0, 0.99])];
        assert_eq!(decode(&encode(&dots)), dots);
        assert_eq!(decode(&[]), Vec::new());
        assert_eq!(decode(&[5, 1]), Vec::new());
    }

    #[test]
    fn splotch_seed_is_javas() {
        // pos 0xF7 reads as -9 in Java; val = side 2 | colour 14 << 3 | 128 = 242: |−9 + 242| = 233.
        let s = Splotch {
            pos: 0xF7,
            side: 2,
            color: 14,
            lumen: true,
        };
        assert_eq!(s.seed(), 233);
    }

    #[test]
    fn oldest_splotch_goes_and_lumens_light() {
        let mut dots = Vec::new();
        for i in 0..25u8 {
            push_dot(
                &mut dots,
                Splotch {
                    pos: i,
                    side: 0,
                    color: 0,
                    lumen: false,
                },
            );
        }
        assert_eq!(dots.len(), 21);
        assert_eq!(dots[0].pos, 4);
        assert_eq!(light_level(&dots), 0);
        dots[3].lumen = true;
        assert_eq!(light_level(&dots), 1);
        dots[5].lumen = true;
        dots[6].lumen = true;
        assert_eq!(light_level(&dots), 2);
    }
}
