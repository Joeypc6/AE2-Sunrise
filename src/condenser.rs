//! AE2's matter condenser (CondenserBlockEntity, CondenserMenu, CondenserScreen).
//!
//! Slots: 0 input (whatever goes in is destroyed: an item is one power, a fluid one per 125 mB), 1 output, 2 the
//! storage component (its bytes times 8 is the most power it keeps). The output mode (Settings.CONDENSER_OUTPUT):
//! trash, matter balls (256 power each) or singularities (256 000 each), AEConfig's `Condenser` section. Hoppers and
//! pipes put into the input and take from the output from every side; fluids go in through its tank. It is no
//! network node.

use crate::cells::*;
use crate::machines::{CONDENSER, TEXT, base_screen, inside, slot_icon};
use crate::screens::{spr, tr};
use crate::util::*;
use mod_api::*;

pub(crate) const SLOTS: usize = 3;
const INPUT: usize = 0;
const OUTPUT: usize = 1;
const COMPONENT: usize = 2;
/// Its fluid input's tank (emptied into power each tick).
pub(crate) const TANK_MB: i32 = 16_000;
/// CondenserBlockEntity.BYTE_MULTIPLIER.
const BYTE_MULTIPLIER: f64 = 8.0;
/// The mode button (ServerSettingToggleButton at 128, 52).
pub(crate) const BTN_MODE: i32 = 1;
const MODE_AT: [f32; 4] = [128.0, 52.0, 18.0, 18.0];
const BAR_AT: [f32; 4] = [120.0, 25.0, 6.0, 18.0];

/// A storage component's bytes (StorageComponentItem.getBytes): the cell components and the spatial ones.
pub(crate) fn component_bytes(item: &str) -> Option<f64> {
    let kb = match item.strip_prefix("ae2:")? {
        "cell_component_1k" => 1,
        "cell_component_4k" => 4,
        "cell_component_16k" => 16,
        "cell_component_64k" => 64,
        "cell_component_256k" => 256,
        "spatial_cell_component_2" => 2,
        "spatial_cell_component_16" => 16,
        "spatial_cell_component_128" => 128,
        _ => return None,
    };
    Some(f64::from(kb) * 1024.0)
}

/// CondenserBlockEntity.getStorage: the most power the component keeps.
pub(crate) fn storage_of(component: Option<&str>) -> f64 {
    component.and_then(component_bytes).map_or(0.0, |b| b * BYTE_MULTIPLIER)
}

/// CondenserOutput: what a mode makes and the power each takes (0: trash).
pub(crate) fn output_of(mode: i64, matter_balls: i32, singularity: i32) -> Option<(&'static str, f64)> {
    match mode {
        1 => Some(("ae2:matter_ball", f64::from(matter_balls))),
        2 => Some(("ae2:singularity", f64::from(singularity))),
        _ => None,
    }
}

/// CondenserBlockEntity.addPower: the power with `added` in, kept to the storage, and how many products it makes
/// (each `required`), at most `room`.
pub(crate) fn condense(power: f64, added: f64, storage: f64, required: f64, room: u32) -> (f64, u32) {
    let mut power = (power + added).clamp(0.0, storage.max(0.0));
    let mut made = 0;
    while required > 0.0 && power >= required && made < room {
        power -= required;
        made += 1;
    }
    (power, made)
}

/// The power, storage and required as data slots (16 bits on the network): each shifted right by `shift`, which
/// is sent with them (`shift << 1 | storage enough`).
pub(crate) fn encode(power: f64, storage: f64, required: f64) -> [i32; 3] {
    let big = power.max(required).max(0.0) as i64;
    let mut shift = 0;
    while (big >> shift) > 32_767 {
        shift += 1;
    }
    [(power as i64 >> shift) as i32, ((shift as i32) << 1) | i32::from(required > 0.0 && storage >= required), (required as i64 >> shift) as i32]
}

pub(crate) fn tick(host: &mut dyn ServerHost, _pos: Pos, fields: &mut Fields) {
    if fields.items.len() < SLOTS {
        fields.items.resize(SLOTS, None);
    }
    if fields.data.len() < 4 {
        fields.data.resize(4, 0);
    }
    let cfg = crate::config::config();
    let mode = nbt_i64(&fields.nbt, "mode").clamp(0, 2);
    let storage = storage_of(fields.items[COMPONENT].as_ref().map(sid));
    let mut added = 0.0;
    // CondenseItemHandler.insertItem: the input is gone, as power.
    if let Some(input) = fields.items[INPUT].take() {
        added += f64::from(input.count());
    }
    // CondenseFluidHandler: a fluid's amount over AEKeyType.fluids().getAmountPerOperation().
    if let Some(Tag::List(tanks)) = fields.nbt.get(&java("fluids")).cloned() {
        let mut out = Vec::new();
        let mut changed = false;
        for t in tanks {
            let Tag::Compound(mut c) = t else {
                out.push(t);
                continue;
            };
            if let Some(Tag::Int(a)) = c.get(&java("Amount")).cloned()
                && a > 0
            {
                added += f64::from(a) / 125.0;
                c.insert(java("Amount"), Tag::Int(0));
                c.insert(java("Fluid"), Tag::String(java("minecraft:empty")));
                changed = true;
            }
            out.push(Tag::Compound(c));
        }
        if changed {
            fields.nbt.insert(java("fluids"), Tag::List(out));
        }
    }
    let power = match fields.nbt.get(&java("storedPower")) {
        Some(Tag::Double(b)) => f64::from_bits(*b),
        _ => 0.0,
    };
    let product = output_of(mode, cfg.condenser_matter_balls, cfg.condenser_singularity);
    let required = product.map_or(0.0, |(_, r)| r);
    // canAddOutput: the output empty, or the product with room.
    let made_stack = product.and_then(|(id, _)| new_stack(&*host, id, 1));
    let room = match (&made_stack, &fields.items[OUTPUT]) {
        (None, _) => 0,
        (Some(_), None) => made_stack.as_ref().map_or(0, |m| max_stack(&*host, m)),
        (Some(m), Some(o)) if o.is_stackable_with(m) => max_stack(&*host, o).saturating_sub(o.count()),
        _ => 0,
    };
    let (power, made) = condense(power, added, storage, required, room);
    if made > 0
        && let Some(m) = made_stack
    {
        let have = fields.items[OUTPUT].as_ref().map_or(0, |o| o.count());
        fields.items[OUTPUT] = with_count(&m, have + made);
    }
    if fields.nbt.get(&java("storedPower")) != Some(&Tag::Double(power.to_bits())) {
        fields.nbt.insert(java("storedPower"), Tag::Double(power.to_bits()));
    }
    let [p, flags, r] = encode(power, storage, required);
    let data = [mode as i32, flags, p, r];
    if fields.data[..4] != data {
        fields.data[..4].copy_from_slice(&data);
    }
}

// ---- Automation and the menu's rules

pub(crate) fn insert_limit(slot: usize) -> u32 {
    if slot == INPUT { u32::MAX } else { 0 }
}

pub(crate) fn may_extract(slot: usize) -> bool {
    slot == OUTPUT
}

/// ITEMS for the condenser: every face reaches the input (in) and the output (out).
pub(crate) fn items(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    crate::machines::handler(host, &at, CONDENSER, vec![INPUT, OUTPUT])
}

/// CondenserMenu's slots: anything into the input (a storage component only when its own slot is taken, so a
/// shift-click doesn't destroy one), nothing into the output, one storage component into its slot.
pub(crate) fn slot_rules(context: &MenuSlotContext, slot: usize, stack: &ItemStack, may_place: &mut bool, max_stack: &mut u32) {
    let component = component_bytes(sid(stack)).is_some();
    match slot {
        INPUT => *may_place = !component || context.slot(MenuContainer::Main, COMPONENT).is_some(),
        OUTPUT => *may_place = false,
        _ => {
            *may_place = component;
            *max_stack = 1;
        }
    }
}

pub(crate) fn button(fields: &mut Fields, id: i32, back: bool) {
    if id == BTN_MODE {
        crate::machines::cycle(&mut fields.nbt, "mode", 3, back);
    }
}

// ---- The screen (condenser.json over condenser.png)

pub(crate) fn screen() -> MenuScreenLayout {
    base_screen(176.0, 197.0, vec![[51.0, 52.0], [105.0, 52.0], [101.0, 26.0]], 115.0)
}

/// The data slots back: mode, power, required, whether the component keeps enough.
fn decode(data: &[i32]) -> (i64, i64, i64, bool) {
    let v = |i: usize| i64::from(data.get(i).copied().unwrap_or(0));
    let shift = (v(1) >> 1).clamp(0, 40);
    (v(0).clamp(0, 2), v(2) << shift, v(3) << shift, v(1) & 1 != 0)
}

const MODE_ICONS: [&str; 3] = ["minecraft:lava_bucket", "ae2:matter_ball", "ae2:singularity"];

pub(crate) fn draw(host: &dyn ClientHost, data: &[i32], d: &mut dyn DrawContext, o: [f32; 2]) {
    let (mode, power, required, enough) = decode(data);
    spr(d, "ae2:gui/condenser", o, [0.0, 0.0], [0.0, 0.0, 176.0, 197.0]);
    d.text(&tr(host, "gui.ae2.Condenser", "Matter Condenser"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
    d.text(&tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 104.0], TEXT, false);
    // RestrictedInputSlot's TRASH and STORAGE_COMPONENT icons.
    slot_icon(d, o, [51.0, 52.0], [240.0, 80.0]);
    slot_icon(d, o, [101.0, 26.0], [240.0, 48.0]);
    // The progress bar fills upwards, power over what the product takes.
    if required > 0 {
        let h = (18.0 * power.min(required) as f32 / required as f32).floor();
        if h > 0.0 {
            d.sprite("ae2:gui/condenser", [178.0, 25.0 + 18.0 - h, 6.0, h], [o[0] + 120.0, o[1] + 25.0 + 18.0 - h, 6.0, h], None);
        }
    }
    d.gui_nine_slice("widget/button", [o[0] + MODE_AT[0], o[1] + MODE_AT[1], MODE_AT[2], MODE_AT[3]], 0xFFFF_FFFF);
    if let Some(icon) = new_stack(host, MODE_ICONS[mode as usize], 1) {
        d.gui_draw_item_with(&icon, [o[0] + MODE_AT[0] + 1.0, o[1] + MODE_AT[1] + 1.0], None);
    }
    let line = if mode == 0 {
        tr(host, "gui.tooltips.ae2.Trash", "Destroy Items")
    } else if !enough {
        "Needs a bigger storage component".to_owned()
    } else {
        format!("{power} / {required}")
    };
    d.text(&line, [o[0] + 8.0, o[1] + 78.0], if mode != 0 && !enough { 0xFFAA_0000 } else { TEXT }, false);
}

pub(crate) fn click(at: [f32; 2]) -> Option<i32> {
    inside(at, MODE_AT).then_some(BTN_MODE)
}

pub(crate) fn tooltip(host: &dyn ClientHost, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) {
    let (mode, power, required, _) = decode(data);
    if inside(at, MODE_AT) {
        lines.push(tr(host, "gui.tooltips.ae2.CondenserOutput", "ME Condenser - Output"));
        let cfg = crate::config::config();
        let line = match mode {
            1 => tr(host, "gui.tooltips.ae2.MatterBalls", "Condense Into Matter Balls\n%s per item").replace("%s", &cfg.condenser_matter_balls.to_string()),
            2 => tr(host, "gui.tooltips.ae2.Singularity", "Condense Into Singularities\n%s per item").replace("%s", &cfg.condenser_singularity.to_string()),
            _ => tr(host, "gui.tooltips.ae2.Trash", "Destroy Items"),
        };
        for l in line.lines() {
            lines.push(format!("§7{l}"));
        }
    } else if inside(at, BAR_AT) && required > 0 {
        lines.push(format!("{power} / {required}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_by_component() {
        assert_eq!(storage_of(None), 0.0);
        assert_eq!(storage_of(Some("minecraft:dirt")), 0.0);
        assert_eq!(storage_of(Some("ae2:cell_component_1k")), 8192.0);
        assert_eq!(storage_of(Some("ae2:cell_component_256k")), 2_097_152.0);
        // Singularities (256 000) need a 64k component or bigger.
        assert!(storage_of(Some("ae2:cell_component_16k")) < 256_000.0);
        assert!(storage_of(Some("ae2:cell_component_64k")) >= 256_000.0);
    }

    #[test]
    fn thresholds() {
        // Trash: nothing kept, nothing made.
        assert_eq!(condense(0.0, 64.0, 0.0, 0.0, 64), (0.0, 0));
        // Matter balls: 256 each, the rest kept.
        assert_eq!(condense(200.0, 100.0, 8192.0, 256.0, 64), (44.0, 1));
        assert_eq!(condense(0.0, 1024.0, 8192.0, 256.0, 64), (0.0, 4));
        // No room in the output: the power waits, kept to the storage.
        assert_eq!(condense(8000.0, 1000.0, 8192.0, 256.0, 0), (8192.0, 0));
        assert_eq!(condense(0.0, 1024.0, 8192.0, 256.0, 2), (512.0, 2));
        // Too small a component never reaches a singularity.
        assert_eq!(condense(0.0, 1e9, 131_072.0, 256_000.0, 64), (131_072.0, 0));
        assert_eq!(output_of(0, 256, 256_000), None);
        assert_eq!(output_of(2, 256, 256_000), Some(("ae2:singularity", 256_000.0)));
    }

    #[test]
    fn data_fits_sixteen_bits() {
        let [p, f, r] = encode(2_097_152.0, 2_097_152.0, 256_000.0);
        assert!(p <= 32_767 && r <= 32_767);
        let (_, power, required, enough) = decode(&[2, f, p, r]);
        assert!(enough);
        assert!((power - 2_097_152).abs() < 128 && (required - 256_000).abs() < 128);
        let [p, f, r] = encode(100.0, 8192.0, 256.0);
        assert_eq!((p, f, r), (100, 1, 256));
        let [_, f, _] = encode(0.0, 8192.0, 256_000.0);
        assert_eq!(f & 1, 0);
    }
}
