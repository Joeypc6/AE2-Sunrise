//! A crafting job's time left (this port's addition, as on the Nightly port; AE2 shows only the time so far): the job
//! played forward by `CraftingCpu::forecast` with each pattern's machines' pace, at the server's real tick rate
//! (`tick_stats`: the rate, or 1000 / milliseconds a tick when it falls behind). Worked out at most once a second and
//! only for a CPU whose screen someone has open; counted down between.
//!
//! A machine's pace: a molecular assembler's craft time at its speed cards; for a processing pattern's machine (the
//! first one beside the provider, on the side the provider pushes into), in this order:
//! 1. what the machine says through the shared `machine_work:machine_work` block capability (the Machine Work library,
//!    an optional dependency): `pace` for the first of the pattern's item inputs it takes gives the ticks an operation
//!    and the operations at once (`craft_time`). Vanilla furnaces, smokers, blast furnaces and brewing stands answer it
//!    through the library, Iron Furnaces' furnaces themselves;
//! 2. what another mod says of its machine (`crate::api::register_machine`, kept for mods without the library);
//! 3. a furnace's, blast furnace's or smoker's the server's cooking recipe for the pattern's first input
//!    (`Recipe::cooking_time`); a brewing stand's 400 ticks;
//! 4. any other machine's what its results' arrivals have measured (the arrivals of a key the CPU waits for, smoothed).
//!    A measured pace also overrides 1 to 3 when it is more than 10% slower (short of fuel or power).
//!
//! How far a machine is with its current operation: the capability's `progress` (the ticks it has left); a registered
//! one's, what its mod says; a vanilla one's, its saved cooking or brewing time (`block_entity_data`). The capability is
//! asked outside any transaction, once a forecast for each machine and pattern (`WORK`).
//!
//! On top of the forecast (the Horizon port's `job_seconds_left`): a job that has stood past twice its slowest
//! machine's time is late by as long as it stands (the time climbs while it is held up), and results that come back
//! through an import bus wait for the bus's own schedule (TickRates.ImportBus: 5 to 40 ticks, 2 faster for each run
//! that took something, 1 slower for each that didn't), followed to the end of the job.
use super::*;

/// A brewing stand's brew (`BrewingStandBlockEntity`'s brewTime starts at 400).
const BREW_TICKS: f64 = 400.0;
/// A machine nothing says the pace of, before anything of it has been measured.
const UNKNOWN_TICKS: f64 = 40.0;
/// Arrivals further apart than this are a pause (no power, a job ended), not the machine's pace: measuring starts over.
const PAUSE_TICKS: u64 = 1200;
/// TickRates.ImportBus: an import bus's interval, fastest and slowest.
const BUS_FASTEST: f64 = 5.0;
const BUS_SLOWEST: f64 = 40.0;

/// What a key's arrivals measured: the last one's tick, and the smoothed ticks between and items each brought.
#[derive(Clone, Copy)]
pub(crate) struct Pace {
    last: u64,
    ticks: f64,
    items: f64,
}

impl Pace {
    /// Ticks an item, once at least two arrivals were seen.
    fn per_item(&self) -> Option<f64> {
        (self.items > 0.0 && self.ticks > 0.0).then(|| self.ticks / self.items)
    }
}

/// `n` of `key` reached a CPU waiting for it at `tick`.
pub(super) fn arrived(s: &mut State, dim: &'static str, key: &AeKey, n: i64, tick: u64) {
    if n <= 0 {
        return;
    }
    let Some(p) = s.paces.get_mut(&(dim, key.clone())) else {
        s.paces.insert((dim, key.clone()), Pace { last: tick, ticks: 0.0, items: 0.0 });
        return;
    };
    let dt = tick.saturating_sub(p.last);
    p.last = tick;
    if dt > PAUSE_TICKS {
        (p.ticks, p.items) = (0.0, 0.0);
        return;
    }
    p.ticks = p.ticks * 0.8 + dt as f64;
    p.items = p.items * 0.8 + n as f64;
}

/// The recipe type a vanilla cooking block works.
fn cook_kind(block: &str) -> Option<&'static str> {
    match block {
        "minecraft:furnace" => Some("minecraft:smelting"),
        "minecraft:blast_furnace" => Some("minecraft:blasting"),
        "minecraft:smoker" => Some("minecraft:smoking"),
        _ => None,
    }
}

/// A vanilla machine's ticks a craft, for what the pattern puts in it.
fn known_ticks(host: &dyn Host, block: &str, pattern: &Pattern) -> Option<f64> {
    if block == "minecraft:brewing_stand" {
        return Some(BREW_TICKS);
    }
    let kind = cook_kind(block)?;
    let input = pattern.inputs.first()?.possible.first()?.what.id().to_owned();
    let recipes = host.recipes_of_type(kind);
    let time = recipes.iter().find(|r| r.ingredients.first().is_some_and(|i| i.items.contains(&input)))?.cooking_time?;
    Some(f64::from(time.max(1)))
}

/// What a registered mod says of its machine at `q` for this pattern (`api::register_machine`).
fn registered(host: &dyn Host, level: LevelRef, q: Pos, block: &str, pattern: &Pattern) -> Option<crate::api::MachineTime> {
    let inputs: Vec<(&str, i64)> = pattern.inputs.iter().filter_map(|i| i.possible.first().map(|p| (p.what.id(), p.amount * i.multiplier.max(1)))).collect();
    crate::api::machine_time(host, level, q, block, &inputs)
}

/// What a machine said through `machine_work::MACHINE_WORK`'s `pace`: ticks one operation takes, operations at once.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(not(feature = "machine_work"), allow(dead_code))]
struct WorkSaid {
    ticks: u32,
    batch: u32,
}

/// A craft of a pattern on a machine that said `said` for the pattern's input it takes, `n` of which go in a craft,
/// with `out` of the primary output coming out: (ticks a craft on average, ticks from a craft going in to its last
/// result out). A craft runs `ceil(n / batch)` operations in a row. A batch's slots go to items, not to crafts (a
/// brewing stand's batch is its bottles, one brew a craft), so crafts overlap in the machine only `batch / max(n, out)`
/// at once; a craft bigger than a batch keeps every slot busy, `n / batch` operations a craft. None for a time of 0.
fn craft_time(said: WorkSaid, n: i64, out: i64) -> Option<(f64, f64)> {
    if said.ticks == 0 {
        return None;
    }
    let ticks = f64::from(said.ticks);
    let batch = i64::from(said.batch.max(1));
    let n = n.max(1);
    let latency = ticks * ((n + batch - 1) / batch) as f64;
    let average = if n > batch { ticks * n as f64 / batch as f64 } else { latency / (batch / n.max(out.max(1))).max(1) as f64 };
    Some((average, latency))
}

/// The machines' `MACHINE_WORK` answers in one forecast (`seconds_left` starts it over): `pace` by machine and pattern,
/// `progress` by machine (None inside: the machine has no `MACHINE_WORK`; `Some(None)`: it has, and is idle), and the
/// side each machine was asked from.
#[derive(Default)]
struct WorkCache {
    paces: HashMap<(Pos, AeKey), Option<(WorkSaid, i64)>>,
    progress: HashMap<Pos, Option<Option<u32>>>,
    sides: HashMap<Pos, Direction>,
}

static WORK: Mutex<Option<WorkCache>> = Mutex::new(None);

fn with_work<R>(f: impl FnOnce(&mut WorkCache) -> R) -> R {
    let mut g = WORK.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(WorkCache::default))
}

/// What the machine at `q` (asked from side `f`) says of a craft of `pattern` through `machine_work::MACHINE_WORK`:
/// (ticks a craft on average, latency), see `craft_time`. None: the library isn't installed, the machine doesn't
/// answer, or it takes none of the pattern's item inputs.
fn work_time(host: &mut dyn ServerHost, level: LevelRef, q: Pos, f: Direction, pattern: &Pattern) -> Option<(f64, f64)> {
    let key = (q, pattern.definition.clone());
    let cached = with_work(|w| {
        w.sides.insert(q, f);
        w.paces.get(&key).copied()
    });
    let said = match cached {
        Some(said) => said,
        None => {
            #[cfg_attr(not(feature = "machine_work"), allow(unused_mut))]
            let mut said = None;
            mod_api::if_installed!(host, machine_work, {
                said = ask_pace(host, level, q, f, pattern);
            });
            let _ = level;
            with_work(|w| w.paces.insert(key, said));
            said
        }
    };
    said.and_then(|(w, n)| craft_time(w, n, pattern.primary_output().amount))
}

/// `MACHINE_WORK`'s `pace` for the first of the pattern's item inputs the machine takes, and how many of it go in a
/// craft.
#[cfg(feature = "machine_work")]
fn ask_pace(host: &mut dyn ServerHost, level: LevelRef, q: Pos, f: Direction, pattern: &Pattern) -> Option<(WorkSaid, i64)> {
    let work = level.capability(host, &machine_work::MACHINE_WORK, q, Some(f))?;
    for input in &pattern.inputs {
        let Some(p) = input.possible.first() else {
            continue;
        };
        if p.what.key_type() != KeyType::Items {
            continue;
        }
        let Some(res) = crate::parts::item_resource(&*host, &p.what) else {
            continue;
        };
        if let Some(pace) = work.pace(host, &res)
            && pace.ticks > 0
        {
            return Some((WorkSaid { ticks: pace.ticks, batch: pace.batch.max(1) }, p.amount.max(1) * input.multiplier.max(1)));
        }
    }
    None
}

/// The ticks the machine at `q` has left on its operation under way, through `machine_work::MACHINE_WORK`'s
/// `progress`: None when it has no such capability, `Some(None)` when it has and is idle.
fn work_left(host: &mut dyn ServerHost, level: LevelRef, q: Pos) -> Option<Option<u32>> {
    let (cached, side) = with_work(|w| (w.progress.get(&q).copied(), w.sides.get(&q).copied()));
    if let Some(left) = cached {
        return left;
    }
    #[cfg_attr(not(feature = "machine_work"), allow(unused_mut))]
    let mut left = None;
    mod_api::if_installed!(host, machine_work, {
        left = level.capability(host, &machine_work::MACHINE_WORK, q, side).map(|work| work.progress(host).map(|p| p.remaining()));
    });
    let _ = (level, side);
    with_work(|w| w.progress.insert(q, left));
    left
}

/// Ticks done on a craft whose machine has `left` ticks to go on its operation, against the `latency` a craft takes.
fn done_of_left(left: Option<u32>, latency: f64) -> f64 {
    let latency = latency.max(0.0);
    left.map_or(0.0, |left| (latency - f64::from(left)).clamp(0.0, latency))
}

/// A vanilla machine's progress on its current craft in ticks: f64::MAX when a result waits in its output slot (as good
/// as made). None for any other block.
fn known_progress(host: &dyn Host, level: LevelRef, q: Pos, block: &str) -> Option<f64> {
    let nbt = host.block_entity(level, q)?.data(host)?;
    if cook_kind(block).is_some() {
        let output = match nbt.get(&java("Items")) {
            Some(Tag::List(items)) => items.iter().any(|t| matches!(t, Tag::Compound(c) if nbt_i64(c, "Slot") == 2)),
            _ => false,
        };
        return Some(if output { f64::MAX } else { nbt_i64(&nbt, "cooking_time_spent") as f64 });
    }
    if block == "minecraft:brewing_stand" {
        let left = nbt_i64(&nbt, "BrewTime") as f64;
        return Some(if left > 0.0 { BREW_TICKS - left } else { 0.0 });
    }
    None
}

/// How many of `key` (an item) wait in the machine at `q`, for anything to take out.
fn results_in(host: &mut dyn ServerHost, level: LevelRef, q: Pos, key: &AeKey) -> i64 {
    if key.key_type() != KeyType::Items {
        return 0;
    }
    let Some(h) = level.capability(host, &ITEMS, q, None) else {
        return 0;
    };
    let mut n = 0i64;
    for i in 0..h.size(host).min(64) {
        let amount = h.amount(host, i);
        if amount > 0
            && let Some(res) = h.resource(host, i)
            && key_of(&*host, res.as_stack()) == *key
        {
            n += i64::from(amount);
        }
    }
    n
}

/// What a pattern's machines do now: its crafts a tick, the ticks a craft takes, which machines they are (a key of
/// their sorted positions: patterns pushed into the same machines share them) and their positions.
#[derive(Clone, Debug, PartialEq)]
struct Machines {
    rate: f64,
    latency: f64,
    group: u64,
    /// fixD: with their levels (a grid over several levels has machines in each).
    at: Vec<Loc>,
}

/// The pace the results' arrivals measured (ticks a craft over all the pattern's machines together) put against what
/// was said: the measured one when nothing said it, or when it is well behind what was said (short of fuel or power).
/// (crafts a tick, ticks a craft's results take), from what was said: `rate`, `latency` of `count` machines.
fn with_measured(rate: f64, latency: f64, count: usize, unknown: bool, measured: Option<f64>) -> (f64, f64) {
    match measured {
        Some(m) if m > 0.0 && (unknown || 1.0 / m < rate / 1.1) => (1.0 / m, latency.max(m * count.max(1) as f64)),
        _ => (rate, latency),
    }
}

/// A pattern's machines (see `Machines`), over the providers that hold it, each once: copies in one provider push to
/// the same machines, and a machine two providers push to is one machine. None: nothing can make it.
///
/// fixD: the providers in every level of the grid, each read in its own level; one whose level or chunk isn't loaded
/// counts for nothing (it gets nothing pushed meanwhile).
fn pattern_machines(host: &mut dyn ServerHost, patterns: &PatternList, paces: &HashMap<AeKey, Pace>, pattern: &Pattern) -> Option<Machines> {
    let mut providers: Vec<Loc> = patterns.iter().filter(|(_, p)| **p == *pattern).map(|(at, _)| *at).collect();
    providers.sort_unstable();
    providers.dedup();
    let out = pattern.primary_output();
    let per = out.amount.max(1) as f64;
    let (mut rate, mut latency, mut count, mut unknown) = (0.0f64, 0.0f64, 0usize, false);
    let mut used: Vec<Loc> = Vec::new();
    for (d, at) in providers {
        let Some(level) = loaded_level(&*host, d, at) else {
            continue;
        };
        let block = crate::net::block_id(&*host, level, at);
        if !is_processing(pattern) {
            for (q, _) in targets(&*host, level, at, &block) {
                if !used.contains(&(d, q)) && crate::net::block_id(&*host, level, q) == "ae2:molecular_assembler" {
                    let ticks = ASM_TICKS[speed_cards(&*host, level, q).min(5)] as f64;
                    rate += 1.0 / ticks;
                    latency = latency.max(ticks);
                    used.push((d, q));
                }
            }
            continue;
        }
        // `push_pattern` feeds the first machine beside the provider that takes the inputs.
        let Some((q, f)) = machines(host, level, at, &block).into_iter().next() else {
            continue;
        };
        if used.contains(&(d, q)) {
            continue;
        }
        let mb = crate::net::block_id(&*host, level, q);
        let (ticks, lat) = if let Some(t) = work_time(host, level, q, f, pattern) {
            t
        } else if let Some(m) = registered(&*host, level, q, &mb, pattern) {
            (m.ticks, m.latency)
        } else if let Some(t) = known_ticks(&*host, &mb, pattern) {
            (t, t)
        } else {
            unknown = true;
            (UNKNOWN_TICKS, UNKNOWN_TICKS)
        };
        rate += 1.0 / ticks;
        latency = latency.max(lat);
        count += 1;
        used.push((d, q));
    }
    if is_processing(pattern) {
        let measured = paces.get(&out.what).and_then(Pace::per_item).map(|t| t * per);
        (rate, latency) = with_measured(rate, latency, count, unknown, measured);
    }
    used.sort_unstable();
    let group = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        used.hash(&mut h);
        h.finish()
    };
    (rate > 0.0).then_some(Machines { rate, latency, group, at: used })
}

/// How far the machines making `key` are with their current crafts: (items a tick, ticks done on the next), and the
/// ticks a craft of theirs takes. A registered machine's results still in it (waiting for a bus to take them) are as
/// good as made, a craft's time each. `waiting`: how many the CPU waits for.
fn owed_pace(
    host: &mut dyn ServerHost,
    patterns: &PatternList,
    paces: &HashMap<AeKey, Pace>,
    tick: u64,
    key: &AeKey,
    waiting: i64,
) -> ((f64, f64), f64) {
    for (_, p) in patterns.iter() {
        let Some(out) = p.outputs.iter().find(|o| o.what == *key) else {
            continue;
        };
        let Some(m) = pattern_machines(host, patterns, paces, p) else {
            continue;
        };
        let each = out.amount.max(1) as f64;
        let mut since = 0.0f64;
        if is_processing(p) {
            for &(d, q) in &m.at {
                // fixD: each machine in its own level.
                let Some(level) = loaded_level(&*host, d, q) else {
                    continue;
                };
                let mb = crate::net::block_id(&*host, level, q);
                let done = if let Some(left) = work_left(host, level, q) {
                    let back = results_in(host, level, q, key).min(waiting).max(0) as f64;
                    let next = done_of_left(left, m.latency);
                    Some(if back > 0.0 { back / each / m.rate + next } else { next })
                } else if let Some(said) = registered(&*host, level, q, &mb, p) {
                    let back = results_in(host, level, q, key).min(waiting).max(0) as f64;
                    let next = said.done.unwrap_or(0.0).min(m.latency);
                    Some(if back > 0.0 { back / each / m.rate + next } else { next })
                } else {
                    known_progress(&*host, level, q, &mb)
                }
                .or_else(|| paces.get(key).map(|pace| tick.saturating_sub(pace.last) as f64));
                since = since.max(done.unwrap_or(0.0).min(m.latency));
            }
        }
        return ((m.rate * each, since), m.latency);
    }
    ((1.0, 0.0), 0.0)
}

/// Ticks a job is late: how long it has stood without progress past its grace (twice its slowest machine's time, and
/// at least `STALL_TICKS`).
fn late_ticks(stood: u64, grace: u64) -> f64 {
    (stood as f64 - grace.max(STALL_TICKS) as f64).max(0.0)
}

/// An import bus taking a machine's results, as its last run left it.
#[derive(Clone, Copy, Debug)]
struct Puller {
    /// Its interval now, and the ticks to its next run (0 or below: due).
    interval: u64,
    due_in: i64,
    /// Its runs in a row that took something: the run of them now, and the last whole run of them.
    run: u32,
    last_run: u32,
    /// Crafts still to come back from the machine it takes from, and the ticks between them there.
    crafts: f64,
    gap: f64,
}

/// The ticks from the forecast's end (`end`, ticks from now) to the bus's first run after it (the Horizon port's
/// BUS_RATES / BUS_BURSTS tail). Its schedule is followed from now: it slows by a tick each run that finds nothing, to
/// 40, and speeds up by 2 each run that takes something, to 5. The machine's results come out a craft apart up to the
/// end, and each keeps the bus at work for as many runs as the last of them did (a machine may make more than one stack
/// a craft).
fn bus_wait(end: f64, b: &Puller) -> f64 {
    let burst = b.last_run.clamp(1, 64);
    let crafts = b.crafts.max(1.0);
    let (mut at, mut interval) = (b.due_in.max(0) as f64, (b.interval as f64).clamp(BUS_FASTEST, BUS_SLOWEST));
    // Runs still to come for a result it has started on, then the results to come before the last.
    let mut queue = if b.run > 0 { burst.saturating_sub(b.run) } else { 0 };
    let mut next = 0.0;
    for _ in 0..20_000 {
        if at >= end {
            return at - end;
        }
        while next < crafts - 1.0 && end - (crafts - 1.0 - next) * b.gap <= at {
            queue += burst;
            next += 1.0;
        }
        if queue > 0 {
            queue -= 1;
            interval = (interval - 2.0).max(BUS_FASTEST);
        } else {
            interval = (interval + 1.0).min(BUS_SLOWEST);
        }
        at += interval;
    }
    // Too far ahead to follow: half an interval, on average.
    interval / 2.0
}

/// The job's time left on the CPU at `key` in seconds, -1 while nothing can make it go on (or no job).
///
/// fixD: the CPU at `key` in `dim` (`tick`: that level's), its grid's providers and machines in every level it reaches.
pub(super) fn seconds_left(host: &mut dyn ServerHost, dim: &'static str, key: Pos, tick: u64) -> f64 {
    let cached = with_state(|s| s.forecasts.get(&(dim, key)).copied());
    if let Some((at, ticks, rate)) = cached
        && tick.saturating_sub(at) < 20
    {
        return ticks.map_or(-1.0, |t| (t - tick.saturating_sub(at) as f64).max(0.0) / rate);
    }
    let parts = joint_grids(dim, key);
    if parts.is_empty() {
        with_state(|s| s.forecasts.remove(&(dim, key)));
        return -1.0;
    }
    // The machines' `MACHINE_WORK` answers are asked afresh for each forecast.
    with_work(|w| *w = WorkCache::default());
    let patterns = joint_patterns(&parts);
    // What the forecast asks of the machines, worked out first: the CPU stays in the state meanwhile.
    let Some((tasks, waiting, paces, last)) = with_state(|s| {
        let c = s.cpus.get(&(dim, key))?;
        c.cpu.job()?;
        let paces: HashMap<AeKey, Pace> = s.paces.iter().filter(|((d, _), _)| *d == dim).map(|((_, k), p)| (k.clone(), *p)).collect();
        let waiting: Vec<(AeKey, i64)> = c.cpu.waiting_keys().into_iter().map(|k| (k.clone(), c.cpu.waiting_for(&k))).collect();
        let last = s.job_watch.get(&(dim, key)).map_or(tick, |w| w.1);
        Some((c.cpu.task_patterns(), waiting, paces, last))
    }) else {
        with_state(|s| s.forecasts.remove(&(dim, key)));
        return -1.0;
    };
    // The job's patterns: what is still to start, and what it waits to get back.
    let mut working: Vec<Arc<Pattern>> = Vec::new();
    for p in tasks
        .iter()
        .cloned()
        .chain(patterns.iter().filter(|(_, p)| p.outputs.iter().any(|o| waiting.iter().any(|(k, _)| *k == o.what))).map(|(_, p)| p.clone()))
    {
        if !working.iter().any(|w| **w == *p) {
            working.push(p);
        }
    }
    let machines: Vec<(Arc<Pattern>, Option<Machines>)> = working
        .into_iter()
        .map(|p| {
            let m = pattern_machines(host, &patterns, &paces, &p);
            (p, m)
        })
        .collect();
    let mut slowest = machines.iter().filter_map(|(_, m)| m.as_ref().map(|m| m.latency)).fold(0.0f64, f64::max);
    let owed: Vec<(AeKey, (f64, f64))> = waiting
        .iter()
        .map(|(k, n)| {
            let (m, latency) = owed_pace(host, &patterns, &paces, tick, k, *n);
            slowest = slowest.max(latency);
            (k.clone(), m)
        })
        .collect();
    // A job stands that long between results of its slowest machine without being held up (`job_stall`).
    let grace = (slowest * 2.0).min(1e9) as u64;
    with_state(|s| s.graces.insert((dim, key), grace));
    let late = late_ticks(tick.saturating_sub(last), grace);
    // Each machine with the crafts still to come back from it and the ticks between them there (its pattern's pace
    // shared out over its machines), for the import buses that take their results.
    let mut outs: Vec<(Loc, f64, f64)> = Vec::new();
    with_state(|s| {
        let Some(c) = s.cpus.get(&(dim, key)) else {
            return;
        };
        for (p, m) in &machines {
            let Some(m) = m.as_ref().filter(|_| is_processing(p)) else {
                continue;
            };
            let out = p.primary_output();
            let items = c.cpu.waiting_for(&out.what) + c.cpu.pending_outputs(&out.what);
            let crafts = (items as f64 / out.amount.max(1) as f64).ceil();
            let share = m.at.len().max(1) as f64;
            outs.extend(m.at.iter().map(|q| (*q, (crafts / share).ceil(), share / m.rate)));
        }
    });
    // fixD: the grid's import buses in every level it reaches, on the machines of their own level.
    let mut levels: Vec<&'static str> = parts.iter().map(|p| p.0).collect();
    levels.sort_unstable();
    levels.dedup();
    let pullers: Vec<Puller> = levels
        .into_iter()
        .flat_map(|d| crate::parts::import_buses(d).into_iter().map(move |b| (d, b)))
        .filter_map(|(d, (bus, target, interval, due_in, run, last_run))| {
            let &(_, crafts, gap) = outs.iter().find(|o| o.0 == (d, target) && o.1 > 0.0)?;
            on_parts(&parts, d, bus).then_some(Puller { interval, due_in, run, last_run, crafts, gap })
        })
        .collect();
    let ticks = with_state(|s| {
        let c = s.cpus.get(&(dim, key))?;
        c.cpu.forecast(&mut |p| machines.iter().find(|(q, _)| **q == *p).and_then(|(_, m)| m.as_ref().map(|m| (m.rate, m.latency, m.group))), &mut |k| {
            owed.iter().find(|(q, _)| q == k).map_or((1.0, 0.0), |(_, m)| *m)
        })
    })
    .map(|t| {
        let t = t + late;
        // The first of the buses to take the last results.
        if pullers.is_empty() { t } else { t + pullers.iter().map(|b| bus_wait(t, b)).fold(f64::MAX, f64::min) }
    });
    let (rate, mspt) = host.tick_stats();
    let rate = f64::from(rate).min(if mspt > 0.0 { 1000.0 / mspt } else { f64::MAX }).max(0.01);
    with_state(|s| s.forecasts.insert((dim, key), (tick, ticks, rate)));
    ticks.map_or(-1.0, |t| t / rate)
}

/// `/ae2debug eta`: each busy CPU of `dim`, what it makes, its time left, why it stands, its grace and what it holds and
/// still has to push.
pub(crate) fn debug_lines(host: &mut dyn ServerHost, dim: &'static str) -> Vec<String> {
    if host.level(dim).is_none() {
        return Vec::new();
    }
    let tick = with_state(|s| s.ticks.get(dim).copied().unwrap_or(0));
    let mut keys: Vec<Pos> = with_state(|s| s.cpus.iter().filter(|((d, _), c)| *d == dim && c.cpu.has_job()).map(|((_, k), _)| *k).collect());
    keys.sort_unstable();
    let mut lines = Vec::new();
    for key in keys {
        let left = seconds_left(host, dim, key, tick);
        let stall = job_stall(dim, key, left, tick);
        let line = with_state(|s| {
            let c = s.cpus.get(&(dim, key))?;
            let what = c.cpu.final_output().map_or("-".to_owned(), |f| format!("{}x{}", f.what.id(), f.amount));
            let grace = s.graces.get(&(dim, key)).copied().unwrap_or(STALL_TICKS).max(STALL_TICKS);
            let stood = tick.saturating_sub(s.job_watch.get(&(dim, key)).map_or(tick, |w| w.1));
            Some(format!(
                "CPU {} at {},{},{}: {what} left {left:.1}s stall {stall} stood {stood}t grace {grace}t mode {} {}",
                c.number,
                key[0],
                key[1],
                key[2],
                c.mode,
                c.cpu.debug_state()
            ))
        });
        lines.extend(line);
    }
    if lines.is_empty() {
        lines.push("No crafting CPU in this dimension has a job".to_owned());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::GenericStack;
    use crate::crafting::{PatternBook, calculate};
    use crate::execution::CpuNetwork;

    fn item(id: &str) -> AeKey {
        AeKey::item(id, "", 0, 0)
    }

    fn processing(name: &str, input: (&str, i64), output: (&str, i64)) -> Pattern {
        Pattern {
            definition: AeKey::item("ae2:processing_pattern", name, 0, 0),
            inputs: vec![PatternInput::of(item(input.0), input.1)],
            outputs: vec![GenericStack { what: item(output.0), amount: output.1 }],
        }
    }

    #[derive(Default)]
    struct Net(KeyCounter);

    impl CpuNetwork for Net {
        fn extract(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
            crate::execution::list_extract(&mut self.0, what, amount, mode)
        }
        fn insert(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
            if mode == Actionable::Modulate {
                self.0.add(what, amount);
            }
            amount
        }
        fn provider_count(&self, _: &Pattern) -> usize {
            1
        }
        fn provider_busy(&self, _: &Pattern, _: usize) -> bool {
            false
        }
        fn push_pattern(&mut self, _: &Pattern, _: usize, _: &[KeyCounter]) -> bool {
            true
        }
        fn extract_power(&mut self, amount: f64, _: Actionable) -> f64 {
            amount
        }
    }

    /// A job smelting `n` ore in one machine.
    fn smelting_job(n: i64) -> (CraftingCpu, Pattern) {
        let pattern = processing("smelt", ("minecraft:iron_ore", 1), ("minecraft:iron_ingot", 1));
        let mut book = PatternBook::new();
        book.add(pattern.clone(), 0);
        let mut net = Net::default();
        net.0.add(&item("minecraft:iron_ore"), n);
        let plan = calculate(&book, &net.0, &item("minecraft:iron_ingot"), n, CalculationStrategy::ReportMissingItems).unwrap();
        let mut cpu = CraftingCpu::new(1 << 20, 0);
        cpu.submit(&plan, &mut net, false, None).unwrap();
        (cpu, pattern)
    }

    /// A registered machine's time (an Iron Furnace at 160 ticks, or four smelts at once in 200) drives the forecast:
    /// 8 crafts in one machine take about 8 of its crafts' time.
    #[test]
    fn registered_machine_times_drive_the_forecast() {
        let (cpu, _) = smelting_job(8);
        let said = crate::api::sane(crate::api::MachineTime { ticks: 160.0, latency: 160.0, done: None }).unwrap();
        let t = cpu.forecast(&mut |_| Some((1.0 / said.ticks, said.latency, 1)), &mut |_| (1.0, 0.0)).unwrap();
        assert!((t - 8.0 * 160.0).abs() / (8.0 * 160.0) < 0.02, "{t}");
        // Four at once in 200 ticks: 50 a craft on average, the last out 200 after it went in.
        let said = crate::api::sane(crate::api::MachineTime { ticks: 50.0, latency: 200.0, done: None }).unwrap();
        let t = cpu.forecast(&mut |_| Some((1.0 / said.ticks, said.latency, 1)), &mut |_| (1.0, 0.0)).unwrap();
        assert!((t - 400.0).abs() / 400.0 < 0.05, "{t}");
    }

    /// Patterns pushed into the same machines share them: one shared credit, so 4 crafts each of two patterns in one
    /// 100-tick machine take about as long as 8 crafts there, not 4 (the Horizon port's shared-machine fix).
    #[test]
    fn a_shared_machine_is_counted_once() {
        let a = processing("a", ("minecraft:iron_ore", 1), ("minecraft:iron_ingot", 1));
        let b = processing("b", ("minecraft:gold_ore", 1), ("minecraft:gold_ingot", 1));
        let alloy = Pattern {
            definition: AeKey::item("ae2:processing_pattern", "alloy", 0, 0),
            inputs: vec![PatternInput::of(item("minecraft:iron_ingot"), 1), PatternInput::of(item("minecraft:gold_ingot"), 1)],
            outputs: vec![GenericStack { what: item("minecraft:netherite_scrap"), amount: 1 }],
        };
        let mut book = PatternBook::new();
        for p in [&a, &b, &alloy] {
            book.add(p.clone(), 0);
        }
        let mut net = Net::default();
        net.0.add(&item("minecraft:iron_ore"), 4);
        net.0.add(&item("minecraft:gold_ore"), 4);
        let plan = calculate(&book, &net.0, &item("minecraft:netherite_scrap"), 4, CalculationStrategy::ReportMissingItems).unwrap();
        let mut cpu = CraftingCpu::new(1 << 20, 0);
        cpu.submit(&plan, &mut net, false, None).unwrap();
        let fast = |p: &Pattern| {
            if *p == alloy { Some((1.0, 1.0, 99)) } else { None }
        };
        // The ores in one furnace (group 7), the alloy instantly elsewhere.
        let shared = cpu.forecast(&mut |p| fast(p).or(Some((0.01, 100.0, 7))), &mut |_| (1.0, 0.0)).unwrap();
        // Each ore in a furnace of its own.
        let apart = cpu.forecast(&mut |p| fast(p).or(Some((0.01, 100.0, if *p == a { 7 } else { 8 }))), &mut |_| (1.0, 0.0)).unwrap();
        assert!((shared - 800.0).abs() / 800.0 < 0.05, "{shared}");
        assert!((apart - 400.0).abs() / 400.0 < 0.05, "{apart}");
    }

    #[test]
    fn measured_pace_overrides_only_when_well_behind() {
        // Said 100 a craft; measured 105: within 10%, what was said stands.
        assert_eq!(with_measured(0.01, 100.0, 1, false, Some(105.0)), (0.01, 100.0));
        // Measured 300 (out of fuel): the measured pace.
        let (rate, latency) = with_measured(0.01, 100.0, 1, false, Some(300.0));
        assert!((rate - 1.0 / 300.0).abs() < 1e-12 && latency == 300.0);
        // Nothing said: whatever was measured, even faster.
        assert_eq!(with_measured(1.0 / UNKNOWN_TICKS, UNKNOWN_TICKS, 1, true, Some(20.0)), (0.05, UNKNOWN_TICKS));
        // Nothing measured.
        assert_eq!(with_measured(0.01, 100.0, 1, true, None), (0.01, 100.0));
    }

    #[test]
    fn a_machine_work_pace_becomes_a_craft_time() {
        let said = |ticks, batch| WorkSaid { ticks, batch };
        // One at a time: an ore a craft takes an operation; 8 ore, 8 in a row.
        assert_eq!(craft_time(said(200, 1), 1, 1), Some((200.0, 200.0)));
        assert_eq!(craft_time(said(160, 1), 8, 8), Some((1280.0, 1280.0)));
        // Four at once (a Rainbow Furnace's mult): four crafts of one ore overlap, 50 a craft, each out 200 after.
        assert_eq!(craft_time(said(200, 4), 1, 1), Some((50.0, 200.0)));
        // Six factory rows, 8 ore a craft: two operations for the craft's last, the rows busy throughout.
        let (average, latency) = craft_time(said(100, 6), 8, 8).unwrap();
        assert!((average - 800.0 / 6.0).abs() < 1e-9 && latency == 200.0);
        // A brewing stand: a nether wart and three bottles a craft, a batch of three bottles: a brew a craft, and the
        // bottles fill the batch, so crafts don't overlap.
        assert_eq!(craft_time(said(400, 3), 1, 3), Some((400.0, 400.0)));
        // A log into four planks, one at a time: one operation a craft, not four.
        assert_eq!(craft_time(said(100, 1), 1, 4), Some((100.0, 100.0)));
        // A time of 0 says nothing; a batch of 0 counts as 1.
        assert_eq!(craft_time(said(0, 1), 1, 1), None);
        assert_eq!(craft_time(said(50, 0), 2, 2), Some((100.0, 100.0)));
    }

    #[test]
    fn machine_work_progress_is_ticks_done_of_the_craft() {
        // 60 ticks left of a 200-tick craft: 140 done.
        assert_eq!(done_of_left(Some(60), 200.0), 140.0);
        // Idle, or more left than a craft takes (it slowed down): nothing done.
        assert_eq!(done_of_left(None, 200.0), 0.0);
        assert_eq!(done_of_left(Some(500), 200.0), 0.0);
    }

    #[test]
    fn a_stalled_job_is_late_past_its_grace() {
        // Within the grace (twice the slowest machine, at least STALL_TICKS): not late.
        assert_eq!(late_ticks(30, 0), 0.0);
        assert_eq!(late_ticks(300, 320), 0.0);
        // Past it: late by as long as it stood past it, so the time left climbs while it stands.
        assert_eq!(late_ticks(100, 0), 60.0);
        assert_eq!(late_ticks(500, 320), 180.0);
    }

    #[test]
    fn an_idle_import_bus_runs_on_its_slowest_schedule() {
        // At 40 ticks, due in 10: runs at 10, 50, 90, 130; the job's end at 100 waits 30 for it.
        let b = Puller { interval: 40, due_in: 10, run: 0, last_run: 0, crafts: 1.0, gap: 0.0 };
        assert_eq!(bus_wait(100.0, &b), 30.0);
        // Due already: a run now, then slower by one each idle run (5+1, +1 ...).
        let b = Puller { interval: 5, due_in: -3, run: 0, last_run: 0, crafts: 1.0, gap: 0.0 };
        // Runs at 0, 6, 13, 21, 30: 30 - 25 = 5.
        assert_eq!(bus_wait(25.0, &b), 5.0);
    }

    #[test]
    fn a_busy_import_bus_keeps_fast_up_to_the_end() {
        // Results every 10 ticks up to the end keep it at its fastest: the last is taken within a few ticks.
        let busy = Puller { interval: 5, due_in: 0, run: 1, last_run: 1, crafts: 40.0, gap: 10.0 };
        let idle = Puller { interval: 40, due_in: 0, run: 0, last_run: 0, crafts: 1.0, gap: 0.0 };
        let end = 403.0;
        let (fast, slow) = (bus_wait(end, &busy), bus_wait(end, &idle));
        assert!(fast < 10.0 && fast >= 0.0, "{fast}");
        assert!(slow > fast, "{slow} {fast}");
        // A result of three stacks keeps it at work three runs each: never slower than one result a run.
        let bursty = Puller { interval: 5, due_in: 0, run: 0, last_run: 3, crafts: 40.0, gap: 10.0 };
        assert!(bus_wait(end, &bursty) <= fast + 5.0);
    }

    #[test]
    fn bus_bursts_count_runs_in_a_row() {
        assert_eq!(crate::parts::next_burst(0, 0, true), (1, 0));
        assert_eq!(crate::parts::next_burst(1, 0, true), (2, 0));
        assert_eq!(crate::parts::next_burst(2, 0, false), (0, 2));
        assert_eq!(crate::parts::next_burst(0, 2, false), (0, 2));
    }
}
