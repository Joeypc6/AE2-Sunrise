//! AE2's storage and conversion monitors (AbstractMonitorPart, StorageMonitorPart, ConversionMonitorPart), hosted on a
//! cable bus:
//! - a use with an item (unlocked) shows that item, a filled bucket on an item it already shows switches to its fluid,
//!   an empty hand clears it; Shift with an empty hand locks or unlocks it ("Monitor is now Locked.");
//! - the screen shows the item and the network's amount (`AmountFormat.SLOT`, "Craft" for none of a craftable),
//!   drawn on the part's face (`AbstractMonitorPart.renderDynamic`, `BlockEntityRenderHelper.renderItem2dWithAmount`);
//! - the conversion monitor stores the held stack when it shows that item (locked: any held stack, and an empty hand
//!   stores every carried stack of the shown item), and a punch takes a stack out (Shift: one).
//!
//! The part's own nbt (`p<side>.nbt` of its cell): `item` (a key, `cells::key_to_string`), `locked`, and what the
//! clients draw: `amount`, `craft`. The cell's update tag carries `m<side>` for each monitor (`update_tag`).

use crate::cells::{key_from_string, key_of, key_to_string, sid};
use crate::core::{AeKey, AmountFormat, KeyType};
use crate::storage::Scope;
use crate::util::*;
use mod_api::*;

pub(crate) fn is_monitor(id: &str) -> bool {
    matches!(id, "ae2:storage_monitor" | "ae2:conversion_monitor")
}

fn nbt_str(nbt: &Compound, key: &str) -> String {
    match nbt.get(&java(key)) {
        Some(Tag::String(s)) => s.to_string_lossy().to_string(),
        _ => String::new(),
    }
}

/// What the screen says under the item (renderItem2dWithAmount).
pub(crate) fn amount_text(key: &AeKey, amount: i64, can_craft: bool) -> String {
    if amount == 0 && can_craft {
        "Craft".to_owned()
    } else {
        key.format_amount(amount, AmountFormat::Slot)
    }
}

/// What a use with `held` does to an unlocked monitor's item (AbstractMonitorPart.onPartActivate): a held container of
/// the shown item shows its contents (a bucket's fluid), else the held item (none for an empty hand).
pub(crate) fn configured_after_use(
    shown: Option<&AeKey>,
    held: Option<&AeKey>,
    held_fluid: Option<&str>,
) -> Option<AeKey> {
    match (shown, held) {
        (Some(s), Some(h)) if s == h => match held_fluid {
            Some(f) => Some(AeKey::fluid(f, "")),
            None => Some(h.clone()),
        },
        _ => held.cloned(),
    }
}

/// What a use on a conversion monitor does (ConversionMonitorPart.onPartActivate).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConvUse {
    /// Every carried stack of the shown item into the network.
    StoreAll,
    /// The held stack into the network.
    StoreHeld,
    /// As a storage monitor (configure).
    Configure,
}

pub(crate) fn conversion_use(
    locked: bool,
    held_empty: bool,
    held_matches: bool,
    held_is_wrench: bool,
) -> ConvUse {
    if locked {
        if held_empty {
            ConvUse::StoreAll
        } else if held_is_wrench && !held_matches {
            ConvUse::Configure
        } else {
            ConvUse::StoreHeld
        }
    } else if held_matches {
        ConvUse::StoreHeld
    } else {
        ConvUse::Configure
    }
}

/// The part's tick: the network's amount of its item (every ten ticks), for the screen.
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let dim = host.dimension();
    if crate::net::with_net(dim, |n| n.tick) % 10 != 0 {
        return;
    }
    let key = key_from_string(&nbt_str(&fields.nbt, "item"));
    let active = crate::net::device_state(dim, pos).2;
    let (amount, craft) = match (&key, active) {
        (Some(k), true) => {
            let held = crate::storage::with_storage(host, Scope::Grid(pos), None, |st| {
                st.contents().get(k)
            })
            .unwrap_or(0);
            let craft = crate::autocraft::craftables(dim, pos).0.contains(k);
            (held, craft)
        }
        _ => (0, false),
    };
    if nbt_i64(&fields.nbt, "amount") != amount {
        set_nbt_i64(&mut fields.nbt, "amount", amount);
    }
    if (nbt_i64(&fields.nbt, "craft") != 0) != craft {
        set_nbt_i64(&mut fields.nbt, "craft", i64::from(craft));
    }
}

/// For the cell's update tag: what each monitor's screen shows (`m<side>`: item, amount, craft, locked).
pub(crate) fn update_tag(part: &Compound) -> Option<Compound> {
    let Some(Tag::Compound(own)) = part.get(&java("nbt")) else {
        return None;
    };
    let mut out = Compound::new();
    out.insert(java("item"), Tag::String(java(&nbt_str(own, "item"))));
    for k in ["amount", "craft", "locked"] {
        out.insert(java(k), Tag::Long(nbt_i64(own, k)));
    }
    Some(out)
}

fn is_wrench(id: &str) -> bool {
    matches!(
        id,
        "ae2:certus_quartz_wrench" | "ae2:nether_quartz_wrench" | "ae2:network_tool"
    )
}

fn sneaking(host: &dyn ServerHost, player: u128) -> bool {
    host.player(player).is_some_and(|p| {
        let e = p.entity();
        e.is_shift_key_down(host).unwrap_or(false) || e.is_crouching(host).unwrap_or(false)
    })
}

fn hand_slot(host: &dyn ServerHost, player: u128, hand: Hand) -> Option<i32> {
    match hand {
        Hand::Main => host
            .player(player)
            .and_then(|p| p.selected_slot(host))
            .map(|s| s as i32),
        Hand::Off => Some(-106),
    }
}

/// Up to `count` of `key` from the player's slot `slot` into the network (powered): what went in. Taken from the
/// player first, in a transaction, so nothing is ever made twice; what the network refused is put back.
fn store_from_slot(
    host: &mut dyn ServerHost,
    player: u128,
    vpos: Pos,
    slot: i32,
    key: &AeKey,
    stack: &ItemStack,
) -> i64 {
    let fits = crate::storage::simulate(
        host,
        Scope::Grid(vpos),
        None,
        key,
        i64::from(stack.count()),
        true,
    );
    let dim = host.dimension();
    let fits = crate::storage::power_allows(dim, vpos, key, fits).min(i64::from(stack.count()));
    if fits <= 0 {
        return 0;
    }
    let place = ItemPlace::PlayerSlot { player, slot };
    let res = ItemResource::of(stack);
    let taken = host
        .transaction(|host, tx| Ok::<u32, ()>(place.extract(host, tx, &res, fits as u32)))
        .unwrap_or(0);
    if taken == 0 {
        return 0;
    }
    let went = crate::storage::insert(host, Scope::Grid(vpos), None, key, i64::from(taken));
    let back = i64::from(taken) - went;
    if back > 0
        && let Some(rest) = crate::cells::with_count(stack, back as u32)
    {
        crate::wrench::give_or_drop(host, player, rest);
    }
    went
}

/// A use of a monitor (`vpos`, its hosted node). Some when it took the click.
pub(crate) fn use_part(
    host: &mut dyn ServerHost,
    click: &BlockClick<'_>,
    vpos: Pos,
    id: &str,
) -> Option<InteractionResult> {
    let Some(level) = here(&*host) else {
        debug("monitor: use_part: no level");
        return None;
    };
    let dim = host.dimension();
    // A monitor off its network does nothing (getMainNode().isActive()).
    let state = crate::net::device_state(dim, vpos);
    if !state.2 {
        debug(&format!(
            "monitor: use_part {id} {vpos:?}: not active (grid {:?} online {}), pass",
            state.0, state.1
        ));
        return Some(InteractionResult::Pass);
    }
    // The main hand only: after a main-hand use the game can still ask with the (empty) off hand, which would read
    // as "an empty hand clears it" and undo the click (vanilla stops at the hand whose use consumed the action).
    if click.hand != Hand::Main {
        debug(&format!("monitor: use_part {id} {vpos:?}: off hand, pass"));
        return Some(InteractionResult::Pass);
    }
    let Some(own) = crate::bus::nbt_at(&*host, level, vpos) else {
        debug(&format!("monitor: use_part {id} {vpos:?}: no part nbt"));
        return None;
    };
    let locked = nbt_i64(&own, "locked") != 0;
    let shown = key_from_string(&nbt_str(&own, "item"));
    let held = click.stack.filter(|s| s.count() > 0).cloned();
    let player = click.player;
    let sneak = sneaking(&*host, player);
    debug(&format!(
        "monitor: use_part {id} {vpos:?}: locked {locked} shown {:?} held {:?} hand {:?} sneaking {sneak}",
        nbt_str(&own, "item"),
        held.as_ref().map(|s| (sid(s).to_owned(), s.count())),
        click.hand
    ));
    if sneak {
        // onPartShiftActivate: an empty hand locks or unlocks.
        if held.is_none() {
            let mut own = own;
            set_nbt_i64(&mut own, "locked", i64::from(!locked));
            crate::bus::set_nbt_at(host, level, vpos, own);
            let msg = if locked {
                "chat.ae2.isNowUnlocked"
            } else {
                "chat.ae2.isNowLocked"
            };
            if let Some(p) = host.player(player) {
                let _ = p.action_bar(host, Text::translate(msg, Vec::new()));
            }
        }
        debug(&format!(
            "monitor: use_part {id}: sneaking, lock toggled {}",
            held.is_none()
        ));
        return Some(InteractionResult::SuccessServer);
    }
    crate::cells::ids(&*host);
    let held_key = held.as_ref().map(|s| key_of(&*host, s));
    if id == "ae2:conversion_monitor" {
        let matches = held_key.is_some() && held_key == shown;
        let wrench = held.as_ref().is_some_and(|s| is_wrench(sid(s)));
        let what = conversion_use(locked, held.is_none(), matches, wrench);
        debug(&format!("monitor: conversion use {what:?}"));
        let configure = match what {
            ConvUse::StoreAll => {
                if let Some(key) = shown.as_ref().filter(|k| k.key_type() == KeyType::Items) {
                    for slot in (0..36).chain(std::iter::once(-106)) {
                        let Some(stack) = (ItemPlace::PlayerSlot { player, slot }).stack_owned(host)
                        else {
                            continue;
                        };
                        if key_of(&*host, &stack) == *key {
                            store_from_slot(host, player, vpos, slot, key, &stack);
                        }
                    }
                }
                false
            }
            ConvUse::StoreHeld => {
                if let (Some(stack), Some(key), Some(slot)) =
                    (&held, &held_key, hand_slot(&*host, player, click.hand))
                {
                    let went = store_from_slot(host, player, vpos, slot, key, stack);
                    debug(&format!(
                        "monitor: stored {went} of {} from slot {slot}",
                        key_to_string(key)
                    ));
                } else {
                    debug(&format!(
                        "monitor: store held: no slot for hand {:?}",
                        click.hand
                    ));
                }
                false
            }
            ConvUse::Configure => true,
        };
        if !configure {
            return Some(InteractionResult::SuccessServer);
        }
    }
    if locked {
        debug(&format!("monitor: use_part {id}: locked, not configured"));
        return Some(InteractionResult::SuccessServer);
    }
    let fluid = held
        .as_ref()
        .and_then(|s| crate::cells::bucket_fluid(sid(s)));
    let item = configured_after_use(shown.as_ref(), held_key.as_ref(), fluid.as_deref())
        .map(|k| key_to_string(&k))
        .unwrap_or_default();
    let mut own = own;
    own.insert(java("item"), Tag::String(java(&item)));
    // The amount is worked out again at the next tick.
    set_nbt_i64(&mut own, "amount", 0);
    set_nbt_i64(&mut own, "craft", 0);
    let written = crate::bus::set_nbt_at(host, level, vpos, own);
    debug(&format!(
        "monitor: use_part {id} {vpos:?}: configured {item:?} written {written}"
    ));
    Some(InteractionResult::SuccessServer)
}

/// A punch on a cell (BlockAttackHook → ConversionMonitorPart.onClicked / onShiftClicked): a stack of the shown item
/// out of the network (Shift: one) into the inventory. True when a conversion monitor took the punch (it then isn't
/// dug).
pub(crate) fn punch(host: &mut dyn ServerHost, pos: Pos, face: Pos) -> bool {
    let Some(level) = here(&*host) else {
        return false;
    };
    let Some((player, side)) = crate::bus::punched_part(&*host, level, pos, face) else {
        return false;
    };
    let Some(c) = crate::bus::cell(&*host, level, pos) else {
        return false;
    };
    if c.parts[side].as_deref() != Some("ae2:conversion_monitor") {
        return false;
    }
    let vpos = hosted(pos, side);
    let dim = host.dimension();
    if !crate::net::device_state(dim, vpos).2 {
        return false;
    }
    let Some(own) = crate::bus::nbt_at(&*host, level, vpos) else {
        return true;
    };
    let Some(key) =
        key_from_string(&nbt_str(&own, "item")).filter(|k| k.key_type() == KeyType::Items)
    else {
        return true;
    };
    crate::cells::ids(&*host);
    let one = sneaking(&*host, player);
    let Some(sample) = crate::cells::stack_of(&*host, &key, 1) else {
        return true;
    };
    let max = i64::from(host.max_stack_size(sample.item()).unwrap_or(64).max(1));
    let want = if one { 1 } else { max };
    let got = crate::storage::extract(host, Scope::Grid(vpos), None, &key, want);
    if got > 0
        && let Some(stack) = crate::cells::stack_of(&*host, &key, got as u32)
    {
        crate::wrench::give_or_drop(host, player, stack);
    }
    true
}

// ---- Client

/// The pose with +z out of side `s` of the block, from the block's centre, its up as AE2's unspun part's.
pub(crate) fn face_pose(draw: &mut dyn BlockEntityDrawContext, s: usize) {
    let c = [0.5, 0.5, 0.5];
    match s {
        0 => draw.rotate_about([1.0, 0.0, 0.0], 90.0, c),
        1 => draw.rotate_about([1.0, 0.0, 0.0], -90.0, c),
        2 => draw.rotate_about([0.0, 1.0, 0.0], 180.0, c),
        4 => draw.rotate_about([0.0, 1.0, 0.0], -90.0, c),
        5 => draw.rotate_about([0.0, 1.0, 0.0], 90.0, c),
        _ => {}
    }
    draw.translate([0.5, 0.5, 0.5]);
}

/// The cell's monitors' screens: the item flat on the front (GUI look, 0.4 of a block, 0.05 above the middle) and
/// the amount under it (a font pixel 1/124 of a block, 0.23 below, fluix's contrast colour black), full bright; the
/// lock on a locked monitor's lights. Only while the monitor has its channel (isActive).
pub(crate) fn submit(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    nbt: &Compound,
    parts: &[Option<String>; 6],
    looks: &Compound,
) {
    for s in 0..6 {
        let Some(id) = parts[s].as_deref().filter(|id| is_monitor(id)) else {
            continue;
        };
        let Some(Tag::Compound(m)) = nbt.get(&java(&format!("m{s}"))) else {
            continue;
        };
        let look = nbt_str(looks, &format!("l{s}"));
        let active = look == "11";
        let locked = nbt_i64(m, "locked") != 0;
        draw.push();
        face_pose(draw, s);
        let light = draw.light();
        draw.set_light(0xF0_00F0);
        if locked && look.starts_with('1') {
            // The lock of `*_monitor_locked_on`'s dark lights, tinted as fluix's dark (AEColor.TRANSPARENT).
            let sprite = if id == "ae2:conversion_monitor" {
                "ae2:block/part/conversion_monitor_medium_locked"
            } else {
                "ae2:block/part/storage_monitor_dark_locked"
            };
            let tint = if id == "ae2:conversion_monitor" {
                9_002_152
            } else {
                1_778_500
            };
            let (a, b, z) = (-6.0 / 16.0, 6.0 / 16.0, 0.5 + 0.0015);
            draw.quad(
                sprite,
                [[a, b, z], [a, a, z], [b, a, z], [b, b, z]],
                [2.0, 2.0, 14.0, 14.0],
                tint,
            );
        }
        if active && let Some(key) = key_from_string(&nbt_str(m, "item")) {
            draw.push();
            draw.translate([0.0, 0.05, 0.5]);
            // The amount.
            let text = amount_text(&key, nbt_i64(m, "amount"), nbt_i64(m, "craft") != 0);
            let t = Text::literal(text);
            draw.push();
            draw.translate([0.0, -0.23, 0.01]);
            draw.scale([1.0 / 124.0, -1.0 / 124.0, 1.0 / 124.0]);
            let w = draw.text_width(&t);
            let _ = draw.text(&t, [-w / 2.0, 0.0], 0x000000, true, None);
            draw.pop();
            // The item (AEKeyRendering.drawOnBlockFace).
            draw.translate([0.0, 0.0, 0.005]);
            if key.key_type() == KeyType::Fluids {
                if let Some(fk) = host.fluid_key(key.id()) {
                    let r = 0.2;
                    let _ = draw.fluid_box(
                        &FluidStack::new(fk, 1000),
                        [-r, -r, 0.0, r, r, 0.001],
                        FluidBoxOptions::new(true, 0.0),
                    );
                }
            } else if let Some(stack) = crate::cells::stack_of(host, &key, 1) {
                draw.scale([0.4, 0.4, 0.0001]);
                draw.item(&stack, ItemDisplayContext::Gui, None, 0);
            }
            draw.pop();
        }
        draw.set_light(light);
        draw.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_texts() {
        let iron = AeKey::item("minecraft:iron_ingot", "", 0, 0);
        assert_eq!(amount_text(&iron, 0, true), "Craft");
        assert_eq!(amount_text(&iron, 0, false), "0");
        assert_eq!(amount_text(&iron, 64, true), "64");
        assert_eq!(amount_text(&iron, 12_345, false), "12K");
        let water = AeKey::fluid("minecraft:water", "");
        assert_eq!(amount_text(&water, 500, false), ".5");
    }

    #[test]
    fn configuring() {
        let bucket = AeKey::item("minecraft:water_bucket", "", 0, 0);
        let stone = AeKey::item("minecraft:stone", "", 0, 0);
        // A new item replaces; an empty hand clears.
        assert_eq!(
            configured_after_use(Some(&stone), Some(&bucket), Some("minecraft:water")),
            Some(bucket.clone())
        );
        assert_eq!(configured_after_use(Some(&stone), None, None), None);
        // The bucket on a monitor showing buckets: its water.
        assert_eq!(
            configured_after_use(Some(&bucket), Some(&bucket), Some("minecraft:water")),
            Some(AeKey::fluid("minecraft:water", ""))
        );
        assert_eq!(
            configured_after_use(Some(&stone), Some(&stone), None),
            Some(stone)
        );
    }

    #[test]
    fn conversion_uses() {
        assert_eq!(conversion_use(true, true, false, false), ConvUse::StoreAll);
        assert_eq!(
            conversion_use(true, false, false, false),
            ConvUse::StoreHeld
        );
        assert_eq!(conversion_use(true, false, false, true), ConvUse::Configure);
        assert_eq!(conversion_use(true, false, true, true), ConvUse::StoreHeld);
        assert_eq!(
            conversion_use(false, false, true, false),
            ConvUse::StoreHeld
        );
        assert_eq!(
            conversion_use(false, false, false, false),
            ConvUse::Configure
        );
        assert_eq!(
            conversion_use(false, true, false, false),
            ConvUse::Configure
        );
    }
}
