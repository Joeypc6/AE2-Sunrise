//! AE2's powered items (AEBasePoweredItem): the charge bar under the item, the "Stored Energy" tooltip line, their
//! capacity with Energy Cards, and FE charging by other mods' chargers (the item's `ITEM_ENERGY`, as AE2's
//! PoweredItemCapabilities). Upstream's `item_battery`, `battery_max`, `energy_line` and `item_bar`
//! (`../ae2-horizon/mixins/ae2/me.rs`). The charge itself is `internalCurrentPower` in the item's custom data
//! (`world::item_power`), which the charger already fills.

use crate::cells::*;
use crate::world::{item_battery, item_power, set_item_power};
use crate::util::PlaceCopy;
use mod_api::*;

/// The items with a battery (each gets the bar and the FE capability).
pub(crate) const POWERED_ITEMS: [&str; 16] = [
    "ae2:wireless_terminal",
    "ae2:wireless_crafting_terminal",
    "ae2:entropy_manipulator",
    "ae2:matter_cannon",
    "ae2:charged_staff",
    "ae2:color_applicator",
    "ae2:portable_item_cell_1k",
    "ae2:portable_item_cell_4k",
    "ae2:portable_item_cell_16k",
    "ae2:portable_item_cell_64k",
    "ae2:portable_item_cell_256k",
    "ae2:portable_fluid_cell_1k",
    "ae2:portable_fluid_cell_4k",
    "ae2:portable_fluid_cell_16k",
    "ae2:portable_fluid_cell_64k",
    "ae2:portable_fluid_cell_256k",
];

/// Whether this item is a wireless terminal (its Energy Cards only double the battery).
pub(crate) fn is_wireless_terminal(item: &str) -> bool {
    matches!(item, "ae2:wireless_terminal" | "ae2:wireless_crafting_terminal")
}

/// A battery's capacity with `energy_cards` Energy Cards (`setAEMaxPowerMultiplier`): one more base capacity a card on
/// a wireless terminal, eight more on the others (pure: unit tested).
pub(crate) fn capacity_with_cards(base: f64, energy_cards: usize, wireless: bool) -> f64 {
    let per = if wireless { 1.0 } else { 8.0 };
    base * (1.0 + energy_cards as f64 * per)
}

/// A powered item's capacity in AE, its cards counted; None for an item with no battery.
pub(crate) fn capacity(stack: &ItemStack) -> Option<f64> {
    let item = sid(stack);
    let (base, _) = item_battery(item)?;
    let cards = cell_cards(stack).iter().filter(|c| c.as_str() == "ae2:energy_card").count();
    Some(capacity_with_cards(base, cards, is_wireless_terminal(item)))
}

/// Uses `ae` of the stack's charge (AEBasePoweredItem.extractAEPower): false, and nothing taken, when it holds less.
pub(crate) fn use_power(stack: &mut ItemStack, ae: f64) -> bool {
    let have = item_power(stack);
    if have + 1e-9 < ae {
        return false;
    }
    set_item_power(stack, (have - ae).max(0.0));
    true
}

/// The bar's width (0..=13) for a charge out of its capacity (AEBasePoweredItem.getBarWidth).
pub(crate) fn bar_width(power: f64, most: f64) -> i32 {
    if most <= 0.0 {
        return 0;
    }
    ((13.0 * (power / most).clamp(0.0, 1.0)).round() as i32).clamp(0, 13)
}

/// Tooltips.getMaxedAmount: both numbers in the larger's unit (k, M, G...), "#.###" style.
pub(crate) fn maxed_amount(amount: f64, max: f64) -> (String, String, &'static str) {
    const UNITS: [&str; 6] = ["k", "M", "G", "T", "P", "E"];
    let short = |v: f64, by: f64| {
        let f = v / by;
        let s = if f < 10.0 {
            format!("{f:.3}")
        } else if f < 100.0 {
            format!("{f:.2}")
        } else {
            format!("{f:.1}")
        };
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
    };
    if max < 10_000.0 {
        return (short(amount, 1.0), short(max, 1.0), "");
    }
    let mut i = 0;
    while i + 1 < UNITS.len() && max / 1000f64.powi(i as i32 + 1) >= 1000.0 {
        i += 1;
    }
    let by = 1000f64.powi(i as i32 + 1);
    (short(amount, by), short(max, by), UNITS[i])
}

/// Tooltips.energyStorageComponent: "Stored Energy: 1.2/1.6M AE (75%)".
pub(crate) fn energy_line(label: &str, power: f64, most: f64) -> String {
    let (cur, max, unit) = maxed_amount(power, most);
    let ratio = if most > 0.0 { power / most } else { 0.0 };
    let pct = format!("{:.2}", ratio * 100.0);
    let pct = pct.trim_end_matches('0').trim_end_matches('.');
    let number = if cur == "0" { cur } else { format!("{cur}{unit}") };
    format!("{label}: {number}/{max}{unit} AE ({pct}%)")
}

/// The tooltip's energy line for a powered item (none for others).
pub(crate) fn tooltip(host: &dyn ClientHost, stack: &ItemStack) -> Option<String> {
    let most = capacity(stack)?;
    let label = crate::screens::tr(host, "gui.ae2.StoredEnergy", "Stored Energy");
    Some(energy_line(&label, item_power(stack), most))
}

/// The charge bar's Part (AEBasePoweredItem.isBarVisible/getBarWidth/getBarColor: always shown, green).
pub(crate) struct PoweredBar;

#[mod_api::sunrise_part]
impl ItemPart for PoweredBar {
    fn item_bar(&self, host: &dyn ClientHost, event: &mut ItemBarEvent<'_>) {
        ids(host);
        let Some(most) = capacity(event.stack) else { return };
        *event.bar = Some((bar_width(item_power(event.stack), most), 0x00FF00));
    }
}

/// The FE a powered item takes from other mods' chargers (PoweredItemCapabilities: receive only, 1 FE is
/// `net::ae_per_fe` AE).
struct ItemFe(ItemPlace);

impl EnergyHandler for ItemFe {
    fn amount(&self, host: &mut dyn ServerHost) -> u32 {
        let Some(stack) = self.0.stack_owned(host) else { return 0 };
        (item_power(&stack) / crate::net::ae_per_fe().max(1e-9)).floor().clamp(0.0, f64::from(u32::MAX)) as u32
    }
    fn capacity(&self, host: &mut dyn ServerHost) -> u32 {
        let Some(stack) = self.0.stack_owned(host) else { return 0 };
        capacity(&stack).map_or(0, |most| (most / crate::net::ae_per_fe().max(1e-9)).floor().clamp(0.0, f64::from(u32::MAX)) as u32)
    }
    fn insert(&self, host: &mut dyn ServerHost, tx: &TransactionContext, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        let Some(stack) = self.0.stack_owned(host) else { return Ok(0) };
        let Some(most) = capacity(&stack) else { return Ok(0) };
        let per = crate::net::ae_per_fe().max(1e-9);
        let have = item_power(&stack);
        let room = ((most - have) / per).floor().max(0.0);
        let n = f64::from(amount).min(room) as u32;
        if n == 0 {
            return Ok(0);
        }
        let edited = crate::cells::edit_stack(&self.0, host, tx, |s| set_item_power(s, (have + f64::from(n) * per).min(most))).is_ok();
        if edited { Ok(n) } else { Ok(0) }
    }
    fn extract(&self, _host: &mut dyn ServerHost, _tx: &TransactionContext, _amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        Ok(0)
    }
}

pub(crate) fn register(r: &mut dyn Registrar) {
    for id in POWERED_ITEMS {
        if item_battery(id).is_none() {
            continue;
        }
        let k = r.item_key(id);
        r.item_part(k, PoweredBar);
        r.item_capability(&ITEM_ENERGY, id, |_host, place| Some(Box::new(ItemFe(place.clone())) as Box<dyn EnergyHandler>));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_cards_raise_capacity() {
        assert_eq!(capacity_with_cards(1000.0, 0, false), 1000.0);
        assert_eq!(capacity_with_cards(1000.0, 1, false), 9000.0);
        assert_eq!(capacity_with_cards(1000.0, 2, true), 3000.0);
    }

    #[test]
    fn bar_follows_charge() {
        assert_eq!(bar_width(0.0, 100.0), 0);
        assert_eq!(bar_width(50.0, 100.0), 7);
        assert_eq!(bar_width(100.0, 100.0), 13);
        assert_eq!(bar_width(500.0, 100.0), 13);
        assert_eq!(bar_width(5.0, 0.0), 0);
    }

    #[test]
    fn stored_energy_line() {
        assert_eq!(energy_line("Stored Energy", 0.0, 20_000.0), "Stored Energy: 0/20k AE (0%)");
        assert_eq!(energy_line("Stored Energy", 1_200_000.0, 1_600_000.0), "Stored Energy: 1.2M/1.6M AE (75%)");
        assert_eq!(energy_line("Stored Energy", 500.0, 8000.0), "Stored Energy: 500/8000 AE (6.25%)");
    }
}
