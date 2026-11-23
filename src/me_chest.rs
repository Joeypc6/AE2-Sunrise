//! The ME chest's own parts beyond its cell slot and terminal (`storage`, `screens`): its power buffer
//! (ChestBlockEntity, an AENetworkPowerBlockEntity of 500 AE), hoppers and pipes reaching its cell
//! (ChestBlockEntity.getExposedInventoryForSide: the front is the cell slot, the other sides put into the cell), and
//! the cell with its status light on its front (ChestRenderer).

use crate::cells::{is_cell_item, key_of, max_stack, sid};
use crate::config::config;
use crate::storage::Scope;
use crate::util::*;
use mod_api::*;

/// The buffer's most (AEConfig's usage multiplier on ChestBlockEntity's 500).
pub(crate) fn buffer_max() -> f64 {
    500.0 * config().power_usage_multiplier
}

/// The buffer, a second at a time: on a network whose stores are full it fills from the grid (what is injected past
/// the cells comes to EnergyService's requesters); off one, or unpowered, its idle draw comes out of it.
pub(crate) fn buffer_step(stored: f64, most: f64, powered: bool, grid_full: bool, idle_per_tick: f64) -> (f64, f64) {
    if !powered {
        return ((stored - idle_per_tick * 20.0).max(0.0), 0.0);
    }
    if grid_full && stored < most { (stored, most - stored) } else { (stored, 0.0) }
}

/// The chest's tick (every 20): its buffer.
pub(crate) fn chest_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let dim = host.dimension();
    if crate::net::with_net(dim, |n| n.tick) % 20 != 0 {
        return;
    }
    let stored = crate::net::internal_power(&fields.nbt);
    let most = buffer_max();
    let (_, online, _) = crate::net::device_state(dim, pos);
    let full = crate::net::grid_power(dim, pos).is_some_and(|(e, cap)| e >= cap - 1.0);
    let idle = crate::net::node_kind("ae2:chest").idle_power * config().power_usage_multiplier;
    let (mut now, want) = buffer_step(stored, most, online, full, idle);
    if want > 0.0 {
        now += crate::net::extract_power(dim, pos, want);
    }
    if now != stored {
        crate::net::set_internal_power(&mut fields.nbt, now);
    }
}

fn face_vec(d: Direction) -> Pos {
    match d {
        Direction::Down => [0, -1, 0],
        Direction::Up => [0, 1, 0],
        Direction::North => [0, 0, -1],
        Direction::South => [0, 0, 1],
        Direction::West => [-1, 0, 0],
        _ => [1, 0, 0],
    }
}

/// What hoppers and pipes reach on the chest: on its front the cell slot (a storage cell in or out), on its other sides
/// the cell's storage, insert only, as far as the cell takes it (its partition, types and bytes) and the network's
/// power allows.
pub(crate) fn chest_items(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    let side = at.side?;
    let front = facing_dir(at.level.block_property(&*host, at.pos, "facing").as_deref());
    if face_vec(side) == front {
        let inner = at.level.capability(host, &ITEMS, at.pos, None)?;
        return Some(Box::new(CellSlot { inner: Box::new(crate::bus::Slice { inner, base: 0, len: 1 }) }));
    }
    Some(Box::new(CellInput { pos: at.pos }))
}

struct CellSlot {
    inner: Box<dyn ResourceHandler<ItemResource>>,
}

impl ResourceHandler<ItemResource> for CellSlot {
    fn size(&self, host: &mut dyn ServerHost) -> usize {
        self.inner.size(host).min(1)
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        self.inner.resource(host, index)
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        self.inner.amount(host, index)
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, resource: Option<&ItemResource>) -> u32 {
        self.inner.capacity(host, index, resource).min(1)
    }
    fn is_valid(&self, _host: &mut dyn ServerHost, index: usize, resource: &ItemResource) -> bool {
        index == 0 && is_cell_item(sid(resource.as_stack()))
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &Transaction, index: usize, resource: &ItemResource, amount: u32) -> u32 {
        if !self.is_valid(host, index, resource) || self.inner.amount(host, index) > 0 {
            return 0;
        }
        self.inner.insert_at(host, tx, index, resource, amount.min(1)).min(1)
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &Transaction, index: usize, resource: &ItemResource, amount: u32) -> u32 {
        if index != 0 {
            return 0;
        }
        self.inner.extract_at(host, tx, index, resource, amount).min(amount)
    }
}

/// ChestBlockEntity's input inventory: what comes in goes straight into the cell (never kept in a slot).
struct CellInput {
    pos: Pos,
}

impl ResourceHandler<ItemResource> for CellInput {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        1
    }
    fn resource(&self, _host: &mut dyn ServerHost, _index: usize) -> Option<ItemResource> {
        None
    }
    fn amount(&self, _host: &mut dyn ServerHost, _index: usize) -> u32 {
        0
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, resource: Option<&ItemResource>) -> u32 {
        // Asked for no resource: the cell's storage has no general stack limit.
        if index == 0 { resource.map_or(u32::MAX, |r| max_stack(&*host, r.as_stack())) } else { 0 }
    }
    fn is_valid(&self, host: &mut dyn ServerHost, index: usize, resource: &ItemResource) -> bool {
        if index != 0 {
            return false;
        }
        let key = key_of(&*host, resource.as_stack());
        crate::storage::simulate(host, Scope::Chest(self.pos), None, &key, 1, true) > 0
    }
    fn insert_at(&self, host: &mut dyn ServerHost, _tx: &Transaction, index: usize, resource: &ItemResource, amount: u32) -> u32 {
        if index != 0 || amount == 0 {
            return 0;
        }
        // (The cell is written back through its slot in a transaction nested in the caller's: an abort puts the cell
        // back as it was, and storage reads it again since its version on the item moved back.)
        let key = key_of(&*host, resource.as_stack());
        let done = crate::storage::insert(host, Scope::Chest(self.pos), None, &key, i64::from(amount));
        done.clamp(0, i64::from(amount)) as u32
    }
    fn extract_at(&self, _host: &mut dyn ServerHost, _tx: &Transaction, _index: usize, _resource: &ItemResource, _amount: u32) -> u32 {
        0
    }
}

/// ChestRenderer: the cell in the chest's front, its status light beside it (the drive's cell plates, `cells` in the
/// update tag as the drive's).
pub(crate) fn chest_submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let Some(Tag::List(cells)) = event.entity.nbt.get(&java("cells")) else { return };
    let Some(Tag::String(look)) = cells.first() else { return };
    let look = look.to_string_lossy();
    let Some((model, state)) = look.split_once(':') else { return };
    let Some(level) = host.level(host.dimension()) else { return };
    let facing = level.block_property(host, event.pos, "facing").unwrap_or_else(|| "north".to_owned());
    let spin: f32 = level.block_property(host, event.pos, "spin").and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let draw = &mut *event.draw;
    draw.push();
    let c = [0.5, 0.5, 0.5];
    match facing.as_str() {
        "south" => draw.rotate_about([0.0, 1.0, 0.0], 180.0, c),
        "east" => draw.rotate_about([0.0, 1.0, 0.0], -90.0, c),
        "west" => draw.rotate_about([0.0, 1.0, 0.0], 90.0, c),
        "up" => draw.rotate_about([1.0, 0.0, 0.0], 90.0, c),
        "down" => draw.rotate_about([1.0, 0.0, 0.0], -90.0, c),
        _ => {}
    }
    draw.rotate_about([0.0, 0.0, 1.0], -90.0 * spin, c);
    // The front's cell opening, 5.5 to 10.5 across and 4 to 6 up, a plate a fifth of a texel before the face.
    let (x0, y0) = (5.5_f32, 4.0_f32);
    let sprite = format!("ae2:block/drive/cells/{model}");
    let face = |uv: [f32; 4]| Some(CubeFace::new(sprite.as_str()).with_uv(uv));
    draw.cube([x0, y0, -0.2], [x0 + 5.0, y0 + 2.0, 0.0], [face([0.0, 4.0, 5.0, 6.0]), face([0.0, 0.0, 5.0, 2.0]), face([0.0, 2.0, 5.0, 4.0]), None, None, None], 0xFFFF_FFFF);
    let light = crate::cells::STATUS_COLORS[state.parse::<usize>().unwrap_or(0).min(4)];
    if light != 0 {
        draw.cube([x0 + 4.0, y0 + 0.5, -0.3], [x0 + 5.0, y0 + 1.5, -0.2], [None, None, face([5.0, 2.0, 6.0, 3.0]), None, None, None], 0xFF00_0000 | light);
    }
    draw.pop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer() {
        // Off a network: a second of idle draw out of it, never below empty.
        assert_eq!(buffer_step(100.0, 500.0, false, false, 1.0), (80.0, 0.0));
        assert_eq!(buffer_step(10.0, 500.0, false, false, 1.0), (0.0, 0.0));
        // On a network with full stores: it asks for what it lacks.
        assert_eq!(buffer_step(100.0, 500.0, true, true, 1.0), (100.0, 400.0));
        // Stores not full: the cells come first.
        assert_eq!(buffer_step(100.0, 500.0, true, false, 1.0), (100.0, 0.0));
        assert_eq!(buffer_step(500.0, 500.0, true, true, 1.0), (500.0, 0.0));
    }
}
