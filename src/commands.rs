//! AE2's server commands (appeng.server.AECommand and its subcommands) and the port's `/ae2debug`:
//! `/ae2 channelmode|chunklogger|reloadconfig|tickmonitor|spatial|testmeteorites`, `/ae2debug synth|node|places`.
//! Upstream's `ae2_run` and `debug_command` (`ae2-horizon/mixins/ae2/me.rs`) on Sunrise's `register_commands` and
//! command executors.
//!
//! Other modules add their own `/ae2debug <name> [args]` with [`add_debug`] from `Hooks::register` (the ETA's
//! `/ae2debug eta` lives in `eta.rs`): the handler gets the host, the command's position and its words, and answers
//! the lines to say.

use crate::util::*;
use mod_api::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A `/ae2debug <name>` handler: (host, the command's position, its arguments' words) to the lines it answers.
pub(crate) type DebugFn = fn(&mut dyn ServerHost, [f64; 3], &[&str]) -> Vec<String>;

static DEBUG: Mutex<Vec<(&'static str, DebugFn)>> = Mutex::new(Vec::new());

/// Adds `/ae2debug <name> [args:text]` (once per name; a second call replaces the handler).
pub(crate) fn add_debug(name: &'static str, f: DebugFn) {
    let mut d = DEBUG.lock().unwrap_or_else(|e| e.into_inner());
    d.retain(|(n, _)| *n != name);
    d.push((name, f));
}

/// ChunkLogger: chunks logged as they load and unload.
static CHUNK_LOGGER: AtomicBool = AtomicBool::new(false);
/// TickMonitoring: AE2's own time each level tick, summed and logged once a second.
static TICK_MONITOR: AtomicBool = AtomicBool::new(false);
static TICK_NANOS: AtomicU64 = AtomicU64::new(0);
static TICK_COUNT: AtomicU64 = AtomicU64::new(0);
/// Parts and facades placed since the server started (`/ae2debug places`).
pub(crate) static PLACES: AtomicU64 = AtomicU64::new(0);

/// ChannelMode's names, in `config.channels`' order (0 is infinite).
pub(crate) const CHANNEL_MODES: [(&str, u32); 5] = [
    ("infinite", 0),
    ("default", 1),
    ("x2", 2),
    ("x3", 3),
    ("x4", 4),
];

pub(crate) fn channel_mode_name(factor: u32) -> &'static str {
    CHANNEL_MODES
        .iter()
        .find(|(_, f)| *f == factor)
        .map_or("default", |(n, _)| n)
}

pub(crate) fn channel_mode(name: &str) -> Option<u32> {
    CHANNEL_MODES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, f)| *f)
}

/// The usages, AECommand's tree (operators of level 4) and the debug commands (level 2).
pub(crate) fn register(event: &mut RegisterCommandsEvent<'_>) {
    // The bare `/ae2` is the commands menu (`commands_menu.rs`, everyone's), pushed beside these by the mod.
    for usage in [
        "ae2 help [command:word]",
        "ae2 list",
        "ae2 reloadconfig",
        "ae2 chunklogger",
        "ae2 tickmonitor [enable:boolean]",
        "ae2 channelmode [mode:word]",
        "ae2 spatial",
        "ae2 spatial info <plot:integer>",
        "ae2 spatial tp <plot:integer>",
        "ae2 spatial tpback",
        "ae2 spatial givecell <plot:integer>",
        "ae2 testmeteorites [radius:integer]",
    ] {
        event.commands.push(Command::op(usage, 4).executes(|host, source, c| run(host, source, c).then_some(1).ok_or_else(|| Box::new(Text::literal("")))));
    }
    for usage in [
        "ae2debug synth <cells:integer> <types:integer>",
        "ae2debug node <pos:block_pos> [side:integer]",
        "ae2debug places",
    ] {
        event.commands.push(Command::op(usage, 2).executes(|host, source, c| run(host, source, c).then_some(1).ok_or_else(|| Box::new(Text::literal("")))));
    }
    for (name, _) in DEBUG.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        event.commands.push(
            Command::op(&format!("ae2debug {name} [args:text]"), 2)
                .executes(|host, source, c| run(host, source, c).then_some(1).ok_or_else(|| Box::new(Text::literal("")))),
        );
    }
}

fn say(host: &mut dyn ServerHost, line: &str) {
    let _ = host.commands().feedback(Text::literal(line));
}

fn int_arg(c: &CommandInvocation, name: &str) -> Option<i32> {
    match c.argument(name) {
        Some(CommandArgumentValue::Integer(n)) => Some(*n),
        _ => None,
    }
}

fn word_arg<'a>(c: &'a CommandInvocation, name: &str) -> Option<&'a str> {
    match c.argument(name) {
        Some(CommandArgumentValue::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// A UUID as commands take it (8-4-4-4-12 hex).
pub(crate) fn uuid_text(u: u128) -> String {
    let h = format!("{u:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

const USAGE: &str = "Commands provided by Applied Energistics 2 - use /ae2 list for a list, and /ae2 help _____ for help with a command.";

fn help(what: &str) -> &'static str {
    match what {
        "reloadconfig" => "/ae2 reloadconfig: reads AE2's configuration files again",
        "chunklogger" => "/ae2 chunklogger: logs chunks as they load and unload, or stops",
        "tickmonitor" => "/ae2 tickmonitor [enable]: logs AE2's time per tick once a second",
        "channelmode" => {
            "/ae2 channelmode [infinite|default|x2|x3|x4]: shows or sets the channel mode"
        }
        "spatial" => {
            "/ae2 spatial [info <plot>|tp <plot>|tpback|givecell <plot>]: spatial storage plots"
        }
        "testmeteorites" => {
            "/ae2 testmeteorites [radius]: the meteorites worked out around here, not yet placed"
        }
        _ => USAGE,
    }
}

/// A command's executor: true when the command was one of AE2's.
pub(crate) fn run(
    host: &mut dyn ServerHost,
    source: &CommandSource,
    c: &CommandInvocation,
) -> bool {
    let words: Vec<&str> = c.literals.iter().map(String::as_str).collect();
    match words.first().map(|w| w.trim_start_matches('/')) {
        Some("ae2") => ae2(host, source, c, &words[1..]),
        Some("ae2debug") => debug(host, source, c, &words[1..]),
        _ => return false,
    }
    true
}

fn ae2(host: &mut dyn ServerHost, source: &CommandSource, c: &CommandInvocation, words: &[&str]) {
    match words {
        [] => say(host, USAGE),
        ["list"] => say(
            host,
            "reloadconfig, chunklogger, tickmonitor, channelmode, spatial, testmeteorites",
        ),
        ["help"] => {
            let line = help(word_arg(c, "command").unwrap_or(""));
            say(host, line);
        }
        ["reloadconfig"] => {
            let was = crate::grid::channel_factor();
            crate::config::load(&*host);
            if crate::grid::channel_factor() != was {
                crate::net::mark_all_dirty();
            }
            say(host, "AE2 configuration reloaded");
        }
        ["chunklogger"] => {
            let on = !CHUNK_LOGGER.fetch_xor(true, Ordering::Relaxed);
            say(
                host,
                if on {
                    "Chunk Logging is now on"
                } else {
                    "Chunk Logging is now off"
                },
            );
        }
        ["tickmonitor"] => {
            let on = match c.argument("enable") {
                Some(CommandArgumentValue::Boolean(b)) => *b,
                _ => !TICK_MONITOR.load(Ordering::Relaxed),
            };
            TICK_MONITOR.store(on, Ordering::Relaxed);
            say(
                host,
                &format!("Tick monitoring is now {}", if on { "on" } else { "off" }),
            );
        }
        ["channelmode"] => match word_arg(c, "mode") {
            None => {
                let line = format!(
                    "Current channel mode: {}",
                    channel_mode_name(crate::grid::channel_factor())
                );
                say(host, &line);
            }
            Some(mode) => match channel_mode(mode) {
                None => say(host, &format!("Unknown channel mode: {mode}")),
                Some(f) => {
                    // AEConfig.setChannelModel: applied and saved; PathingService.repath on every grid.
                    crate::grid::CHANNEL_MODE.store(f, Ordering::Relaxed);
                    save_channel_mode(&*host, channel_mode_name(f));
                    let grids = crate::net::mark_all_dirty();
                    say(
                        host,
                        &format!(
                            "Channel mode set to {}. Updated {grids} grids.",
                            channel_mode_name(f)
                        ),
                    );
                }
            },
        },
        ["spatial", rest @ ..] => spatial(host, source, c, rest),
        ["testmeteorites"] => {
            let radius = int_arg(c, "radius").unwrap_or(1000).clamp(16, 30_000);
            let at = [c.position[0].floor() as i32, c.position[2].floor() as i32];
            let found = crate::meteor::known_near(at, radius);
            if found.is_empty() {
                say(
                    host,
                    &format!("No meteorites waiting to be placed within {radius} blocks."),
                );
            }
            for (center, r, y) in found {
                let d = ((center[0] - at[0]) as f64).hypot((center[1] - at[1]) as f64) as i64;
                say(
                    host,
                    &format!(
                        "Meteorite at {} {y} {} (radius {r:.1}), {d} blocks away",
                        center[0], center[1]
                    ),
                );
            }
        }
        _ => say(host, USAGE),
    }
}

/// `general.channels` written into common.json (the file's other keys kept).
fn save_channel_mode(host: &dyn Host, name: &str) {
    use mod_api::serde_json::{Map, Value};
    let files = host.config_files();
    let Ok(path) = ConfigPath::new(crate::config::COMMON_PATH) else {
        return;
    };
    let mut doc = files
        .read(&path)
        .ok()
        .and_then(|b| mod_api::serde_json::from_slice::<Value>(&b).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    if let Some(root) = doc.as_object_mut() {
        let general = root
            .entry("general")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(g) = general.as_object_mut() {
            g.insert("channels".to_owned(), Value::from(name));
        }
    }
    if let Ok(text) = mod_api::serde_json::to_string_pretty(&doc) {
        let _ = files.write(&path, text.as_bytes());
    }
}

/// SpatialStorageCommand.
fn spatial(
    host: &mut dyn ServerHost,
    source: &CommandSource,
    c: &CommandInvocation,
    words: &[&str],
) {
    let last = crate::spatial::last_plot(&*host);
    let plot = int_arg(c, "plot");
    let player = match source {
        CommandSource::Player { player, .. } => Some(*player),
        _ => None,
    };
    let known = |p: i32| p >= 1 && p <= last;
    match (words, plot) {
        ([], _) => {
            say(host, &format!("{last} spatial storage plots in use."));
            for p in 1..=last.min(20) {
                let o = crate::spatial::plot_origin(p);
                say(
                    host,
                    &format!("Plot {p}: origin {} {} {}", o[0], o[1], o[2]),
                );
            }
        }
        (["info"], Some(p)) if known(p) => {
            let o = crate::spatial::plot_origin(p);
            say(
                host,
                &format!(
                    "Plot {p} in {}: origin {} {} {}",
                    crate::spatial::DIMENSION,
                    o[0],
                    o[1],
                    o[2]
                ),
            );
        }
        (["tp"], Some(p)) if known(p) => {
            let Some(uuid) = player.and_then(|pl| pl.entity().uuid(host)) else {
                say(host, "Only a player can be teleported.");
                return;
            };
            let o = crate::spatial::plot_origin(p);
            let line = format!(
                "execute in {} run tp {} {} {} {}",
                crate::spatial::DIMENSION,
                uuid_text(uuid),
                o[0],
                o[1],
                o[2]
            );
            let _ = host.commands().run_command(&line);
            say(host, &format!("Teleporting to plot {p}."));
        }
        (["tpback"], _) => {
            let Some(pl) = player else {
                say(host, "Only a player can be teleported.");
                return;
            };
            let Some(uuid) = pl.entity().uuid(host) else {
                return;
            };
            // The player's spawn point, else the world's.
            let (dim, at) = match pl.respawn_config(&*host).flatten() {
                Some(cfg) => {
                    let p = cfg.position.pos();
                    (cfg.position.dimension().to_string(), [p.x, p.y, p.z])
                }
                // The world spawn, in the level `/setworldspawn` ran in.
                None => host
                    .level("minecraft:overworld")
                    .and_then(|l| l.respawn_data(&*host))
                    .map(|r| {
                        let p = r.position.pos();
                        (r.position.dimension().to_string(), [p.x, p.y, p.z])
                    })
                    .unwrap_or_else(|| ("minecraft:overworld".to_owned(), [0, 64, 0])),
            };
            let line = format!(
                "execute in {dim} run tp {} {} {} {}",
                uuid_text(uuid),
                at[0],
                at[1],
                at[2]
            );
            let _ = host.commands().run_command(&line);
            say(host, "Teleporting back.");
        }
        (["givecell"], Some(p)) if known(p) => {
            let Some(pl) = player else {
                say(host, "Only a player can be given a cell.");
                return;
            };
            let Some(mut cell) = crate::cells::new_stack(&*host, "ae2:spatial_storage_cell_128", 1)
            else {
                return;
            };
            crate::cells::edit_custom_data(&mut cell, |d| {
                d.insert(java("ae2:plot_id"), Tag::Int(p));
            });
            let Some(uuid) = pl.entity().uuid(host) else {
                return;
            };
            crate::wireless::give_back(host, uuid, cell);
            say(host, &format!("Gave a spatial storage cell of plot {p}."));
        }
        (_, Some(p)) => say(
            host,
            &format!("There is no spatial storage plot with id {p}."),
        ),
        _ => say(host, help("spatial")),
    }
}

fn debug(
    host: &mut dyn ServerHost,
    _source: &CommandSource,
    c: &CommandInvocation,
    words: &[&str],
) {
    let lines: Vec<String> = match words {
        ["synth"] => synth(
            int_arg(c, "cells").unwrap_or(1),
            int_arg(c, "types").unwrap_or(1),
        ),
        ["node"] => {
            let Some(CommandArgumentValue::BlockPos(p)) = c.argument("pos") else {
                return;
            };
            let p = *p;
            let pos = match int_arg(c, "side") {
                Some(side) if (0..6).contains(&side) => hosted(p, side as usize),
                _ => p,
            };
            let dim = host.dimension();
            vec![node_text(dim, pos)]
        }
        ["places"] => vec![format!("Places tried: {}", PLACES.load(Ordering::Relaxed))],
        [name] => {
            let f = DEBUG
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, f)| *f);
            let Some(f) = f else { return };
            let args = word_arg(c, "args").unwrap_or("");
            let words: Vec<&str> = args.split_whitespace().collect();
            f(host, c.position, &words)
        }
        _ => vec!["ae2debug synth <cells> <types> | node <x y z> [side] | places".to_owned()],
    };
    for line in lines {
        say(host, &line);
    }
}

/// What the network says of a block (or of the part on `side` of a cable bus).
fn node_text(dim: &'static str, pos: Pos) -> String {
    crate::net::with_net(dim, |n| {
        match n
            .grid_of
            .get(&pos)
            .copied()
            .and_then(|g| n.grids.get(g).map(|i| (g, i)))
        {
            Some((g, i)) => format!(
                "node grid={g} online={} conflict={} nested={} too_many={} active={} channels={} booting={}",
                i.online,
                i.conflict,
                i.nested_p2p,
                i.too_many,
                i.active.contains(&pos),
                i.cable_channels.get(&pos).copied().unwrap_or(0),
                false
            ),
            None => "node: no grid".to_owned(),
        }
    })
}

/// A network of `cells` 256k item cells in memory only, `types` kinds spread over them, filled: how long AE2's storage
/// paths take at that size, with no world in the way.
pub(crate) fn synth(cells: i32, types: i32) -> Vec<String> {
    use crate::core::{
        Actionable, AeKey, BasicCellInventory, CellContents, CellUpgrades, FuzzyMode,
    };
    use crate::grid::{MeStorage, NetworkStorage};
    let (cells, types) = (
        cells.clamp(1, 4096) as i64,
        types.clamp(1, 1_000_000) as i64,
    );
    let Some(cell) = crate::cells::cell_type("ae2:item_storage_cell_256k") else {
        return vec!["no 256k cell".to_owned()];
    };
    let mut st: NetworkStorage<Box<dyn MeStorage>> = NetworkStorage::new();
    for c in 0..cells as u64 {
        st.mount(
            c,
            0,
            Box::new(BasicCellInventory::new(
                cell.clone(),
                &[],
                FuzzyMode::IgnoreAll,
                CellUpgrades::default(),
                &CellContents::default(),
            )),
        );
    }
    let t = std::time::Instant::now();
    let each = (cells * 2_000_000 / types).max(1);
    let mut stored = 0i64;
    for k in 0..types {
        stored += st.insert(
            &AeKey::item("minecraft:diamond_sword", &format!("n{k}"), 0, 0),
            each,
            Actionable::Modulate,
        );
    }
    let fill = t.elapsed();
    let t = std::time::Instant::now();
    let kinds = st.contents().size();
    let scan = t.elapsed();
    let t = std::time::Instant::now();
    let key = AeKey::item("minecraft:diamond_sword", &format!("n{}", types / 2), 0, 0);
    for _ in 0..1000 {
        let got = st.extract(&key, 64, Actionable::Modulate);
        st.insert(&key, got, Actionable::Modulate);
    }
    let moves = t.elapsed();
    vec![format!(
        "{cells} cells, {kinds} types, {stored} items: fill {:.1} ms, full scan {:.2} ms, 1000 take+put {:.2} ms",
        fill.as_secs_f64() * 1000.0,
        scan.as_secs_f64() * 1000.0,
        moves.as_secs_f64() * 1000.0
    )]
}

/// `chunk_load` / `chunk_unload`, while the logger is on (ChunkLogger.onChunkLoadEvent).
pub(crate) fn chunk_logged(dimension: &str, chunk: [i32; 2], load: bool) {
    if CHUNK_LOGGER.load(Ordering::Relaxed) {
        eprintln!(
            "[AE2] {} chunk {} {} in {dimension}",
            if load { "Loaded" } else { "Unloaded" },
            chunk[0],
            chunk[1]
        );
    }
}

/// Whether the tick monitor wants AE2's level ticks timed.
pub(crate) fn monitoring() -> bool {
    TICK_MONITOR.load(Ordering::Relaxed)
}

/// One level tick of AE2's took `nanos`: a line in the log each second (20 ticks of every level).
pub(crate) fn tick_timed(nanos: u64) {
    let total = TICK_NANOS.fetch_add(nanos, Ordering::Relaxed) + nanos;
    let n = TICK_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if n >= 20 {
        TICK_NANOS.store(0, Ordering::Relaxed);
        TICK_COUNT.store(0, Ordering::Relaxed);
        eprintln!(
            "[AE2] tick monitor: {:.3} ms over {n} level ticks ({:.3} ms each)",
            total as f64 / 1e6,
            total as f64 / 1e6 / n as f64
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_mode_names() {
        assert_eq!(channel_mode("X3"), Some(3));
        assert_eq!(channel_mode("infinite"), Some(0));
        assert_eq!(channel_mode("x5"), None);
        assert_eq!(channel_mode_name(0), "infinite");
        assert_eq!(channel_mode_name(1), "default");
    }

    #[test]
    fn uuids_as_commands_take_them() {
        assert_eq!(
            uuid_text(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
            "01234567-89ab-cdef-0123-456789abcdef"
        );
        assert_eq!(uuid_text(1), "00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn synthetic_network_counts_its_types() {
        let lines = synth(2, 10);
        assert!(lines[0].starts_with("2 cells, 10 types"), "{}", lines[0]);
    }

    #[test]
    fn debug_handlers_register_once() {
        fn h(_: &mut dyn ServerHost, _: [f64; 3], _: &[&str]) -> Vec<String> {
            Vec::new()
        }
        add_debug("test_handler", h);
        add_debug("test_handler", h);
        assert_eq!(
            DEBUG
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, _)| *n == "test_handler")
                .count(),
            1
        );
    }
}
