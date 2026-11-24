//! Items used in the hand: storage cells taken apart, crafting blocks taken apart, and the memory card on a pattern
//! provider (its patterns) with the card's colour code.
//! - BasicStorageCell.disassembleDrive: sneaking, an empty storage cell used in the air gives back its housing, its
//!   component and its upgrade cards; one with anything in it is refused. Portable cells the same way when common.json's
//!   `PortableCells.allowDisassembly` allows (ME chest, component, energy cell with the cell's power, housing, cards).
//! - CraftingBlockItem.use: sneaking, a crafting storage, accelerator or monitor stack goes back to crafting units and
//!   what made them, when `crafting.disassemblyCrafting` allows.
//! - PatternProviderLogic.exportSettings / importSettings (SettingsFrom.MEMORY_CARD): sneaking, the card takes the
//!   provider's settings and a copy of its patterns; used on another provider it pastes them, encoding each pattern
//!   again from a blank pattern of the player's (the provider's old patterns count as blanks, a creative player needs
//!   none).
//! - MemoryCardItem's colour code: eight squares on the card's icon, a stored P2P frequency's four digits two squares
//!   each (AEColor's medium shades), else all TRANSPARENT's.
//!
//! Upstream: `use_item` and `tool_use_air`, `tool_use` (memory card), `pattern_decoration` (`../ae2-horizon/mixins/ae2/me.rs`).

use crate::cells::*;
use crate::util::*;
use mod_api::*;

const SETTINGS: &str = "ae2:settings";
const SETTINGS_FROM: &str = "ae2:settings_from";
const PROVIDER: &str = "ae2:pattern_provider";
const BLANK: &str = "ae2:blank_pattern";

fn sneaking(host: &dyn Host, player: u128) -> bool {
    host.player(player).is_some_and(|p| {
        let e = p.entity();
        e.is_shift_key_down(host).unwrap_or(false) || e.is_crouching(host).unwrap_or(false)
    })
}

fn say(host: &mut dyn ServerHost, player: u128, text: &str) {
    if let Some(p) = host.player(player) {
        let _ = p.action_bar(host, Text::literal(text));
    }
}

// ---- Taking items apart (pure)

/// A storage cell's parts: its housing and its component.
pub(crate) fn storage_cell_parts(item: &str) -> Option<(&'static str, String)> {
    let name = item.strip_prefix("ae2:")?;
    let (housing, size) = match name.strip_prefix("item_storage_cell_") {
        Some(size) => ("ae2:item_cell_housing", size),
        None => ("ae2:fluid_cell_housing", name.strip_prefix("fluid_storage_cell_")?),
    };
    cell_type(item)?;
    Some((housing, format!("ae2:cell_component_{size}")))
}

/// A portable cell's parts beside its energy cell: housing, component.
pub(crate) fn portable_cell_parts(item: &str) -> Option<(&'static str, String)> {
    let name = item.strip_prefix("ae2:")?;
    let (housing, size) = match name.strip_prefix("portable_item_cell_") {
        Some(size) => ("ae2:item_cell_housing", size),
        None => ("ae2:fluid_cell_housing", name.strip_prefix("portable_fluid_cell_")?),
    };
    matches!(size, "1k" | "4k" | "16k" | "64k" | "256k").then(|| (housing, format!("ae2:cell_component_{size}")))
}

/// AEBlocks.craftingBlock's disassemblyExtra: what a crafting block gives back beside its crafting unit.
pub(crate) fn crafting_block_extra(item: &str) -> Option<&'static str> {
    match item {
        "ae2:crafting_accelerator" => Some("ae2:engineering_processor"),
        "ae2:1k_crafting_storage" => Some("ae2:cell_component_1k"),
        "ae2:4k_crafting_storage" => Some("ae2:cell_component_4k"),
        "ae2:16k_crafting_storage" => Some("ae2:cell_component_16k"),
        "ae2:64k_crafting_storage" => Some("ae2:cell_component_64k"),
        "ae2:256k_crafting_storage" => Some("ae2:cell_component_256k"),
        "ae2:crafting_monitor" => Some("ae2:storage_monitor"),
        _ => None,
    }
}

/// The items a sneaking use takes apart (their `TakeApart` part).
pub(crate) const TAKEN_APART: [&str; 27] = [
    "ae2:item_storage_cell_1k", "ae2:item_storage_cell_4k", "ae2:item_storage_cell_16k", "ae2:item_storage_cell_64k", "ae2:item_storage_cell_256k",
    "ae2:fluid_storage_cell_1k", "ae2:fluid_storage_cell_4k", "ae2:fluid_storage_cell_16k", "ae2:fluid_storage_cell_64k", "ae2:fluid_storage_cell_256k",
    "ae2:portable_item_cell_1k", "ae2:portable_item_cell_4k", "ae2:portable_item_cell_16k", "ae2:portable_item_cell_64k", "ae2:portable_item_cell_256k",
    "ae2:portable_fluid_cell_1k", "ae2:portable_fluid_cell_4k", "ae2:portable_fluid_cell_16k", "ae2:portable_fluid_cell_64k", "ae2:portable_fluid_cell_256k",
    "ae2:crafting_accelerator", "ae2:1k_crafting_storage", "ae2:4k_crafting_storage", "ae2:16k_crafting_storage", "ae2:64k_crafting_storage",
    "ae2:256k_crafting_storage", "ae2:crafting_monitor",
];

/// The items `use_item` takes apart.
pub(crate) fn takes_apart(item: &str) -> bool {
    storage_cell_parts(item).is_some() || portable_cell_parts(item).is_some() || crafting_block_extra(item).is_some()
}

// ---- Taking items apart (the hand)

/// The hand's stack taken (`count` of it) and `parts` given, in one go: nothing given unless the hand gave it up.
fn trade(host: &mut dyn ServerHost, player: u128, hand: Hand, stack: &ItemStack, count: u32, parts: Vec<ItemStack>) -> bool {
    let res = ItemResource::of(stack);
    let place = ItemPlace::PlayerHand { player, hand };
    let took = host.transaction(|host, tx| if place.extract(host, tx, &res, count).unwrap_or(0) == count { Ok(()) } else { Err(()) }).is_ok();
    if took {
        for p in parts {
            crate::wrench::give_or_drop(host, player, p);
        }
    }
    took
}

/// A sneaking use in the air of a cell or crafting block: taken apart. Some when it took the use.
pub(crate) fn use_item(host: &mut dyn ServerHost, stack: &ItemStack, hand: Hand, player: u128) -> Option<InteractionResult> {
    let item = sid(stack);
    if !takes_apart(item) || !sneaking(&*host, player) {
        return None;
    }
    let config = crate::config::config();
    if let Some(extra) = crafting_block_extra(item) {
        if !config.disassembly_crafting {
            return None;
        }
        let n = stack.count();
        let parts: Vec<ItemStack> = [new_stack(&*host, "ae2:crafting_unit", n), new_stack(&*host, extra, n)].into_iter().flatten().collect();
        if parts.len() == 2 {
            trade(host, player, hand, stack, n, parts);
        }
        return Some(InteractionResult::SuccessServer);
    }
    let portable = portable_cell_parts(item);
    if portable.is_some() && !config.portable_cell_disassembly {
        return None;
    }
    if read_cell(stack).item_count > 0 {
        say(host, player, "Only empty storage cells can be disassembled.");
        return Some(InteractionResult::SuccessServer);
    }
    let mut parts: Vec<Option<ItemStack>> = Vec::new();
    if let Some((housing, component)) = storage_cell_parts(item) {
        parts.push(new_stack(&*host, housing, 1));
        parts.push(new_stack(&*host, &component, 1));
    } else if let Some((housing, component)) = portable {
        // AbstractPortableCell: the ME chest, the component, the energy cell (with what the cell had stored), the housing.
        let mut cell = new_stack(&*host, "ae2:energy_cell", 1);
        let power = crate::world::item_power(stack);
        if power > 0.0
            && let Some(c) = cell.as_mut()
        {
            edit_custom_data(c, |d| {
                d.insert(java("internalCurrentPower"), Tag::Double(power.to_bits()));
            });
        }
        parts.extend([new_stack(&*host, "ae2:chest", 1), new_stack(&*host, &component, 1), cell, new_stack(&*host, housing, 1)]);
    }
    for card in cell_cards(stack) {
        parts.push(new_stack(&*host, &card, 1));
    }
    if parts.iter().any(Option::is_none) {
        return Some(InteractionResult::SuccessServer);
    }
    trade(host, player, hand, &with_count(stack, 1)?, 1, parts.into_iter().flatten().collect());
    Some(InteractionResult::SuccessServer)
}

// ---- The memory card on a pattern provider

/// How the provider's slots change for a paste: the patterns kept (`used` of the wanted ones, in slot order), and the
/// blank patterns the player gives (+) or gets back (-). `old` encoded patterns in the provider count as blanks;
/// `blanks` is what the player carries (None: a creative player, who needs none and gets none back).
pub(crate) fn paste_plan(wanted: &[bool], old: u32, blanks: Option<u32>) -> (Vec<bool>, i64) {
    let mut left = blanks.map(|b| i64::from(b) + i64::from(old));
    let keep: Vec<bool> = wanted
        .iter()
        .map(|w| {
            if !*w {
                return false;
            }
            match left.as_mut() {
                Some(l) if *l <= 0 => false,
                Some(l) => {
                    *l -= 1;
                    true
                }
                None => true,
            }
        })
        .collect();
    let used = keep.iter().filter(|k| **k).count() as i64;
    let paid = if blanks.is_some() { used - i64::from(old) } else { 0 };
    (keep, paid)
}

/// The player's inventory slots (main, then the off hand).
fn player_slots() -> impl Iterator<Item = i32> {
    (0..36).chain(std::iter::once(-106))
}

/// The memory card in `hand` replaced by `new` (in the slot it is in: a creative hand would keep the old one).
fn replace_card(host: &mut dyn ServerHost, player: u128, hand: Hand, old: &ItemStack, new: ItemStack) -> bool {
    let slot = match hand {
        Hand::Main => match host.player(player).and_then(|p| p.selected_slot(&*host)) {
            Some(s) => s as i32,
            None => return false,
        },
        Hand::Off => -106,
    };
    let place = ItemPlace::PlayerSlot { player, slot };
    host.transaction(|host, tx| {
        if place.extract(host, tx, &ItemResource::of(old), old.count()).unwrap_or(0) != old.count() {
            return Err(());
        }
        if place.insert(host, tx, &ItemResource::of(&new), new.count()).unwrap_or(0) != new.count() {
            return Err(());
        }
        Ok(())
    })
    .is_ok()
}

/// MemoryCardItem on a pattern provider (block or part at `pos`): sneaking saves its settings and patterns, else pastes
/// them. Some when the card took the click.
pub(crate) fn provider_use(host: &mut dyn ServerHost, held: &ItemStack, click: &BlockClick<'_>, block: &str, pos: Pos) -> Option<InteractionResult> {
    if sid(held) != "ae2:memory_card" || !crate::autocraft::is_provider(block) {
        return None;
    }
    let level = here(&*host)?;
    let player = click.player;
    let own = crate::bus::nbt_at(&*host, level, pos).unwrap_or_default();
    if sneaking(&*host, player) {
        let mut saved = Compound::new();
        for k in ["blocking", "lock", "priority"] {
            if let Some(v) = own.get(&java(k)) {
                saved.insert(java(k), v.clone());
            }
        }
        let mut patterns = Vec::new();
        for i in 0..9 {
            if let Some(s) = crate::bus::item_slot(&*host, level, pos, i)
                && crate::autocraft::is_pattern(sid(&s))
                && let Some(n) = s.to_nbt(&*host)
            {
                let mut e = Compound::new();
                e.insert(java("Slot"), Tag::Int(i as i32));
                e.insert(java("item"), Tag::Compound(n));
                patterns.push(Tag::Compound(e));
            }
        }
        saved.insert(java("patterns"), Tag::List(patterns));
        let mut card = held.clone();
        edit_custom_data(&mut card, |d| {
            d.insert(java(SETTINGS), Tag::Compound(saved));
            d.insert(java(SETTINGS_FROM), Tag::String(java(PROVIDER)));
            d.remove(&java("ae2:p2p_freq"));
            d.remove(&java("ae2:p2p_kind"));
        });
        if replace_card(host, player, click.hand, held, card) {
            say(host, player, "Settings saved to memory card.");
        }
        return Some(InteractionResult::SuccessServer);
    }
    let data = custom_data(held)?;
    let (Some(Tag::Compound(saved)), Some(Tag::String(from))) = (data.get(&java(SETTINGS)).cloned(), data.get(&java(SETTINGS_FROM)).cloned()) else {
        return None;
    };
    if from.to_string_lossy() != PROVIDER {
        say(host, player, "Invalid memory card settings for this machine.");
        return Some(InteractionResult::SuccessServer);
    }
    let mut out = own;
    for k in ["blocking", "lock", "priority"] {
        match saved.get(&java(k)) {
            Some(v) => out.insert(java(k), v.clone()),
            None => out.remove(&java(k)),
        };
    }
    crate::bus::set_nbt_at(host, level, pos, out);
    if let Some(Tag::List(list)) = saved.get(&java("patterns")) {
        paste_patterns(host, level, pos, player, list);
    }
    say(host, player, "Settings loaded from memory card.");
    Some(InteractionResult::SuccessServer)
}

/// PatternProviderLogic.importSettings' patterns: the provider's old patterns out (as blanks), the card's in, each
/// paid for with a blank pattern, all in one transaction.
fn paste_patterns(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, player: u128, list: &[Tag]) {
    let mut wanted: Vec<Option<ItemStack>> = vec![None; 9];
    for t in list {
        let Tag::Compound(e) = t else { continue };
        let (Some(Tag::Int(i)), Some(Tag::Compound(n))) = (e.get(&java("Slot")), e.get(&java("item"))) else { continue };
        let Some(stack) = ItemStack::from_nbt(&*host, n) else { continue };
        if (0..9).contains(i) && crate::autocraft::is_pattern(sid(&stack)) && crate::autocraft::decode(&*host, &stack).is_some() {
            wanted[*i as usize] = with_count(&stack, 1);
        }
    }
    let current: Vec<Option<ItemStack>> = (0..9).map(|i| crate::bus::item_slot(&*host, level, pos, i)).collect();
    let old: u32 = current.iter().flatten().filter(|s| crate::autocraft::is_pattern(sid(s))).map(|s| s.count()).sum();
    let others: Vec<ItemStack> = current.iter().flatten().filter(|s| !crate::autocraft::is_pattern(sid(s))).cloned().collect();
    let creative = host.player(player).and_then(|p| p.is_creative(&*host)).unwrap_or(false);
    let blank_res = new_stack(&*host, BLANK, 1).map(|b| ItemResource::of(&b));
    let Some(blank_res) = blank_res else { return };
    let carried: u32 = player_slots().filter_map(|slot| (ItemPlace::PlayerSlot { player, slot }).stack_owned(host)).filter(|s| sid(s) == BLANK).map(|s| s.count()).sum();
    let (keep, paid) = paste_plan(&wanted.iter().map(Option::is_some).collect::<Vec<_>>(), old, (!creative).then_some(carried));
    let done = host.transaction(|host, tx| {
        for i in 0..9 {
            let (at, slot) = crate::wrench::slot_target(pos, i);
            let be = host.block_entity(level, at).ok_or(())?;
            let new = if keep[i] { wanted[i].clone() } else { None };
            if be.set_item_slot(host, tx, slot, new).is_err() {
                return Err(());
            }
        }
        let mut owed = paid.max(0) as u32;
        for slot in player_slots() {
            if owed == 0 {
                break;
            }
            owed -= (ItemPlace::PlayerSlot { player, slot }).extract(host, tx, &blank_res, owed).unwrap_or(0).min(owed);
        }
        if owed > 0 { Err(()) } else { Ok(()) }
    });
    if done.is_err() {
        debug(&format!("memory card: pasting patterns at {pos:?} refused"));
        return;
    }
    if paid < 0
        && let Some(b) = new_stack(&*host, BLANK, (-paid) as u32)
    {
        crate::wrench::give_or_drop(host, player, b);
    }
    for s in others {
        crate::wrench::give_or_drop(host, player, s);
    }
}

// ---- The memory card's colour code

/// AEColor's medium shades, white to black (P2PTunnelPart's frequency colours).
const AE_COLOR_MEDIUM: [u32; 16] = [
    0xDBDBDB, 0xFAAE44, 0xB82AB8, 0x82ACE7, 0xF8FF4A, 0xBBFF51, 0xF8B5D7, 0xA0A0A0, 0xCDCDCD, 0x51AAC6, 0xA453CE, 0x514AFF, 0xB7967F,
    0x60E32E, 0xFF003C, 0x565656,
];
/// AEColor.TRANSPARENT's medium shade: a card with no frequency.
const NO_FREQ: u32 = 0x895CA8;

/// MemoryCardItem.getColorCode: the eight squares' colours, two for each of a frequency's four hex digits.
pub(crate) fn color_code(freq: Option<u16>) -> [u32; 8] {
    std::array::from_fn(|i| freq.map_or(NO_FREQ, |f| AE_COLOR_MEDIUM[usize::from((f >> (4 * (3 - i / 2))) & 0xF)]))
}

/// MemoryCardBakedModel's squares, four to a row, drawn over the card's icon.
pub(crate) fn memory_card_decorations(event: &mut ItemDecorationsEvent<'_>) {
    let freq = match custom_data(event.stack).and_then(|d| d.get(&java("ae2:p2p_freq"))) {
        Some(Tag::Long(f)) => Some(*f as u16),
        _ => None,
    };
    let (x, y) = (event.x, event.y);
    for (i, c) in color_code(freq).iter().enumerate() {
        let i = i as f32;
        event.draw.fill([x + 7.0 + i % 4.0, y + 6.0 + (i / 4.0).floor(), 1.0, 1.0], 0xFF00_0000 | c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_come_apart() {
        assert_eq!(storage_cell_parts("ae2:item_storage_cell_4k"), Some(("ae2:item_cell_housing", "ae2:cell_component_4k".to_owned())));
        assert_eq!(storage_cell_parts("ae2:fluid_storage_cell_256k"), Some(("ae2:fluid_cell_housing", "ae2:cell_component_256k".to_owned())));
        assert_eq!(storage_cell_parts("ae2:creative_item_cell"), None);
        assert_eq!(portable_cell_parts("ae2:portable_fluid_cell_16k"), Some(("ae2:fluid_cell_housing", "ae2:cell_component_16k".to_owned())));
        assert_eq!(portable_cell_parts("ae2:portable_item_cell_9k"), None);
        assert_eq!(crafting_block_extra("ae2:crafting_monitor"), Some("ae2:storage_monitor"));
        assert!(!takes_apart("ae2:crafting_unit"));
        assert!(TAKEN_APART.iter().all(|i| takes_apart(i)));
    }

    #[test]
    fn pasting_patterns_costs_blanks() {
        let w = [true, false, true, true, false, false, false, false, false];
        // Two old patterns and one blank carried: three of the card's go in, the player pays one.
        let (keep, paid) = paste_plan(&w, 2, Some(1));
        assert_eq!(keep.iter().filter(|k| **k).count(), 3);
        assert_eq!(paid, 1);
        // Not enough: the first ones in slot order, the old patterns used up.
        let (keep, paid) = paste_plan(&w, 1, Some(0));
        assert_eq!(keep, vec![true, false, false, false, false, false, false, false, false]);
        assert_eq!(paid, 0);
        // More old patterns than wanted: blanks come back.
        let (_, paid) = paste_plan(&w, 7, Some(0));
        assert_eq!(paid, -4);
        // A creative player: all of them, nothing paid or returned.
        let (keep, paid) = paste_plan(&w, 5, None);
        assert_eq!(keep, w.to_vec());
        assert_eq!(paid, 0);
    }

    #[test]
    fn color_code_shows_the_frequency() {
        assert_eq!(color_code(None), [NO_FREQ; 8]);
        let c = color_code(Some(0x0F1E));
        assert_eq!(c[0], AE_COLOR_MEDIUM[0]);
        assert_eq!(c[1], AE_COLOR_MEDIUM[0]);
        assert_eq!(c[2], AE_COLOR_MEDIUM[15]);
        assert_eq!(c[5], AE_COLOR_MEDIUM[1]);
        assert_eq!(c[7], AE_COLOR_MEDIUM[14]);
    }
}
