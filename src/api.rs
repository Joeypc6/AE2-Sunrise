//! AE2's API for other Sunrise mods, in Rust. A mod names `ae2` under `[dependencies]` in its `sunrise-mod.toml`
//! (`ae2 = { version = "^15.4.13", optional = true }`) and calls these from its hooks inside
//! `mod_api::if_installed!(host, ae2, { ... })`; the launcher links the two crates, so the calls reach this crate's
//! statics.
//!
//! A machine's time for a crafting job's time left (the CPU status screen and the terminal's Crafting Status tab show
//! `(~hh:mm:ss)`): AE2 knows its own machines and vanilla's cooking blocks; any other machine it only learns by watching
//! its results come back. A mod that says how long its machine takes gets a right time from the first craft, its speed
//! upgrades and recipe times included:
//!
//! ```rust,ignore
//! fn machine_time(host: &dyn mod_api::Host, level: mod_api::LevelRef, pos: [i32; 3], block: &str, inputs: &[(&str, i64)])
//!     -> Option<ae2::api::MachineTime> {
//!     if block != "mymod:press" { return None; }
//!     Some(ae2::api::MachineTime { ticks: 100.0, latency: 100.0, done: None })
//! }
//! mod_api::if_installed!(host, ae2, {
//!     ae2::api::register_machine(machine_time);
//! });
//! ```
//!
//! (The Horizon port's `register_machine` / `MachineTime`, me.rs.)
//!
//! Prefer the Machine Work library's shared capability (`machine_work::MACHINE_WORK`, an optional dependency): the ETA asks
//! the machine a provider pushes into for it first, and Jade or a recipe viewer can read the same answer. This API stays
//! as the fallback for mods without the library (`eta.rs` has the order).

use mod_api::{Host, LevelRef};
use std::sync::Mutex;

/// What a machine takes for one craft of a processing pattern (one push of its inputs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MachineTime {
    /// Ticks a craft takes on average, as the machine works now: its throughput (a machine working four crafts at once
    /// in 200 ticks says 50).
    pub ticks: f64,
    /// Ticks from a craft going in to the last of its results coming out (200 in that example). At least `ticks`.
    pub latency: f64,
    /// Ticks the work under way has had so far; None while it is idle.
    pub done: Option<f64>,
}

/// A mod's answer for the block at `pos` in `level`: given the block's id and the pattern's inputs (item or fluid id,
/// and the amount one craft takes; fluids in mB), how long its machine takes. None: not its machine, or it can't work
/// on those inputs. Called on the server thread, at most once a second for each CPU whose screen someone has open.
pub type MachineProvider = fn(&dyn Host, LevelRef, [i32; 3], &str, &[(&str, i64)]) -> Option<MachineTime>;

static PROVIDERS: Mutex<Vec<MachineProvider>> = Mutex::new(Vec::new());

/// Lets a mod tell AE2 how long its machine takes per craft. Calling again with the same function does nothing, so it
/// may be called from a hook that runs more than once (`server_starting`).
pub fn register_machine(f: MachineProvider) {
    let mut v = PROVIDERS.lock().unwrap_or_else(|e| e.into_inner());
    if !v.iter().any(|g| *g as usize == f as usize) {
        v.push(f);
    }
}

/// What a registered mod says of the machine at `pos`: the first answer with a time above 0, its latency at least its
/// time and its progress within its latency.
pub(crate) fn machine_time(host: &dyn Host, level: LevelRef, pos: [i32; 3], block: &str, inputs: &[(&str, i64)]) -> Option<MachineTime> {
    let providers = PROVIDERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    providers.iter().find_map(|p| p(host, level, pos, block, inputs)).and_then(sane)
}

/// A mod's answer as AE2 uses it: none without a finite time above 0; the latency at least the time; the progress
/// within 0 and the latency.
pub(crate) fn sane(m: MachineTime) -> Option<MachineTime> {
    if !(m.ticks.is_finite() && m.ticks > 0.0) {
        return None;
    }
    let latency = if m.latency.is_finite() { m.latency.max(m.ticks) } else { m.ticks };
    let done = m.done.filter(|d| d.is_finite()).map(|d| d.clamp(0.0, latency));
    Some(MachineTime { ticks: m.ticks, latency, done })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_made_sane() {
        assert_eq!(sane(MachineTime { ticks: 0.0, latency: 10.0, done: None }), None);
        assert_eq!(sane(MachineTime { ticks: f64::NAN, latency: 10.0, done: None }), None);
        assert_eq!(sane(MachineTime { ticks: 50.0, latency: 20.0, done: Some(300.0) }), Some(MachineTime { ticks: 50.0, latency: 50.0, done: Some(50.0) }));
        assert_eq!(sane(MachineTime { ticks: 50.0, latency: 200.0, done: Some(-3.0) }), Some(MachineTime { ticks: 50.0, latency: 200.0, done: Some(0.0) }));
    }

    #[test]
    fn registering_twice_keeps_one() {
        fn a(_: &dyn Host, _: LevelRef, _: [i32; 3], _: &str, _: &[(&str, i64)]) -> Option<MachineTime> {
            None
        }
        register_machine(a);
        register_machine(a);
        let n = PROVIDERS.lock().unwrap().iter().filter(|g| **g as usize == a as *const () as usize).count();
        assert_eq!(n, 1);
    }
}
