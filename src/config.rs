//! AE2's options (AEConfig): `config/ae2/common.json` and `config/ae2/client.json`, AE2 15.4.12's sections, keys,
//! defaults, ranges and comments (ConfigSection's shape: a `<key>_comment` beside a commented key). Read through the
//! fork's `config_files` service when a server starts and when the client starts (`load`); a file that isn't JSON goes
//! aside as `<file>.bak` and the defaults are written (AEConfig.makeBackupAndReset); missing keys are written back.
//! The reader is upstream's (`../ae2-horizon/mixins/ae2/me.rs`, `ConfigReader`), nested in `load` so no
//! `serde_json` type is in an injection point's signature.

use mod_api::*;
use std::sync::{Arc, LazyLock, RwLock};

pub(crate) const COMMON_PATH: &str = "ae2/common.json";
pub(crate) const CLIENT_PATH: &str = "ae2/client.json";
pub(crate) const TERMINAL_STYLES: [&str; 4] = ["small", "medium", "tall", "full"];

#[derive(Clone)]
pub(crate) struct AeConfig {
    /// ChannelMode: 0 infinite, 1 default, 2..4 times.
    pub(crate) channels: u32,
    pub(crate) matter_cannon_block_damage: bool,
    pub(crate) tiny_tnt_block_damage: bool,
    pub(crate) formation_plane_entity_limit: i32,
    pub(crate) crafting_simulated_extraction: bool,
    /// Not AE2's: this port's own (`craftingCPU.craftingCpuPullsFromMachines`).
    pub(crate) machine_pull: bool,
    pub(crate) disassembly_crafting: bool,
    /// Ticks between two growth accelerator pulses.
    pub(crate) growth_accelerator_speed: i32,
    pub(crate) spatial_power_multiplier: f64,
    pub(crate) spatial_power_exponent: f64,
    pub(crate) spatial_anchor_random_ticks: bool,
    pub(crate) charger_charge_rate: f64,
    pub(crate) wireless_terminal_battery: i32,
    pub(crate) charged_staff_battery: i32,
    pub(crate) entropy_manipulator_battery: i32,
    pub(crate) portable_cell_battery: i32,
    pub(crate) color_applicator_battery: i32,
    pub(crate) matter_cannon_battery: i32,
    pub(crate) spawn_presses_in_meteorites: bool,
    pub(crate) spawn_flawless_only: bool,
    pub(crate) wireless_base_cost: f64,
    pub(crate) wireless_cost_multiplier: f64,
    pub(crate) wireless_base_range: f64,
    pub(crate) wireless_booster_range_multiplier: f64,
    pub(crate) wireless_booster_exp: f64,
    pub(crate) wireless_high_wireless_count: f64,
    pub(crate) wireless_terminal_drain_multiplier: f64,
    pub(crate) portable_cell_disassembly: bool,
    /// `PowerRatios.TechReborn`: AE for one E (FE on the fork).
    pub(crate) power_ratio_tech_reborn: f64,
    pub(crate) power_usage_multiplier: f64,
    pub(crate) grid_energy_storage_per_node: f64,
    pub(crate) crystal_resonance_generator_rate: f64,
    pub(crate) condenser_matter_balls: i32,
    pub(crate) condenser_singularity: i32,
    /// TickRates: (name, min, max), AnnihilationPlane etc.
    pub(crate) tick_rates: Vec<(String, i32, i32)>,
    pub(crate) vibration_base_energy: f64,
    pub(crate) vibration_min_energy: i32,
    pub(crate) vibration_max_energy: i32,
    // client.json
    pub(crate) enable_effects: bool,
    pub(crate) use_large_fonts: bool,
    pub(crate) use_colored_crafting_status: bool,
    pub(crate) show_placement_preview: bool,
    pub(crate) enable_facades_in_jei: bool,
    pub(crate) terminal_margin: i32,
    pub(crate) tooltip_show_cell_upgrades: bool,
    pub(crate) tooltip_show_cell_content: bool,
    pub(crate) tooltip_max_cell_content_shown: i32,
    pub(crate) enable_guide_hotkey: bool,
    /// `PowerUnit`: TR rather than AE.
    pub(crate) power_tr: bool,
    pub(crate) term_style: u8,
    pub(crate) settings: [bool; 9],
}

impl Default for AeConfig {
    fn default() -> Self {
        Self {
            channels: 1,
            matter_cannon_block_damage: true,
            tiny_tnt_block_damage: true,
            formation_plane_entity_limit: 128,
            crafting_simulated_extraction: false,
            machine_pull: false,
            disassembly_crafting: true,
            growth_accelerator_speed: 10,
            spatial_power_multiplier: 1250.0,
            spatial_power_exponent: 1.35,
            spatial_anchor_random_ticks: true,
            charger_charge_rate: 1.0,
            wireless_terminal_battery: 1_600_000,
            charged_staff_battery: 8000,
            entropy_manipulator_battery: 200_000,
            portable_cell_battery: 20_000,
            color_applicator_battery: 20_000,
            matter_cannon_battery: 200_000,
            spawn_presses_in_meteorites: true,
            spawn_flawless_only: false,
            wireless_base_cost: 8.0,
            wireless_cost_multiplier: 1.0,
            wireless_base_range: 16.0,
            wireless_booster_range_multiplier: 1.0,
            wireless_booster_exp: 1.5,
            wireless_high_wireless_count: 64.0,
            wireless_terminal_drain_multiplier: 1.0,
            portable_cell_disassembly: true,
            power_ratio_tech_reborn: 2.0,
            power_usage_multiplier: 1.0,
            grid_energy_storage_per_node: 25.0,
            crystal_resonance_generator_rate: 20.0,
            condenser_matter_balls: 256,
            condenser_singularity: 256_000,
            tick_rates: TICK_RATES.iter().map(|(n, a, b)| (n.to_string(), *a, *b)).collect(),
            vibration_base_energy: 5.0,
            vibration_min_energy: 4,
            vibration_max_energy: 40,
            enable_effects: true,
            use_large_fonts: false,
            use_colored_crafting_status: true,
            show_placement_preview: true,
            enable_facades_in_jei: true,
            terminal_margin: 25,
            tooltip_show_cell_upgrades: true,
            tooltip_show_cell_content: true,
            tooltip_max_cell_content_shown: 5,
            enable_guide_hotkey: true,
            power_tr: false,
            term_style: 0,
            settings: SETTINGS_DEFAULT,
        }
    }
}

impl AeConfig {
    /// AEConfig.wireless_getMaxRange.
    pub(crate) fn wireless_max_range(&self, boosters: f64) -> f64 {
        self.wireless_base_range + self.wireless_booster_range_multiplier * boosters.powf(self.wireless_booster_exp)
    }

    /// AEConfig.wireless_getPowerDrain.
    pub(crate) fn wireless_power_drain(&self, boosters: f64) -> f64 {
        self.wireless_base_cost + self.wireless_cost_multiplier * boosters.powf(1.0 + boosters / self.wireless_high_wireless_count)
    }

    /// TickRates `<name>Min`/`Max`.
    pub(crate) fn tick_rate(&self, name: &str) -> (i32, i32) {
        self.tick_rates.iter().find(|(n, _, _)| n == name).map(|(_, a, b)| (*a, *b)).unwrap_or((5, 20))
    }
}

/// TickRates: the name, and the slowest and fastest AE2 ticks it.
const TICK_RATES: [(&str, i32, i32); 12] = [
    ("Interface", 5, 120),
    ("ImportBus", 5, 40),
    ("ExportBus", 5, 60),
    ("AnnihilationPlane", 2, 120),
    ("METunnel", 5, 20),
    ("Inscriber", 1, 20),
    ("Charger", 10, 10),
    ("IOPort", 1, 5),
    ("VibrationChamber", 10, 40),
    ("StorageBus", 5, 60),
    ("ItemTunnel", 5, 60),
    ("LightTunnel", 5, 60),
];

const SETTINGS_DEFAULT: [bool; 9] = [false, true, true, true, true, false, false, true, true];
/// The terminal settings' places in `AeConfig::settings` (TerminalSettingsScreen's options).
pub(crate) const S_AUTO_FOCUS: usize = 0;
pub(crate) const S_REMEMBER_SEARCH: usize = 1;
pub(crate) const S_PIN: usize = 3;
pub(crate) const S_NOTIFY: usize = 4;
pub(crate) const S_CLEAR_GRID: usize = 5;
pub(crate) const S_EXTERNAL_SEARCH: usize = 6;
pub(crate) const S_SYNC_EXTERNAL: usize = 7;
pub(crate) const S_CLEAR_EXTERNAL: usize = 8;
/// client.json's `PowerUnit` names (AE2's PowerUnits as its config writes them; `tr` is the other unit, shown as FE
/// here: the fork's energy).
const POWER_UNITS: [&str; 2] = ["ae", "tr"];
/// The terminal settings' place in client.json, and AE2's comment on each (2 is nothing's).
const SETTINGS_KEYS: [(&str, &str, &str); 9] = [
    ("search", "autoFocusSearch", "Automatically focuses the search field when the terminal opens"),
    ("search", "rememberLastSearch", "Remembers the last search term and restores it when the terminal opens"),
    ("", "", ""),
    ("terminals", "pinAutoCraftedItems", "Pin items that the player auto-crafts to the top of the terminal"),
    ("client", "notifyForFinishedCraftingJobs", "Show toast when long-running crafting jobs finish."),
    ("client", "clearGridOnClose", "Automatically clear the crafting/encoding grid when closing the terminal"),
    ("search", "useExternalSearch", "Replaces AEs own search with the search of REI or JEI"),
    ("search", "syncWithExternalSearch", "When REI/JEI is installed, automatically set the AE or REI/JEI search text when either is changed while the terminal is open"),
    ("search", "clearExternalSearchOnOpen", "When using useExternalSearch, clears the search when the terminal opens"),
];

static CONFIG: LazyLock<RwLock<Arc<AeConfig>>> = LazyLock::new(|| RwLock::new(Arc::new(AeConfig::default())));

pub(crate) fn config() -> Arc<AeConfig> {
    CONFIG.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// AEConfig's constructor and `/ae2 reloadconfig`: both files read (defaults and missing keys written back), the
/// options applied (the channel mode at once; the rest are read where they're used).
pub(crate) fn load(host: &dyn Host) {
    use mod_api::serde_json::{Map, Value};

    struct Reader<'a> {
        files: &'a dyn ConfigFiles,
        path: &'static str,
        had: Value,
        fresh: bool,
        out: Map<String, Value>,
    }

    impl<'a> Reader<'a> {
        fn open(files: &'a dyn ConfigFiles, path: &'static str) -> Self {
            let empty = Value::Object(Map::new());
            let cp = ConfigPath::new(path).ok();
            let bytes = cp.as_ref().and_then(|p| files.read(p).ok());
            let (had, fresh) = match bytes.map(|b| mod_api::serde_json::from_slice::<Value>(&b)) {
                None => (empty, true),
                Some(Ok(doc)) if doc.is_object() => (doc, false),
                Some(_) => {
                    // AEConfig.makeBackupAndReset: the broken file kept beside, the defaults used.
                    if let (Some(p), Some(bak)) = (cp.as_ref(), ConfigPath::new(&format!("{path}.bak")).ok()) {
                        if let Ok(b) = files.read(p) {
                            let _ = files.write(&bak, &b);
                        }
                    }
                    crate::util::debug(&format!("config: {path} isn't a JSON object: kept as {path}.bak, defaults used"));
                    (empty, true)
                }
            };
            Self { files, path, had, fresh, out: Map::new() }
        }

        fn section(&mut self, section: &str) -> Option<&mut Map<String, Value>> {
            self.out.entry(section.to_owned()).or_insert_with(|| Value::Object(Map::new())).as_object_mut()
        }

        fn note(&mut self, section: &str, comment: &str) {
            if let Some(s) = self.section(section) {
                s.insert("_comment".to_owned(), Value::from(comment));
            }
        }

        fn read<T: Copy>(&mut self, section: &str, key: &str, default: T, comment: &str, parse: impl Fn(&Value) -> Option<T>, write: impl Fn(T) -> Value) -> T {
            let value = match self.had.get(section).and_then(|s| s.get(key)) {
                Some(v) => parse(v).unwrap_or(default),
                None => default,
            };
            if let Some(s) = self.section(section) {
                if !comment.is_empty() {
                    s.insert(format!("{key}_comment"), Value::from(comment));
                }
                s.insert(key.to_owned(), write(value));
            }
            value
        }

        fn boolean(&mut self, section: &str, key: &str, default: bool, comment: &str) -> bool {
            self.read(section, key, default, comment, |v| v.as_bool(), Value::from)
        }

        fn int_in(&mut self, section: &str, key: &str, default: i32, min: i32, max: i32, comment: &str) -> i32 {
            let parse = |v: &Value| {
                let n = v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))?;
                i32::try_from(n).ok().filter(|n| (min..=max).contains(n))
            };
            self.read(section, key, default, comment, parse, Value::from)
        }

        fn int(&mut self, section: &str, key: &str, default: i32) -> i32 {
            self.int_in(section, key, default, i32::MIN, i32::MAX, "")
        }

        fn double_in(&mut self, section: &str, key: &str, default: f64, min: f64, max: f64, comment: &str) -> f64 {
            let parse = |v: &Value| v.as_f64().filter(|n| n.is_finite() && (min..=max).contains(n));
            self.read(section, key, default, comment, parse, Value::from)
        }

        fn double(&mut self, section: &str, key: &str, default: f64) -> f64 {
            // ConfigSection.addDouble without a range: above Double.MIN_VALUE.
            self.double_in(section, key, default, 5e-324, f64::MAX, "")
        }

        /// EnumOption: one of `names`, written in lower case, read in any case.
        fn choice(&mut self, section: &str, key: &str, default: usize, names: &[&'static str], comment: &str) -> usize {
            let parse = |v: &Value| {
                let text = v.as_str()?;
                names.iter().find(|n| n.eq_ignore_ascii_case(text)).copied()
            };
            let name = self.read(section, key, names[default], comment, parse, Value::from);
            names.iter().position(|n| *n == name).unwrap_or(default)
        }

        /// ConfigFileManager.save, unless the file already holds exactly this.
        fn finish(self) {
            let out = Value::Object(self.out);
            if self.fresh || out != self.had {
                if let (Ok(p), Ok(text)) = (ConfigPath::new(self.path), mod_api::serde_json::to_string_pretty(&out)) {
                    let _ = self.files.write(&p, text.as_bytes());
                }
            }
        }
    }

    let files = host.config_files();
    let mut common = Reader::open(files, COMMON_PATH);
    let mut client = Reader::open(files, CLIENT_PATH);

    // In the file as in AE2, with nothing to do in this port.
    common.boolean("general", "unsupportedDeveloperTools", false, "");
    let spatial_anchor_random_ticks = common.boolean("general", "spatialAnchorEnableRandomTicks", true, "Whether Spatial Anchors should force random chunk ticks and entity spawning.");
    common.boolean("facades", "allowBlockEntities", false, "Unsupported: Allows whitelisting block entities as facades. Could work, have render issues, or corrupt your world. USE AT YOUR OWN RISK.");
    common.int("craftingCPU", "craftingCalculationTimePerTick", 5);
    for key in ["blockUpdateLog", "packetLog", "craftingLog", "debugLog", "gridLog"] {
        common.boolean("logging", key, false, "");
    }
    common.boolean("logging", "chunkLoggerTrace", false, "Enable stack trace logging for the chunk loading debug command");
    common.note("tickRates", " Min / Max Tickrates for dynamic ticking, most of these components also use sleeping, to prevent constant ticking, adjust with care, non standard rates are not supported or tested.");
    let mut tick_rates = Vec::new();
    for (name, min, max) in TICK_RATES {
        let a = common.int("tickRates", &format!("{name}Min"), min);
        let b = common.int("tickRates", &format!("{name}Max"), max);
        tick_rates.push((name.to_owned(), a, b));
    }
    client.boolean("client", "disableColoredCableRecipesInJEI", true, "");
    let enable_facades_in_jei = client.boolean("client", "enableFacadesInJEI", true, "Show facades in JEI ingredient list");
    client.boolean("client", "enableFacadeRecipesInJEI", true, "Show facade recipes in JEI for supported blocks");
    client.boolean("client", "exposeInventoryToEmi", false, "Expose the full network inventory to EMI, which might cause performance problems.");
    client.boolean("client", "showDebugGuiOverlays", false, "Show debugging GUI overlays");
    let show_placement_preview = client.boolean("client", "showPlacementPreview", true, "Show a preview of part and facade placement");
    client.boolean("search", "searchModNameInTooltips", false, "Should the mod name be included when searching in tooltips.");
    let mut settings = SETTINGS_DEFAULT;
    for (i, (section, key, comment)) in SETTINGS_KEYS.iter().enumerate() {
        if !key.is_empty() {
            settings[i] = client.boolean(section, key, SETTINGS_DEFAULT[i], comment);
        }
    }
    common.note("vibrationChamber", "Settings for the Vibration Chamber");
    let vibration_min_energy = common.int_in("vibrationChamber", "minEnergyPerGameTick", 4, 0, 1000, "Minimum amount of AE/t the vibration chamber can slow down to when energy is being wasted.");
    let vibration_max_energy = common.int_in("vibrationChamber", "baseMaxEnergyPerGameTick", 40, 1, 1000, "Maximum amount of AE/t the vibration chamber can speed up to when generated energy is being fully consumed.");

    let cfg = AeConfig {
        channels: common.choice("general", "channels", 1, &["infinite", "default", "x2", "x3", "x4"], "Changes the channel capacity that cables provide in AE2.") as u32,
        matter_cannon_block_damage: common.boolean("general", "matterCannonBlockDamage", true, "Enables the ability of the Matter Cannon to break blocks."),
        tiny_tnt_block_damage: common.boolean("general", "tinyTntBlockDamage", true, "Enables the ability of Tiny TNT to break blocks."),
        formation_plane_entity_limit: common.int("automation", "formationPlaneEntityLimit", 128),
        crafting_simulated_extraction: common.boolean("craftingCPU", "craftingSimulatedExtraction", false, "When true: simulate extraction of all the network's contents when starting a crafting job calculation. When false: use the cached available content list (same as terminals). Enabling might work a bit better, but it will significantly reduce performance."),
        machine_pull: common.boolean("craftingCPU", "craftingCpuPullsFromMachines", false, "Not an AE2 option, this port's own. When true: crafting CPUs fetch what a pattern's machine made from that machine. When false, as in AE2: the output has to find its own way back."),
        disassembly_crafting: common.boolean("crafting", "disassemblyCrafting", true, "Enable shift-clicking with the crafting units in hand to disassemble them."),
        growth_accelerator_speed: common.int_in("crafting", "growthAccelerator", 10, 1, 100, "Number of ticks between two crystal growth accelerator ticks"),
        spatial_power_multiplier: common.double("spatialio", "spatialPowerMultiplier", 1250.0),
        spatial_power_exponent: common.double("spatialio", "spatialPowerExponent", 1.35),
        spatial_anchor_random_ticks,
        charger_charge_rate: common.double_in("battery", "chargerChargeRate", 1.0, 0.1, 10.0, "The chargers charging rate factor, which is applied to the charged items charge rate. 2 means it charges everything twice as fast. 0.5 half as fast."),
        wireless_terminal_battery: common.int_in("battery", "wirelessTerminal", 1_600_000, 0, i32::MAX, ""),
        charged_staff_battery: common.int_in("battery", "chargedStaff", 8000, 0, i32::MAX, ""),
        entropy_manipulator_battery: common.int_in("battery", "entropyManipulator", 200_000, 0, i32::MAX, ""),
        portable_cell_battery: common.int_in("battery", "portableCell", 20_000, 0, i32::MAX, ""),
        color_applicator_battery: common.int_in("battery", "colorApplicator", 20_000, 0, i32::MAX, ""),
        matter_cannon_battery: common.int_in("battery", "matterCannon", 200_000, 0, i32::MAX, ""),
        spawn_presses_in_meteorites: common.boolean("worldGen", "spawnPressesInMeteorites", true, ""),
        spawn_flawless_only: common.boolean("worldGen", "spawnFlawlessOnly", false, ""),
        wireless_base_cost: common.double("wireless", "wirelessBaseCost", 8.0),
        wireless_cost_multiplier: common.double("wireless", "wirelessCostMultiplier", 1.0),
        wireless_base_range: common.double("wireless", "wirelessBaseRange", 16.0),
        wireless_booster_range_multiplier: common.double("wireless", "wirelessBoosterRangeMultiplier", 1.0),
        wireless_booster_exp: common.double("wireless", "wirelessBoosterExp", 1.5),
        wireless_high_wireless_count: common.double("wireless", "wirelessHighWirelessCount", 64.0),
        wireless_terminal_drain_multiplier: common.double("wireless", "wirelessTerminalDrainMultiplier", 1.0),
        portable_cell_disassembly: common.boolean("PortableCells", "allowDisassembly", true, "Allow disassembly of portable cells into the recipe ingredients using shift+right-click"),
        power_ratio_tech_reborn: common.double("PowerRatios", "TechReborn", 2.0),
        power_usage_multiplier: common.double_in("PowerRatios", "UsageMultiplier", 1.0, 0.01, f64::MAX, ""),
        grid_energy_storage_per_node: common.double_in("PowerRatios", "GridEnergyStoragePerNode", 25.0, 1.0, 1_000_000.0, "How much energy can the internal grid buffer storage per node attached to the grid."),
        crystal_resonance_generator_rate: common.double_in("PowerRatios", "CrystalResonanceGeneratorRate", 20.0, 0.0, 1_000_000.0, "How much energy a crystal resonance generator generates per tick."),
        condenser_matter_balls: common.int("Condenser", "MatterBalls", 256),
        condenser_singularity: common.int("Condenser", "Singularity", 256_000),
        tick_rates,
        vibration_base_energy: common.double_in("vibrationChamber", "baseEnergyPerFuelTick", 5.0, 0.1, 1000.0, "AE energy produced per fuel burn tick (reminder: coal = 1600, block of coal = 16000, lava bucket = 20000 burn ticks)"),
        vibration_min_energy,
        vibration_max_energy: vibration_max_energy.max(vibration_min_energy),
        enable_effects: client.boolean("client", "enableEffects", true, ""),
        use_large_fonts: client.boolean("client", "useTerminalUseLargeFont", false, ""),
        use_colored_crafting_status: client.boolean("client", "useColoredCraftingStatus", true, ""),
        show_placement_preview,
        enable_facades_in_jei,
        terminal_margin: client.int_in("client", "terminalMargin", 25, i32::MIN, i32::MAX, "The vertical margin to apply when sizing terminals. Used to make room for centered item mod search bars"),
        tooltip_show_cell_upgrades: client.boolean("tooltips", "showCellUpgrades", true, "Show installed upgrades in the tooltips of storage cells, color applicators and matter cannons"),
        tooltip_show_cell_content: client.boolean("tooltips", "showCellContent", true, "Show a preview of the content in the tooltips of storage cells, color applicators and matter cannons"),
        tooltip_max_cell_content_shown: client.int_in("tooltips", "maxCellContentShown", 5, 1, 32, "The maximum number of content entries to show in the tooltip of storage cells, color applicators and matter cannons"),
        enable_guide_hotkey: client.boolean("tooltips", "enableGuideHotkey", true, "Enables the 'hold key to show guide' functionality in tooltips"),
        power_tr: client.choice("client", "PowerUnit", 0, &POWER_UNITS, "Power unit shown in AE UIs") == 1,
        term_style: client.choice("terminals", "terminalStyle", 0, &TERMINAL_STYLES, "") as u8,
        settings,
    };
    common.finish();
    client.finish();
    crate::grid::CHANNEL_MODE.store(cfg.channels, std::sync::atomic::Ordering::Relaxed);
    crate::util::debug(&format!("config: channels {} growthAccelerator {} spatialPowerMultiplier {}", cfg.channels, cfg.growth_accelerator_speed, cfg.spatial_power_multiplier));
    *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(cfg);
}

/// The client's own options changed in a screen (the terminal settings, the power unit): applied at once and saved into
/// client.json (AE2's options save as they are set), the file's other keys kept as they are.
pub(crate) fn update_client(host: &dyn Host, change: impl FnOnce(&mut AeConfig)) {
    let cfg = {
        let mut guard = CONFIG.write().unwrap_or_else(|e| e.into_inner());
        let mut cfg = (**guard).clone();
        change(&mut cfg);
        *guard = Arc::new(cfg.clone());
        cfg
    };
    let files = host.config_files();
    let Ok(path) = ConfigPath::new(CLIENT_PATH) else { return };
    let mut doc = files
        .read(&path)
        .ok()
        .and_then(|b| mod_api::serde_json::from_slice::<mod_api::serde_json::Value>(&b).ok())
        .filter(mod_api::serde_json::Value::is_object)
        .unwrap_or_else(|| mod_api::serde_json::Value::Object(mod_api::serde_json::Map::new()));
    write_client_doc(&mut doc, &cfg.settings, cfg.power_tr);
    if let Ok(text) = mod_api::serde_json::to_string_pretty(&doc) {
        let _ = files.write(&path, text.as_bytes());
    }
}

/// The terminal settings and the power unit into client.json's document (each in its section, as `load` reads it).
pub(crate) fn write_client_doc(doc: &mut mod_api::serde_json::Value, settings: &[bool; 9], power_tr: bool) {
    use mod_api::serde_json::{Map, Value};
    let Some(root) = doc.as_object_mut() else { return };
    let mut set = |section: &str, key: &str, value: Value| {
        let s = root.entry(section.to_owned()).or_insert_with(|| Value::Object(Map::new()));
        if !s.is_object() {
            *s = Value::Object(Map::new());
        }
        if let Some(s) = s.as_object_mut() {
            s.insert(key.to_owned(), value);
        }
    };
    for (i, (section, key, _)) in SETTINGS_KEYS.iter().enumerate() {
        if !key.is_empty() {
            set(section, key, Value::from(settings[i]));
        }
    }
    set("client", "PowerUnit", Value::from(POWER_UNITS[usize::from(power_tr)]));
}

/// The terminal settings and the power unit as client.json's document has them (defaults for what it lacks).
pub(crate) fn read_client_doc(doc: &mod_api::serde_json::Value) -> ([bool; 9], bool) {
    let mut settings = SETTINGS_DEFAULT;
    for (i, (section, key, _)) in SETTINGS_KEYS.iter().enumerate() {
        if let Some(v) = doc.get(section).and_then(|s| s.get(key)).and_then(mod_api::serde_json::Value::as_bool) {
            settings[i] = v;
        }
    }
    let power_tr = doc.get("client").and_then(|s| s.get("PowerUnit")).and_then(|v| v.as_str()).is_some_and(|u| u.eq_ignore_ascii_case(POWER_UNITS[1]));
    (settings, power_tr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mod_api::serde_json::{Value, json};

    #[test]
    fn settings_round_trip() {
        let mut doc = json!({"client": {"enableEffects": true, "PowerUnit": "ae"}, "search": {"searchModNameInTooltips": false}});
        assert_eq!(read_client_doc(&doc), (SETTINGS_DEFAULT, false));
        let mut s = SETTINGS_DEFAULT;
        s[S_AUTO_FOCUS] = true;
        s[S_PIN] = false;
        s[S_CLEAR_GRID] = true;
        s[S_EXTERNAL_SEARCH] = true;
        write_client_doc(&mut doc, &s, true);
        assert_eq!(read_client_doc(&doc), (s, true));
        // The file's other keys stay.
        assert_eq!(doc["client"]["enableEffects"], Value::from(true));
        assert_eq!(doc["search"]["searchModNameInTooltips"], Value::from(false));
        assert_eq!(doc["terminals"]["pinAutoCraftedItems"], Value::from(false));
        assert_eq!(doc["client"]["PowerUnit"], Value::from("tr"));
    }

    #[test]
    fn broken_sections_replaced() {
        let mut doc = json!({"search": 5});
        write_client_doc(&mut doc, &SETTINGS_DEFAULT, false);
        assert_eq!(read_client_doc(&doc), (SETTINGS_DEFAULT, false));
        assert!(doc["search"].is_object());
    }
}
