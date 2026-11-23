//! AE2's Crafting Card on machines (`MultiCraftingTracker`, `ICraftingRequester`): the export bus crafts what it should
//! export and the network lacks (or, in "craft only" mode, only what it crafted), the interface crafts what its stock
//! config is missing, and the level emitter's two crafting modes ("emit to craft": the network may plan with it as a
//! source of the item, "emit while crafting": on while a CPU waits for the item).
//!
//! Upstream: `run_buses`, `interface_tick`, `emitable_keys`, `request_craft` (`../ae2-horizon/mixins/ae2/me.rs`).
//!
//! A machine's request is queued from its tick and worked out at the level's tick end (`level_tick`), a few a tick,
//! through `autocraft::request_for_machine` (CRAFT_LESS, an automation source's CPU choice). One request at a time for
//! a key on a grid: none is queued while one for the same machine and key waits or runs, or while any CPU of the grid
//! already crafts that key; a request that couldn't start waits `RETRY_TICKS` before the machine asks again. The job's
//! final output goes into the network (as a player's), where the machine takes it from: the export bus in "craft only"
//! mode exports no more than its own jobs made (`credit`), which is kept in memory only.

use crate::cells::*;
use crate::core::{AeKey, KeyType};
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::Mutex;

/// Ticks a machine waits after a request that couldn't start (missing ingredients, no free CPU).
const RETRY_TICKS: u64 = 40;
/// The most requests worked out in one level tick (each plans on the server thread).
const PER_TICK: usize = 2;

/// Who asked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Who {
    ExportBus,
    Interface,
}

#[derive(Clone, Debug)]
struct Job {
    dim: &'static str,
    pos: Pos,
    key: AeKey,
    amount: i64,
    who: Who,
}

type At = (&'static str, Pos, AeKey);

#[derive(Default)]
pub(crate) struct Requests {
    /// Waiting to be worked out at the tick's end: where, what, how many, who.
    queue: Vec<(&'static str, Pos, AeKey, i64, Who)>,
    /// Running jobs a machine asked for, by craft id.
    jobs: HashMap<u128, Job>,
    /// What each export bus's finished jobs made that it hasn't exported yet ("craft only").
    credits: HashMap<At, i64>,
    /// The tick a machine may ask for a key again.
    retry: HashMap<At, u64>,
    ticks: HashMap<&'static str, u64>,
    /// Level emitters with a crafting card set to "emit to craft", and their configured key.
    emitters: HashMap<(&'static str, Pos), AeKey>,
    /// Changes whenever `emitters` does (terminals list their craftables again).
    emit_gen: u64,
}

impl Requests {
    fn now(&self, dim: &'static str) -> u64 {
        self.ticks.get(dim).copied().unwrap_or(0)
    }

    /// Whether the machine at `pos` may ask for `key` now: nothing queued or running for it, not waiting to retry.
    fn may_ask(&self, dim: &'static str, pos: Pos, key: &AeKey) -> bool {
        let queued = self.queue.iter().any(|(d, p, k, _, _)| *d == dim && *p == pos && k == key);
        let running = self.jobs.values().any(|j| j.dim == dim && j.pos == pos && j.key == *key);
        let waiting = self.retry.get(&(dim, pos, key.clone())).is_some_and(|t| *t > self.now(dim));
        !queued && !running && !waiting
    }

    fn want(&mut self, dim: &'static str, pos: Pos, key: &AeKey, amount: i64, who: Who) -> bool {
        if amount <= 0 || !self.may_ask(dim, pos, key) || self.queue.len() >= 4096 {
            return false;
        }
        self.queue.push((dim, pos, key.clone(), amount, who));
        true
    }

    fn ended(&mut self, id: u128, done: bool) {
        let Some(j) = self.jobs.remove(&id) else { return };
        if done && j.who == Who::ExportBus {
            *self.credits.entry((j.dim, j.pos, j.key)).or_insert(0) += j.amount;
        }
    }

    fn credit(&self, dim: &'static str, pos: Pos, key: &AeKey) -> i64 {
        self.credits.get(&(dim, pos, key.clone())).copied().unwrap_or(0)
    }

    fn spend(&mut self, dim: &'static str, pos: Pos, key: &AeKey, n: i64) {
        let at = (dim, pos, key.clone());
        if let Some(c) = self.credits.get_mut(&at) {
            *c -= n.min(*c);
            if *c <= 0 {
                self.credits.remove(&at);
            }
        }
    }
}

static STATE: Mutex<Option<Requests>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut Requests) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Requests::default))
}

pub(crate) fn forget_all() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// A machine's request for `amount` of `key`, worked out at the tick's end (if it may ask now).
pub(crate) fn want(dim: &'static str, pos: Pos, key: &AeKey, amount: i64, who: Who) -> bool {
    with_state(|s| s.want(dim, pos, key, amount, who))
}

/// A job ended (`autocraft::run_cpus`): finished (`done`) or cancelled.
pub(crate) fn job_ended(id: u128, done: bool) {
    with_state(|s| s.ended(id, done));
}

/// What an export bus's own finished jobs made that it may still export in "craft only" mode.
pub(crate) fn credit(dim: &'static str, pos: Pos, key: &AeKey) -> i64 {
    with_state(|s| s.credit(dim, pos, key))
}

pub(crate) fn spend(dim: &'static str, pos: Pos, key: &AeKey, n: i64) {
    with_state(|s| s.spend(dim, pos, key, n));
}

/// An export bus not in "craft only" mode keeps no credit (it exports from the network anyway).
pub(crate) fn forget_credit(dim: &'static str, pos: Pos) {
    with_state(|s| {
        if s.credits.keys().any(|(d, p, _)| *d == dim && *p == pos) {
            s.credits.retain(|(d, p, _), _| !(*d == dim && *p == pos));
        }
    });
}

/// How much of `key` the face `face` of `target` takes now (an insert in a transaction that is undone).
fn room(host: &mut dyn ServerHost, level: LevelRef, target: Pos, face: Direction, key: &AeKey) -> i64 {
    fn fits<R>(host: &mut dyn ServerHost, h: &Capability<dyn ResourceHandler<R>>, res: &R, amount: u32) -> u32 {
        host.transaction(|host, tx| Err::<(), u32>(h.insert(host, tx, res, amount).unwrap_or(0))).err().unwrap_or(0)
    }
    if key.key_type() == KeyType::Fluids {
        let (Some(h), Some(res)) = (level.capability(host, &FLUIDS, target, Some(face)), crate::parts::fluid_resource(&*host, key)) else { return 0 };
        i64::from(fits(host, &h, &res, 1000))
    } else {
        let (Some(h), Some(res)) = (level.capability(host, &ITEMS, target, Some(face)), crate::parts::item_resource(&*host, key)) else { return 0 };
        i64::from(fits(host, &h, &res, 1))
    }
}

/// ExportBusPart with a Crafting Card: when nothing of `key` went out this run, it asks for `amount` to be crafted,
/// if what it faces has room for it and (unless "craft only") the network has none.
#[allow(clippy::too_many_arguments)]
pub(crate) fn export_bus(host: &mut dyn ServerHost, level: LevelRef, pos: Pos, target: Pos, face: Direction, key: &AeKey, amount: i64, craft_only: bool) {
    let dim = host.dimension();
    if amount <= 0 || !with_state(|s| s.may_ask(dim, pos, key)) {
        return;
    }
    if room(host, level, target, face, key) <= 0 {
        return;
    }
    if !craft_only && crate::storage::simulate(host, crate::storage::Scope::Grid(pos), None, key, 1, false) > 0 {
        return;
    }
    want(dim, pos, key, amount, Who::ExportBus);
}

/// StorageLevelEmitterPart with a Crafting Card: noted at its tick, with its key when set to "emit to craft".
pub(crate) fn note_emitter(dim: &'static str, pos: Pos, emits: Option<AeKey>) {
    with_state(|s| {
        let changed = match &emits {
            Some(k) => s.emitters.insert((dim, pos), k.clone()).as_ref() != Some(k),
            None => s.emitters.remove(&(dim, pos)).is_some(),
        };
        if changed {
            s.emit_gen = s.emit_gen.wrapping_add(1);
        }
    });
}

/// StorageLevelEmitterPart.getEmitableItems over a grid: the keys its "emit to craft" emitters make.
pub(crate) fn emitables(dim: &'static str, grid: usize) -> Vec<AeKey> {
    let list: Vec<(Pos, AeKey)> = with_state(|s| s.emitters.iter().filter(|((d, _), _)| *d == dim).map(|((_, p), k)| (*p, k.clone())).collect());
    let mut out: Vec<AeKey> = Vec::new();
    for (pos, key) in list {
        let (g, online, _) = crate::net::device_state(dim, pos);
        let still = crate::net::with_net(dim, |n| n.devices.get(&pos).is_some_and(|(b, _)| b == "ae2:level_emitter"));
        if !still {
            note_emitter(dim, pos, None);
            continue;
        }
        if online && g == Some(grid) && !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

pub(crate) fn emitables_gen() -> u64 {
    with_state(|s| s.emit_gen)
}

/// The level's tick end: requests worked out (a few a tick), jobs no CPU runs any more forgotten.
pub(crate) fn level_tick(host: &mut dyn ServerHost, _level: LevelRef) {
    let dim = host.dimension();
    let now = with_state(|s| {
        let t = s.ticks.entry(dim).or_insert(0);
        *t += 1;
        *t
    });
    if now % 100 == 0 {
        let running = crate::autocraft::running_jobs();
        with_state(|s| {
            s.jobs.retain(|id, _| running.contains(id));
            s.retry.retain(|(d, _, _), t| *d != dim || *t > now);
        });
    }
    let todo: Vec<(&'static str, Pos, AeKey, i64, Who)> = with_state(|s| {
        let mut mine = Vec::new();
        let mut i = 0;
        while i < s.queue.len() && mine.len() < PER_TICK {
            if s.queue[i].0 == dim {
                mine.push(s.queue.remove(i));
            } else {
                i += 1;
            }
        }
        mine
    });
    for (dim, pos, key, amount, who) in todo {
        match crate::autocraft::request_for_machine(host, pos, &key, amount) {
            Ok((id, made)) => with_state(|s| {
                s.jobs.insert(id, Job { dim, pos, key, amount: made, who });
            }),
            Err(e) => {
                debug(&format!("machine request at {dim} {} for {}: {e}", pos_text(pos), key_to_string(&key))); // fixF
                with_state(|s| s.retry.insert((dim, pos, key), now + RETRY_TICKS));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(id: &str) -> AeKey {
        AeKey::item(id, "", 0, 0)
    }

    #[test]
    fn one_request_at_a_time_per_machine_and_key() {
        let mut s = Requests::default();
        let (a, b) = ([0, 64, 0], [1, 64, 0]);
        assert!(s.want("o", a, &k("minecraft:stick"), 8, Who::ExportBus));
        // Queued already: not again, not for zero, another key or machine may.
        assert!(!s.want("o", a, &k("minecraft:stick"), 8, Who::ExportBus));
        assert!(!s.want("o", a, &k("minecraft:torch"), 0, Who::ExportBus));
        assert!(s.want("o", a, &k("minecraft:torch"), 1, Who::ExportBus));
        assert!(s.want("o", b, &k("minecraft:stick"), 1, Who::Interface));
        // Running: still not.
        let (d, p, key, _, who) = s.queue.remove(0);
        s.jobs.insert(7, Job { dim: d, pos: p, key, amount: 4, who });
        assert!(!s.want("o", a, &k("minecraft:stick"), 8, Who::ExportBus));
        // Finished: the export bus may export what it made, and ask again.
        s.ended(7, true);
        assert_eq!(s.credit("o", a, &k("minecraft:stick")), 4);
        assert!(s.want("o", a, &k("minecraft:stick"), 8, Who::ExportBus));
    }

    #[test]
    fn credit_is_spent_exactly() {
        let mut s = Requests::default();
        let (a, key) = ([0, 64, 0], k("minecraft:stick"));
        s.jobs.insert(1, Job { dim: "o", pos: a, key: key.clone(), amount: 10, who: Who::ExportBus });
        s.jobs.insert(2, Job { dim: "o", pos: a, key: key.clone(), amount: 5, who: Who::Interface });
        s.ended(1, true);
        // An interface's job gives no credit; a cancelled one neither.
        s.ended(2, true);
        assert_eq!(s.credit("o", a, &key), 10);
        s.spend("o", a, &key, 3);
        assert_eq!(s.credit("o", a, &key), 7);
        s.spend("o", a, &key, 100);
        assert_eq!(s.credit("o", a, &key), 0);
        s.jobs.insert(3, Job { dim: "o", pos: a, key: key.clone(), amount: 9, who: Who::ExportBus });
        s.ended(3, false);
        assert_eq!(s.credit("o", a, &key), 0);
    }

    #[test]
    fn a_failed_request_waits() {
        let mut s = Requests::default();
        let (a, key) = ([0, 64, 0], k("minecraft:stick"));
        s.retry.insert(("o", a, key.clone()), 40);
        s.ticks.insert("o", 39);
        assert!(!s.may_ask("o", a, &key));
        s.ticks.insert("o", 40);
        assert!(s.may_ask("o", a, &key));
    }
}
