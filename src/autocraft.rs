//! Stage 5, autocrafting, on the server: encoded patterns on their items, pattern providers (block and part) pushing
//! into the machine beside them through the fork's `ITEMS`/`FLUIDS` capabilities (blocking mode), the molecular
//! assembler, crafting CPUs (one per formed cluster) running `execution.rs`'s jobs, the crafting requests from
//! terminals (plan, start) and the pattern encoding terminal's buttons.
//!
//! The upstream port's logic (`../ae2-horizon/mixins/ae2/me.rs`: CpuNet, run_cpus, decode_pattern, write_pattern,
//! encoder_button, submit_craft) on the fork's API. What enters a network that a CPU waits for goes to the CPU
//! (CraftingServiceStorage, `grid::CPU_WATCH`): a provider's return slots, an interface or an import bus, any of them
//! bring a machine's output back (AE2's return path). `craftingCpuPullsFromMachines` in `config/ae2/common.json`
//! makes the CPU also take the outputs out of the machine itself (the upstream port's old way).

use crate::cells::*;
use crate::core::{Actionable, AeKey, KeyCounter, KeyType};
use crate::crafting::{self, CalculationStrategy, CraftingPlan, Pattern, PatternBook, PatternInput};
use crate::execution::{CpuNetwork, CraftingCpu};
use crate::grid::CPU_WATCH;
use crate::storage::{self, Scope};
use crate::util::*;
use mod_api::*;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[path = "eta.rs"]
pub(crate) mod eta;

/// Client to server: `plan <entry> <amount>`, `start`, `close`; the terminal's crafting status tab (`status auto|<n>|off`,
/// `cpucancel <n>`); the player's terminal settings the server acts on (`prefs <notify> <clear grid>`).
pub(crate) const NET_CRAFT: &str = "ae2:craft";
/// Server to client: a crafting plan, or `#started` / `#error <why>`; `#jobdone <amount> <key>` when a job the player
/// asked for finished.
pub(crate) const NET_PLAN: &str = "ae2:plan";
/// Server to client: the open crafting CPU's status.
pub(crate) const NET_CPU: &str = "ae2:cpu";
/// Server to client: the open pattern encoding terminal's mode, cells and result.
pub(crate) const NET_ENC: &str = "ae2:enc";

/// Menu buttons of stage 5 (above every terminal grid index, `index << 3`).
pub(crate) const ENC: i32 = 1 << 28;
pub(crate) const ENC_ENCODE: i32 = ENC | 1;
pub(crate) const ENC_CLEAR: i32 = ENC | 2;
/// `ENC_MODE + mode`: crafting, processing, smithing, stonecutting.
pub(crate) const ENC_MODE: i32 = ENC | 16;
pub(crate) const PP_BLOCKING: i32 = ENC | 32;
pub(crate) const CPU_CANCEL: i32 = ENC | 48;
pub(crate) const PP_LOCK: i32 = ENC | 33;
/// A crafting CPU's auto-selection mode (Settings.CPU_SELECTION_MODE), on and back.
pub(crate) const CPU_MODE: i32 = ENC | 49;
pub(crate) const CPU_MODE_BACK: i32 = ENC | 50;
/// A job showing no progress this long counts as held up whatever its machines' pace (upstream's STALL_TICKS).
const STALL_TICKS: u64 = 40;
/// The most CPUs listed in a terminal's crafting status.
const STATUS_CPUS: usize = 64;
/// The most busy CPUs' outputs a terminal is told of (its pinned row's crafting animation).
const TERM_JOB_KEYS: usize = 16;
/// `PP_PRIORITY + i`: priority up by 1, 10, 100, 1000 (i 0..3) or down (i 4..7).
pub(crate) const PP_PRIORITY: i32 = ENC | 64;
pub(crate) const ENC_SUBS: i32 = ENC | 3;
pub(crate) const ENC_FLUID_SUBS: i32 = ENC | 4;
/// `PAT_SLOT | provider << 4 | slot`: a pattern access terminal's slot.
pub(crate) const PAT_SLOT: i32 = ENC | (1 << 16);
/// Server to client: the pattern access terminal's providers.
pub(crate) const NET_PAT: &str = "ae2:pat";
/// Server to client: the open molecular assembler's craft (its pattern and grid).
pub(crate) const NET_ASM: &str = "ae2:asm";
/// `ENC_GHOST | cell << 3 | action` (0 left, 1 right, 2 wheel up, 3 wheel down).
pub(crate) const ENC_GHOST: i32 = ENC | (1 << 12);
/// The encoder's cells (enc: AE2's 81 processing inputs and 27 outputs, `encoder::input_cell`/`output_cell`; the
/// first twelve are where the nine-input, three-output encoder saved its cells).
pub(crate) const ENC_CELLS: usize = 108;
/// enc: the provider's "show in pattern access terminal" (Settings.PATTERN_ACCESS_TERMINAL).
pub(crate) const PP_PAT_SHOW: i32 = ENC | 34;

const PATTERN_KEY: &str = "ae2:pattern";
/// MolecularAssemblerBlockEntity: ten ticks a craft without acceleration cards, 10 AE a tick.
const ASSEMBLER_TICKS: u64 = 10;
const ASSEMBLER_AE: f64 = 100.0;

pub(crate) fn is_pattern(id: &str) -> bool {
    matches!(id, "ae2:crafting_pattern" | "ae2:processing_pattern" | "ae2:smithing_table_pattern" | "ae2:stonecutting_pattern")
}

pub(crate) fn is_provider(block: &str) -> bool {
    matches!(block, "ae2:pattern_provider" | "ae2:cable_pattern_provider")
}

/// Blocks whose screen this module serves.
pub(crate) fn has_screen(block: &str) -> bool {
    is_provider(block) || crate::net::is_crafting(block) || matches!(block, "ae2:molecular_assembler" | "ae2:pattern_access_terminal")
}

/// MolecularAssemblerBlockEntity: ticks a craft and AE a tick by acceleration cards (0 to 5).
const ASM_TICKS: [u64; 6] = [10, 8, 6, 5, 4, 2];
const ASM_AE: [f64; 6] = [10.0, 16.9, 28.9, 40.0, 62.5, 250.0];

fn speed_cards(host: &dyn Host, level: LevelRef, asm: Pos) -> usize {
    let Some(be) = host.block_entity(level, asm) else { return 0 };
    (1..6u32).filter(|i| be.item_slot(host, *i).ok().flatten().is_some_and(|s| sid(&s) == "ae2:speed_card")).count()
}

/// `craftingCpuPullsFromMachines` (this port's key, not AE2's): read once from `config/ae2/common.json`, written there
/// with its default when missing.
pub(crate) fn machine_pull() -> bool {
    crate::config::config().machine_pull
}

// ---- Encoded patterns (on the item's custom data)

fn entries(list: &[(AeKey, i64)]) -> Tag {
    Tag::List(list.iter().map(|(k, n)| Tag::String(java(&format!("{n}*{}", key_to_string(k))))).collect())
}

fn read_entries(tag: Option<&Tag>) -> Vec<(AeKey, i64)> {
    let Some(Tag::List(list)) = tag else { return Vec::new() };
    list.iter()
        .filter_map(|t| {
            let Tag::String(s) = t else { return None };
            let s = s.to_string_lossy();
            let (n, key) = s.split_once('*')?;
            let n: i64 = n.parse().ok().filter(|n| *n > 0)?;
            Some((key_from_string(key)?, n))
        })
        .collect()
}

/// An encoded pattern item (`item`): its inputs and outputs, and what each crafting input leaves in the grid.
pub(crate) fn pattern_stack(host: &dyn Host, item: &str, inputs: &[(AeKey, i64)], outputs: &[(AeKey, i64)], left: &[Option<AeKey>]) -> Option<ItemStack> {
    pattern_stack_subs(host, item, inputs, outputs, left, &[], &[])
}

/// With AE2's substitutions: `subs` by input, the other items its slot takes; `fluids` by input, the fluid a bucket
/// there may be instead (fluid substitution).
pub(crate) fn pattern_stack_subs(
    host: &dyn Host,
    item: &str,
    inputs: &[(AeKey, i64)],
    outputs: &[(AeKey, i64)],
    left: &[Option<AeKey>],
    subs: &[String],
    fluids: &[String],
) -> Option<ItemStack> {
    let mut stack = new_stack(host, item, 1)?;
    edit_custom_data(&mut stack, |d| {
        let mut p = Compound::new();
        let strings = |l: &[String]| Tag::List(l.iter().map(|x| Tag::String(java(x))).collect());
        if subs.iter().any(|x| !x.is_empty()) {
            p.insert(java("subs"), strings(subs));
        }
        if fluids.iter().any(|x| !x.is_empty()) {
            p.insert(java("fluids"), strings(fluids));
        }
        p.insert(java("in"), entries(inputs));
        p.insert(java("out"), entries(outputs));
        if left.iter().any(Option::is_some) {
            p.insert(java("rem"), Tag::List(left.iter().map(|l| Tag::String(java(&l.as_ref().map(key_to_string).unwrap_or_default()))).collect()));
        }
        d.insert(java(PATTERN_KEY), Tag::Compound(p));
    });
    Some(stack)
}

/// What an encoded pattern's item holds: inputs and outputs as written, `rem` by input.
pub(crate) fn pattern_lists(stack: &ItemStack) -> Option<(Vec<(AeKey, i64)>, Vec<(AeKey, i64)>, Vec<Option<AeKey>>)> {
    if !is_pattern(sid(stack)) {
        return None;
    }
    let Some(Tag::Compound(p)) = custom_data(stack)?.get(&java(PATTERN_KEY)) else { return None };
    let left = match p.get(&java("rem")) {
        Some(Tag::List(l)) => l.iter().map(|t| if let Tag::String(s) = t { key_from_string(&s.to_string_lossy()) } else { None }).collect(),
        _ => Vec::new(),
    };
    Some((read_entries(p.get(&java("in"))), read_entries(p.get(&java("out"))), left))
}

/// Where each input of a pattern sits in the crafting grid (`grid`, written by the encoder; in order without it).
fn grid_cells(host: &dyn Host, pattern: &Pattern) -> Vec<usize> {
    let Some(st) = stack_of(host, &pattern.definition, 1) else { return Vec::new() };
    let Some(Tag::Compound(p)) = custom_data(&st).and_then(|d| d.get(&java(PATTERN_KEY))).cloned() else { return Vec::new() };
    match p.get(&java("grid")) {
        Some(Tag::String(g)) => g.to_string_lossy().split(',').filter_map(|x| x.parse().ok()).collect(),
        _ => Vec::new(),
    }
}

fn string_list(stack: &ItemStack, key: &str) -> Vec<String> {
    let Some(Tag::Compound(p)) = custom_data(stack).and_then(|d| d.get(&java(PATTERN_KEY))) else { return Vec::new() };
    match p.get(&java(key)) {
        Some(Tag::List(l)) => l.iter().map(|t| if let Tag::String(s) = t { s.to_string_lossy() } else { String::new() }).collect(),
        _ => Vec::new(),
    }
}

/// The pattern an encoded pattern item holds (AECraftingPattern / AEProcessingPattern / smithing / stonecutting).
pub(crate) fn decode(host: &dyn Host, stack: &ItemStack) -> Option<Pattern> {
    let (inputs, outputs, left) = pattern_lists(stack)?;
    if inputs.is_empty() || outputs.is_empty() {
        return None;
    }
    let processing = sid(stack) == "ae2:processing_pattern";
    let subs = string_list(stack, "subs");
    let fluids = string_list(stack, "fluids");
    let inputs = inputs
        .into_iter()
        .enumerate()
        .map(|(i, (k, n))| {
            let mut input = PatternInput::of(k.clone(), n);
            if processing {
                // AEProcessingPattern.Input: a unit's template, the amount times over (getMultiplier).
                let unit = k.key_type().amount_per_unit().max(1);
                let per = if n % unit == 0 { unit } else { n };
                input.multiplier = n / per;
                input.possible[0].amount = per;
            }
            if let Some(Some(l)) = left.get(i) {
                input.remainders.push((k.clone(), l.clone()));
            }
            // Substitutions: the slot's other items; fluid substitution: the bucket's fluid, a bucket's worth (no
            // empty bucket left over then).
            for alt in subs.get(i).map_or("", String::as_str).split('|').filter(|a| !a.is_empty()) {
                if let Some(st) = new_stack(host, alt, 1) {
                    let what = key_of(host, &st);
                    if what != k {
                        input.possible.push(crate::core::GenericStack { what, amount: input.possible[0].amount });
                        input.substitute = true;
                    }
                }
            }
            if let Some(f) = fluids.get(i).filter(|f| !f.is_empty()) {
                input.possible.push(crate::core::GenericStack { what: AeKey::fluid(f, ""), amount: 1000 });
                input.substitute = true;
            }
            input
        })
        .collect();
    Some(Pattern {
        definition: key_of(host, &with_count(stack, 1)?),
        inputs,
        outputs: outputs.into_iter().map(|(what, amount)| crate::core::GenericStack { what, amount }).collect(),
    })
}

/// The tooltip of an encoded pattern (EncodedPatternItem.appendHoverText): what it makes and from what.
pub(crate) fn pattern_tooltip(host: &dyn ClientHost, stack: &ItemStack) -> Vec<String> {
    let Some((inputs, outputs, _)) = pattern_lists(stack) else { return Vec::new() };
    let name = |k: &AeKey| crate::screens::key_name(host, k);
    let amount = |k: &AeKey, n: i64| k.format_amount(n, crate::core::AmountFormat::Full);
    let mut lines = Vec::new();
    for (i, (k, n)) in outputs.iter().enumerate() {
        let head = if i == 0 { "Produces:" } else { "and" };
        lines.push(format!("{head} {} {}", amount(k, *n), name(k)));
    }
    for (i, (k, n)) in inputs.iter().enumerate() {
        let head = if i == 0 { "Ingredients:" } else { "and" };
        lines.push(format!("{head} {} {}", amount(k, *n), name(k)));
    }
    lines
}

// ---- State

/// A crafting CPU: a formed cluster (keyed by its lowest block), its logic and blocks.
pub(crate) struct Cpu {
    pub(crate) cpu: CraftingCpu,
    pub(crate) blocks: Vec<Pos>,
    pub(crate) number: usize,
    /// What was on its way back when the world was saved (an assembler's craft): counted as arrived on load.
    pub(crate) owed: Vec<(AeKey, i64)>,
    /// CpuSelectionMode: 0 any request, 1 players' only, 2 automation's only.
    pub(crate) mode: u8,
}

/// A craft in a molecular assembler, done at `done`, its outputs for the CPU at `cpu`.
pub(crate) struct Asm {
    /// What the assembler's screen shows meanwhile: the pattern and its grid (MolecularAssemblerBlockEntity's gridInv).
    pattern: AeKey,
    grid: Vec<(usize, AeKey)>,
    start: u64,
    done: u64,
    /// fixD: the CPU's level and key (the assembler may be in another level of the grid); `start`/`done` are its
    /// level's ticks.
    cpu: Loc,
    outputs: Vec<(AeKey, i64)>,
}

/// Outputs the CPU takes out of a machine itself (`craftingCpuPullsFromMachines`).
pub(crate) struct Await {
    /// fixD: the machine's level.
    dim: &'static str,
    target: Pos,
    face: Direction,
    /// fixD: the CPU's level and key.
    cpu: Loc,
    outputs: Vec<(AeKey, i64)>,
}

/// A plan worked out for a player, waiting for their Start.
pub(crate) struct PlanReq {
    dim: &'static str,
    terminal: Pos,
    plan: CraftingPlan,
}

/// fixD: each pattern with its provider's level and position (a grid over several levels has providers in each).
pub(crate) type PatternList = Arc<Vec<(Loc, Arc<Pattern>)>>;

/// fixD: a place in a level: a provider, an assembler, a machine, a CPU.
pub(crate) type Loc = (&'static str, Pos);

/// fixD: storing noted for the CPUs (`grid::CPU_WATCH`): (level, grid index, key, amount).
type Noted = (&'static str, usize, AeKey, i64);

#[derive(Default)]
pub(crate) struct State {
    ticks: HashMap<&'static str, u64>,
    pub(crate) cpus: HashMap<(&'static str, Pos), Cpu>,
    assembling: HashMap<(&'static str, Pos), Asm>,
    awaiting: Vec<Await>,
    plans: HashMap<u128, PlanReq>,
    /// Each online grid's patterns (by grid index), scanned once a second.
    patterns: HashMap<(&'static str, usize), PatternList>,
    pattern_gen: u64,
    sent: HashMap<(u128, &'static str), u64>,
    /// Saved CPU states read from the clusters' block entities, waiting for their CPU to form.
    pending: HashMap<(&'static str, Pos), String>,
    /// Providers' send lists (PatternProviderLogic.sendList): the target and what it didn't take yet.
    pub(crate) send: HashMap<(&'static str, Pos), (Pos, Vec<(AeKey, i64)>)>,
    send_seen: HashSet<(&'static str, Pos)>,
    /// Providers locked until a pulse (1) or until a result comes back (2).
    locks: HashMap<(&'static str, Pos), u8>,
    signals: HashMap<(&'static str, Pos), bool>,
    /// What the arrivals of each key a CPU waits for measured, for the time left (`eta`).
    paces: HashMap<(&'static str, AeKey), eta::Pace>,
    /// Each CPU's last forecast: when, its ticks left (None: nothing can go on) and the server's ticks a second then.
    forecasts: HashMap<(&'static str, Pos), (u64, Option<f64>, f64)>,
    /// Block entities whose client copy changed (the crafting monitor's item).
    syncs: Vec<(&'static str, Pos)>,
    /// Terminal viewers with the crafting status tab open: the CPU they look at (None: the first busy one).
    status_view: HashMap<u128, Option<usize>>,
    /// Jobs players started from a terminal, by craft id: who asked, for what (the finished-job notice goes to them).
    requesters: HashMap<u128, (u128, AeKey, i64)>,
    /// Each player's terminal settings the server acts on: notify about finished jobs, clear the crafting grid on close.
    prefs: HashMap<u128, (bool, bool)>,
    /// fixD: storing noted in one level's tick for a grid that goes on in other levels, with the level it was noted
    /// in: the CPUs of the other levels take it on their tick; it goes when that level ticks again.
    xlog: Vec<(&'static str, Noted)>,
    /// Each CPU's job's progress as last seen and the tick it changed (what holds a job up).
    job_watch: HashMap<(&'static str, Pos), ((i64, i64, i64), u64)>,
    /// How long each CPU's job may show no progress before its machines count as holding it up (twice its slowest).
    graces: HashMap<(&'static str, Pos), u64>,
    /// fixF: what `spill` drops in another level, dropped there on its tick.
    spills: Vec<(&'static str, Pos, AeKey, i64)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

pub(crate) fn forget_all() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The CPUs' own storage moves: CraftingServiceStorage doesn't hand those back to them.
fn quiet<R>(f: impl FnOnce() -> R) -> R {
    let was = CPU_WATCH.with_borrow_mut(|w| std::mem::replace(&mut w.quiet, true));
    let out = f();
    CPU_WATCH.with_borrow_mut(|w| w.quiet = was);
    out
}

fn be_nbt(host: &dyn Host, level: LevelRef, pos: Pos) -> Compound {
    crate::bus::nbt_at(host, level, pos).unwrap_or_default()
}

fn grid_of(dim: &'static str, pos: Pos) -> Option<usize> {
    let (grid, online, _) = crate::net::device_state(dim, pos);
    grid.filter(|_| online)
}

fn patterns_of(dim: &'static str, grid: usize) -> PatternList {
    with_state(|s| s.patterns.get(&(dim, grid)).cloned()).unwrap_or_default()
}

// ---- fixD: a grid over several levels (quantum bridges, `xlink`): its CPUs, providers and assemblers in all of them

/// fixD: the online grid at `pos` and its parts in other levels (`net::grid_parts`), as (level, grid index); itself
/// first. Empty when it is offline.
pub(crate) fn joint_grids(dim: &'static str, pos: Pos) -> Vec<(&'static str, usize)> {
    let Some(g) = grid_of(dim, pos) else { return Vec::new() };
    let mut out = vec![(dim, g)];
    for (d, p) in crate::net::grid_parts(dim, pos).into_iter().skip(1) {
        if let Some(g) = grid_of(d, p)
            && !out.contains(&(d, g))
        {
            out.push((d, g));
        }
    }
    out
}

/// fixD: `joint_grids` of a grid known by its index in `dim`.
fn joint_grids_of(dim: &'static str, grid: usize) -> Vec<(&'static str, usize)> {
    let at = crate::net::with_net(dim, |n| n.grids.get(grid).and_then(|g| g.root()));
    at.map_or_else(Vec::new, |p| joint_grids(dim, p))
}

/// fixD: whether the block at `pos` in `dim` is on one of `parts` (a grid's parts, `joint_grids`).
fn on_parts(parts: &[(&'static str, usize)], dim: &'static str, pos: Pos) -> bool {
    grid_of(dim, pos).is_some_and(|g| parts.contains(&(dim, g)))
}

/// fixD: the patterns of every part of a grid, each with its provider's level.
fn joint_patterns(parts: &[(&'static str, usize)]) -> PatternList {
    match parts {
        [] => PatternList::default(),
        [(d, g)] => patterns_of(d, *g),
        _ => Arc::new(parts.iter().flat_map(|(d, g)| patterns_of(d, *g).iter().cloned().collect::<Vec<_>>()).collect()),
    }
}

/// fixD: what the grid's level emitters set to "emit to craft" make, over its parts.
fn joint_emitables(parts: &[(&'static str, usize)]) -> Vec<AeKey> {
    let mut out: Vec<AeKey> = Vec::new();
    for (d, g) in parts {
        for k in crate::craft_cards::emitables(d, *g) {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

/// fixD: the level `d` with the block at `pos` loaded, for reading or pushing into it there: None while that level or
/// its chunk isn't (a provider there is skipped that tick, as AE2 skips an unloaded provider).
fn loaded_level(host: &dyn Host, d: &'static str, pos: Pos) -> Option<LevelRef> {
    host.level(d).filter(|l| l.is_loaded(host, real(pos)))
}

/// fixD: the server ticks of level `d` as `level_tick` counts them.
fn tick_of(d: &'static str) -> u64 {
    with_state(|s| s.ticks.get(d).copied().unwrap_or(0))
}

/// fixD: the CPUs of a grid over all its parts (`joint_grids`), with the numbers its terminals show: a CPU of the
/// asking level keeps its own; one in another level is numbered after the highest in use, in (level, number, key)
/// order, so two levels' CPU 1 stay apart.
fn number_cpus(dim: &'static str, mut list: Vec<(Loc, usize)>) -> Vec<(Loc, usize)> {
    list.sort_by_key(|((d, k), n)| (*d != dim, *d, *n, *k));
    let mut next = list.iter().filter(|((d, _), _)| *d == dim).map(|(_, n)| *n).max().unwrap_or(0);
    for ((d, _), n) in list.iter_mut() {
        if *d != dim {
            next += 1;
            *n = next;
        }
    }
    list
}

/// fixD: storing noted since the last tick of `dim` for the CPUs (`grid::CPU_WATCH`'s `local` log), and what other
/// levels noted for grids that go on here (kept for a round: every other level ticks once before its own level's
/// next tick, so what is left of its own when it comes round again has been offered to all of them).
fn take_noted(s: &mut State, dim: &'static str, local: Vec<Noted>) -> Vec<(&'static str, Noted)> {
    s.xlog.retain(|(origin, _)| *origin != dim);
    let mut out = std::mem::take(&mut s.xlog);
    out.extend(local.into_iter().map(|n| (dim, n)));
    out
}

/// fixD: what no CPU of `dim` took: noted for a grid that goes on in other levels, kept for their CPUs (a grid of
/// this level alone: dropped, as before).
fn keep_noted(s: &mut State, rest: Vec<(&'static str, Noted)>, spans: impl Fn(&'static str, usize) -> bool) {
    for (origin, n) in rest {
        if n.3 > 0 && spans(n.0, n.1) {
            s.xlog.push((origin, n));
        }
    }
    // A level that stopped ticking (unloaded) leaves its entries behind: only so many are kept.
    if s.xlog.len() > 4096 {
        let cut = s.xlog.len() - 4096;
        s.xlog.drain(..cut);
    }
}

/// fixD: whether the grid `grid` of `dim` goes on in other levels.
fn spans_levels(dim: &'static str, grid: usize) -> bool {
    crate::net::with_net(dim, |n| n.grids.get(grid).is_some_and(|g| !g.xlinks.is_empty()))
}

/// What a terminal at `pos` can request (the network's patterns' outputs), and a number that changes with them.
pub(crate) fn craftables(dim: &'static str, pos: Pos) -> (Vec<AeKey>, u64) {
    // fixD: over every level of the grid.
    let parts = joint_grids(dim, pos);
    if parts.is_empty() {
        return (Vec::new(), 0);
    }
    let list = joint_patterns(&parts);
    let mut keys: Vec<AeKey> = Vec::new();
    for (_, p) in list.iter() {
        for o in &p.outputs {
            if !keys.contains(&o.what) {
                keys.push(o.what.clone());
            }
        }
    }
    // StorageLevelEmitterPart.getEmitableItems: what emitters set to "emit to craft" make (`craft_cards`).
    for k in joint_emitables(&parts) {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    // fixD: a link made or broken changes the list too.
    let linked = (parts.len() as u64).wrapping_mul(0x9E37_79B9);
    (keys, with_state(|s| s.pattern_gen).wrapping_add(crate::craft_cards::emitables_gen()).wrapping_add(linked))
}

/// Each online grid's providers' patterns (a provider with a channel), by priority.
fn scan_patterns(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str) {
    let providers: Vec<(Pos, usize)> = crate::net::with_net(dim, |n| {
        n.devices.iter().filter(|(_, (b, _))| is_provider(b)).filter_map(|(p, _)| Some((*p, *n.grid_of.get(p)?))).collect()
    });
    let mut by_grid: HashMap<usize, Vec<(Loc, Arc<Pattern>)>> = HashMap::new();
    for (p, g) in providers {
        let (_, _, active) = crate::net::device_state(dim, p);
        if !active {
            continue;
        }
        for i in 0..9 {
            if let Some(stack) = crate::bus::item_slot(&*host, level, p, i)
                && let Some(pattern) = decode(&*host, &stack)
            {
                by_grid.entry(g).or_default().push(((dim, p), Arc::new(pattern))); // fixD
            }
        }
    }
    with_state(|s| {
        let old: Vec<String> = s.patterns.iter().filter(|((d, _), _)| *d == dim).flat_map(|(_, l)| l.iter().map(|(_, p)| p.definition.components().to_owned())).collect();
        let new: Vec<String> = by_grid.values().flat_map(|l| l.iter().map(|(_, p)| p.definition.components().to_owned())).collect();
        if old.len() != new.len() || old.iter().collect::<HashSet<_>>() != new.iter().collect::<HashSet<_>>() {
            s.pattern_gen += 1;
        }
        s.patterns.retain(|(d, _), _| *d != dim);
        for (g, list) in by_grid {
            s.patterns.insert((dim, g), Arc::new(list));
        }
    });
}

// ---- Providers and machines

/// What a provider pushes to: the part its front, the block every side (PatternProviderLogic's targets).
pub(crate) fn targets(host: &dyn Host, level: LevelRef, at: Pos, block: &str) -> Vec<(Pos, Direction)> {
    if block == "ae2:cable_pattern_provider" {
        return vec![crate::parts::front_of(host, level, at)];
    }
    SIDES.iter().map(|d| (add(at, *d), crate::parts::dir_of(crate::parts::neg(*d)))).collect()
}

/// Machines beside a provider: anything not of the network with an item or fluid handler on that face.
fn machines(host: &mut dyn ServerHost, level: LevelRef, at: Pos, block: &str) -> Vec<(Pos, Direction)> {
    let mut out = Vec::new();
    for (q, f) in targets(&*host, level, at, block) {
        let b = crate::net::block_id(&*host, level, q);
        if crate::net::is_network_block(&b) {
            continue;
        }
        if level.capability(host, &ITEMS, q, Some(f)).is_some() || level.capability(host, &FLUIDS, q, Some(f)).is_some() {
            out.push((q, f));
        }
    }
    out
}

/// Blocking mode (`containsPatternInput`): the machine holds an input of one of the provider's processing patterns.
fn holds_input(host: &mut dyn ServerHost, level: LevelRef, q: Pos, f: Direction, inputs: &[AeKey]) -> bool {
    if let Some(h) = level.capability(host, &ITEMS, q, Some(f)) {
        for i in 0..h.size(host) {
            if h.amount(host, i) > 0
                && let Some(res) = h.resource(host, i)
                && inputs.contains(&key_of(&*host, res.as_stack()))
            {
                return true;
            }
        }
    }
    if let Some(h) = level.capability(host, &FLUIDS, q, Some(f)) {
        for i in 0..h.size(host) {
            if h.amount(host, i) > 0
                && let Some(res) = h.resource(host, i)
                && let Some(id) = crate::parts::fluid_id(&*host, &res)
                && inputs.contains(&AeKey::fluid(&id, ""))
            {
                return true;
            }
        }
    }
    false
}

fn free_assembler(host: &dyn Host, level: LevelRef, at: Pos, block: &str, busy: &HashSet<Pos>) -> Option<Pos> {
    targets(host, level, at, block)
        .into_iter()
        .map(|(q, _)| q)
        .find(|q| !busy.contains(q) && crate::net::block_id(host, level, *q) == "ae2:molecular_assembler")
}

fn is_processing(p: &Pattern) -> bool {
    p.definition.id() == "ae2:processing_pattern"
}

/// A provider as a CPU's push sees it this tick.
#[derive(Clone)]
pub(crate) struct Prov {
    lock: i64,
    block: String,
    priority: i64,
    blocking: bool,
    inputs: Vec<AeKey>,
    craft_free: bool,
    proc_free: bool,
}

/// fixD: the providers that hold `pattern` (over every level of the grid), highest priority first (CraftingService's
/// provider order); one whose level or chunk isn't loaded has no `Prov` this tick and counts as busy.
fn providers_of(patterns: &PatternList, provs: &HashMap<Loc, Prov>, pattern: &Pattern) -> Vec<Loc> {
    let set: BTreeSet<Loc> = patterns.iter().filter(|(_, p)| **p == *pattern).map(|(q, _)| *q).collect();
    let mut out: Vec<Loc> = set.into_iter().collect();
    out.sort_by_key(|p| (-provs.get(p).map_or(0, |v| v.priority), *p));
    out
}

/// fixD: PatternProviderLogic.pushPattern's refusals for the `provider`th of `pattern`'s providers: none there, its level
/// or chunk not loaded (no `Prov`), no free assembler for a crafting pattern, no free machine for a processing one.
fn provider_busy_in(patterns: &PatternList, provs: &HashMap<Loc, Prov>, pattern: &Pattern, provider: usize) -> bool {
    let Some(at) = providers_of(patterns, provs, pattern).get(provider).copied() else { return true };
    let Some(p) = provs.get(&at) else { return true };
    if is_processing(pattern) { !p.proc_free } else { !p.craft_free }
}

/// A CPU's view of its network for `execution.rs`: storage, the providers, power. fixD: the CPU is at `at` in `dim`;
/// its providers, assemblers and machines may be in any level of its grid, each worked through its own level.
pub(crate) struct CpuNet<'a> {
    host: &'a mut dyn ServerHost,
    dim: &'static str,
    at: Pos,
    tick: u64,
    patterns: PatternList,
    provs: HashMap<Loc, Prov>,
    busy_asm: HashSet<Loc>,
    jobs: Vec<(Loc, Asm)>,
    awaits: Vec<Await>,
}

impl<'a> CpuNet<'a> {
    fn new(host: &'a mut dyn ServerHost, dim: &'static str, at: Pos, tick: u64, patterns: PatternList, busy_asm: HashSet<Loc>) -> Self {
        let mut net = Self { host, dim, at, tick, patterns, provs: HashMap::new(), busy_asm, jobs: Vec::new(), awaits: Vec::new() };
        let at_list: BTreeSet<Loc> = net.patterns.iter().map(|(p, _)| *p).collect();
        for p in at_list {
            net.refresh(p);
        }
        net
    }

    fn refresh(&mut self, loc: Loc) {
        let (d, p) = loc;
        let host = &mut *self.host;
        // fixD: a provider in a level or chunk not loaded now is skipped this tick (left out: busy).
        let Some(level) = loaded_level(&*host, d, p) else {
            self.provs.remove(&loc);
            return;
        };
        let block = crate::net::block_id(&*host, level, p);
        let nbt = be_nbt(&*host, level, p);
        let inputs: Vec<AeKey> = self
            .patterns
            .iter()
            .filter(|(q, pat)| *q == loc && is_processing(pat))
            .flat_map(|(_, pat)| pat.inputs.iter().flat_map(|i| i.possible.iter().map(|g| g.what.clone())))
            .collect();
        let blocking = nbt_i64(&nbt, "blocking") != 0;
        // LockCraftingMode: while the redstone is high or low, until a pulse, until a result comes back.
        let lock = nbt_i64(&nbt, "lock");
        let locked = match lock {
            1 => level.has_neighbor_signal(&*host, real(p)),
            2 => !level.has_neighbor_signal(&*host, real(p)),
            3 | 4 => with_state(|s| s.locks.contains_key(&loc)),
            _ => false,
        } || with_state(|s| s.send.contains_key(&loc));
        let busy: HashSet<Pos> = self.busy_asm.iter().filter(|(bd, _)| *bd == d).map(|(_, q)| *q).collect();
        let craft_free = !locked && free_assembler(&*host, level, p, &block, &busy).is_some();
        let proc_free = !locked && machines(host, level, p, &block).into_iter().any(|(q, f)| !(blocking && holds_input(host, level, q, f, &inputs)));
        self.provs.insert(loc, Prov { lock, block, priority: nbt_i64(&nbt, "priority"), blocking, inputs, craft_free, proc_free });
    }

    /// The providers that hold `pattern`, highest priority first (CraftingService's provider order).
    fn providers_of(&self, pattern: &Pattern) -> Vec<Loc> {
        providers_of(&self.patterns, &self.provs, pattern)
    }

    /// A lock mode's after-push: until a pulse, or until a result comes back.
    fn after_push(&mut self, at: Loc, prov: &Prov) {
        match prov.lock {
            3 => with_state(|s| s.locks.insert(at, 1)),
            4 => with_state(|s| s.locks.insert(at, 2)),
            _ => None,
        };
    }

    /// fixD: the grid's storage, from whichever level the host is in.
    fn storage<R>(&mut self, f: impl FnOnce(&mut crate::grid::NetworkStorage<storage::Cell>) -> R) -> Option<R> {
        let (dim, at) = (self.dim, self.at);
        quiet(|| storage::with_storage_in(self.host, dim, Scope::Grid(at), None, f))
    }

    fn store_rest(&mut self, what: &AeKey, amount: i64, at: Loc) {
        if amount <= 0 {
            return;
        }
        let stored = self.storage(|st| st.insert(what, amount, Actionable::Modulate)).unwrap_or(0);
        spill(self.host, at, what, amount - stored);
    }
}

/// One input into a machine's item or fluid handler on face `f`, in `tx`: how much went in.
fn insert_into(host: &mut dyn ServerHost, level: LevelRef, tx: &TransactionContext, q: Pos, f: Direction, k: &AeKey, n: i64) -> i64 {
    let n = n.clamp(0, i64::from(u32::MAX)) as u32;
    let went = match k.key_type() {
        KeyType::Items => {
            let (Some(h), Some(res)) = (level.capability(host, &ITEMS, q, Some(f)), crate::parts::item_resource(&*host, k)) else { return 0 };
            h.insert(host, tx, &res, n).unwrap_or(0)
        }
        KeyType::Fluids => {
            let (Some(h), Some(res)) = (level.capability(host, &FLUIDS, q, Some(f)), crate::parts::fluid_resource(&*host, k)) else { return 0 };
            h.insert(host, tx, &res, n).unwrap_or(0)
        }
    };
    i64::from(went)
}

/// PatternProviderLogic.sendStacksOut: what a machine didn't take yet, offered again each tick.
fn flush_sends(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str) {
    let sends: Vec<(Pos, Pos, Vec<(AeKey, i64)>)> =
        with_state(|s| s.send.iter().filter(|((d, _), _)| *d == dim).map(|((_, p), (q, l))| (*p, *q, l.clone())).collect());
    for (at, q, list) in sends {
        let d = [q[0] - at[0], q[1] - at[1], q[2] - at[2]];
        let f = crate::parts::dir_of(crate::parts::neg(d));
        let left: Vec<(AeKey, i64)> = host
            .transaction(|host, tx| {
                Ok::<_, ()>(list.iter().filter_map(|(k, n)| {
                    let rest = n - insert_into(host, level, tx, q, f, k, *n);
                    (rest > 0).then(|| (k.clone(), rest))
                }).collect())
            })
            .unwrap_or(list);
        with_state(|s| {
            if left.is_empty() {
                s.send.remove(&(dim, at));
            } else {
                s.send.insert((dim, at), (q, left));
            }
        });
    }
}

/// What nothing takes, dropped at `at`. fixD: in its level; while that level isn't there (unloaded), in the host's own
/// level at the same place rather than lost.
fn spill(host: &mut dyn ServerHost, at: Loc, what: &AeKey, amount: i64) {
    if amount <= 0 {
        return;
    }
    let (d, at) = at;
    // fixF: an item spawns only in the level in place (the host gives none in another): dropped in its own level on
    // that level's tick (`level_tick`), not lost.
    if d != host.dimension() && host.level(d).is_some() {
        with_state(|s| s.spills.push((d, at, what.clone(), amount)));
        return;
    }
    let Some(level) = host.level(d).or_else(|| here(&*host)) else { return };
    if let Some(s) = stack_of(&*host, what, amount.min(i64::from(u32::MAX)) as u32) {
        let _ = level.spawn_item(host, f64::from(at[0]) + 0.5, f64::from(at[1]) + 1.2, f64::from(at[2]) + 0.5, s, 10);
    }
}

impl CpuNetwork for CpuNet<'_> {
    fn extract(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
        self.storage(|st| st.extract(what, amount, mode)).unwrap_or(0)
    }

    fn insert(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
        self.storage(|st| st.insert(what, amount, mode)).unwrap_or(0)
    }

    fn provider_count(&self, pattern: &Pattern) -> usize {
        self.providers_of(pattern).len()
    }

    /// PatternProviderLogic.pushPattern's refusals: no free assembler for a crafting pattern; no machine, or blocking
    /// mode with a pattern input still in every machine, for a processing one. fixD: or its level or chunk unloaded.
    fn provider_busy(&self, pattern: &Pattern, provider: usize) -> bool {
        provider_busy_in(&self.patterns, &self.provs, pattern, provider)
    }

    /// A crafting pattern goes to a molecular assembler beside the provider; a processing pattern's inputs into the
    /// first machine beside it that takes them all (adapterAcceptsAll), through the face it touches. fixD: through the
    /// provider's own level (a provider in another level of the grid), only while it and its chunk are loaded.
    fn push_pattern(&mut self, pattern: &Pattern, provider: usize, inputs: &[KeyCounter]) -> bool {
        let Some(loc) = self.providers_of(pattern).get(provider).copied() else { return false };
        let Some(prov) = self.provs.get(&loc).cloned() else { return false };
        let (pd, at) = loc;
        let Some(level) = loaded_level(&*self.host, pd, at) else { return false };
        if !is_processing(pattern) {
            let busy: HashSet<Pos> = self.busy_asm.iter().filter(|(bd, _)| *bd == pd).map(|(_, q)| *q).collect();
            let Some(asm) = free_assembler(&*self.host, level, at, &prov.block, &busy) else { return false };
            let mut outputs: Vec<(AeKey, i64)> = pattern.outputs.iter().map(|o| (o.what.clone(), o.amount)).collect();
            for (input, list) in pattern.inputs.iter().zip(inputs) {
                for (key, n) in list.iter() {
                    if let Some(left) = input.remaining_key(key) {
                        outputs.push((left.clone(), n));
                    }
                }
            }
            let cards = speed_cards(&*self.host, level, asm);
            let ticks = ASM_TICKS[cards.min(5)];
            crate::net::extract_power(pd, at, ASM_AE[cards.min(5)] * ticks as f64);
            self.busy_asm.insert((pd, asm));
            debug(&format!("push {} to the assembler at {pd} {asm:?} from {at:?}, {cards} cards, {ticks} ticks", pattern.primary_output().what.id()));
            let cells = grid_cells(&*self.host, pattern);
            let grid = inputs.iter().enumerate().filter_map(|(j, list)| Some((cells.get(j).copied().unwrap_or(j), list.iter().next()?.0.clone()))).collect();
            self.jobs.push(((pd, asm), Asm { pattern: pattern.definition.clone(), grid, start: self.tick, done: self.tick + ticks, cpu: (self.dim, self.at), outputs }));
            self.after_push(loc, &prov);
            self.refresh(loc);
            return true;
        }
        let host = &mut *self.host;
        for (q, f) in machines(host, level, at, &prov.block) {
            if prov.blocking && holds_input(host, level, q, f, &prov.inputs) {
                continue;
            }
            // adapterAcceptsAll: every input is taken at least in part (tried, then undone).
            let accepts = host.transaction(|host, tx| -> Result<(), bool> {
                for list in inputs {
                    for (k, n) in list.iter() {
                        if insert_into(host, level, tx, q, f, k, n) == 0 {
                            return Err(false);
                        }
                    }
                }
                Err(true)
            });
            if !matches!(accepts, Err(TransactionError::Aborted(true))) {
                continue;
            }
            // What doesn't fit now goes on the send list, and the provider is busy until it is gone.
            let left: Vec<(AeKey, i64)> = host
                .transaction(|host, tx| {
                    let mut left = Vec::new();
                    for list in inputs {
                        for (k, n) in list.iter() {
                            let went = insert_into(host, level, tx, q, f, k, n);
                            if went < n {
                                left.push((k.clone(), n - went));
                            }
                        }
                    }
                    Ok::<_, ()>(left)
                })
                .unwrap_or_default();
            debug(&format!("push {} to the machine at {pd} {q:?} from {at:?}, {} left to send", pattern.primary_output().what.id(), left.len()));
            if !left.is_empty() {
                // fixD: on the provider's level's send list (that level's tick offers it again).
                with_state(|s| s.send.insert(loc, (q, left)));
            }
            self.after_push(loc, &prov);
            if machine_pull() {
                let outputs = pattern.outputs.iter().filter(|o| o.what.key_type() == KeyType::Items).map(|o| (o.what.clone(), o.amount)).collect();
                self.awaits.push(Await { dim: pd, target: q, face: f, cpu: (self.dim, self.at), outputs });
            }
            self.refresh(loc);
            return true;
        }
        false
    }

    fn extract_power(&mut self, amount: f64, mode: Actionable) -> f64 {
        let mult = crate::config::config().power_usage_multiplier.max(1e-9);
        let have = crate::net::grid_power(self.dim, self.at).map_or(0.0, |(e, _)| e);
        let got = (amount * mult).min(have);
        if mode == Actionable::Modulate && got > 0.0 {
            crate::net::extract_power(self.dim, self.at, got);
        }
        got / mult
    }
}

// ---- CPUs

fn storage_bytes(block: &str) -> i64 {
    match crate::net::ae2_name(block) {
        Some("1k_crafting_storage") => 1024,
        Some("4k_crafting_storage") => 4096,
        Some("16k_crafting_storage") => 16384,
        Some("64k_crafting_storage") => 65536,
        Some("256k_crafting_storage") => 262_144,
        _ => 0,
    }
}

/// The formed clusters now (CraftingCPUCalculator): new ones get a CPU, a changed one keeps its job, a broken one
/// cancels it and its items go back to the network (or drop where it stood).
fn refresh_cpus(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str) {
    let mut blocks: HashMap<Pos, String> =
        crate::net::with_net(dim, |n| n.devices.iter().filter(|(_, (b, _))| crate::net::is_crafting(b)).map(|(p, (b, _))| (*p, b.clone())).collect());
    blocks.retain(|p, b| crate::net::block_id(&*host, level, *p) == *b);
    let (_, mut clusters) = crate::net::crafting_clusters(&blocks);
    clusters.iter_mut().for_each(|c| c.sort());
    clusters.sort();
    let mut seen = HashSet::new();
    for (n, cluster) in clusters.iter().enumerate() {
        let key = cluster[0];
        let bytes: i64 = cluster.iter().map(|p| storage_bytes(&blocks[p])).sum();
        let co = cluster.iter().filter(|p| crate::net::ae2_name(&blocks[*p]) == Some("crafting_accelerator")).count() as i32;
        seen.insert(key);
        let restore = with_state(|s| if s.cpus.contains_key(&(dim, key)) { None } else { s.pending.remove(&(dim, key)) });
        with_state(|s| {
            let e = s.cpus.entry((dim, key)).or_insert_with(|| Cpu { cpu: CraftingCpu::new(bytes, co), blocks: Vec::new(), number: 0, owed: Vec::new(), mode: 0 });
            e.cpu.storage_bytes = bytes;
            e.cpu.co_processors = co;
            e.blocks = cluster.clone();
            e.number = n + 1;
        });
        if let Some(text) = restore {
            restore_cpu(&*host, dim, key, &text);
        }
    }
    let gone: Vec<(Pos, Cpu)> = with_state(|s| {
        let keys: Vec<Pos> = s.cpus.keys().filter(|(d, p)| *d == dim && !seen.contains(p)).map(|(_, p)| *p).collect();
        keys.into_iter().filter_map(|k| Some((k, s.cpus.remove(&(dim, k))?))).collect()
    });
    for (key, c) in gone {
        let mut held = KeyCounter::new();
        c.cpu.all_items(&mut held);
        let held: Vec<(AeKey, i64)> = held.iter().map(|(k, _)| (k.clone(), c.cpu.stored(k))).filter(|(_, n)| *n > 0).collect();
        // The cluster's other blocks may still be on the network: through one of them.
        let via = c.blocks.iter().copied().find(|p| blocks.contains_key(p) && grid_of(dim, *p).is_some());
        for (k, n) in held {
            let stored = match via {
                Some(v) => quiet(|| storage::with_storage(host, Scope::Grid(v), None, |st| st.insert(&k, n, Actionable::Modulate)).unwrap_or(0)),
                None => 0,
            };
            spill(host, (dim, key), &k, n - stored);
        }
    }
}

/// A level's end of tick: the clusters and patterns once a second, every CPU's tick, assemblers' and machines' results,
/// what entered storage for the CPUs, and the screens' updates.
pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let tick = with_state(|s| {
        let t = s.ticks.entry(dim).or_insert(0);
        *t += 1;
        *t
    });
    if tick % 20 == 1 {
        refresh_cpus(host, level, dim);
        scan_patterns(host, level, dim);
    }
    flush_sends(host, level, dim);
    // fixF: what other levels' CPUs dropped here.
    let spills: Vec<(&'static str, Pos, AeKey, i64)> = with_state(|s| {
        let (here, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut s.spills).into_iter().partition(|x| x.0 == dim);
        s.spills = rest;
        here
    });
    for (d, at, what, amount) in spills {
        spill(host, (d, at), &what, amount);
    }
    run_cpus(host, dim, tick);
    if tick % 5 == 0 {
        send_screens(host, level, dim);
    }
    for (d, p) in with_state(|s| std::mem::take(&mut s.syncs)) {
        if d == dim {
            host.block_entity_sync(p);
        }
    }
    // CraftingServiceStorage: the keys any CPU waits for.
    let keys: Vec<AeKey> = with_state(|s| {
        let mut k: Vec<AeKey> = s.cpus.values().flat_map(|c| c.cpu.waiting_keys()).collect();
        k.sort_by(|a, b| a.id().cmp(b.id()));
        k.dedup();
        k
    });
    CPU_WATCH.with_borrow_mut(|w| w.keys = keys);
}

fn run_cpus(host: &mut dyn ServerHost, dim: &'static str, tick: u64) {
    // fixD: what was stored for the CPUs, by (level, grid): this level's since its last tick and what other levels
    // noted for grids that go on here (`take_noted`).
    let local: Vec<Noted> = CPU_WATCH.with_borrow_mut(|w| std::mem::take(&mut w.log));
    let mut log: Vec<(&'static str, Noted)> = with_state(|s| take_noted(s, dim, local));
    // fixD: an assembler's craft and a machine's results are this level's CPUs' wherever the assembler or machine is.
    let (keys, mut done, mut awaits, busy): (Vec<Pos>, Vec<(Loc, Asm)>, Vec<Await>, HashSet<Loc>) = with_state(|s| {
        let keys = s.cpus.keys().filter(|(d, _)| *d == dim).map(|(_, p)| *p).collect();
        let finished: Vec<Loc> = s.assembling.iter().filter(|(_, a)| a.cpu.0 == dim && a.done <= tick).map(|(k, _)| *k).collect();
        let done = finished.into_iter().filter_map(|k| Some((k, s.assembling.remove(&k)?))).collect();
        let (here, rest): (Vec<Await>, Vec<Await>) = std::mem::take(&mut s.awaiting).into_iter().partition(|a| a.cpu.0 == dim);
        s.awaiting = rest;
        let busy = s.assembling.keys().copied().collect();
        (keys, done, here, busy)
    });
    let mut busy = busy;
    // Finished jobs' notices for the players who asked for them (PendingCraftingJobs, `jobdone`).
    let mut notices: Vec<(u128, String)> = Vec::new();
    // What reached a CPU waiting for it this tick, for the machines' measured pace.
    let mut arrivals: Vec<(AeKey, i64)> = Vec::new();
    // Jobs that ended this tick (a machine's request hears it: `craft_cards`).
    let mut ended: Vec<(u128, bool)> = Vec::new();
    for key in keys {
        let Some(mut c) = with_state(|s| s.cpus.remove(&(dim, key))) else { continue };
        // fixD: the CPU's grid over every level it reaches (quantum bridges): its providers and storing there too.
        let parts = joint_grids(dim, key);
        c.cpu.active = !parts.is_empty();
        if parts.is_empty() {
            with_state(|s| s.cpus.insert((dim, key), c));
            continue;
        }
        let patterns = joint_patterns(&parts);
        let mut net = CpuNet::new(host, dim, key, tick, patterns, busy.clone());
        // What entered this grid's storage that the CPU waits for (fixD: in any of its levels).
        for (_, (d, g, k, n)) in log.iter_mut() {
            if !parts.contains(&(*d, *g)) || *n <= 0 {
                continue;
            }
            let want = c.cpu.waiting_for(k).min(*n);
            if want <= 0 {
                continue;
            }
            arrivals.push((k.clone(), want));
            let final_key = c.cpu.final_output().is_some_and(|f| f.what == *k);
            if final_key {
                // A player's job: the final output stays in storage, only counted.
                c.cpu.insert(k, want, Actionable::Modulate, &mut net);
            } else {
                let got = net.extract(k, want, Actionable::Modulate);
                if got > 0 {
                    let took = c.cpu.insert(k, got, Actionable::Modulate, &mut net);
                    net.store_rest(k, got - took, (dim, key));
                }
            }
            *n -= want;
        }
        // What was on its way back when the world was saved.
        for (k, n) in std::mem::take(&mut c.owed) {
            let took = c.cpu.insert(&k, n, Actionable::Modulate, &mut net);
            net.store_rest(&k, n - took, (dim, key));
        }
        // An assembler's craft finished: its outputs to the CPU, the rest to storage.
        for (asm, job) in done.iter_mut().filter(|(_, j)| j.cpu == (dim, key)) {
            for (k, n) in std::mem::take(&mut job.outputs) {
                let took = c.cpu.insert(&k, n, Actionable::Modulate, &mut net);
                net.store_rest(&k, n - took, *asm);
            }
        }
        // craftingCpuPullsFromMachines: the outputs straight out of the machine (fixD: through its own level, while
        // its chunk is loaded).
        for a in awaits.iter_mut().filter(|a| a.cpu == (dim, key)) {
            if !c.cpu.has_job() {
                a.outputs.clear();
                continue;
            }
            let Some(level) = loaded_level(&*net.host, a.dim, a.target) else { continue };
            let Some(h) = level.capability(net.host, &ITEMS, a.target, Some(a.face)) else { continue };
            for (k, left) in a.outputs.iter_mut() {
                for i in 0..h.size(net.host) {
                    if *left <= 0 {
                        break;
                    }
                    let Some(res) = h.resource(net.host, i) else { continue };
                    if key_of(&*net.host, res.as_stack()) != *k || h.amount(net.host, i) == 0 {
                        continue;
                    }
                    let want = (*left).min(i64::from(u32::MAX)) as u32;
                    let got = net.host.transaction(|host, tx| Ok::<u32, ()>(h.extract_at(host, tx, i, &res, want).unwrap_or(0))).unwrap_or(0);
                    if got > 0 {
                        arrivals.push((k.clone(), i64::from(got)));
                        let took = c.cpu.insert(k, i64::from(got), Actionable::Modulate, &mut net);
                        net.store_rest(k, i64::from(got) - took, (a.dim, a.target));
                        *left -= i64::from(got);
                    }
                }
            }
            a.outputs.retain(|(_, n)| *n > 0);
        }
        c.cpu.tick(&mut net);
        let (jobs, new_awaits) = (std::mem::take(&mut net.jobs), std::mem::take(&mut net.awaits));
        busy.extend(jobs.iter().map(|(p, _)| *p));
        let events = c.cpu.take_events();
        let _ = c.cpu.take_dirty();
        let mark = c.cpu.progress_mark();
        with_state(|s| {
            for event in events {
                match event {
                    crate::execution::CpuEvent::Finished { craft_id, .. } => {
                        ended.push((craft_id, true));
                        if let Some((player, what, amount)) = s.requesters.remove(&craft_id)
                            && s.prefs.get(&player).is_none_or(|p| p.0)
                        {
                            notices.push((player, format!("#jobdone {amount} {}", key_to_string(&what))));
                        }
                    }
                    crate::execution::CpuEvent::Cancelled { craft_id, .. } => {
                        ended.push((craft_id, false));
                        s.requesters.remove(&craft_id);
                    }
                    crate::execution::CpuEvent::Started { .. } => {}
                }
            }
            // The job's progress: the tick it last got anywhere (`job_stall`).
            match mark {
                Some(m) => {
                    let seen = s.job_watch.entry((dim, key)).or_insert((m, tick));
                    if seen.0 != m {
                        *seen = (m, tick);
                    }
                }
                None => {
                    s.job_watch.remove(&(dim, key));
                }
            }
            for (p, j) in jobs {
                s.assembling.insert(p, j);
            }
            s.awaiting.extend(new_awaits);
            s.cpus.insert((dim, key), c);
        });
    }
    // Results with no CPU left: into the assembler's network (fixD: in its own level), or dropped.
    for ((ad, asm), job) in done {
        for (k, n) in job.outputs {
            let stored = quiet(|| storage::with_storage_in(host, ad, Scope::Grid(asm), None, |st| st.insert(&k, n, Actionable::Modulate)).unwrap_or(0));
            spill(host, (ad, asm), &k, n - stored);
        }
    }
    awaits.retain(|a| !a.outputs.is_empty());
    for (player, text) in notices {
        let _ = crate::util::send_to_player(host, player, NET_PLAN, text.as_bytes());
    }
    for (craft_id, done) in ended {
        crate::craft_cards::job_ended(craft_id, done);
    }
    // fixD: what no CPU here took, for the CPUs of the grid's other levels.
    log.retain(|(_, n)| n.3 > 0);
    let spans: Vec<(&'static str, usize)> = log.iter().map(|(_, n)| (n.0, n.1)).filter(|(d, g)| spans_levels(d, *g)).collect();
    with_state(|s| {
        keep_noted(s, log, |d, g| spans.contains(&(d, g)));
        s.awaiting.extend(awaits);
        for (k, n) in &arrivals {
            eta::arrived(s, dim, k, *n, tick);
        }
    });
}

/// The CPU whose cluster holds `pos`.
fn cpu_at(dim: &'static str, pos: Pos) -> Option<Pos> {
    with_state(|s| s.cpus.iter().find(|((d, _), c)| *d == dim && c.blocks.contains(&pos)).map(|((_, k), _)| *k))
}

// ---- Requests from terminals

/// `plan <entry> <amount>`, `start`, `close` from a player with a terminal open (CraftAmountMenu, CraftConfirmMenu).
pub(crate) fn from_client(host: &mut dyn ServerHost, player: u128, data: &[u8]) {
    if data.len() > 64 || !storage::rate_ok(player) {
        return;
    }
    let text = String::from_utf8_lossy(data).to_string();
    let mut words = text.split_whitespace();
    match words.next() {
        Some("plan") => {
            let index: usize = words.next().and_then(|w| w.parse().ok()).unwrap_or(usize::MAX);
            let amount: i64 = words.next().and_then(|w| w.parse().ok()).unwrap_or(0).clamp(1, i64::from(i32::MAX));
            let reply = make_plan(host, player, index, amount).unwrap_or_else(|e| format!("#error {e}"));
            debug(&format!("plan {index} x{amount}: {}", reply.lines().collect::<Vec<_>>().join(" | ")));
            let _ = crate::util::send_to_player(host, player, NET_PLAN, reply.as_bytes());
        }
        Some("start") => {
            let chosen: usize = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
            let reply = match start(host, player, chosen) {
                Ok(n) => format!("#started {n}"),
                Err(e) => format!("#error {e}"),
            };
            debug(&format!("start: {reply}"));
            let _ = crate::util::send_to_player(host, player, NET_PLAN, reply.as_bytes());
        }
        Some("close") => {
            with_state(|s| s.plans.remove(&player));
        }
        // enc: the encoder's amount dialog, and the crafting terminal's Ctrl for JEI's "+".
        Some("encamt") => {
            let cell = words.next().and_then(|w| w.parse().ok());
            let amount = words.next().and_then(|w| w.parse().ok());
            crate::encoder::amount_message(&*host, player, cell, amount);
        }
        Some("ctrl") => crate::ct_transfer::ctrl_message(player, words.next() == Some("1")),
        // The terminal's crafting status tab: which CPU the player looks at (CraftingStatusMenu's selection).
        Some("status") => {
            let pick = match words.next() {
                Some("off") => {
                    with_state(|s| s.status_view.remove(&player));
                    return;
                }
                Some("auto") => None,
                Some(n) => match n.parse::<usize>() {
                    Ok(n) if n < STATUS_CPUS => Some(n),
                    _ => return,
                },
                None => return,
            };
            if status_terminal(&*host, player).is_none() {
                return;
            }
            with_state(|s| {
                s.status_view.insert(player, pick);
                // Sent again in full, whatever it was last.
                s.sent.remove(&(player, NET_CPU));
            });
        }
        // CraftingStatusMenu: cancel the job of the grid's `n`th CPU.
        Some("cpucancel") => {
            let Some(n) = words.next().and_then(|w| w.parse::<usize>().ok()).filter(|n| *n < STATUS_CPUS) else { return };
            let Some(pos) = status_terminal(&*host, player) else { return };
            let dim = host.dimension();
            let Some(&(key, _)) = grid_cpus(dim, pos).get(n) else { return };
            debug(&format!("status: cancel CPU {key:?} for {player:x}"));
            cancel_cpu(host, key); // fixD: in whichever level of the grid it is
        }
        // The terminal settings the server acts on (sent when a terminal opens and when they change).
        Some("prefs") => {
            let flag = |w: Option<&str>| match w {
                Some("1") => Some(true),
                Some("0") => Some(false),
                _ => None,
            };
            let (Some(notify), Some(clear)) = (flag(words.next()), flag(words.next())) else { return };
            with_state(|s| {
                if s.prefs.len() > 4096 {
                    s.prefs.clear();
                }
                s.prefs.insert(player, (notify, clear));
                s.sent.remove(&(player, NET_CPU));
            });
        }
        _ => {}
    }
}

/// The terminal (not an ME chest's own) a player has open and may still use: the crafting status tab's.
fn status_terminal(host: &dyn Host, player: u128) -> Option<Pos> {
    let (pos, block, chest) = storage::viewer_for_button(host, player)?;
    (!chest && storage::is_terminal(&block)).then_some(pos)
}

/// The CPUs on the grid of `pos`, by their number: (key, number). fixD: in every level of the grid (`joint_cpus`).
fn grid_cpus(dim: &'static str, pos: Pos) -> Vec<(Loc, usize)> {
    let mut list = joint_cpus(dim, &joint_grids(dim, pos));
    list.truncate(STATUS_CPUS);
    list
}

/// fixD: the CPUs on a grid's parts (`joint_grids`, asked from `dim`), with the numbers its terminals there show
/// (`number_cpus`).
fn joint_cpus(dim: &'static str, parts: &[(&'static str, usize)]) -> Vec<(Loc, usize)> {
    if parts.is_empty() {
        return Vec::new();
    }
    let mut list: Vec<(Loc, usize)> =
        with_state(|s| s.cpus.iter().filter(|((d, _), _)| parts.iter().any(|p| p.0 == *d)).map(|(k, c)| (*k, c.number)).collect());
    list.retain(|((d, k), _)| on_parts(parts, d, *k));
    number_cpus(dim, list)
}

/// CraftingCPULogic.cancel on the CPU at `key`: its job dropped, what it holds back to the network. fixD: `key` with
/// its level (the host may be another level's: a terminal there).
fn cancel_cpu(host: &mut dyn ServerHost, key: Loc) {
    let (dim, key) = key;
    let Some(mut c) = with_state(|s| s.cpus.remove(&(dim, key))) else { return };
    let parts = joint_grids(dim, key);
    if !parts.is_empty() {
        let mut net = CpuNet::new(host, dim, key, tick_of(dim), joint_patterns(&parts), HashSet::new());
        debug(&format!("cancel the job of CPU {dim} {key:?}"));
        c.cpu.cancel(&mut net);
    }
    with_state(|s| s.cpus.insert((dim, key), c));
}

/// Whether the player wants the crafting terminal's grid cleared into the network when they close it.
pub(crate) fn clears_grid(player: u128) -> bool {
    with_state(|s| s.prefs.get(&player).is_some_and(|p| p.1))
}

fn make_plan(host: &mut dyn ServerHost, player: u128, index: usize, amount: i64) -> Result<String, String> {
    let (pos, block, chest) = storage::viewer_for_button(&*host, player).ok_or("no terminal")?;
    if chest || !storage::is_terminal(&block) {
        return Err("not a terminal".into());
    }
    let dim = host.dimension();
    let key = storage::viewer_key(player, index).ok_or("no such entry")?;
    // fixD: the grid over every level it reaches.
    let parts = joint_grids(dim, pos);
    if parts.is_empty() {
        return Err("Network offline".into());
    }
    let book = pattern_book(&*host, &parts);
    let mut stock = KeyCounter::new();
    for (k, n) in storage::contents(host, Scope::Grid(pos), None) {
        stock.add(&k, n);
    }
    crafting::limit_calculation(1_000_000, None);
    let plan = crafting::calculate(&book, &stock, &key, amount, CalculationStrategy::ReportMissingItems).map_err(|e| e.to_string())?;
    let mut crafts = KeyCounter::new();
    for (p, times) in &plan.pattern_times {
        for o in &p.outputs {
            crafts.add(&o.what, o.amount * times);
        }
    }
    let mut keys: Vec<AeKey> = Vec::new();
    for c in [&plan.used_items, &crafts, &plan.missing_items] {
        for (k, _) in c.iter() {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
    }
    // fixD: the grid's free CPUs in every level, by the numbers its terminals show.
    let cpus = joint_cpus(dim, &parts);
    let mut free: Vec<usize> = with_state(|s| {
        cpus.iter().filter(|(k, _)| s.cpus.get(k).is_some_and(|c| !c.cpu.has_job() && c.cpu.storage_bytes >= plan.bytes)).map(|(_, n)| *n).collect()
    });
    free.sort();
    let list: Vec<String> = free.iter().map(usize::to_string).collect();
    let mut out = format!("#plan {} {} {} {}\n#cpus {} {}\n", plan.bytes, u8::from(plan.simulation), plan.final_output.amount, key_to_string(&plan.final_output.what), free.len(), list.join(" "));
    for k in keys {
        out.push_str(&format!("{}\t{}\t{}\t{}\n", plan.used_items.get(&k), crafts.get(&k), plan.missing_items.get(&k), key_to_string(&k)));
        if out.len() > 28_000 {
            break;
        }
    }
    with_state(|s| s.plans.insert(player, PlanReq { dim, terminal: pos, plan }));
    Ok(out)
}

/// CraftConfirmMenu.startJob: the plan goes to a free CPU on the terminal's network big enough for it (most
/// co-processors first, then the smallest storage).
fn start(host: &mut dyn ServerHost, player: u128, chosen: usize) -> Result<usize, String> {
    let req = with_state(|s| s.plans.remove(&player)).ok_or("no plan")?;
    let (pos, _, _) = storage::viewer_for_button(&*host, player).ok_or("no terminal")?;
    let dim = host.dimension();
    if pos != req.terminal || dim != req.dim {
        return Err("another terminal".into());
    }
    if req.plan.simulation {
        return Err("Missing ingredients".into());
    }
    // fixD: a CPU in any level of the grid; it runs there (its level's ticks), its storage the grid's.
    let parts = joint_grids(dim, pos);
    if parts.is_empty() {
        return Err("Network offline".into());
    }
    let cpus = joint_cpus(dim, &parts);
    let pick = with_state(|s| pick_cpu(s, &cpus, req.plan.bytes, chosen, true));
    let (cd, key) = pick.ok_or_else(|| "No CPU is free and big enough".to_owned())?;
    let number = cpus.iter().find(|(k, _)| *k == (cd, key)).map_or(0, |(_, n)| *n);
    let mut c = with_state(|s| s.cpus.remove(&(cd, key))).ok_or("CPU gone")?;
    let busy = with_state(|s| s.assembling.keys().copied().collect());
    let mut net = CpuNet::new(host, cd, key, tick_of(cd), joint_patterns(&joint_grids(cd, key)), busy);
    debug(&format!("start on CPU {cd} {key:?}: providers {:?}", net.provs.iter().map(|(p, v)| (*p, v.priority, v.craft_free, v.proc_free)).collect::<Vec<_>>()));
    let r = c.cpu.submit(&req.plan, &mut net, false, None);
    let what = (req.plan.final_output.what.clone(), req.plan.final_output.amount);
    with_state(|s| {
        s.cpus.insert((cd, key), c);
        if let Ok(id) = &r {
            if s.requesters.len() > 4096 {
                s.requesters.clear();
            }
            s.requesters.insert(*id, (player, what.0, what.1));
        }
    });
    r.map(|_| number).map_err(|e| format!("{e:?}"))
}

/// findSuitableCraftingCPU: of the grid's free CPUs big enough, not kept for the other kind of request (CpuSelectionMode
/// 1 players' only, 2 automation's only), one kept for this kind first, then the most co-processors, then the least
/// storage that fits. `chosen` (a player's CPU button, its number) picks that one instead.
///
/// fixD: over `cpus`, the grid's CPUs in every level with the numbers its terminals show (`joint_cpus`): a CPU in
/// another level of the grid is as good as one here (AE2's one grid).
fn pick_cpu(s: &State, cpus: &[(Loc, usize)], bytes: i64, chosen: usize, player: bool) -> Option<Loc> {
    let (own, other) = if player { (1, 2) } else { (2, 1) };
    let mut free: Vec<(Loc, &Cpu)> = cpus
        .iter()
        .filter_map(|(k, n)| Some((*k, *n, s.cpus.get(k)?)))
        .filter(|(_, _, c)| !c.cpu.has_job() && c.cpu.storage_bytes >= bytes)
        .filter(|(_, n, c)| if chosen == 0 { c.mode != other } else { *n == chosen })
        .map(|(k, _, c)| (k, c))
        .collect();
    free.sort_by_key(|(k, c)| (c.mode != own, -c.cpu.co_processors, c.cpu.storage_bytes, *k));
    free.first().map(|(k, _)| *k)
}

/// The grid's patterns by their providers' priority, and what its emitters set to craft by redstone can make.
/// fixD: over the grid's parts in every level (`joint_grids`), each provider's priority read in its own level.
fn pattern_book(host: &dyn Host, parts: &[(&'static str, usize)]) -> PatternBook {
    let mut book = PatternBook::new();
    for ((d, p), pattern) in joint_patterns(parts).iter() {
        let prio = host.level(d).map_or(0, |level| nbt_i64(&be_nbt(host, level, *p), "priority") as i32);
        book.add((**pattern).clone(), prio);
    }
    for k in joint_emitables(parts) {
        book.add_emitable(k);
    }
    book
}

/// The most steps a machine's plan may take on the server thread (a player's has a million).
const MACHINE_PLAN_STEPS: u64 = 200_000;

/// A machine's request (MultiCraftingTracker.handleCrafting with CRAFT_LESS, then submitJob from an automation
/// source): as much of `amount` of `key` as the network can make, on a CPU chosen as for a player's job but for
/// automation. The final output goes into the network as a player's does (the machine takes it from there). The job's
/// id and how many it makes.
pub(crate) fn request_for_machine(host: &mut dyn ServerHost, pos: Pos, key: &AeKey, amount: i64) -> Result<(u128, i64), String> {
    use crate::crafting::CraftingService;
    let dim = host.dimension();
    // fixD: the grid over every level it reaches.
    let parts = joint_grids(dim, pos);
    if parts.is_empty() {
        return Err("Network offline".into());
    }
    let grid = parts[0].1;
    if amount <= 0 {
        return Err("nothing to craft".into());
    }
    if grid_crafting(dim, grid, key) {
        return Err("already crafting".into());
    }
    let book = pattern_book(&*host, &parts);
    if book.crafting_for(key).is_empty() && !book.can_emit_for(key) {
        return Err("not craftable".into());
    }
    // A free CPU at all first: no plan is worked out for a network that couldn't start it.
    let cpus = joint_cpus(dim, &parts);
    if with_state(|s| pick_cpu(s, &cpus, 0, 0, false)).is_none() {
        return Err("No CPU is free".into());
    }
    let mut stock = KeyCounter::new();
    for (k, n) in storage::contents(host, Scope::Grid(pos), None) {
        stock.add(&k, n);
    }
    crafting::limit_calculation(MACHINE_PLAN_STEPS, None);
    let plan = crafting::calculate(&book, &stock, key, amount.min(i64::from(i32::MAX)), CalculationStrategy::CraftLess).map_err(|e| e.to_string())?;
    if plan.simulation || plan.final_output.amount <= 0 {
        return Err("Missing ingredients".into());
    }
    let (cd, cpu) = with_state(|s| pick_cpu(s, &cpus, plan.bytes, 0, false)).ok_or("No CPU is free and big enough")?;
    let mut c = with_state(|s| s.cpus.remove(&(cd, cpu))).ok_or("CPU gone")?;
    let busy = with_state(|s| s.assembling.keys().copied().collect());
    let mut net = CpuNet::new(host, cd, cpu, tick_of(cd), joint_patterns(&joint_grids(cd, cpu)), busy);
    let r = c.cpu.submit(&plan, &mut net, false, None);
    with_state(|s| s.cpus.insert((cd, cpu), c));
    let id = r.map_err(|e| format!("{e:?}"))?;
    // fixF: a cable bus part's node position (y past the world's top) as its cell and side.
    debug(&format!("machine request at {dim} {}: {} x{} on CPU {cd} {cpu:?}", pos_text(pos), key_to_string(key), plan.final_output.amount));
    Ok((id, plan.final_output.amount))
}

/// Whether a CPU on the grid already crafts `key` as its job's final output.
/// fixD: a CPU in any level of the grid.
pub(crate) fn grid_crafting(dim: &'static str, grid: usize, key: &AeKey) -> bool {
    let parts = joint_grids_of(dim, grid);
    let keys: Vec<Loc> = with_state(|s| s.cpus.iter().filter(|(_, c)| c.cpu.final_output().is_some_and(|f| f.what == *key)).map(|(k, _)| *k).collect());
    keys.into_iter().any(|(d, k)| on_parts(&parts, d, k))
}

/// CraftingService.isRequesting / isRequestingAny on the grid of `pos`: a CPU there waits for `key` (with none, any
/// CPU there has a job).
/// fixD: a CPU in any level of the grid.
pub(crate) fn grid_waiting(dim: &'static str, pos: Pos, key: Option<&AeKey>) -> bool {
    let parts = joint_grids(dim, pos);
    if parts.is_empty() {
        return false;
    }
    let keys: Vec<Loc> = with_state(|s| {
        s.cpus
            .iter()
            .filter(|(_, c)| match key {
                Some(key) => c.cpu.waiting_for(key) > 0,
                None => c.cpu.has_job(),
            })
            .map(|(k, _)| *k)
            .collect()
    });
    keys.into_iter().any(|(d, k)| on_parts(&parts, d, k))
}

/// The ids of every job a CPU runs now.
pub(crate) fn running_jobs() -> HashSet<u128> {
    with_state(|s| s.cpus.values().filter_map(|c| c.cpu.job().map(|j| j.craft_id)).collect())
}

// ---- Screens' updates

fn hash_of(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// The open crafting CPU screens' status and pattern encoding terminals' cells, when they changed.
fn send_screens(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str) {
    let viewers: Vec<(u128, Pos, String)> =
        storage::with_store(|s| s.viewers.iter().filter(|(_, v)| v.dim == dim).map(|(u, v)| (*u, v.pos, v.block.clone())).collect());
    let tick = with_state(|s| s.ticks.get(dim).copied().unwrap_or(0));
    for (player, pos, block) in viewers {
        // A terminal's crafting status: its tab's job count, and the CPUs while the tab is open.
        if storage::is_terminal(&block) && storage::with_store(|s| s.viewers.get(&player).is_some_and(|v| !v.chest_term)) {
            let text = terminal_status(host, dim, player, pos);
            send_changed(host, player, NET_CPU, &text);
        }
        let (channel, text) = if crate::net::is_crafting(&block) {
            let key = cpu_at(dim, pos);
            let left = key.map_or(-1.0, |key| eta::seconds_left(host, dim, key, tick));
            let extra = key.map_or(String::new(), |key| cpu_extra(dim, key, left, tick));
            (NET_CPU, cpu_status(dim, pos, left, None) + &extra)
        } else if block == "ae2:pattern_encoding_terminal" {
            (NET_ENC, crate::encoder::view(host, level, dim, pos))
        } else if block == "ae2:pattern_access_terminal" {
            (NET_PAT, crate::access_term::view(host, level, dim, player, pos))
        } else if block == "ae2:molecular_assembler" {
            (NET_ASM, with_state(|s| {
                s.assembling.get(&(dim, pos)).map_or("-\n".to_owned(), |a| {
                    let mut t = format!("{}\n", key_to_string(&a.pattern));
                    for (i, k) in &a.grid {
                        t.push_str(&format!("{i} {}\n", key_to_string(k)));
                    }
                    t
                })
            }))
        } else {
            continue;
        };
        send_changed(host, player, channel, &text);
    }
    // Status tabs of players no longer at a terminal.
    with_state(|s| {
        if s.status_view.len() > 64 {
            s.status_view.retain(|p, _| storage::with_store(|st| st.viewers.contains_key(p)));
        }
    });
}

/// enc: a viewer's screen sent again in full on its next update (after a click changed it).
pub(crate) fn resend(player: u128, channel: &'static str) {
    with_state(|s| s.sent.remove(&(player, channel)));
}

/// A screen's update to its viewer, unless it is what they were last sent on that channel.
fn send_changed(host: &mut dyn ServerHost, player: u128, channel: &'static str, text: &str) {
    let h = hash_of(text);
    if with_state(|s| s.sent.get(&(player, channel)) == Some(&h)) {
        return;
    }
    if channel == NET_CPU || channel == NET_ASM {
        debug(&format!("{channel}: {}", text.lines().collect::<Vec<_>>().join(" | ")));
    }
    if crate::util::send_to_player(host, player, channel, text.as_bytes()).is_ok() {
        with_state(|s| s.sent.insert((player, channel), h));
    }
}

/// What holds a CPU's job up (upstream's `job_stall`): 0 nothing, 1 no power, 2 its machines, 3 nothing can make it go
/// on (no pattern's machine for what it still needs).
fn job_stall(dim: &'static str, key: Pos, left: f64, tick: u64) -> u8 {
    let Some((last, grace)) = with_state(|s| {
        let c = s.cpus.get(&(dim, key))?;
        c.cpu.job()?;
        let last = s.job_watch.get(&(dim, key)).map_or(tick, |w| w.1);
        Some((last, s.graces.get(&(dim, key)).copied().unwrap_or(STALL_TICKS).max(STALL_TICKS)))
    }) else {
        return 0;
    };
    let (_, online, _) = crate::net::device_state(dim, key);
    let unpowered = crate::net::grid_power(dim, key).is_none_or(|(stored, _)| stored < 1.0);
    let stood = tick.saturating_sub(last);
    if !online || (unpowered && stood >= STALL_TICKS) {
        1
    } else if left < 0.0 {
        3
    } else if stood >= grace {
        2
    } else {
        0
    }
}

/// The status lines past CraftingCPUMenu's: the stall reason, the CPU's selection mode, whether it can't store its
/// contents.
fn cpu_extra(dim: &'static str, key: Pos, left: f64, tick: u64) -> String {
    let stall = job_stall(dim, key, left, tick);
    let (mode, cant) = with_state(|s| s.cpus.get(&(dim, key)).map_or((0, false), |c| (c.mode, c.cpu.cant_store_items())));
    format!("#stall {stall}\n#mode {mode}\n#cant {}\n", u8::from(cant))
}

/// A terminal viewer's crafting status (CraftingStatusMenu): `#jobs <busy CPUs>` and `#jk <key>` for what they make;
/// with the tab open, `#list <number> <bytes> <co-processors> <mode> <permille> <amount> <key or ->` for each CPU of the
/// grid, `#sel <index>` and the selected CPU's status as a crafting block's screen has it.
///
/// fixD: the grid's CPUs in every level it reaches (`grid_cpus`), each one's time and status from its own level.
fn terminal_status(host: &mut dyn ServerHost, dim: &'static str, player: u128, pos: Pos) -> String {
    let cpus = grid_cpus(dim, pos);
    let mut out = String::new();
    let (busy, list) = with_state(|s| {
        let mut busy = Vec::new();
        let mut list = String::new();
        for (key, number) in &cpus {
            let Some(c) = s.cpus.get(key) else { continue };
            match c.cpu.final_output() {
                Some(f) => {
                    busy.push(f.what.clone());
                    let permille = c.cpu.job().map_or(0, |j| (f64::from(j.time.progress()) * 1000.0) as i64);
                    list.push_str(&format!("#list {number} {} {} {} {permille} {} {}\n", c.cpu.storage_bytes, c.cpu.co_processors, c.mode, f.amount, key_to_string(&f.what)));
                }
                None => list.push_str(&format!("#list {number} {} {} {} 0 0 -\n", c.cpu.storage_bytes, c.cpu.co_processors, c.mode)),
            }
        }
        (busy, list)
    });
    out.push_str(&format!("#jobs {}\n", busy.len()));
    for k in busy.iter().take(TERM_JOB_KEYS) {
        out.push_str(&format!("#jk {}\n", key_to_string(k)));
    }
    let Some(view) = with_state(|s| s.status_view.get(&player).copied()) else { return out };
    out.push_str(&list);
    if cpus.is_empty() {
        out.push_str("#none\n");
        return out;
    }
    // CraftingStatusMenu's choice while none is made: the first CPU with a job, or else the first.
    let index = match view {
        Some(n) => n.min(cpus.len() - 1),
        None => with_state(|s| cpus.iter().position(|(k, _)| s.cpus.get(k).is_some_and(|c| c.cpu.has_job()))).unwrap_or(0),
    };
    out.push_str(&format!("#sel {index}\n"));
    let ((cd, key), number) = cpus[index];
    let tick = tick_of(cd);
    let left = eta::seconds_left(host, cd, key, tick);
    out.push_str(&cpu_extra(cd, key, left, tick));
    let status = cpu_status(cd, key, left, Some(number));
    // The whole message stays within one payload.
    for line in status.lines() {
        if out.len() + line.len() > 28_000 {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// CraftingCPUMenu's status: `#cpu bytes co-processors job elapsed-seconds remaining seconds-left final-key` (seconds
/// left, this port's: -1 when unknown), then a line per key: stored, crafting (in machines), scheduled. fixD: `number`,
/// the number a terminal of the grid shows for it (`grid_cpus`) when it isn't its own.
fn cpu_status(dim: &'static str, pos: Pos, left: f64, number: Option<usize>) -> String {
    let Some(key) = cpu_at(dim, pos) else { return "#none\n".to_owned() };
    with_state(|s| {
        let Some(c) = s.cpus.get(&(dim, key)) else { return "#none\n".to_owned() };
        let cpu = &c.cpu;
        let number = number.unwrap_or(c.number);
        let mut out = match cpu.final_output() {
            Some(f) => format!(
                "#cpu {} {} {} 1 {} {} {} {}\n",
                number,
                cpu.storage_bytes,
                cpu.co_processors,
                cpu.job().map_or(0, |j| j.time.elapsed_nanos() / 1_000_000_000),
                f.amount,
                if left < 0.0 { -1 } else { left.ceil() as i64 },
                key_to_string(&f.what)
            ),
            None => format!("#cpu {} {} {} 0 0 0 -1 -\n", number, cpu.storage_bytes, cpu.co_processors),
        };
        let mut all = KeyCounter::new();
        cpu.all_items(&mut all);
        for (k, _) in all.iter() {
            let line = format!("{}\t{}\t{}\t{}\n", cpu.stored(k), cpu.waiting_for(k), cpu.pending_outputs(k), key_to_string(k));
            if out.len() + line.len() > 28_000 {
                break;
            }
            out.push_str(&line);
        }
        out
    })
}

// ---- Menu buttons (encoder, provider, CPU)

/// Stage 5's menu buttons: false when `id` isn't one (or not for this block).
pub(crate) fn menu_button(host: &mut dyn ServerHost, player: u128, pos: Pos, block: &str, id: i32, fields: &mut Fields, carried: &mut Option<ItemStack>) -> bool {
    if id < ENC {
        return false;
    }
    if !storage::rate_ok(player) || storage::viewer_for_button(&*host, player).is_none_or(|(p, _, _)| p != pos) {
        return true;
    }
    let dim = host.dimension();
    match id {
        PP_BLOCKING if is_provider(block) => {
            let on = nbt_i64(&fields.nbt, "blocking") == 0;
            set_nbt_i64(&mut fields.nbt, "blocking", i64::from(on));
        }
        PP_LOCK if is_provider(block) => {
            let next = (nbt_i64(&fields.nbt, "lock") + 1) % 5;
            set_nbt_i64(&mut fields.nbt, "lock", next);
            with_state(|s| s.locks.remove(&(dim, pos)));
        }
        _ if is_provider(block) && (PP_PRIORITY..PP_PRIORITY + 8).contains(&id) => {
            let i = id - PP_PRIORITY;
            let step = [1, 10, 100, 1000][(i % 4) as usize];
            let p = nbt_i64(&fields.nbt, "priority") + if i < 4 { step } else { -step };
            set_nbt_i64(&mut fields.nbt, "priority", p.clamp(-999_999, 999_999));
        }
        // enc: the pattern access terminal's display mode and slots (`access_term`).
        PP_PAT_SHOW if is_provider(block) => {
            let hide = nbt_i64(&fields.nbt, "pat_hide") == 0;
            set_nbt_i64(&mut fields.nbt, "pat_hide", i64::from(hide));
        }
        _ if block == "ae2:pattern_access_terminal" => crate::access_term::button(host, player, id, fields, carried),
        CPU_CANCEL if crate::net::is_crafting(block) => {
            let Some(key) = cpu_at(dim, pos) else { return true };
            cancel_cpu(host, (dim, key));
        }
        CPU_MODE | CPU_MODE_BACK if crate::net::is_crafting(block) => {
            let Some(key) = cpu_at(dim, pos) else { return true };
            let step = if id == CPU_MODE { 1 } else { 2 };
            with_state(|s| {
                if let Some(c) = s.cpus.get_mut(&(dim, key)) {
                    c.mode = (c.mode + step) % 3;
                }
            });
        }
        _ if block == "ae2:pattern_encoding_terminal" => crate::encoder::button(host, id, fields, &carried.clone()),
        _ => {}
    }
    true
}

// ---- Block entities

/// A pattern provider's tick: what machines put in its return slots goes into the network (PatternProviderReturnInventory);
/// its blocking mode for the screen.
pub(crate) fn provider_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    if fields.data.is_empty() {
        fields.data.resize(4, 0);
    }
    fields.data[0] = nbt_i64(&fields.nbt, "blocking") as i32;
    fields.data[1] = nbt_i64(&fields.nbt, "lock") as i32;
    fields.data[2] = nbt_i64(&fields.nbt, "priority").clamp(-32768, 32767) as i32;
    // enc: shown in pattern access terminals (0) or hidden (1).
    fields.data[3] = nbt_i64(&fields.nbt, "pat_hide").clamp(0, 1) as i32;
    let dim = host.dimension();
    let mut returned = false;
    for slot in 9..18 {
        let Some(stack) = fields.items.get(slot).cloned().flatten() else { continue };
        let key = key_of(&*host, &stack);
        let stored = storage::insert(host, Scope::Grid(pos), None, &key, i64::from(stack.count()));
        if stored > 0 {
            returned = true;
            fields.items[slot] = with_count(&stack, stack.count() - stored as u32);
        }
    }
    // The return tanks (the fork's mod tanks, `nbt["fluids"]`): into the network too.
    if let Some(Tag::List(tanks)) = fields.nbt.get(&java("fluids")).cloned() {
        let mut out = Vec::new();
        for t in tanks {
            let Tag::Compound(mut c) = t else {
                out.push(t);
                continue;
            };
            let fluid = match c.get(&java("Fluid")) {
                Some(Tag::String(f)) => f.to_string_lossy(),
                _ => String::new(),
            };
            let amount = match c.get(&java("Amount")) {
                Some(Tag::Int(a)) => *a,
                _ => 0,
            };
            if amount > 0 && !fluid.is_empty() && fluid != "minecraft:empty" {
                let stored = storage::insert(host, Scope::Grid(pos), None, &AeKey::fluid(&fluid, ""), i64::from(amount)) as i32;
                if stored > 0 {
                    returned = true;
                    let left = amount - stored;
                    c.insert(java("Amount"), Tag::Int(left));
                    if left <= 0 {
                        c.insert(java("Fluid"), Tag::String(java("minecraft:empty")));
                    }
                }
            }
            out.push(Tag::Compound(c));
        }
        fields.nbt.insert(java("fluids"), Tag::List(out));
    }
    // Lock modes: a result back unlocks "until result", a rising edge "until pulse".
    let signal = here(&*host).is_some_and(|l| l.has_neighbor_signal(&*host, real(pos)));
    with_state(|s| {
        let was = s.signals.insert((dim, pos), signal).unwrap_or(false);
        match s.locks.get(&(dim, pos)) {
            Some(2) if returned => {
                s.locks.remove(&(dim, pos));
            }
            Some(1) if signal && !was => {
                s.locks.remove(&(dim, pos));
            }
            _ => {}
        }
    });
    // The send list with the world: read once after a load, written as it changes.
    let saved = match fields.nbt.get(&java("send")) {
        Some(Tag::String(t)) => t.to_string_lossy(),
        _ => String::new(),
    };
    let first = with_state(|s| s.send_seen.insert((dim, pos)));
    if first && !saved.is_empty() {
        let mut lines = saved.lines();
        let q: Vec<i32> = lines.next().unwrap_or("").split(' ').filter_map(|x| x.parse().ok()).collect();
        let list: Vec<(AeKey, i64)> = lines.filter_map(|l| l.split_once(' ').and_then(|(n, k)| Some((key_from_string(k)?, n.parse().ok()?)))).collect();
        if let [x, y, z] = q[..]
            && !list.is_empty()
        {
            with_state(|s| s.send.insert((dim, pos), ([x, y, z], list)));
        }
    }
    let now = with_state(|s| {
        s.send.get(&(dim, pos)).map_or(String::new(), |(q, l)| {
            let mut t = format!("{} {} {}", q[0], q[1], q[2]);
            for (k, n) in l {
                t.push_str(&format!("\n{n} {}", key_to_string(k)));
            }
            t
        })
    });
    if now != saved {
        if now.is_empty() {
            fields.nbt.remove(&java("send"));
        } else {
            fields.nbt.insert(java("send"), Tag::String(java(&now)));
        }
    }
}

// polish: a provider waiting to unlock (Jade): 1 for a redstone pulse, 2 for a result back.
pub(crate) fn lock_pending(dim: &'static str, pos: Pos) -> Option<u8> {
    with_state(|s| s.locks.get(&(dim, pos)).copied())
}

// polish: the item an assembler is making now (its renderer's AssemblerAnimationStatus), as a key string.
pub(crate) fn assembling_output(dim: &'static str, pos: Pos) -> Option<String> {
    with_state(|s| s.assembling.get(&(dim, pos)).and_then(|a| a.outputs.first()).map(|(k, _)| key_to_string(k)))
}

/// A molecular assembler's tick: its craft's progress for the screen (data 0, in percent).
pub(crate) fn assembler_tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    if fields.data.len() < 2 {
        fields.data.resize(2, 0);
    }
    let dim = host.dimension();
    fields.data[0] = with_state(|s| {
        // fixD: its craft's times are in its CPU's level's ticks.
        s.assembling.get(&(dim, pos)).map_or(0, |a| {
            let now = s.ticks.get(a.cpu.0).copied().unwrap_or(0);
            (((now.saturating_sub(a.start)) * 100) / (a.done - a.start).max(1)).min(100) as i32
        })
    });
}

/// A crafting block's tick: the CPU saved on its lowest block (once a second), a saved one read back after a load,
/// and the crafting monitor's item (the job's final output) for its renderer.
pub(crate) fn crafting_tick(host: &mut dyn ServerHost, pos: Pos, block: &str, fields: &mut Fields) {
    let dim = host.dimension();
    let tick = with_state(|s| s.ticks.get(dim).copied().unwrap_or(0));
    let save = with_state(|s| {
        let c = s.cpus.get(&(dim, pos))?;
        Some((tick % 20 == 5).then(|| save_text(s, dim, pos, c)))
    });
    match save {
        Some(Some(text)) => {
            let old = match fields.nbt.get(&java("ae2:cpu")) {
                Some(Tag::String(t)) => t.to_string_lossy(),
                _ => String::new(),
            };
            if text != old {
                if text.is_empty() {
                    fields.nbt.remove(&java("ae2:cpu"));
                } else {
                    fields.nbt.insert(java("ae2:cpu"), Tag::String(java(&text)));
                }
            }
        }
        Some(None) => {}
        None => {
            let _ = tick;
            if let Some(Tag::String(t)) = fields.nbt.get(&java("ae2:cpu"))
            {
                let t = t.to_string_lossy();
                with_state(|s| s.pending.insert((dim, pos), t));
            }
        }
    }
    if crate::net::ae2_name(block) == Some("crafting_monitor") {
        let shown = cpu_at(dim, pos)
            .and_then(|k| with_state(|s| s.cpus.get(&(dim, k)).and_then(|c| c.cpu.final_output())))
            .map_or(String::new(), |f| key_to_string(&f.what));
        let old = match fields.nbt.get(&java("shown")) {
            Some(Tag::String(t)) => t.to_string_lossy(),
            _ => String::new(),
        };
        if shown != old {
            fields.nbt.insert(java("shown"), Tag::String(java(&shown)));
            with_state(|s| s.syncs.push((dim, pos)));
        }
    }
}

/// The CPU as text: what it holds, its job, what it waits for, its tasks (by pattern item), its timer, and what an
/// assembler is still making for it.
fn save_text(s: &State, dim: &'static str, key: Pos, c: &Cpu) -> String {
    let save = c.cpu.save_state();
    let mut out = String::new();
    if c.mode != 0 {
        out.push_str(&format!("mode\t{}\n", c.mode));
    }
    for (k, n) in &save.inventory {
        out.push_str(&format!("inv\t{n}\t{}\n", key_to_string(k)));
    }
    if let Some(j) = &save.job {
        out.push_str(&format!(
            "job\t{}\t{}\t{}\t{}\t{}\n",
            j.craft_id,
            j.remaining_amount,
            j.final_output.amount,
            u8::from(j.has_requester),
            key_to_string(&j.final_output.what)
        ));
        for (k, n) in &j.waiting_for {
            out.push_str(&format!("wait\t{n}\t{}\n", key_to_string(k)));
        }
        for (k, n) in &j.tasks {
            out.push_str(&format!("task\t{n}\t{}\n", key_to_string(k)));
        }
        out.push_str(&format!("time\t{}\n", j.time.iter().map(i64::to_string).collect::<Vec<_>>().join(",")));
        // fixD: its assemblers in every level (the CPU's own level and key mark them).
        for a in s.assembling.values() {
            if a.cpu == (dim, key) {
                for (k, n) in &a.outputs {
                    out.push_str(&format!("asm\t{n}\t{}\n", key_to_string(k)));
                }
            }
        }
    }
    for (k, n) in &c.owed {
        out.push_str(&format!("asm\t{n}\t{}\n", key_to_string(k)));
    }
    out
}

/// A saved CPU back (CraftingCPUCluster.readFromNBT): tasks decoded from their pattern items.
fn restore_cpu(host: &dyn Host, dim: &'static str, key: Pos, text: &str) {
    use crate::execution::{CpuSave, JobSave};
    let mut save = CpuSave { job: None, inventory: Vec::new() };
    let mut owed = Vec::new();
    let mut job: Option<JobSave> = None;
    let mut mode = 0u8;
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let kn = |i: usize, j: usize| -> Option<(AeKey, i64)> { Some((key_from_string(f.get(j)?)?, f.get(i)?.parse().ok()?)) };
        match f.first().copied() {
            Some("inv") => save.inventory.extend(kn(1, 2)),
            Some("mode") => mode = f.get(1).and_then(|m| m.parse::<u8>().ok()).filter(|m| *m < 3).unwrap_or(0),
            Some("asm") => owed.extend(kn(1, 2)),
            Some("job") if f.len() >= 6 => {
                job = Some(JobSave {
                    craft_id: f[1].parse().unwrap_or(0),
                    remaining_amount: f[2].parse().unwrap_or(0),
                    final_output: crate::core::GenericStack { what: key_from_string(f[5]).unwrap_or_else(|| AeKey::item("minecraft:air", "", 0, 0)), amount: f[3].parse().unwrap_or(0) },
                    has_requester: f[4] == "1",
                    player: None,
                    waiting_for: Vec::new(),
                    tasks: Vec::new(),
                    time: Vec::new(),
                });
            }
            Some("wait") => {
                if let (Some(j), Some(x)) = (job.as_mut(), kn(1, 2)) {
                    j.waiting_for.push(x);
                }
            }
            Some("task") => {
                if let (Some(j), Some(x)) = (job.as_mut(), kn(1, 2)) {
                    j.tasks.push(x);
                }
            }
            Some("time") => {
                if let Some(j) = job.as_mut() {
                    j.time = f.get(1).unwrap_or(&"").split(',').filter_map(|x| x.parse().ok()).collect();
                }
            }
            _ => {}
        }
    }
    save.job = job;
    let mut decode_key = |k: &AeKey| -> Option<Arc<Pattern>> { stack_of(host, k, 1).and_then(|s| decode(host, &s)).map(Arc::new) };
    with_state(|s| {
        if let Some(c) = s.cpus.get_mut(&(dim, key)) {
            c.cpu.restore_state(save, &mut decode_key);
            c.owed = owed;
            c.mode = mode;
        }
    });
    debug(&format!("restored CPU {key:?}"));
}

/// Slot rules: encoded patterns in a provider's nine pattern slots, nothing put in its return slots by hand; the
/// encoder's blank and encoded pattern slots.
pub(crate) fn slot_rules(menu: &str, slot: usize, stack: &ItemStack, may_place: &mut bool, max_stack: &mut u32) {
    let id = sid(stack);
    match (menu, slot) {
        ("ae2:pattern_provider", 0..9) => {
            *may_place = is_pattern(id);
            *max_stack = 1;
        }
        ("ae2:pattern_provider", _) => *may_place = false,
        ("ae2:pattern_encoding_terminal", 0) => *may_place = id != "ae2:blank_pattern" && !is_pattern(id),
        ("ae2:pattern_encoding_terminal", 1) => *may_place = id == "ae2:blank_pattern",
        ("ae2:molecular_assembler", 1..=5) => {
            *may_place = id == "ae2:speed_card";
            *max_stack = 1;
        }
        ("ae2:molecular_assembler", _) => *may_place = false,
        ("ae2:pattern_encoding_terminal", 2) => {
            *may_place = is_pattern(id);
            *max_stack = 1;
        }
        _ => {}
    }
}

// fixD: autocrafting over a grid that two levels share through a quantum network bridge (`xlink`).
#[cfg(test)]
mod xlink_craft_tests {
    use super::*;
    use crate::core::GenericStack;
    use crate::crafting::{PatternBook, PatternInput, calculate};
    use crate::net::{GridInfo, LevelNet, with_net};
    use crate::xlink::{Links, join, tests::level};

    /// The overworld's terminal and CPU spot (`level(2, true)`'s devices), the nether's provider and CPU
    /// (`level(3, false)`'s).
    const TERM: Pos = [1, 65, 0];
    const OVER_CPU: Pos = [2, 65, 0];
    const PROV: Pos = [2, 65, 0];
    const NETHER_CPU: Pos = [3, 65, 0];

    fn item(id: &str) -> AeKey {
        AeKey::item(id, "", 0, 0)
    }

    fn ore() -> AeKey {
        item("minecraft:iron_ore")
    }

    fn ingot() -> AeKey {
        item("minecraft:iron_ingot")
    }

    fn smelt() -> Pattern {
        Pattern {
            definition: AeKey::item("ae2:processing_pattern", "fixd_smelt", 0, 0),
            inputs: vec![PatternInput::of(ore(), 1)],
            outputs: vec![GenericStack { what: ingot(), amount: 1 }],
        }
    }

    /// Both levels' grids as their rebuilds keep them with the bridge linked: the overworld's controller and two
    /// devices, the nether's three devices; one grid over both (`xlink::join`).
    fn link(over: &'static str, nether: &'static str) {
        let mut l = Links::default();
        l.tick(over);
        l.tick(nether);
        l.report(over, &[([0, 64, 0], 7)]);
        l.report(nether, &[([0, 64, 0], 7)]);
        let (o, n) = (level(2, true), level(3, false));
        l.keep(over, &o);
        l.keep(nether, &n);
        for (dim, graph) in [(over, &o), (nether, &n)] {
            let j = join(&l, dim, graph, &[[0, 64, 0]]).expect("linked");
            let grids: Vec<GridInfo> = j
                .grids
                .iter()
                .zip(&j.remote)
                .map(|(g, r)| GridInfo { nodes: g.nodes.iter().filter_map(|n| graph.position(*n)).collect(), online: true, xlinks: r.clone(), ..Default::default() })
                .collect();
            with_net(dim, |net| {
                *net = LevelNet::default();
                net.grid_of = grids.iter().enumerate().flat_map(|(i, g)| g.nodes.iter().map(move |p| (*p, i))).collect();
                net.grids = grids;
            });
        }
    }

    /// The bridge broken (or its chunk unloaded): the next rebuilds leave each level its own grid.
    fn unlink(dims: [&'static str; 2]) {
        for d in dims {
            with_net(d, |n| n.grids.iter_mut().for_each(|g| g.xlinks.clear()));
        }
    }

    fn forget(dims: [&'static str; 2]) {
        for d in dims {
            with_net(d, |n| *n = LevelNet::default());
        }
        with_state(|s| {
            s.patterns.retain(|(d, _), _| !dims.contains(d));
            s.cpus.retain(|(d, _), _| !dims.contains(d));
            s.xlog.retain(|(o, _)| !dims.contains(o));
        });
    }

    fn provider() -> Prov {
        Prov { lock: 0, block: "ae2:pattern_provider".to_owned(), priority: 0, blocking: false, inputs: Vec::new(), craft_free: true, proc_free: true }
    }

    /// A CPU's network as `CpuNet` sees it, minus the world: the grid's storage, its providers (`providers_of`,
    /// `provider_busy_in`: a provider whose level or chunk isn't loaded has no `Prov`) and what went into the machines.
    struct JointNet {
        store: KeyCounter,
        patterns: PatternList,
        provs: HashMap<Loc, Prov>,
        pushed: Vec<Loc>,
        machines: KeyCounter,
    }

    impl JointNet {
        fn new(patterns: PatternList, ore_count: i64) -> Self {
            let mut store = KeyCounter::new();
            store.add(&ore(), ore_count);
            Self { store, patterns, provs: HashMap::new(), pushed: Vec::new(), machines: KeyCounter::new() }
        }
    }

    impl CpuNetwork for JointNet {
        fn extract(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
            crate::execution::list_extract(&mut self.store, what, amount, mode)
        }
        fn insert(&mut self, what: &AeKey, amount: i64, mode: Actionable) -> i64 {
            if mode == Actionable::Modulate {
                self.store.add(what, amount);
            }
            amount
        }
        fn provider_count(&self, pattern: &Pattern) -> usize {
            providers_of(&self.patterns, &self.provs, pattern).len()
        }
        fn provider_busy(&self, pattern: &Pattern, provider: usize) -> bool {
            provider_busy_in(&self.patterns, &self.provs, pattern, provider)
        }
        fn push_pattern(&mut self, pattern: &Pattern, provider: usize, inputs: &[KeyCounter]) -> bool {
            let Some(at) = providers_of(&self.patterns, &self.provs, pattern).get(provider).copied() else { return false };
            if !self.provs.contains_key(&at) {
                return false;
            }
            self.pushed.push(at);
            for list in inputs {
                for (k, n) in list.iter() {
                    self.machines.add(k, n);
                }
            }
            true
        }
        fn extract_power(&mut self, amount: f64, _: Actionable) -> f64 {
            amount
        }
    }

    /// A job for `n` ingots out of the grid's patterns, on a fresh CPU.
    fn job(patterns: &PatternList, net: &mut JointNet, n: i64) -> CraftingCpu {
        let mut book = PatternBook::new();
        for (_, p) in patterns.iter() {
            book.add((**p).clone(), 0);
        }
        let plan = calculate(&book, &net.store, &ingot(), n, CalculationStrategy::ReportMissingItems).expect("a plan");
        assert!(!plan.simulation, "the grid has the ore");
        let mut cpu = CraftingCpu::new(1 << 20, 0);
        cpu.submit(&plan, net, false, None).expect("submitted");
        cpu
    }

    /// No ore made or lost: in storage, in the CPU, in the machines.
    fn ore_total(cpu: &CraftingCpu, net: &JointNet) -> i64 {
        net.store.get(&ore()) + cpu.stored(&ore()) + net.machines.get(&ore())
    }

    #[test]
    fn an_overworld_job_uses_a_nether_provider() {
        const OVER: &str = "fixd1:overworld";
        const NETHER: &str = "fixd1:the_nether";
        link(OVER, NETHER);
        with_state(|s| s.patterns.insert((NETHER, 0), Arc::new(vec![((NETHER, PROV), Arc::new(smelt()))])));
        // The overworld's terminal sees the nether's pattern: one grid, its patterns in both levels.
        let parts = joint_grids(OVER, TERM);
        assert_eq!(parts, vec![(OVER, 0), (NETHER, 0)]);
        assert!(craftables(OVER, TERM).0.contains(&ingot()));
        let patterns = joint_patterns(&joint_grids(OVER, OVER_CPU));
        assert_eq!(patterns.iter().map(|(at, _)| *at).collect::<Vec<_>>(), vec![(NETHER, PROV)]);
        // The overworld's CPU pushes every craft through the nether's provider.
        let mut net = JointNet::new(patterns, 4);
        net.provs.insert((NETHER, PROV), provider());
        let mut cpu = job(&net.patterns.clone(), &mut net, 4);
        // No co-processor: a push, then none while the last three ticks' count is used up.
        for _ in 0..16 {
            cpu.tick(&mut net);
            assert_eq!(ore_total(&cpu, &net), 4);
        }
        assert_eq!(net.pushed, vec![(NETHER, PROV); 4]);
        assert_eq!(net.machines.get(&ore()), 4);
        assert_eq!(cpu.waiting_for(&ingot()), 4);
        // The machine's results enter storage on the nether's tick (noted for its grid there); no CPU there waits for
        // them, so they are kept for the grid's other level, and the overworld's CPU takes them on its tick.
        net.store.add(&ingot(), 4);
        with_state(|s| {
            let rest = take_noted(s, NETHER, vec![(NETHER, 0, ingot(), 4)]);
            keep_noted(s, rest, spans_levels);
        });
        let mut log = with_state(|s| take_noted(s, OVER, Vec::new()));
        let cpu_parts = joint_grids(OVER, OVER_CPU);
        for (_, (d, g, k, n)) in log.iter_mut() {
            if cpu_parts.contains(&(*d, *g)) {
                let want = cpu.waiting_for(k).min(*n);
                cpu.insert(k, want, Actionable::Modulate, &mut net);
                *n -= want;
            }
        }
        assert!(!cpu.has_job(), "the job finished");
        assert_eq!(net.store.get(&ingot()), 4, "the ingots stay in storage, counted once");
        assert_eq!((net.store.get(&ore()), cpu.stored(&ore())), (0, 0), "all the ore went into the nether's machine");
        // What was taken isn't kept again; what is left of the nether's goes when the nether ticks again.
        with_state(|s| {
            keep_noted(s, log, spans_levels);
            assert!(take_noted(s, NETHER, Vec::new()).is_empty());
        });
        forget([OVER, NETHER]);
    }

    #[test]
    fn a_job_goes_to_a_nether_cpu_when_the_overworld_has_none() {
        const OVER: &str = "fixd2:overworld";
        const NETHER: &str = "fixd2:the_nether";
        link(OVER, NETHER);
        with_state(|s| {
            s.cpus.insert((NETHER, NETHER_CPU), Cpu { cpu: CraftingCpu::new(1 << 16, 0), blocks: vec![NETHER_CPU], number: 1, owed: Vec::new(), mode: 0 });
            s.patterns.insert((OVER, 0), Arc::new(vec![((OVER, OVER_CPU), Arc::new(smelt()))]));
        });
        let parts = joint_grids(OVER, TERM);
        let cpus = joint_cpus(OVER, &parts);
        assert_eq!(cpus, vec![((NETHER, NETHER_CPU), 1)]);
        assert_eq!(with_state(|s| pick_cpu(s, &cpus, 1024, 0, true)), Some((NETHER, NETHER_CPU)));
        assert_eq!(with_state(|s| pick_cpu(s, &cpus, 1024, 1, true)), Some((NETHER, NETHER_CPU)), "the player's choice");
        assert_eq!(with_state(|s| pick_cpu(s, &cpus, 1 << 20, 0, true)), None, "too small");
        // Started there: it takes the grid's ore and pushes through the grid's provider (in the overworld here).
        let mut net = JointNet::new(joint_patterns(&joint_grids(NETHER, NETHER_CPU)), 2);
        net.provs.insert((OVER, OVER_CPU), provider());
        let mut c = with_state(|s| s.cpus.remove(&(NETHER, NETHER_CPU))).expect("cpu");
        let mut book = PatternBook::new();
        book.add(smelt(), 0);
        let plan = calculate(&book, &net.store, &ingot(), 2, CalculationStrategy::ReportMissingItems).expect("a plan");
        c.cpu.submit(&plan, &mut net, false, None).expect("submitted");
        c.cpu.tick(&mut net);
        assert_eq!(net.pushed, vec![(OVER, OVER_CPU)]);
        with_state(|s| s.cpus.insert((NETHER, NETHER_CPU), c));
        // Busy now: no other CPU is free.
        assert_eq!(with_state(|s| pick_cpu(s, &cpus, 0, 0, true)), None);
        // Unlinked, the overworld's grid has no CPU at all.
        unlink([OVER, NETHER]);
        assert!(joint_cpus(OVER, &joint_grids(OVER, TERM)).is_empty());
        // A CPU in each level numbered 1: the other level's is numbered after the asking level's.
        let list = vec![((NETHER, [5, 0, 0]), 2), ((OVER, [9, 0, 0]), 1), ((NETHER, [4, 0, 0]), 1)];
        assert_eq!(number_cpus(OVER, list), vec![((OVER, [9, 0, 0]), 1), ((NETHER, [4, 0, 0]), 2), ((NETHER, [5, 0, 0]), 3)]);
        forget([OVER, NETHER]);
    }

    #[test]
    fn an_unloaded_far_level_skips_its_provider_without_losing_items() {
        const OVER: &str = "fixd3:overworld";
        const NETHER: &str = "fixd3:the_nether";
        link(OVER, NETHER);
        with_state(|s| s.patterns.insert((NETHER, 0), Arc::new(vec![((NETHER, PROV), Arc::new(smelt()))])));
        let mut net = JointNet::new(joint_patterns(&joint_grids(OVER, OVER_CPU)), 4);
        // The nether (or the provider's chunk) isn't loaded: `CpuNet::refresh` leaves it out this tick.
        let mut cpu = job(&net.patterns.clone(), &mut net, 4);
        for _ in 0..5 {
            cpu.tick(&mut net);
        }
        assert!(net.pushed.is_empty(), "nothing pushed into an unloaded level");
        assert!(cpu.has_job());
        assert_eq!(cpu.stored(&ore()), 4, "the CPU keeps the ore");
        assert_eq!(ore_total(&cpu, &net), 4);
        // Loaded again: the job goes on.
        net.provs.insert((NETHER, PROV), provider());
        cpu.tick(&mut net);
        assert_eq!(net.pushed, vec![(NETHER, PROV)]);
        assert_eq!(ore_total(&cpu, &net), 4);
        forget([OVER, NETHER]);
    }

    #[test]
    fn unlinking_mid_job_leaves_it_waiting() {
        const OVER: &str = "fixd4:overworld";
        const NETHER: &str = "fixd4:the_nether";
        link(OVER, NETHER);
        with_state(|s| s.patterns.insert((NETHER, 0), Arc::new(vec![((NETHER, PROV), Arc::new(smelt()))])));
        let mut net = JointNet::new(joint_patterns(&joint_grids(OVER, OVER_CPU)), 4);
        net.provs.insert((NETHER, PROV), provider());
        let mut cpu = job(&net.patterns.clone(), &mut net, 4);
        cpu.tick(&mut net);
        assert_eq!(net.pushed.len(), 1);
        // The bridge goes: the CPU's grid is the overworld's alone, with no provider for the pattern.
        unlink([OVER, NETHER]);
        assert_eq!(joint_grids(OVER, OVER_CPU), vec![(OVER, 0)]);
        net.patterns = joint_patterns(&joint_grids(OVER, OVER_CPU));
        assert!(net.patterns.is_empty());
        for _ in 0..5 {
            cpu.tick(&mut net);
        }
        assert_eq!(net.pushed.len(), 1, "nothing more pushed");
        assert!(cpu.has_job(), "the job waits");
        assert_eq!(cpu.waiting_for(&ingot()), 1, "the craft pushed before");
        assert_eq!(cpu.stored(&ore()), 3);
        assert_eq!(ore_total(&cpu, &net), 4, "no ore made or lost");
        // The pushed craft's result comes back into the nether's own grid now: not the overworld CPU's (it stays in
        // the nether's storage), and not kept for other levels.
        with_state(|s| {
            let rest = take_noted(s, NETHER, vec![(NETHER, 0, ingot(), 1)]);
            keep_noted(s, rest, spans_levels);
            assert!(take_noted(s, OVER, Vec::new()).is_empty());
        });
        assert_eq!(cpu.waiting_for(&ingot()), 1, "not taken by the overworld's CPU");
        // Linked again: the job goes on where it stood.
        link(OVER, NETHER);
        net.patterns = joint_patterns(&joint_grids(OVER, OVER_CPU));
        for _ in 0..16 {
            cpu.tick(&mut net);
        }
        assert_eq!(net.pushed.len(), 4);
        assert_eq!(ore_total(&cpu, &net), 4);
        forget([OVER, NETHER]);
    }
}
