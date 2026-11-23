//! fixA: a storage bus's view of the container in front of it (StorageBusPart over StorageAdapter), kept as a cache
//! so big inventories (other mods' chests and drawers with hundreds or thousands of slots) cost little:
//! - the cache holds each slot's resource, key, amount and capacity, an index of the slots holding each key with
//!   its total, and the empty slots;
//! - the bus's tick looks at the container a slice at a time (`SCAN_BUDGET` slots a tick, a rolling cursor), and
//!   after a whole sweep waits StorageBus's tick rate (5 ticks, 5 more after each sweep that found nothing new, up
//!   to 60; back to 5 after a change: TickRates.StorageBus with TickRateModulation's backoff);
//! - the terminal's list and every simulation read the cache (no handler calls); a real insert or extract goes to
//!   the slots the cache names, live, in one transaction for the whole move (`Slots` is that transaction's view), so
//!   a stale cache can only move less (the slot is read again), never duplicate or lose anything; a slot that
//!   refused a key isn't asked for that key again until it changes;
//! - a reading that differs from the cache says the container changed outside the network (terminals list again);
//! - fixG: each slot's extractability is probed once when what it holds changes (StorageAdapter's simulated extract
//!   for STORAGE_FILTER = EXTRACTABLE_ONLY, made in an aborted transaction), so a listing that hides what can't be
//!   extracted reads the cache alone, and a simulated extract skips those slots as the real one would find them.

use crate::core::{AeKey, KeyCounter};
use std::collections::{BTreeSet, HashMap};

/// Slots looked at a tick while a sweep runs.
pub(crate) const SCAN_BUDGET: usize = 128;
/// TickRates.StorageBus (5, 60).
pub(crate) const RATE_MIN: u32 = 5;
pub(crate) const RATE_MAX: u32 = 60;
const RATE_STEP: u32 = 5;

/// One handler as a move (or a sweep) sees it: the calls the cache makes. In game, one transaction's view of a
/// `ResourceHandler` (`parts::LiveSlots`); in tests a counting fake.
pub(crate) trait Slots<R> {
    fn size(&mut self) -> usize;
    fn amount(&mut self, index: usize) -> i64;
    fn resource(&mut self, index: usize) -> Option<R>;
    fn capacity(&mut self, index: usize, resource: &R) -> i64;
    /// An empty slot's capacity, when the kind has one to ask (a tank: the probe fluid's); 0 = the key's stack limit.
    fn empty_capacity(&mut self, index: usize) -> i64;
    fn insert(&mut self, index: usize, resource: &R, amount: i64) -> i64;
    fn extract(&mut self, index: usize, resource: &R, amount: i64) -> i64;
    /// fixG: whether some of `amount` of `resource` could be extracted from slot `index` (StorageAdapter's probe:
    /// 1, then the whole amount, simulated; nothing moves).
    fn can_extract(&mut self, _index: usize, _resource: &R, _amount: i64) -> bool {
        true
    }
    /// The key of a resource (no handler call).
    fn key_of(&mut self, resource: &R) -> AeKey;
    /// The resource of a key (no handler call); None when the game doesn't know it.
    fn resource_of(&mut self, key: &AeKey) -> Option<R>;
}

#[derive(Clone)]
struct Slot<R> {
    res: Option<R>,
    key: Option<AeKey>,
    amount: i64,
    /// Its capacity for what it holds (0: not known, the key's stack limit).
    cap: i64,
    /// A key it refused while empty (its hash), skipped for that key until the slot changes.
    refused: u64,
    /// fixG: what it holds can be extracted (probed when the key changed).
    ext: bool,
}

impl<R> Default for Slot<R> {
    fn default() -> Self {
        Self {
            res: None,
            key: None,
            amount: 0,
            cap: 0,
            refused: 0,
            ext: true,
        }
    }
}

#[derive(Default)]
struct Held {
    total: i64,
    /// fixG: the part of `total` in slots it can be extracted from.
    ext_total: i64,
    slots: BTreeSet<u32>,
}

/// A container's contents as last seen.
pub(crate) struct SlotCache<R> {
    slots: Vec<Slot<R>>,
    held: HashMap<AeKey, Held>,
    empties: BTreeSet<u32>,
    cursor: usize,
    /// Ticks to wait before the next sweep, and the rate it goes at.
    wait: u32,
    rate: u32,
    sweep_changed: bool,
    /// Changed outside the network since the last `take_changed`.
    changed: bool,
    /// Whether a whole sweep has been made (before that the list is partial).
    pub(crate) complete: bool,
}

impl<R> Default for SlotCache<R> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            held: HashMap::new(),
            empties: BTreeSet::new(),
            cursor: 0,
            wait: 0,
            rate: RATE_MIN,
            sweep_changed: false,
            changed: false,
            complete: false,
        }
    }
}

fn key_hash(key: &AeKey) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish() | 1
}

impl<R: Clone + PartialEq> SlotCache<R> {
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(crate) fn rate(&self) -> u32 {
        self.rate
    }

    /// What it held changed outside the network since asked last.
    pub(crate) fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Slot `i` holds `res` (`key`) `amount`, `cap` the most it takes of it; `ext` whether it can be extracted.
    fn set(&mut self, i: usize, res: Option<R>, key: Option<AeKey>, amount: i64, cap: i64, ext: bool) {
        let s = &mut self.slots[i];
        if let Some(k) = s.key.take() {
            if let Some(h) = self.held.get_mut(&k) {
                h.total -= s.amount;
                if s.ext {
                    h.ext_total -= s.amount; // fixG
                }
                h.slots.remove(&(i as u32));
                if h.slots.is_empty() {
                    self.held.remove(&k);
                }
            }
        }
        self.empties.remove(&(i as u32));
        let s = &mut self.slots[i];
        s.refused = 0;
        match key.filter(|_| amount > 0) {
            Some(k) => {
                let h = self.held.entry(k.clone()).or_default();
                h.total += amount;
                if ext {
                    h.ext_total += amount; // fixG
                }
                h.slots.insert(i as u32);
                *s = Slot {
                    res,
                    key: Some(k),
                    amount,
                    cap: cap.max(amount),
                    refused: 0,
                    ext,
                };
            }
            None => {
                *s = Slot {
                    res: None,
                    key: None,
                    amount: 0,
                    cap,
                    refused: 0,
                    ext: true,
                };
                self.empties.insert(i as u32);
            }
        }
    }

    /// Only the amount of a slot holding the same key changed.
    fn set_amount(&mut self, i: usize, amount: i64) {
        let s = &mut self.slots[i];
        let Some(k) = &s.key else { return };
        if amount <= 0 {
            self.set(i, None, None, 0, 0, true);
            return;
        }
        if let Some(h) = self.held.get_mut(k) {
            h.total += amount - s.amount;
            if s.ext {
                h.ext_total += amount - s.amount; // fixG
            }
        }
        s.amount = amount;
        s.cap = s.cap.max(amount);
    }

    fn resize(&mut self, n: usize) {
        while self.slots.len() > n {
            let i = self.slots.len() - 1;
            self.set(i, None, None, 0, 0, true);
            self.empties.remove(&(i as u32));
            self.slots.pop();
            self.changed = true;
        }
        while self.slots.len() < n {
            self.slots.push(Slot::default());
            self.empties.insert(self.slots.len() as u32 - 1);
        }
        self.cursor = self.cursor.min(n);
    }

    /// Reads slot `i` again (2 calls when it holds something, 1 when empty; capacity only when its key changed):
    /// whether it differed.
    fn read(&mut self, src: &mut dyn Slots<R>, i: usize) -> bool {
        // A reading that differs is a change from outside (terminals list again).
        let differed = self.read_slot(src, i);
        self.changed |= differed;
        differed
    }

    fn read_slot(&mut self, src: &mut dyn Slots<R>, i: usize) -> bool {
        let amount = src.amount(i);
        if amount <= 0 {
            if self.slots[i].key.is_none() {
                return false;
            }
            let cap = src.empty_capacity(i);
            self.set(i, None, None, 0, cap, true);
            return true;
        }
        let res = src.resource(i);
        if res.is_some() && res == self.slots[i].res {
            if amount == self.slots[i].amount {
                return false;
            }
            self.set_amount(i, amount);
            return true;
        }
        match res {
            Some(r) => {
                let key = src.key_of(&r);
                let cap = src.capacity(i, &r);
                let ext = src.can_extract(i, &r, amount); // fixG
                self.set(i, Some(r), Some(key), amount, cap, ext);
            }
            None => self.set(i, None, None, 0, 0, true),
        }
        true
    }

    /// One sweep step: up to `budget` slots from the cursor. Whether anything differed, and whether the sweep ended.
    pub(crate) fn scan(&mut self, src: &mut dyn Slots<R>, budget: usize) -> (bool, bool) {
        let mut changed = false;
        if self.cursor == 0 {
            let n = src.size();
            if n != self.slots.len() {
                changed = true;
                self.resize(n);
            }
        }
        let end = (self.cursor + budget).min(self.slots.len());
        for i in self.cursor..end {
            changed |= self.read(src, i);
        }
        self.cursor = end;
        let done = self.cursor >= self.slots.len();
        if done {
            self.cursor = 0;
            self.complete = true;
        }
        self.changed |= changed;
        (changed, done)
    }

    /// The bus's tick: a sweep step when one is due (TickRates.StorageBus pace between sweeps). Whether it changed.
    pub(crate) fn tick(&mut self, src: &mut dyn Slots<R>) -> bool {
        if self.wait > 0 {
            self.wait -= 1;
            return false;
        }
        let (changed, done) = self.scan(src, SCAN_BUDGET);
        self.sweep_changed |= changed;
        if done {
            self.rate = if self.sweep_changed {
                RATE_MIN
            } else {
                (self.rate + RATE_STEP).min(RATE_MAX)
            };
            self.wait = self.rate;
            self.sweep_changed = false;
        }
        changed
    }

    /// Looks again soon (the target or the bus changed).
    pub(crate) fn hurry(&mut self) {
        self.wait = 0;
        self.rate = RATE_MIN;
    }

    /// What it holds, by key (the terminal's list; no handler calls); fixG: with `extractable_only`, only what the
    /// slots' probes found extractable (StorageAdapter with extractableOnly).
    pub(crate) fn available(&self, out: &mut KeyCounter, extractable_only: bool, allow: impl Fn(&AeKey) -> bool) {
        for (k, h) in &self.held {
            let n = if extractable_only { h.ext_total } else { h.total };
            if n > 0 && allow(k) {
                out.add(k, n);
            }
        }
    }

    pub(crate) fn holds(&self, key: &AeKey) -> bool {
        self.held.get(key).is_some_and(|h| h.total > 0)
    }

    pub(crate) fn total(&self, key: &AeKey) -> i64 {
        self.held.get(key).map_or(0, |h| h.total)
    }

    /// Up to `amount` of `key` in: onto the slots holding it first, then empty ones (ItemHandlerUtil's order), none
    /// above `most` (the key's stack limit) or the slot's capacity. `src` None simulates from the cache; with a
    /// source each slot is filled live and the cache follows what it took.
    pub(crate) fn insert(
        &mut self,
        mut src: Option<&mut dyn Slots<R>>,
        key: &AeKey,
        amount: i64,
        most: i64,
    ) -> i64 {
        if amount <= 0 {
            return 0;
        }
        let h = key_hash(key);
        let mut left = amount;
        let holding: Vec<u32> = self
            .held
            .get(key)
            .map(|h| h.slots.iter().copied().collect())
            .unwrap_or_default();
        let mut res: Option<R> = holding
            .first()
            .and_then(|i| self.slots[*i as usize].res.clone());
        for i in holding {
            if left <= 0 {
                break;
            }
            let s = &self.slots[i as usize];
            let limit = if s.cap > 0 { s.cap.min(most) } else { most };
            let room = (limit - s.amount).max(0).min(left);
            if room <= 0 {
                continue;
            }
            match src.as_deref_mut() {
                None => left -= room,
                Some(src) => {
                    let Some(r) = s.res.clone() else { continue };
                    let got = src.insert(i as usize, &r, room).clamp(0, room);
                    if got > 0 {
                        let now = self.slots[i as usize].amount + got;
                        self.set_amount(i as usize, now);
                    }
                    if got < room {
                        self.read(src, i as usize);
                    }
                    left -= got;
                }
            }
        }
        if left > 0 {
            let empties: Vec<u32> = self
                .empties
                .iter()
                .copied()
                .filter(|i| self.slots[*i as usize].refused != h)
                .collect();
            for i in empties {
                if left <= 0 {
                    break;
                }
                let cap = self.slots[i as usize].cap;
                let limit = if cap > 0 { cap.min(most) } else { most };
                let room = limit.min(left);
                if room <= 0 {
                    continue;
                }
                match src.as_deref_mut() {
                    None => left -= room,
                    Some(src) => {
                        if res.is_none() {
                            res = src.resource_of(key);
                        }
                        let Some(r) = res.clone() else {
                            return amount - left;
                        };
                        let got = src.insert(i as usize, &r, room).clamp(0, room);
                        if got > 0 {
                            let cap = src.capacity(i as usize, &r);
                            let ext = src.can_extract(i as usize, &r, got); // fixG
                            self.set(i as usize, Some(r), Some(key.clone()), got, cap, ext);
                        } else {
                            // Refused (a slot for something else) or no longer empty: read it once, and if still
                            // empty, not asked for this key again until it changes.
                            if !self.read(src, i as usize) && self.slots[i as usize].key.is_none() {
                                self.slots[i as usize].refused = h;
                            }
                        }
                        left -= got;
                    }
                }
            }
        }
        amount - left
    }

    /// Up to `amount` of `key` out of the slots holding it, first slot first. `src` None simulates.
    pub(crate) fn extract(
        &mut self,
        mut src: Option<&mut dyn Slots<R>>,
        key: &AeKey,
        amount: i64,
    ) -> i64 {
        if amount <= 0 {
            return 0;
        }
        let holding: Vec<u32> = self
            .held
            .get(key)
            .map(|h| h.slots.iter().copied().collect())
            .unwrap_or_default();
        let mut left = amount;
        for i in holding {
            if left <= 0 {
                break;
            }
            let s = &self.slots[i as usize];
            let take = s.amount.min(left);
            if take <= 0 {
                continue;
            }
            match src.as_deref_mut() {
                // fixG: a simulation counts only the slots its probe could take from.
                None if !s.ext => {}
                None => left -= take,
                Some(src) => {
                    let Some(r) = s.res.clone() else { continue };
                    let got = src.extract(i as usize, &r, take).clamp(0, take);
                    if got > 0 {
                        let now = self.slots[i as usize].amount - got;
                        self.set_amount(i as usize, now);
                    }
                    if got < take {
                        self.read(src, i as usize);
                    }
                    left -= got;
                }
            }
        }
        amount - left
    }

    /// Checks the index against the slots (tests).
    #[cfg(test)]
    fn consistent(&self) -> bool {
        let mut totals: HashMap<AeKey, i64> = HashMap::new();
        let mut ext: HashMap<AeKey, i64> = HashMap::new();
        for (i, s) in self.slots.iter().enumerate() {
            match &s.key {
                Some(k) => {
                    *totals.entry(k.clone()).or_default() += s.amount;
                    *ext.entry(k.clone()).or_default() += if s.ext { s.amount } else { 0 };
                    if !self
                        .held
                        .get(k)
                        .is_some_and(|h| h.slots.contains(&(i as u32)))
                    {
                        return false;
                    }
                }
                None => {
                    if !self.empties.contains(&(i as u32)) {
                        return false;
                    }
                }
            }
        }
        totals.len() == self.held.len()
            && totals
                .iter()
                .all(|(k, t)| self.held.get(k).is_some_and(|h| h.total == *t && h.ext_total == ext[k]))
    }
}

// ---- The host for live moves during a storage call

thread_local! {
    /// The host of the storage call running on this thread (`with_host`), for the storage buses' live moves.
    static HOST: std::cell::Cell<Option<*mut (dyn mod_api::ServerHost + 'static)>> = const { std::cell::Cell::new(None) };
}

/// Runs `f` (a storage call's work on its mounted storage) with `host` reachable through `host_do`. `f` can't hold
/// `host` itself (it is borrowed here), so the pointer is the one way to it while `f` runs.
pub(crate) fn with_host<R>(host: &mut dyn mod_api::ServerHost, f: impl FnOnce() -> R) -> R {
    let p: *mut (dyn mod_api::ServerHost + '_) = host;
    // SAFETY: the pointer is only used while `f` runs (restored on return, and on unwind by the guard), while `host`
    // is mutably borrowed by this frame and nothing else uses it; `host_do` takes it out while in use, so it is
    // never aliased.
    let p: *mut (dyn mod_api::ServerHost + 'static) = unsafe { std::mem::transmute(p) };
    struct Restore(Option<*mut (dyn mod_api::ServerHost + 'static)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HOST.with(|h| h.set(self.0));
        }
    }
    let _restore = Restore(HOST.with(|h| h.replace(Some(p))));
    f()
}

/// Whether a storage call's work is running (`with_host`).
pub(crate) fn in_call() -> bool {
    HOST.with(|h| h.get().is_some())
}

/// Runs `f` with the running storage call's host; None outside one (or while it is already in use).
pub(crate) fn host_do<R>(f: impl FnOnce(&mut dyn mod_api::ServerHost) -> R) -> Option<R> {
    let p = HOST.with(|h| h.take())?;
    struct Back(*mut (dyn mod_api::ServerHost + 'static));
    impl Drop for Back {
        fn drop(&mut self) {
            HOST.with(|h| h.set(Some(self.0)));
        }
    }
    let _back = Back(p);
    // SAFETY: see `with_host`; taken out of the cell, so no other use while `f` runs.
    Some(f(unsafe { &mut *p }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chest of `n` slots, 64 a slot (16 for ender pearls), counting every handler call.
    struct Fake {
        slots: Vec<Option<(AeKey, i64)>>,
        calls: usize,
    }

    fn limit(k: &AeKey) -> i64 {
        if k.id() == "minecraft:ender_pearl" {
            16
        } else {
            64
        }
    }

    impl Fake {
        fn new(n: usize) -> Self {
            Self {
                slots: vec![None; n],
                calls: 0,
            }
        }
        fn total(&self, k: &AeKey) -> i64 {
            self.slots
                .iter()
                .flatten()
                .filter(|(s, _)| s == k)
                .map(|(_, n)| n)
                .sum()
        }
    }

    impl Slots<AeKey> for Fake {
        fn size(&mut self) -> usize {
            self.calls += 1;
            self.slots.len()
        }
        fn amount(&mut self, i: usize) -> i64 {
            self.calls += 1;
            self.slots[i].as_ref().map_or(0, |s| s.1)
        }
        fn resource(&mut self, i: usize) -> Option<AeKey> {
            self.calls += 1;
            self.slots[i].as_ref().map(|s| s.0.clone())
        }
        fn capacity(&mut self, _i: usize, r: &AeKey) -> i64 {
            self.calls += 1;
            limit(r)
        }
        fn empty_capacity(&mut self, _i: usize) -> i64 {
            0
        }
        fn insert(&mut self, i: usize, r: &AeKey, n: i64) -> i64 {
            self.calls += 1;
            let room = match &self.slots[i] {
                None => limit(r),
                Some((k, have)) if k == r => limit(r) - have,
                _ => 0,
            };
            let put = room.min(n).max(0);
            if put > 0 {
                let have = self.slots[i].as_ref().map_or(0, |s| s.1);
                self.slots[i] = Some((r.clone(), have + put));
            }
            put
        }
        fn extract(&mut self, i: usize, r: &AeKey, n: i64) -> i64 {
            self.calls += 1;
            let Some((k, have)) = self.slots[i].clone() else {
                return 0;
            };
            if &k != r {
                return 0;
            }
            let got = have.min(n);
            self.slots[i] = (have - got > 0).then(|| (k, have - got));
            got
        }
        fn key_of(&mut self, r: &AeKey) -> AeKey {
            r.clone()
        }
        fn resource_of(&mut self, key: &AeKey) -> Option<AeKey> {
            Some(key.clone())
        }
    }

    fn item(n: usize) -> AeKey {
        AeKey::item(&format!("minecraft:item_{}", n % 37), "", 0, 0)
    }

    fn filled(n: usize) -> Fake {
        let mut f = Fake::new(n);
        for i in 0..n {
            if i % 5 != 0 {
                f.slots[i] = Some((item(i), (i % 64) as i64 + 1));
            }
        }
        f
    }

    /// A tick's work stays bounded however big the container, and the cache lists it exactly once a sweep is done.
    fn bounded_and_exact(n: usize) -> (usize, usize, usize) {
        let mut f = filled(n);
        let mut c: SlotCache<AeKey> = SlotCache::default();
        let mut ticks = 0;
        let mut most = 0;
        while !c.complete {
            let before = f.calls;
            c.tick(&mut f);
            most = most.max(f.calls - before);
            ticks += 1;
        }
        // A slot read the first time costs three calls (amount, resource, capacity); after that one or two.
        assert!(most <= 3 * SCAN_BUDGET + 1, "{most} calls in one tick");
        assert!(c.consistent());
        let mut out = KeyCounter::new();
        c.available(&mut out, false, |_| true);
        for k in 0..37 {
            assert_eq!(out.get(&item(k)), f.total(&item(k)), "{k}");
        }
        // Idle: after the sweep it waits its rate, then sweeps again finding nothing, slower each time.
        let before = f.calls;
        for _ in 0..600 {
            assert!(!c.tick(&mut f));
        }
        let idle_per_tick = (f.calls - before) / 600;
        assert_eq!(c.rate(), RATE_MAX);
        (most, ticks, idle_per_tick)
    }

    #[test]
    fn big_chests_cost_a_bounded_amount_a_tick() {
        let (most, ticks, idle) = bounded_and_exact(512);
        eprintln!(
            "512 slots: at most {most} handler calls a tick, a sweep in {ticks} ticks, idle {idle} calls/tick"
        );
        assert_eq!(ticks, 4);
        let (most, ticks, idle) = bounded_and_exact(2048);
        eprintln!(
            "2048 slots: at most {most} handler calls a tick, a sweep in {ticks} ticks, idle {idle} calls/tick"
        );
        assert_eq!(ticks, 16);
    }

    #[test]
    fn moves_are_exact_and_keep_stack_limits() {
        for n in [512usize, 2048] {
            let mut f = filled(n);
            let mut c: SlotCache<AeKey> = SlotCache::default();
            while !c.complete {
                c.tick(&mut f);
            }
            let k = item(3);
            let had = f.total(&k);
            // A simulated insert asks the handler nothing; the real one matches it.
            let calls = f.calls;
            let sim = c.insert(None, &k, 3_000, 64);
            assert_eq!(f.calls, calls);
            let went = c.insert(Some(&mut f), &k, 3_000, 64);
            assert_eq!(went, sim);
            assert_eq!(f.total(&k), had + went);
            assert_eq!(c.total(&k), f.total(&k));
            // The calls are those of the slots it filled, not the container's size.
            assert!(
                f.calls - calls <= 2 * (went as usize / 64 + 1) + 37 * 2,
                "{}",
                f.calls - calls
            );
            assert!(f.slots.iter().flatten().all(|(key, a)| *a <= limit(key)));
            // Ender pearls stack to 16 even when told 64 (the slot's capacity rules).
            let pearl = AeKey::item("minecraft:ender_pearl", "", 0, 0);
            let p = c.insert(Some(&mut f), &pearl, 40, 64);
            assert_eq!(p, 40);
            assert!(
                f.slots
                    .iter()
                    .flatten()
                    .filter(|(key, _)| *key == pearl)
                    .all(|(_, a)| *a <= 16)
            );
            // Out again: exactly what is there, no more.
            let all = f.total(&k);
            let got = c.extract(Some(&mut f), &k, all + 500);
            assert_eq!(got, all);
            assert_eq!(f.total(&k), 0);
            assert!(!c.holds(&k));
            assert!(c.consistent());
        }
    }

    #[test]
    fn a_stale_cache_never_duplicates() {
        let mut f = filled(512);
        let mut c: SlotCache<AeKey> = SlotCache::default();
        while !c.complete {
            c.tick(&mut f);
        }
        let k = item(1);
        let cached = c.total(&k);
        // Someone empties the chest of it behind the network's back.
        for s in f.slots.iter_mut() {
            if s.as_ref().is_some_and(|(key, _)| *key == k) {
                *s = None;
            }
        }
        // The cache still lists it, but taking it gets nothing, and the cache learns.
        assert!(cached > 0);
        assert_eq!(c.extract(Some(&mut f), &k, cached), 0);
        assert_eq!(c.total(&k), 0);
        // Filled behind its back: an insert into a slot now full only moves what fits.
        let k2 = item(2);
        for s in f.slots.iter_mut().filter(|s| s.is_none()) {
            *s = Some((AeKey::item("minecraft:dirt", "", 0, 0), 64));
        }
        let had = f.total(&k2);
        let went = c.insert(Some(&mut f), &k2, 100_000, 64);
        assert_eq!(f.total(&k2), had + went);
        assert!(f.slots.iter().flatten().all(|(key, a)| *a <= limit(key)));
        assert!(c.consistent());
        // The next sweep notices the dirt.
        // (The insert above already read the slots it found full: changed from outside either way.)
        c.hurry();
        for _ in 0..8 {
            c.tick(&mut f);
        }
        assert!(c.take_changed());
        assert_eq!(
            c.total(&AeKey::item("minecraft:dirt", "", 0, 0)),
            f.total(&AeKey::item("minecraft:dirt", "", 0, 0))
        );
    }

    #[test]
    fn refusing_slots_are_not_asked_again() {
        // A furnace-like handler: slot 0 takes only coal, the others nothing.
        struct Picky(Fake);
        impl Slots<AeKey> for Picky {
            fn size(&mut self) -> usize {
                self.0.size()
            }
            fn amount(&mut self, i: usize) -> i64 {
                self.0.amount(i)
            }
            fn resource(&mut self, i: usize) -> Option<AeKey> {
                self.0.resource(i)
            }
            fn capacity(&mut self, i: usize, r: &AeKey) -> i64 {
                self.0.capacity(i, r)
            }
            fn empty_capacity(&mut self, _i: usize) -> i64 {
                0
            }
            fn insert(&mut self, i: usize, r: &AeKey, n: i64) -> i64 {
                if i == 0 && r.id() == "minecraft:coal" {
                    self.0.insert(i, r, n)
                } else {
                    self.0.calls += 1;
                    0
                }
            }
            fn extract(&mut self, i: usize, r: &AeKey, n: i64) -> i64 {
                self.0.extract(i, r, n)
            }
            fn key_of(&mut self, r: &AeKey) -> AeKey {
                r.clone()
            }
            fn resource_of(&mut self, k: &AeKey) -> Option<AeKey> {
                Some(k.clone())
            }
        }
        let mut f = Picky(Fake::new(300));
        let mut c: SlotCache<AeKey> = SlotCache::default();
        while !c.complete {
            c.tick(&mut f);
        }
        let stone = AeKey::item("minecraft:stone", "", 0, 0);
        assert_eq!(c.insert(Some(&mut f), &stone, 64, 64), 0);
        let calls = f.0.calls;
        assert_eq!(c.insert(Some(&mut f), &stone, 64, 64), 0);
        assert_eq!(f.0.calls, calls, "refusing slots asked again");
        assert_eq!(
            c.insert(
                Some(&mut f),
                &AeKey::item("minecraft:coal", "", 0, 0),
                100,
                64
            ),
            64
        );
    }

    /// fixG: STORAGE_FILTER = EXTRACTABLE_ONLY: a furnace-like container whose input slot (0) can't be extracted
    /// from lists only its output (2); a simulated extract counts only that, and the probe is made once per change.
    #[test]
    fn extractable_only_hides_what_cannot_be_extracted() {
        struct Furnace(Fake, usize);
        impl Slots<AeKey> for Furnace {
            fn size(&mut self) -> usize {
                self.0.size()
            }
            fn amount(&mut self, i: usize) -> i64 {
                self.0.amount(i)
            }
            fn resource(&mut self, i: usize) -> Option<AeKey> {
                self.0.resource(i)
            }
            fn capacity(&mut self, i: usize, r: &AeKey) -> i64 {
                self.0.capacity(i, r)
            }
            fn empty_capacity(&mut self, _i: usize) -> i64 {
                0
            }
            fn insert(&mut self, i: usize, r: &AeKey, n: i64) -> i64 {
                self.0.insert(i, r, n)
            }
            fn extract(&mut self, i: usize, r: &AeKey, n: i64) -> i64 {
                if i == 0 { 0 } else { self.0.extract(i, r, n) }
            }
            fn can_extract(&mut self, i: usize, r: &AeKey, n: i64) -> bool {
                self.1 += 1;
                // The probe: a simulated extract (here, on a copy), nothing moves.
                let before = self.0.slots.clone();
                let ok = self.extract(i, r, 1) > 0 || self.extract(i, r, n) > 0;
                self.0.slots = before;
                ok
            }
            fn key_of(&mut self, r: &AeKey) -> AeKey {
                r.clone()
            }
            fn resource_of(&mut self, k: &AeKey) -> Option<AeKey> {
                Some(k.clone())
            }
        }
        let ore = AeKey::item("minecraft:iron_ore", "", 0, 0);
        let ingot = AeKey::item("minecraft:iron_ingot", "", 0, 0);
        let mut f = Furnace(Fake::new(3), 0);
        f.0.slots[0] = Some((ore.clone(), 10));
        f.0.slots[2] = Some((ingot.clone(), 5));
        let mut c: SlotCache<AeKey> = SlotCache::default();
        while !c.complete {
            c.tick(&mut f);
        }
        let probes = f.1;
        assert_eq!(probes, 2);
        let list = |c: &SlotCache<AeKey>, only: bool| {
            let mut out = KeyCounter::new();
            c.available(&mut out, only, |_| true);
            (out.get(&ore), out.get(&ingot))
        };
        assert_eq!(list(&c, false), (10, 5), "report inaccessible items: everything");
        assert_eq!(list(&c, true), (0, 5), "extractable only: the output alone");
        assert_eq!(c.extract(None, &ore, 10), 0, "a simulation finds the input locked");
        assert_eq!(c.extract(None, &ingot, 10), 5);
        assert_eq!(f.0.slots, [Some((ore.clone(), 10)), None, Some((ingot.clone(), 5))], "probes moved nothing");
        // Sweeps that find nothing new don't probe again; an amount change keeps the slot's probe.
        for _ in 0..200 {
            c.tick(&mut f);
        }
        assert_eq!(f.1, probes);
        f.0.slots[0] = Some((ore.clone(), 3));
        c.hurry();
        for _ in 0..4 {
            c.tick(&mut f);
        }
        assert_eq!(f.1, probes);
        assert_eq!(list(&c, true), (0, 5));
        assert_eq!(list(&c, false), (3, 5));
        // A live extract takes from the output.
        assert_eq!(c.extract(Some(&mut f), &ingot, 2), 2);
        assert_eq!(list(&c, true), (0, 3));
        assert!(c.consistent());
    }
}
