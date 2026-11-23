//! Small shared pieces: positions and sides, NBT keys, the block entity's fields.

use mod_api::*;

pub(crate) type Pos = [i32; 3];

/// `ModBlockEntity`'s fields (`nbt`, `items`, `data`); the type isn't exported by name.
pub(crate) type Fields = <ModBlockEntity as std::ops::Deref>::Target;

/// The compound's key type: the SDK's own, kept under this path for the port's modules.
pub(crate) use mod_api::JavaString;

pub(crate) fn java(s: &str) -> JavaString {
    JavaString::from(s)
}

/// The six sides, in AE2's (and Direction's) order: down, up, north, south, west, east.
pub(crate) const SIDES: [Pos; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
pub(crate) const SIDE_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// A step from `p`. A hosted part steps from its cable's cell (its target is the cell's real neighbour), as
/// upstream's `add` does.
pub(crate) fn add(p: Pos, d: Pos) -> Pos {
    let p = real(p);
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

// ---- Hosted parts (the cable bus): each part on a cable's cell is a network node of its own at a position past the
// world's top (same x and z, y `1024 + (cell y + 64) * 6 + side`), as upstream; nothing is placed there.

pub(crate) const HOSTED_Y: i32 = 1024;

/// The node position of the part on `side` of the cell at `cell`.
pub(crate) fn hosted(cell: Pos, side: usize) -> Pos {
    [cell[0], HOSTED_Y + (cell[1] + 64) * 6 + side as i32, cell[2]]
}

pub(crate) fn is_hosted(p: Pos) -> bool {
    p[1] >= HOSTED_Y
}

/// A hosted part's cell and side.
pub(crate) fn host_cell(p: Pos) -> (Pos, usize) {
    let n = p[1] - HOSTED_Y;
    ([p[0], n / 6 - 64, p[2]], (n % 6) as usize)
}

/// fixF: a position for the log: a hosted part's as its cell and side (`[219, 2010, 200]` is the north part of the
/// cell at `[219, 100, 200]`), else itself.
pub(crate) fn pos_text(p: Pos) -> String {
    if is_hosted(p) {
        let (c, s) = host_cell(p);
        format!("{c:?} {} part", SIDE_NAMES[s])
    } else {
        format!("{p:?}")
    }
}

/// Where a position is in the world: a hosted part's cell, else itself.
pub(crate) fn real(p: Pos) -> Pos {
    if is_hosted(p) { host_cell(p).0 } else { p }
}

/// A position as a grid node id.
pub(crate) fn pack(p: Pos) -> u64 {
    ((p[0] as i64 as u64 & 0x3FF_FFFF) << 38) | ((p[2] as i64 as u64 & 0x3FF_FFFF) << 12) | (p[1] as i64 as u64 & 0xFFF)
}

/// A `facing` value as a step.
pub(crate) fn facing_dir(facing: Option<&str>) -> Pos {
    match facing {
        Some("down") => [0, -1, 0],
        Some("up") => [0, 1, 0],
        Some("south") => [0, 0, 1],
        Some("west") => [-1, 0, 0],
        Some("east") => [1, 0, 0],
        _ => [0, 0, -1],
    }
}

pub(crate) fn nbt_i64(nbt: &Compound, key: &str) -> i64 {
    match nbt.get(&java(key)) {
        Some(Tag::Long(v)) => *v,
        Some(Tag::Int(v)) => i64::from(*v),
        Some(Tag::Short(v)) => i64::from(*v),
        Some(Tag::Byte(v)) => i64::from(*v),
        _ => 0,
    }
}

pub(crate) fn set_nbt_i64(nbt: &mut Compound, key: &str, value: i64) {
    nbt.insert(java(key), Tag::Long(value));
}

/// The level the host has in place.
pub(crate) fn here(host: &dyn Host) -> Option<LevelRef> {
    host.level(host.dimension())
}

/// A debug line in `ae2-debug.log` in the game's folder, written only while a file `ae2-debug` is there (tests).
pub(crate) fn debug(line: &str) {
    use std::io::Write;
    if std::path::Path::new("ae2-debug").exists()
        && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("ae2-debug.log")
    {
        let _ = writeln!(f, "{line}");
    }
}

/// The fork's `net_send_to_player` on Sunrise's raw payloads: the same bytes on the same channel (declared in
/// `register`); an error when the player is gone or the send is refused.
pub(crate) fn send_to_player(host: &mut dyn ServerHost, player: u128, channel: &str, bytes: &[u8]) -> Result<(), ()> {
    let target = host.player(player).ok_or(())?;
    target.send_payload(host, &PayloadId(channel.to_owned()), PayloadBytes(bytes.to_vec())).map(|_| ()).map_err(|_| ())
}

/// The fork's `net_send_to_server`, likewise.
pub(crate) fn send_to_server(host: &mut dyn ClientHost, channel: &str, bytes: &[u8]) -> Result<(), ()> {
    host.send_payload_to_server(&PayloadId(channel.to_owned()), PayloadBytes(bytes.to_vec())).map(|_| ()).map_err(|_| ())
}

/// `minecraft:item`'s entity type, looked up once at registration (`EntityRef::kind` now gives a key, not a name).
pub(crate) static ITEM_ENTITY: std::sync::OnceLock<EntityTypeKey> = std::sync::OnceLock::new();

pub(crate) fn is_item_entity(kind: EntityTypeKey) -> bool {
    ITEM_ENTITY.get() == Some(&kind)
}

/// A slot of a screen at `position` (GUI pixels from the screen's corner), with no empty-slot icon: Sunrise's screens
/// take `ScreenSlot`s since 2d81263.
pub(crate) fn slot_at(position: [f32; 2]) -> ScreenSlot {
    ScreenSlot { position, no_item_icon: None }
}

#[cfg(test)]
mod fixf_tests {
    use super::*;

    #[test]
    fn a_hosted_part_position_is_its_cell_and_side() {
        // The machine request's log: the export bus on the north (2) and up (1) sides of the cell at y 100.
        assert_eq!(hosted([219, 100, 200], 2), [219, 2010, 200]);
        assert_eq!(host_cell([219, 2010, 200]), ([219, 100, 200], 2));
        assert_eq!(host_cell([219, 2009, 200]), ([219, 100, 200], 1));
        assert_eq!(real([219, 2010, 200]), [219, 100, 200]);
        assert_eq!(pos_text([219, 2010, 200]), "[219, 100, 200] north part");
        assert_eq!(pos_text([219, 100, 200]), "[219, 100, 200]");
        // The whole build height round-trips, every side.
        for y in [-64, 0, 100, 319] {
            for s in 0..6 {
                assert_eq!(host_cell(hosted([5, y, -7], s)), ([5, y, -7], s));
            }
        }
    }
}

// ---- Owned reads (Sunrise main lends stacks and lists to a callback; these copy them out, as the reads used to)

/// A level's entities and players as lists, collected from Sunrise main's visits (`visit_*`). `None` where the level
/// could not be read (stale, unavailable), as before.
pub(crate) trait LevelLists {
    fn all_entities_owned(self, host: &dyn Host) -> Option<Vec<EntityRef>>;
    fn entities_in_owned(self, host: &dyn Host, min: [f64; 3], max: [f64; 3]) -> Option<Vec<EntityRef>>;
    fn online_players_owned(self, host: &dyn Host) -> Option<Vec<PlayerRef>>;
}

impl LevelLists for LevelRef {
    fn all_entities_owned(self, host: &dyn Host) -> Option<Vec<EntityRef>> {
        let mut out = Vec::new();
        // A full visit (nothing stops it): `Ok` is always `Continue`.
        let _visited = self.visit_all_entities(host, &mut |e| {
            out.push(e);
            std::ops::ControlFlow::Continue(())
        })
        .ok()?;
        Some(out)
    }
    fn entities_in_owned(self, host: &dyn Host, min: [f64; 3], max: [f64; 3]) -> Option<Vec<EntityRef>> {
        let mut out = Vec::new();
        // A full visit (nothing stops it): `Ok` is always `Continue`.
        let _visited = self.visit_entities_in(host, min, max, &mut |e| {
            out.push(e);
            std::ops::ControlFlow::Continue(())
        })
        .ok()?;
        Some(out)
    }
    fn online_players_owned(self, host: &dyn Host) -> Option<Vec<PlayerRef>> {
        let mut out = Vec::new();
        // A full visit (nothing stops it): `Ok` is always `Continue`.
        let _visited = self.visit_online_players(host, &mut |p| {
            out.push(p);
            std::ops::ControlFlow::Continue(())
        })
        .ok()?;
        Some(out)
    }
}

/// A player's occupied inventory slots (every one, by its index) as owned stacks. A stack the API cannot represent
/// is left out, as the copying read left it.
pub(crate) trait InventoryCopy {
    fn inventory_slots_owned(self, host: &dyn Host) -> Option<Vec<(u32, ItemStack)>>;
}

impl InventoryCopy for PlayerRef {
    fn inventory_slots_owned(self, host: &dyn Host) -> Option<Vec<(u32, ItemStack)>> {
        self.with_inventory(host, |view| view.iter().filter_map(|(i, s)| Some((i, s.project()?))).collect())
    }
}

/// The stack in a hand, owned.
pub(crate) trait HandCopy {
    fn hand_item_owned(self, host: &dyn Host, hand: Hand) -> Option<ItemStack>;
}

impl HandCopy for LivingRef {
    fn hand_item_owned(self, host: &dyn Host, hand: Hand) -> Option<ItemStack> {
        self.with_hand_item(host, hand, |s| s.project()).flatten()
    }
}

/// A dropped item's stack, owned.
pub(crate) trait DroppedCopy {
    fn stack_owned(self, host: &dyn Host) -> Option<ItemStack>;
}

impl DroppedCopy for ItemEntityRef {
    fn stack_owned(self, host: &dyn Host) -> Option<ItemStack> {
        self.with_stack(host, |s| s.project()).flatten()
    }
}

/// The stack at an item place, owned.
pub(crate) trait PlaceCopy {
    fn stack_owned(&self, host: &mut dyn ServerHost) -> Option<ItemStack>;
}

impl PlaceCopy for ItemPlace {
    fn stack_owned(&self, host: &mut dyn ServerHost) -> Option<ItemStack> {
        self.with_stack(host, |s| s.project()).flatten()
    }
}
