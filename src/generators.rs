//! AE2's generators: the vibration chamber (VibrationChamberBlockEntity, VibrationChamberScreen) and the crystal
//! resonance generator (CrystalResonanceGeneratorBlockEntity). Both put AE into their grid's buffer (`net::inject_power`).
//!
//! The logic is the upstream port's (`ae2-horizon/mixins/ae2/me.rs`, `block_entity_tick`'s vibration and resonance
//! branches, `fuel_ticks`, the chamber's screen), on Sunrise's API: the furnace burn time is `item_burn_time`, the burnt
//! fuel's remainder (a lava bucket's bucket) is `item_crafting_remainder`.

use crate::cells::{sid, with_count};
use crate::config::config;
use crate::screens::{side_panel, spr, tr};
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) const VIBRATION: &str = "ae2:vibration_chamber";
pub(crate) const RESONANCE: &str = "ae2:crystal_resonance_generator";
/// The chamber's container: the fuel, then three upgrade cards (VibrationChamberBlockEntity.upgrades).
pub(crate) const VIB_SLOTS: usize = 4;
pub(crate) const VIB_CARDS: std::ops::Range<usize> = 1..4;
/// UpgradesInit: the chamber takes three Energy Cards and three Acceleration Cards.
const MAX_EACH_CARD: usize = 3;
const TEXT: u32 = 0xFF40_4040;

pub(crate) fn is_vibration(block: &str) -> bool {
    block == VIBRATION
}

pub(crate) fn has_screen(block: &str) -> bool {
    block == VIBRATION
}

/// A furnace fuel's burn time in ticks (the item's `cooking_fuel`), none for what doesn't burn.
pub(crate) fn burn_ticks(host: &dyn Host, stack: &ItemStack) -> Option<i64> {
    host.item_burn_time(stack.item()).filter(|t| *t > 0).map(i64::from)
}

fn cards(fields: &Fields, card: &str) -> usize {
    let n: usize = VIB_CARDS.filter_map(|i| fields.items.get(i).cloned().flatten()).filter(|s| sid(s) == card).map(|s| s.count() as usize).sum();
    n.min(MAX_EACH_CARD)
}

// ---- The vibration chamber's numbers (pure: unit tested)

/// The chamber's rates for its cards: AE per fuel tick, the fastest and slowest burn (fuel ticks a game tick) and the
/// burn it starts at (VibrationChamberBlockEntity: getEnergyPerFuelTick, getMaxFuelPerTick, getMinFuelPerTick,
/// INITIAL_BURN_SPEED).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VibRates {
    pub(crate) per_fuel_tick: f64,
    pub(crate) top: f64,
    pub(crate) least: f64,
    pub(crate) initial: f64,
}

pub(crate) fn vib_rates(base: f64, min_rate: f64, max_rate: f64, energy_cards: usize, speed_cards: usize) -> VibRates {
    let per_fuel_tick = base * (1.0 + energy_cards as f64 / 2.0);
    let top = (max_rate + max_rate * speed_cards as f64 / 2.0) / per_fuel_tick;
    let least = min_rate / per_fuel_tick;
    let initial = (base.clamp(min_rate, max_rate) / per_fuel_tick).clamp(least, top.max(least));
    VibRates { per_fuel_tick, top, least, initial }
}

/// One burning tick: the fuel ticks used, what they make, the burn left and the new burn speed. The chamber speeds
/// up a hundredth of its range while the network takes all it makes, and slows down as much while it can't
/// (`room` is the grid's free AE before this tick's power goes in).
pub(crate) fn vib_burn(r: VibRates, left: f64, rate: f64, room: f64) -> (f64, f64, f64) {
    let used = rate.min(left).max(0.0);
    let made = used * r.per_fuel_tick;
    let step = (r.top - r.least) / 100.0;
    let rate = (if room < made { rate - step } else { rate + step }).clamp(r.least, r.top.max(r.least));
    (made, left - used, rate)
}

/// VibrationChamberMenu's two 16-bit data slots: the percent of the fuel's burn time left, the percent of the top
/// rate burning and the Energy Cards in the first; the power made a tick in hundredths in the second.
pub(crate) fn vib_data(left: f64, total: i64, rate: f64, r: VibRates, energy_cards: usize) -> [i32; 2] {
    let burning = if left > 0.0 { rate } else { 0.0 };
    let flame = ((left * 100.0 / total.max(1) as f64) as i32).clamp(0, 100);
    let bar = if r.top > 0.0 { ((burning / r.top * 100.0) as i32).clamp(0, 100) } else { 0 };
    let packed = flame | bar << 7 | (energy_cards.min(3) as i32) << 14;
    let power = ((burning * r.per_fuel_tick * 100.0) as i64).clamp(0, 65_535) as i32;
    [packed, power]
}

fn nbt_f64(nbt: &Compound, key: &str) -> f64 {
    match nbt.get(&java(key)) {
        Some(Tag::Double(v)) => f64::from_bits(*v),
        Some(Tag::Long(v)) => *v as f64,
        Some(Tag::Int(v)) => f64::from(*v),
        _ => 0.0,
    }
}

fn set_nbt_f64(nbt: &mut Compound, key: &str, v: f64) {
    nbt.insert(java(key), Tag::Double(v.to_bits()));
}

/// The chamber's tick: fuel burns while there is a grid, faster as the grid takes what it makes; Energy Cards make
/// more of each fuel tick, Acceleration Cards raise its top speed. Burning shows as its lit look (`powered`).
pub(crate) fn vibration_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let dim = host.dimension();
    let cfg = config();
    let (energy, speed) = (cards(fields, "ae2:energy_card"), cards(fields, "ae2:speed_card"));
    let r = vib_rates(cfg.vibration_base_energy, f64::from(cfg.vibration_min_energy), f64::from(cfg.vibration_max_energy), energy, speed);
    let on_grid = crate::net::device_state(dim, pos).0.is_some();
    let mut left = nbt_f64(&fields.nbt, "burn").max(0.0);
    let mut rate = match nbt_f64(&fields.nbt, "burn_rate") {
        v if v > 0.0 => v.clamp(r.least, r.top.max(r.least)),
        _ => r.initial,
    };
    if left <= 0.0 {
        rate = r.initial;
        if on_grid {
            left = take_fuel(host, pos, fields).unwrap_or(0) as f64;
        }
    } else {
        let room = crate::net::grid_power(dim, pos).map_or(0.0, |(e, cap)| (cap - e).max(0.0));
        let (made, l, rt) = vib_burn(r, left, rate, room);
        if made > 0.0 {
            crate::net::inject_power(dim, pos, made);
        }
        left = l;
        rate = rt;
    }
    set_nbt_f64(&mut fields.nbt, "burn", left);
    set_nbt_f64(&mut fields.nbt, "burn_rate", rate);
    let total = nbt_i64(&fields.nbt, "burn_total");
    let data = vib_data(left, total, rate, r, energy);
    if fields.data.len() < 2 {
        fields.data.resize(2, 0);
    }
    fields.data[0] = data[0];
    fields.data[1] = data[1];
    let lit = if left > 0.0 { "true" } else { "false" };
    crate::net::with_net(dim, |n| n.prop_sets.push((pos, "powered", lit.to_owned())));
}

/// One fuel item out of the slot: its burn time. A lava bucket leaves its bucket in the slot (the furnace's
/// remainder); with more than one in the slot the remainder drops beside the chamber.
fn take_fuel(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) -> Option<i64> {
    let stack = fields.items.first().cloned().flatten()?;
    let ticks = burn_ticks(&*host, &stack)?;
    let remainder = host.item_crafting_remainder(&stack).filter(|r| r.count() > 0);
    let count = stack.count();
    let after = if count <= 1 { remainder.clone() } else { with_count(&stack, count - 1) };
    if count > 1
        && let Some(rem) = remainder
        && let Some(level) = here(&*host)
    {
        let _ = level.spawn_item(host, f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.0, f64::from(pos[2]) + 0.5, rem, 0);
    }
    if let Some(slot) = fields.items.get_mut(0) {
        *slot = after;
    }
    set_nbt_i64(&mut fields.nbt, "burn_total", ticks);
    Some(ticks)
}

/// `menu_slot_rules` of the chamber's menu: fuel in the fuel slot (RestrictedInputSlot FUEL), its two card kinds in the
/// card slots, one a slot.
pub(crate) fn slot_rules(host: &dyn Host, menu: &str, slot: usize, stack: &ItemStack, may_place: &mut bool, max_stack: &mut u32) {
    if menu != VIBRATION {
        return;
    }
    if slot == 0 {
        *may_place = burn_ticks(host, stack).is_some();
    } else if VIB_CARDS.contains(&slot) {
        *may_place = matches!(sid(stack), "ae2:energy_card" | "ae2:speed_card");
        *max_stack = 1;
    }
}

/// What hoppers and pipes reach on the chamber: its fuel slot, taking only fuel and giving only what no longer burns.
pub(crate) fn vibration_items(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    at.side?;
    let inner = at.level.capability(host, &ITEMS, at.pos, None)?;
    Some(Box::new(FuelSlot { inner: Box::new(crate::bus::Slice { inner, base: 0, len: 1 }) }))
}

struct FuelSlot {
    inner: Box<dyn ResourceHandler<ItemResource>>,
}

impl ResourceHandler<ItemResource> for FuelSlot {
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
        self.inner.capacity(host, index, resource)
    }
    fn is_valid(&self, host: &mut dyn ServerHost, index: usize, resource: &ItemResource) -> bool {
        index == 0 && burn_ticks(&*host, resource.as_stack()).is_some()
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        if !self.is_valid(host, index, resource) {
            return Ok(0);
        }
        self.inner.insert_at(host, tx, index, resource, amount).map(|n| n.min(amount))
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &ItemResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        if index != 0 || burn_ticks(&*host, resource.as_stack()).is_some() {
            return Ok(0);
        }
        self.inner.extract_at(host, tx, index, resource, amount).map(|n| n.min(amount))
    }
}

// ---- The crystal resonance generator

/// Each grid's working generator (by the grid's root): its position and the tick it last ran.
static RESONANCE_CURRENT: Mutex<Option<HashMap<(&'static str, Pos), (Pos, u64)>>> = Mutex::new(None);

pub(crate) fn forget_all() {
    *RESONANCE_CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Whether `pos` takes over (or keeps) its grid's generating: the one already working keeps it while it still ticks.
pub(crate) fn claims(current: Option<(Pos, u64)>, pos: Pos, now: u64) -> bool {
    match current {
        Some((c, seen)) if c != pos && now.saturating_sub(seen) <= 2 => false,
        _ => true,
    }
}

/// EnergyService.onServerStartTick: the grid's one passive generator makes power (AEConfig
/// `CrystalResonanceGeneratorRate`), any other on the same grid is suppressed.
pub(crate) fn resonance_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let dim = host.dimension();
    let (root, now) = crate::net::with_net(dim, |n| (n.grid_of.get(&pos).and_then(|g| n.grids.get(*g)).and_then(|g| g.root()), n.tick));
    let suppressed = match root {
        Some(root) => {
            let mut g = RESONANCE_CURRENT.lock().unwrap_or_else(|e| e.into_inner());
            let map = g.get_or_insert_with(HashMap::new);
            let mine = claims(map.get(&(dim, root)).copied(), pos, now);
            if mine {
                map.insert((dim, root), (pos, now));
            }
            !mine
        }
        None => false,
    };
    if root.is_some() && !suppressed {
        crate::net::inject_power(dim, pos, config().crystal_resonance_generator_rate);
    }
    if (nbt_i64(&fields.nbt, "suppressed") != 0) != suppressed {
        set_nbt_i64(&mut fields.nbt, "suppressed", i64::from(suppressed));
    }
}

// ---- The chamber's screen (VibrationChamberScreen over vibchamber.png)

pub(crate) fn screen(block: &str) -> Option<MenuScreenLayout> {
    if block != VIBRATION {
        return None;
    }
    let mut slots = vec![Some(slot_at([80.0, 37.0]))];
    slots.extend((0..3).map(|i| Some(slot_at([176.0 + 2.0 + 8.0, 8.0 + 18.0 * i as f32]))));
    Some(MenuScreenLayout { width: 176.0, height: 166.0, slots, inventory_y: 84.0, inventory_x: 8.0, own_labels: true, own_background: true, ..MenuScreenLayout::default() })
}

pub(crate) fn screen_draw(host: &dyn ClientHost, block: &str, data: &[i32], d: &mut dyn DrawContext, o: [f32; 2]) -> bool {
    if block != VIBRATION {
        return false;
    }
    spr(d, "ae2:gui/vibchamber", o, [0.0, 0.0], [0.0, 0.0, 176.0, 166.0]);
    side_panel(d, o, 176.0, 3);
    d.text(&tr(host, "gui.ae2.VibrationChamber", "Vibration Chamber"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
    d.text(&tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 166.0 - 93.0], TEXT, false);
    let packed = i32::from(data.first().copied().unwrap_or(0) as u16);
    // The flame over the fuel slot burns down with the fuel's burn time.
    let burn = packed & 0x7F;
    if burn > 0 {
        let f = (burn.min(100) * 13 / 100) as f32;
        d.sprite("ae2:gui/vibchamber", [176.0, 13.0 - f, 14.0, f], [o[0] + 80.0, o[1] + 20.0 + 13.0 - f, 14.0, f], None);
    }
    // The generation rate over its most (ProgressBar VERTICAL), from the foot up.
    let h = (18 * ((packed >> 7) & 0x7F).min(100) / 100) as f32;
    if h > 0.0 {
        d.sprite("ae2:gui/vibchamber", [176.0, 14.0 + 18.0 - h, 6.0, h], [o[0] + 99.0, o[1] + 36.0 + 18.0 - h, 6.0, h], None);
    }
    true
}

pub(crate) fn screen_tooltip(block: &str, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) -> bool {
    if block != VIBRATION || !(at[0] >= 99.0 && at[0] < 105.0 && at[1] >= 36.0 && at[1] < 54.0) {
        return false;
    }
    let packed = i32::from(data.first().copied().unwrap_or(0) as u16);
    let power = f64::from(data.get(1).copied().unwrap_or(0) as u16) / 100.0;
    let energy_cards = (packed >> 14) & 3;
    lines.push(format!("{power:.2} AE/t"));
    lines.push(format!("§7Eff: {}.0%", 100 + energy_cards * 50));
    true
}

pub(crate) fn screen_click(block: &str, at: [f32; 2]) -> Option<Option<i32>> {
    // The flame and the bar are no buttons; everything else is the game's (slots).
    (block == VIBRATION && at[0] >= 99.0 && at[0] < 105.0 && at[1] >= 20.0 && at[1] < 54.0).then_some(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_follow_cards() {
        let r = vib_rates(5.0, 4.0, 40.0, 0, 0);
        assert_eq!(r.per_fuel_tick, 5.0);
        assert_eq!(r.top, 8.0);
        assert!((r.least - 0.8).abs() < 1e-9);
        assert_eq!(r.initial, 1.0);
        // Energy Cards: more AE a fuel tick, so fewer fuel ticks for the same top power.
        let e = vib_rates(5.0, 4.0, 40.0, 3, 0);
        assert_eq!(e.per_fuel_tick, 12.5);
        assert!((e.top * e.per_fuel_tick - 40.0).abs() < 1e-9);
        // Acceleration Cards: a higher top power.
        let s = vib_rates(5.0, 4.0, 40.0, 0, 2);
        assert!((s.top * s.per_fuel_tick - 80.0).abs() < 1e-9);
    }

    #[test]
    fn burn_speeds_up_while_taken_and_slows_while_full() {
        let r = vib_rates(5.0, 4.0, 40.0, 0, 0);
        let (made, left, faster) = vib_burn(r, 1600.0, r.initial, 1e9);
        assert_eq!(made, 5.0);
        assert_eq!(left, 1599.0);
        assert!(faster > r.initial);
        let (_, _, slower) = vib_burn(r, 1600.0, r.initial, 0.0);
        assert!(slower < r.initial);
        // Never past its range.
        let (_, _, top) = vib_burn(r, 1600.0, r.top, 1e9);
        assert_eq!(top, r.top);
        let (_, _, least) = vib_burn(r, 1600.0, r.least, 0.0);
        assert_eq!(least, r.least);
        // The last of a fuel's ticks: no more than is left.
        let (made, left, _) = vib_burn(r, 0.25, 1.0, 1e9);
        assert_eq!(made, 1.25);
        assert_eq!(left, 0.0);
    }

    #[test]
    fn data_fits_sixteen_bits() {
        let r = vib_rates(5.0, 4.0, 1000.0, 3, 3);
        let [packed, power] = vib_data(1600.0, 1600, r.top, r, 3);
        assert!((0..=0xFFFF).contains(&packed) && (0..=0xFFFF).contains(&power));
        assert_eq!(packed & 0x7F, 100);
        assert_eq!((packed >> 7) & 0x7F, 100);
        assert_eq!(packed >> 14, 3);
        let [idle, none] = vib_data(0.0, 1600, r.top, r, 0);
        assert_eq!((idle, none), (0, 0));
    }

    #[test]
    fn one_generator_a_grid() {
        let (a, b) = ([0, 0, 0], [1, 0, 0]);
        assert!(claims(None, a, 10));
        assert!(claims(Some((a, 9)), a, 10));
        assert!(!claims(Some((a, 9)), b, 10));
        // The working one stopped ticking (broken, unloaded): the next takes over.
        assert!(claims(Some((a, 5)), b, 10));
    }
}
