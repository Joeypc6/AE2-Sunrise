//! The ME interface's fluid stock (InterfaceLogic's generic storage for fluids): a config cell holding a fluid keeps
//! the tank under it at that many millibuckets (at most four buckets, InterfaceLogic's fluid capacity) from the network,
//! what is over it or not wanted going back; with a Crafting Card, what the network lacks is asked for (`craft_cards`).
//! The tanks are the block entity's mod tanks (`nbt["fluids"]`, a cable bus side's run of its cell's), so pipes reach
//! them through FLUIDS (`interface_fluids`, each taking only its configured fluid), and a bucket clicked on a stored
//! cell fills or empties its tank (AEBaseMenu's FILL_ITEM / EMPTY_ITEM, a menu button checked on the server).
//!
//! Upstream: `interface_tanks`, `interface_fluids`, `interface_bucket_press` (`../ae2-horizon/mixins/ae2/me.rs`).

use crate::cells::*;
use crate::core::{AeKey, AmountFormat, KeyType};
use crate::storage::{self, Scope};
use crate::util::*;
use mod_api::*;
use std::sync::Mutex;

pub(crate) const TANKS: usize = 9;
/// InterfaceLogic: fluids up to four buckets a slot.
pub(crate) const TANK_MB: i64 = 4000;
/// `TANK_BUTTON | cell`: a bucket clicked on stored cell `cell`.
pub(crate) const TANK_BUTTON: i32 = 1 << 19;
/// The interface's card slot (after its nine stored stacks).
const CARD_SLOT: usize = 9;

type Tank = (Option<String>, i64);

// ---- The tanks as saved (the mod tanks' `fluids` list: `Fluid`, `Amount`)

pub(crate) fn read_tanks(nbt: &Compound) -> Vec<Tank> {
    let mut out = vec![(None, 0); TANKS];
    if let Some(Tag::List(list)) = nbt.get(&java("fluids")) {
        for (i, t) in list.iter().take(TANKS).enumerate() {
            let Tag::Compound(c) = t else { continue };
            let fluid = match c.get(&java("Fluid")) {
                Some(Tag::String(f)) => Some(f.to_string_lossy()).filter(|f| !f.is_empty() && f != "minecraft:empty"),
                _ => None,
            };
            let amount = match c.get(&java("Amount")) {
                Some(Tag::Int(n)) => i64::from(*n),
                Some(Tag::Long(n)) => *n,
                _ => 0,
            };
            out[i] = if amount > 0 && fluid.is_some() { (fluid, amount) } else { (None, 0) };
        }
    }
    out
}

/// Writes the tanks back, keeping what else each entry holds (and any entries past the ninth).
pub(crate) fn write_tanks(nbt: &mut Compound, tanks: &[Tank]) {
    let mut list = match nbt.get(&java("fluids")) {
        Some(Tag::List(l)) => l.clone(),
        _ => Vec::new(),
    };
    while list.len() < TANKS {
        list.push(Tag::Compound(Compound::new()));
    }
    for (i, (fluid, amount)) in tanks.iter().enumerate().take(TANKS) {
        let mut c = match &list[i] {
            Tag::Compound(c) => c.clone(),
            _ => Compound::new(),
        };
        let (f, n) = match fluid {
            Some(f) if *amount > 0 => (f.as_str(), *amount),
            _ => ("minecraft:empty", 0),
        };
        c.insert(java("Fluid"), Tag::String(java(f)));
        c.insert(java("Amount"), Tag::Int(n.clamp(0, i64::from(i32::MAX)) as i32));
        list[i] = Tag::Compound(c);
    }
    nbt.insert(java("fluids"), Tag::List(list));
}

/// A config cell's fluid and amount (clamped to the tank).
fn wanted(cell: &Option<(AeKey, i64)>) -> Option<(String, i64)> {
    match cell {
        Some((k, n)) if k.key_type() == KeyType::Fluids => Some((k.id().to_owned(), (*n).clamp(0, TANK_MB))),
        _ => None,
    }
}

/// What a tank should give back to the network now: all of a fluid it isn't set to, what is over its amount.
pub(crate) fn excess(tank: &Tank, want: &Option<(String, i64)>) -> i64 {
    match (tank, want) {
        ((Some(f), n), Some((w, keep))) if f == w => (n - keep).max(0),
        ((Some(_), n), _) => *n,
        _ => 0,
    }
}

/// What a tank should take from the network now.
pub(crate) fn missing(tank: &Tank, want: &Option<(String, i64)>) -> i64 {
    match (tank, want) {
        ((None, _), Some((_, n))) => *n,
        ((Some(f), have), Some((w, n))) if f == w => (n - have).max(0),
        _ => 0,
    }
}

// ---- The server

/// InterfaceLogic.updateStorage for fluids, on the interface's fields (a cable bus side's are its own run of tanks).
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, _block: &str, fields: &mut Fields) {
    let config = crate::parts::ghosts(&fields.nbt, TANKS);
    let wants: Vec<Option<(String, i64)>> = config.iter().map(wanted).collect();
    let mut tanks = read_tanks(&fields.nbt);
    if wants.iter().all(Option::is_none) && tanks.iter().all(|t| t.0.is_none()) {
        return;
    }
    let scope = Scope::Grid(pos);
    let crafting = crate::machines::cards(&fields.items, CARD_SLOT, 1, "ae2:crafting_card") > 0;
    let before = tanks.clone();
    for i in 0..TANKS {
        let back = excess(&tanks[i], &wants[i]);
        if back > 0
            && let Some(f) = tanks[i].0.clone()
        {
            let stored = storage::insert(host, scope, None, &AeKey::fluid(&f, ""), back).clamp(0, back);
            tanks[i].1 -= stored;
            if tanks[i].1 <= 0 {
                tanks[i] = (None, 0);
            }
        }
        let need = missing(&tanks[i], &wants[i]);
        let Some((fluid, _)) = wants[i].clone() else { continue };
        if need <= 0 {
            continue;
        }
        let key = AeKey::fluid(&fluid, "");
        let got = storage::extract(host, scope, None, &key, need).clamp(0, need);
        if got > 0 {
            tanks[i] = (Some(fluid), tanks[i].1 + got);
        }
        if crafting && got < need {
            crate::craft_cards::want(host.dimension(), pos, &key, need - got, crate::craft_cards::Who::Interface);
        }
    }
    if tanks != before {
        write_tanks(&mut fields.nbt, &tanks);
    }
}

/// The interface's tanks to other mods: each takes only its configured fluid (any while it has none), up to four
/// buckets.
struct InterfaceFluids {
    inner: Box<dyn ResourceHandler<FluidResource>>,
    config: Vec<Option<String>>,
}

impl InterfaceFluids {
    fn allows(&self, host: &dyn Host, index: usize, resource: &FluidResource) -> bool {
        match self.config.get(index) {
            Some(Some(want)) => crate::parts::fluid_id(host, resource).as_deref() == Some(want.as_str()),
            _ => true,
        }
    }
}

impl ResourceHandler<FluidResource> for InterfaceFluids {
    fn size(&self, host: &mut dyn ServerHost) -> usize {
        self.inner.size(host).min(TANKS)
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<FluidResource> {
        self.inner.resource(host, index)
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        self.inner.amount(host, index)
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, resource: Option<&FluidResource>) -> u32 {
        self.inner.capacity(host, index, resource).min(TANK_MB as u32)
    }
    fn is_valid(&self, host: &mut dyn ServerHost, index: usize, resource: &FluidResource) -> bool {
        index < TANKS && self.allows(&*host, index, resource) && self.inner.is_valid(host, index, resource)
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        if index >= TANKS || !self.allows(&*host, index, resource) {
            return Ok(0);
        }
        let room = (TANK_MB as u32).saturating_sub(self.inner.amount(host, index));
        self.inner.insert_at(host, tx, index, resource, amount.min(room)).map(|n| n.min(amount))
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &TransactionContext, index: usize, resource: &FluidResource, amount: u32) -> Result<u32, TransactionError<Overanswered>> {
        if index >= TANKS {
            return Ok(0);
        }
        self.inner.extract_at(host, tx, index, resource, amount).map(|n| n.min(amount))
    }
}

/// FLUIDS for an interface block, asked before the game's (which, asked with no side, gives the tanks themselves).
pub(crate) fn interface_fluids(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<FluidResource>>> {
    at.side?;
    let inner = at.level.capability(host, &FLUIDS, at.pos, None)?;
    fluids_over(host, at.level, at.pos, Box::new(crate::bus::Slice { inner, base: 0, len: TANKS }))
}

/// An interface's tanks over `inner` (its block's, or its side's run of a cable bus's), with its config.
pub(crate) fn fluids_over(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, inner: Box<dyn ResourceHandler<FluidResource>>) -> Option<Box<dyn ResourceHandler<FluidResource>>> {
    let nbt = crate::bus::nbt_at(&*host, level, pos)?;
    let config = crate::parts::ghosts(&nbt, TANKS).iter().map(|c| wanted(c).map(|(f, _)| f)).collect();
    Some(Box::new(InterfaceFluids { inner, config }))
}

/// A bucket clicked on stored cell `i` of the interface the player has open: a filled one empties into its tank (the
/// same fluid or an empty tank, room for a bucket), an empty one fills from it (a bucket's worth there). True when the
/// button is this module's (acted on or refused).
pub(crate) fn menu_button(host: &mut dyn ServerHost, player: u128, pos: Pos, block: &str, id: i32, fields: &mut Fields, carried: &mut Option<ItemStack>) -> bool {
    if id < 0 || id & !0xF != TANK_BUTTON || crate::parts::kind(block) != Some(crate::parts::Kind::Interface) {
        return false;
    }
    let i = (id & 0xF) as usize;
    if i >= TANKS || !storage::rate_ok(player) {
        return true;
    }
    let Some((at, viewed, _)) = storage::viewer_for_button(&*host, player) else { return true };
    if at != pos || viewed != block {
        return true;
    }
    let Some(held) = carried.clone() else { return true };
    let mut tanks = read_tanks(&fields.nbt);
    let item = sid(&held);
    if let Some(fluid) = bucket_fluid(item) {
        // FILL_ITEM's reverse: the bucket into the tank.
        let fits = match &tanks[i] {
            (None, _) => true,
            (Some(f), n) => *f == fluid && n + 1000 <= TANK_MB,
        };
        let configured = wanted(&crate::parts::ghosts(&fields.nbt, TANKS)[i]).map(|(f, _)| f);
        if !fits || held.count() != 1 || configured.as_ref().is_some_and(|c| *c != fluid) {
            return true;
        }
        let Some(empty) = new_stack(&*host, "minecraft:bucket", 1) else { return true };
        tanks[i] = (Some(fluid), tanks[i].1 + 1000);
        *carried = Some(empty);
        write_tanks(&mut fields.nbt, &tanks);
    } else if item == "minecraft:bucket" {
        let (Some(fluid), n) = tanks[i].clone() else { return true };
        if n < 1000 {
            return true;
        }
        let Some(full) = new_stack(&*host, &fluid_bucket(&fluid), 1) else { return true };
        if bucket_fluid(sid(&full)).as_deref() != Some(fluid.as_str()) {
            return true;
        }
        tanks[i] = if n == 1000 { (None, 0) } else { (Some(fluid), n - 1000) };
        if held.count() == 1 {
            *carried = Some(full);
        } else {
            *carried = with_count(&held, held.count() - 1);
            crate::wrench::give_or_drop(host, player, full);
        }
        write_tanks(&mut fields.nbt, &tanks);
    }
    true
}

/// The tank lines of an interface's screen stream (`parts::stream`): `t <cell> <mB> <fluid>`.
pub(crate) fn stream_lines(host: &dyn Host, level: LevelRef, pos: Pos) -> String {
    let tanks = if is_hosted(pos) {
        let (cell, side) = host_cell(pos);
        let Some(nbt) = host.block_entity(level, cell).and_then(|b| b.data(host)) else { return String::new() };
        let mut slice = Compound::new();
        if let Some(Tag::List(t)) = nbt.get(&java("fluids")) {
            slice.insert(java("fluids"), Tag::List(t.iter().skip(side * crate::bus::TANKS).take(TANKS).cloned().collect()));
        }
        read_tanks(&slice)
    } else {
        read_tanks(&crate::bus::nbt_at(host, level, pos).unwrap_or_default())
    };
    let mut out = String::new();
    for (i, (f, n)) in tanks.iter().enumerate() {
        if let Some(f) = f {
            out.push_str(&format!("t {i} {n} {f}\n"));
        }
    }
    out
}

// ---- The client

static CLIENT: Mutex<Vec<Option<(String, i64)>>> = Mutex::new(Vec::new());

/// An `ae2:part` message: its tank lines replace what the screen shows.
pub(crate) fn client_message(text: &str) {
    let mut tanks = vec![None; TANKS];
    for l in text.lines() {
        let Some(rest) = l.strip_prefix("t ") else { continue };
        let mut w = rest.splitn(3, ' ');
        if let (Some(Ok(i)), Some(Ok(n)), Some(f)) = (w.next().map(str::parse::<usize>), w.next().map(str::parse::<i64>), w.next())
            && i < TANKS
        {
            tanks[i] = Some((f.to_owned(), n));
        }
    }
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = tanks;
}

pub(crate) fn client_clear() {
    CLIENT.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

pub(crate) fn client_tank(i: usize) -> Option<(String, i64)> {
    CLIENT.lock().unwrap_or_else(|e| e.into_inner()).get(i).cloned().flatten()
}

/// The stored cell under `at` (screen pixels from its corner).
pub(crate) fn tank_under(at: [f32; 2]) -> Option<usize> {
    (0..TANKS).find(|i| {
        let x = 8.0 + 18.0 * *i as f32;
        at[0] >= x && at[0] < x + 16.0 && at[1] >= 71.0 && at[1] < 87.0
    })
}

/// The tanks in the stored row, with their amounts.
pub(crate) fn draw(_host: &dyn ClientHost, d: &mut dyn DrawContext, o: [f32; 2]) {
    for i in 0..TANKS {
        let Some((f, n)) = client_tank(i) else { continue };
        let at = [o[0] + 8.0 + 18.0 * i as f32, o[1] + 71.0];
        d.fluid(&f, at, 16.0);
        let label = AeKey::fluid(&f, "").format_amount(n, AmountFormat::SlotLargeFont);
        let w = d.text_width(&label);
        d.text(&label, [at[0] + 17.0 - w, at[1] + 9.0], 0xFFFF_FFFF, true);
    }
}

pub(crate) fn tooltip(host: &dyn ClientHost, at: [f32; 2], lines: &mut Vec<String>) -> bool {
    let Some((f, n)) = tank_under(at).and_then(client_tank) else { return false };
    let key = AeKey::fluid(&f, "");
    lines.push(crate::screens::key_name(host, &key));
    lines.push(format!("§7{}", key.format_amount(n, AmountFormat::Full)));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water(n: i64) -> Tank {
        (Some("minecraft:water".to_owned()), n)
    }

    #[test]
    fn tanks_keep_their_config() {
        let want = Some(("minecraft:water".to_owned(), 2000));
        assert_eq!(missing(&(None, 0), &want), 2000);
        assert_eq!(missing(&water(500), &want), 1500);
        assert_eq!(excess(&water(500), &want), 0);
        assert_eq!(excess(&water(3000), &want), 1000);
        // Another fluid goes back whole and nothing comes in until it has.
        let lava = (Some("minecraft:lava".to_owned()), 700);
        assert_eq!(excess(&lava, &want), 700);
        assert_eq!(missing(&lava, &want), 0);
        // No config: all of it back.
        assert_eq!(excess(&water(300), &None), 300);
        assert_eq!(missing(&(None, 0), &None), 0);
    }

    #[test]
    fn config_is_clamped_to_four_buckets() {
        let k = AeKey::fluid("minecraft:water", "");
        assert_eq!(wanted(&Some((k.clone(), 64_000))), Some(("minecraft:water".to_owned(), TANK_MB)));
        assert_eq!(wanted(&Some((AeKey::item("minecraft:stone", "", 0, 0), 5))), None);
    }

    #[test]
    fn saved_tanks_round_trip() {
        let mut nbt = Compound::new();
        let mut tanks = vec![(None, 0); TANKS];
        tanks[2] = water(1234);
        write_tanks(&mut nbt, &tanks);
        assert_eq!(read_tanks(&nbt), tanks);
        tanks[2] = (None, 0);
        write_tanks(&mut nbt, &tanks);
        assert!(read_tanks(&nbt).iter().all(|t| t.0.is_none()));
    }
}
