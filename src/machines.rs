//! AE2's machines with a menu of their own that the network doesn't run: the inscriber (`inscriber`), the ME IO port
//! (`io_port`) and the matter condenser (`condenser`). This file is their shared glue: their containers, their
//! automation access (an ITEMS provider asked before the game's, so every face's rules are AE2's), their menus' slot
//! rules and buttons, and the dispatch from `devices` and `lib`.
//!
//! Ported from the upstream Horizon port (`../ae2-horizon/mixins/ae2/me.rs`: `inscriber_tick`, `io_port_tick`,
//! `condenser_tick` and their screens), with AE2 15.4's Java as the reference for the rules.

use crate::cells::*;
use crate::util::*;
use mod_api::*;

pub(crate) const INSCRIBER: &str = "ae2:inscriber";
pub(crate) const IO_PORT: &str = "ae2:io_port";
pub(crate) const CONDENSER: &str = "ae2:condenser";

/// The machines this module runs.
pub(crate) fn is_machine(block: &str) -> bool {
    matches!(block, INSCRIBER | IO_PORT | CONDENSER)
}

/// Their menus (content menu types, `data/ae2/sunrise/menu/*.json`), as many slots as these containers.
pub(crate) const MENUS: [&str; 3] = [INSCRIBER, IO_PORT, CONDENSER];

/// A use opens the machine's menu (AE2's blocks open their GUI on use).
pub(crate) fn has_screen(block: &str) -> bool {
    is_machine(block)
}

/// A right-click on a toolbar button: the setting before (SettingToggleButton), as the parts' `BACK`.
pub(crate) const BACK: i32 = 1 << 8;

/// The container each declares (`r.block_container`): what the menu shows. Automation goes through `items` instead.
pub(crate) fn container_of(block: &str) -> Option<ModContainer> {
    match block {
        // Top press, middle, bottom press, output; four acceleration cards.
        INSCRIBER => Some(ModContainer {
            slots: crate::inscriber::SLOTS,
            faces: std::array::from_fn(|_| vec![0, 1, 2, 3]),
            inputs: vec![0, 1, 2],
            outputs: vec![crate::inscriber::OUTPUT],
            insert_rules: Vec::new(),
            menu: Some(INSCRIBER.to_owned()),
        }),
        // Six cells in, six out, three cards (IOPortBlockEntity: top and bottom reach the inputs, the sides the
        // outputs).
        IO_PORT => Some(ModContainer {
            slots: crate::io_port::SLOTS,
            faces: std::array::from_fn(|f| if f < 2 { (0..6).collect() } else { (6..12).collect() }),
            inputs: (0..6).collect(),
            outputs: (6..12).collect(),
            insert_rules: Vec::new(),
            menu: Some(IO_PORT.to_owned()),
        }),
        // Input (destroyed into power), output, storage component.
        CONDENSER => Some(ModContainer {
            slots: crate::condenser::SLOTS,
            faces: std::array::from_fn(|_| vec![0, 1]),
            inputs: vec![0],
            outputs: vec![1],
            insert_rules: Vec::new(),
            menu: Some(CONDENSER.to_owned()),
        }),
        _ => None,
    }
}

/// AEBasePoweredBlockEntity's FE storage on the inscriber: what comes in fills its own buffer (1600 AE).
pub(crate) fn energy_of(block: &str) -> Option<ModEnergyBuffer> {
    (block == INSCRIBER).then(|| ModEnergyBuffer {
        capacity: crate::inscriber::FE_BUFFER,
        max_insert: crate::inscriber::FE_BUFFER,
        max_extract: 0,
        insert_faces: [true; 6],
        extract_faces: [false; 6],
    })
}

/// The condenser's fluid input (CondenserBlockEntity's fluid handler): whatever goes in is destroyed into power.
pub(crate) fn fluids_of(block: &str) -> Option<ModFluidTanks> {
    (block == CONDENSER).then(|| {
        let mut t = ModFluidTanks::default();
        t.tanks = vec![crate::condenser::TANK_MB];
        t.faces = std::array::from_fn(|_| vec![0]);
        t.inputs = vec![0];
        t
    })
}

/// The block entity's tick (`devices::run_device`).
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, block: &str, fields: &mut Fields) {
    match block {
        INSCRIBER => crate::inscriber::tick(host, pos, fields),
        IO_PORT => crate::io_port::tick(host, pos, fields),
        CONDENSER => crate::condenser::tick(host, pos, fields),
        _ => {}
    }
}

pub(crate) fn forget_all() {
    crate::inscriber::forget_all();
}

// ---- Cards

/// How many of `card` sit in the slots `from..from + n` (one card a slot).
pub(crate) fn cards(items: &[Option<ItemStack>], from: usize, n: usize, card: &str) -> usize {
    items.iter().skip(from).take(n).flatten().filter(|s| sid(s) == card).map(|s| s.count() as usize).sum()
}

/// Whether `card` may go in upgrade slot `slot` (of `from..from + n`), given the most of it the machine takes
/// (Upgrades.getMaxInstallable) and what the other upgrade slots hold.
pub(crate) fn card_fits(limits: &[(&str, usize)], slots: &[Option<&ItemStack>], slot: usize, card: &str) -> bool {
    let Some((_, most)) = limits.iter().find(|(c, _)| *c == card) else { return false };
    let others = slots.iter().enumerate().filter(|(i, s)| *i != slot && s.is_some_and(|s| sid(s) == card)).count();
    others < *most
}

// ---- Menus

/// `menu_slot_rules` on these menus: AE2's RestrictedInputSlots, the machines' filters and the cards.
pub(crate) fn slot_rules(host: &dyn Host, menu: &str, slot: usize, stack: &ItemStack, context: &MenuSlotContext, may_place: &mut bool, max_stack: &mut u32) {
    match menu {
        INSCRIBER => crate::inscriber::slot_rules(host, context, slot, stack, may_place, max_stack),
        IO_PORT => crate::io_port::slot_rules(context, slot, stack, may_place, max_stack),
        CONDENSER => crate::condenser::slot_rules(context, slot, stack, may_place, max_stack),
        _ => {}
    }
}

/// Whether a menu button may act: a sane id, the player not flooding, in this level within reach of the block,
/// which is still the machine (the engine sends a button only for the menu the player has open).
fn button_allowed(host: &dyn Host, player: u128, pos: Pos, block: &str, id: i32) -> bool {
    if id <= 0 || !crate::storage::rate_ok(player) {
        return false;
    }
    let Some(level) = here(host) else { return false };
    if crate::net::block_id(host, level, pos) != block {
        return false;
    }
    let Some(p) = host.player(player) else { return false };
    if p.entity().level(host) != Some(level) {
        return false;
    }
    let Some(at) = p.entity().position(host) else { return false };
    let d2: f64 = (0..3).map(|i| (at[i] - f64::from(pos[i]) - 0.5).powi(2)).sum();
    d2 <= 64.0
}

/// `menu_button` on these machines: true when the block is one of them (the button handled or refused).
pub(crate) fn menu_button(host: &mut dyn ServerHost, player: u128, pos: Pos, block: &str, id: i32, fields: &mut Fields) -> bool {
    if !is_machine(block) {
        return false;
    }
    if !button_allowed(&*host, player, pos, block, id) {
        debug(&format!("machines: button {id} on {block} at {pos:?} refused"));
        return true;
    }
    let back = id & BACK != 0;
    let id = id & !BACK;
    match block {
        INSCRIBER => crate::inscriber::button(fields, id, back),
        IO_PORT => crate::io_port::button(fields, id, back),
        CONDENSER => crate::condenser::button(fields, id, back),
        _ => {}
    }
    true
}

/// A cyclic setting one step on (or back): its value kept in `0..modes`.
pub(crate) fn cycle(nbt: &mut Compound, key: &str, modes: i64, back: bool) {
    let m = nbt_i64(nbt, key).clamp(0, modes - 1);
    set_nbt_i64(nbt, key, if back { (m + modes - 1) % modes } else { (m + 1) % modes });
}

// ---- Automation (ITEMS, asked before the game's container)

/// A machine's slots one face reaches, each with the machine's rules (`insert_limit`, `may_extract`), written
/// straight to the block entity's slots in the caller's transaction.
pub(crate) struct MachineItems {
    pub(crate) be: BlockEntityRef,
    pub(crate) block: &'static str,
    /// The container slots this face reaches, in order.
    pub(crate) slots: Vec<usize>,
}

/// Those slots' rules, by machine.
pub(crate) struct Access<'a> {
    pub(crate) nbt: &'a Compound,
    pub(crate) items: &'a [Option<ItemStack>],
}

impl MachineItems {
    fn slot(&self, index: usize) -> Option<usize> {
        self.slots.get(index).copied()
    }

    fn read(&self, host: &dyn Host) -> (Compound, Vec<Option<ItemStack>>) {
        let n = match self.block {
            INSCRIBER => crate::inscriber::SLOTS,
            IO_PORT => crate::io_port::SLOTS,
            _ => crate::condenser::SLOTS,
        };
        let nbt = self.be.data(host).unwrap_or_default();
        let items = (0..n).map(|i| self.be.item_slot(host, i as u32).ok().flatten()).collect();
        (nbt, items)
    }

    /// The most of `stack` slot `slot` may hold after an insert (0: it takes none).
    fn insert_limit(&self, host: &dyn Host, slot: usize, stack: &ItemStack) -> u32 {
        let (nbt, items) = self.read(host);
        let at = Access { nbt: &nbt, items: &items };
        let limit = match self.block {
            INSCRIBER => crate::inscriber::insert_limit(host, &at, slot, stack),
            IO_PORT => crate::io_port::insert_limit(slot, stack),
            _ => crate::condenser::insert_limit(slot),
        };
        limit.min(max_stack(host, stack))
    }

    /// A slot's own limit, for no resource in particular (the internal inventories' stack limits).
    fn general_limit(&self, host: &dyn Host, slot: usize) -> u32 {
        match self.block {
            INSCRIBER => {
                let small = self.be.data(host).is_some_and(|n| nbt_i64(&n, "small_buffer") != 0);
                if slot <= 2 && small { 4 } else { 64 }
            }
            IO_PORT => 1,
            _ => crate::condenser::insert_limit(slot).max(64),
        }
    }

    fn may_extract(&self, host: &dyn Host, slot: usize) -> bool {
        let nbt = self.be.data(host).unwrap_or_default();
        match self.block {
            INSCRIBER => crate::inscriber::may_extract(&nbt, slot),
            IO_PORT => crate::io_port::may_extract(slot),
            _ => crate::condenser::may_extract(slot),
        }
    }
}

impl ResourceHandler<ItemResource> for MachineItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        self.slots.len()
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        let slot = self.slot(index)?;
        self.be.item_slot(&*host, slot as u32).ok().flatten().map(|s| ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        let Some(slot) = self.slot(index) else { return 0 };
        self.be.item_slot(&*host, slot as u32).ok().flatten().map_or(0, |s| s.count())
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, resource: Option<&ItemResource>) -> u32 {
        let Some(slot) = self.slot(index) else { return 0 };
        match resource {
            Some(r) => self.insert_limit(&*host, slot, r.as_stack()),
            None => self.general_limit(&*host, slot),
        }
    }
    fn is_valid(&self, host: &mut dyn ServerHost, index: usize, resource: &ItemResource) -> bool {
        self.capacity(host, index, Some(resource)) > 0
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &Transaction, index: usize, resource: &ItemResource, amount: u32) -> u32 {
        let Some(slot) = self.slot(index) else { return 0 };
        if amount == 0 {
            return 0;
        }
        let limit = self.insert_limit(&*host, slot, resource.as_stack());
        let current = self.be.item_slot(&*host, slot as u32).ok().flatten();
        let have = match &current {
            Some(c) if !resource.matches(c) => return 0,
            Some(c) => c.count(),
            None => 0,
        };
        let n = amount.min(limit.saturating_sub(have));
        if n == 0 {
            return 0;
        }
        let Some(new) = resource.to_stack(have + n) else { return 0 };
        if self.be.set_item_slot(host, tx, slot as u32, Some(new)).is_ok() { n } else { 0 }
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &Transaction, index: usize, resource: &ItemResource, amount: u32) -> u32 {
        let Some(slot) = self.slot(index) else { return 0 };
        if amount == 0 || !self.may_extract(&*host, slot) {
            return 0;
        }
        let Some(current) = self.be.item_slot(&*host, slot as u32).ok().flatten() else { return 0 };
        if !resource.matches(&current) {
            return 0;
        }
        let n = amount.min(current.count());
        let left = with_count(&current, current.count() - n);
        if self.be.set_item_slot(host, tx, slot as u32, left).is_ok() { n } else { 0 }
    }
}

/// A face's handler over `slots` of the machine at `at`.
pub(crate) fn handler(host: &mut dyn ServerHost, at: &BlockSide, block: &'static str, slots: Vec<usize>) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    let be = host.block_entity(at.level, at.pos)?;
    Some(Box::new(MachineItems { be, block, slots }))
}

// ---- Screens (client)

pub(crate) fn screen(block: &str) -> Option<MenuScreenLayout> {
    match block {
        INSCRIBER => Some(crate::inscriber::screen()),
        IO_PORT => Some(crate::io_port::screen()),
        CONDENSER => Some(crate::condenser::screen()),
        _ => None,
    }
}

/// The screen's own drawing; true when it is one of these machines'.
pub(crate) fn screen_draw(host: &dyn ClientHost, block: &str, data: &[i32], d: &mut dyn DrawContext, o: [f32; 2]) -> bool {
    match block {
        INSCRIBER => crate::inscriber::draw(host, data, d, o),
        IO_PORT => crate::io_port::draw(host, data, d, o),
        CONDENSER => crate::condenser::draw(host, data, d, o),
        _ => return false,
    }
    true
}

/// A click on the screen: Some(Some(id)) sends a menu button, Some(None) is taken, None isn't these machines'.
pub(crate) fn screen_click(block: &str, data: &[i32], at: [f32; 2], right: bool) -> Option<Option<i32>> {
    let id = match block {
        INSCRIBER => crate::inscriber::click(at),
        IO_PORT => crate::io_port::click(data, at),
        CONDENSER => crate::condenser::click(at),
        _ => return None,
    };
    Some(id.map(|id| if right { id | BACK } else { id }))
}

/// The tooltip under the pointer; true when it is one of these machines' screens.
pub(crate) fn screen_tooltip(host: &dyn ClientHost, block: &str, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) -> bool {
    match block {
        INSCRIBER => crate::inscriber::tooltip(host, data, at, lines),
        IO_PORT => crate::io_port::tooltip(host, data, at, lines),
        CONDENSER => crate::condenser::tooltip(host, data, at, lines),
        _ => return false,
    }
    true
}

pub(crate) const TEXT: u32 = 0xFF40_4040;

pub(crate) fn inside(at: [f32; 2], r: [f32; 4]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// The left toolbar's places (16 pixels, 4 between, from -20, 8), as the parts' screens.
pub(crate) fn bar_place(i: usize) -> [f32; 2] {
    [-20.0, 8.0 + 20.0 * i as f32]
}

/// The upgrade panel's slot `i` (beside a 176-wide screen, as the assembler's).
pub(crate) fn card_slot(i: usize) -> [f32; 2] {
    [176.0 + 2.0 + 8.0, 8.0 + 18.0 * i as f32]
}

/// AE2's icon of an empty restricted slot (`ae2:gui/states`, at 40%): an item in the slot draws over it.
pub(crate) fn slot_icon(d: &mut dyn DrawContext, o: [f32; 2], at: [f32; 2], icon: [f32; 2]) {
    d.sprite("ae2:gui/states", [icon[0], icon[1], 16.0, 16.0], [o[0] + at[0], o[1] + at[1], 16.0, 16.0], Some(0x66FF_FFFF));
}

/// RestrictedInputSlot's UPGRADES icon.
pub(crate) const UPGRADE_ICON: [f32; 2] = [240.0, 208.0];

/// The base of these machines' screens: their own background and labels.
pub(crate) fn base_screen(width: f32, height: f32, slots: Vec<[f32; 2]>, inventory_y: f32) -> MenuScreenLayout {
    MenuScreenLayout {
        width,
        height,
        slots: slots.into_iter().map(|s| Some(slot_at(s))).collect(),
        inventory_y,
        inventory_x: 8.0,
        own_labels: true,
        own_background: true,
        ..MenuScreenLayout::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_settings_wraps_both_ways() {
        let mut nbt = Compound::new();
        cycle(&mut nbt, "m", 3, false);
        assert_eq!(nbt_i64(&nbt, "m"), 1);
        cycle(&mut nbt, "m", 3, false);
        cycle(&mut nbt, "m", 3, false);
        assert_eq!(nbt_i64(&nbt, "m"), 0);
        cycle(&mut nbt, "m", 3, true);
        assert_eq!(nbt_i64(&nbt, "m"), 2);
        // A forged out-of-range value is brought back into range.
        set_nbt_i64(&mut nbt, "m", 99);
        cycle(&mut nbt, "m", 3, false);
        assert_eq!(nbt_i64(&nbt, "m"), 0);
    }
}
