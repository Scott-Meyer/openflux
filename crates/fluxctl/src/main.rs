//! `fluxctl`: 100% CLI parity with OpenFlux Studio for humans and AI coding agents.
//!
//! Every workflow—module scaffolding, forking curated defaults, inspecting install/update
//! disclosures, toggling stackable modules, editing configs, managing curated lists and
//! secret-free presets, publishing/subscribing on the hierarchical bus, simulating analog key
//! presses & impulses, and capturing live GPU-rendered preview frames—is scriptable with `--json`.

use clap::{Parser, Subcommand, ValueEnum};
use openflux_bus::{
    topics, AnalogKeyFrame, AnalogKeySample, Broker, FrameLayout, HostConnectionState, KeyEvent,
    KeyEventKind, KeyboardGeometry, MediaNowPlaying, PublishOptions, SurfaceImpulse, SystemMetrics,
    WeatherState,
};
use openflux_compositor::{Compositor, CompositorOptions};
use openflux_modules::{
    inspect_module_dir, CuratedListDocument, CuratedListSubscription, ModuleCategory,
    ModuleInstanceState, ModuleRegistry, ModuleRuntime, SetupPreset,
};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "fluxctl", version, about = "OpenFlux v2 command-line interface")]
struct Cli {
    /// Emit machine-readable JSON output for scripts and AI coding agents.
    #[arg(long, global = true)]
    json: bool,

    /// Base data directory for installed/local modules and state.
    #[arg(long, global = true, env = "OPENFLUX_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Bundled curated default modules directory.
    #[arg(long, global = true, env = "OPENFLUX_BUNDLED_MODULES_DIR")]
    bundled_dir: Option<PathBuf>,

    /// Optional openfluxd base URL for live daemon operations (e.g. https://127.0.0.1:52323).
    #[arg(long, global = true, env = "OPENFLUX_DAEMON_URL")]
    daemon: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show environment paths, discovered module counts, and GPU compositor backend info.
    Status,

    /// Manage modules: list, info, create, fork, inspect, install, enable, disable, slot, config, validate.
    Modules {
        #[command(subcommand)]
        cmd: ModulesCmd,
    },

    /// Manage shareable curated lists (collections) of modules.
    Lists {
        #[command(subcommand)]
        cmd: ListsCmd,
    },

    /// Export or import shareable setups/presets (automatically strips secret fields).
    Preset {
        #[command(subcommand)]
        cmd: PresetCmd,
    },

    /// Interact with the hierarchical pub/sub bus on a running openfluxd daemon.
    Bus {
        #[command(subcommand)]
        cmd: BusCmd,
    },

    /// Inject synthetic key presses, analog travel/velocity samples, or spatial surface impulses.
    Simulate {
        #[command(subcommand)]
        cmd: SimulateCmd,
    },

    /// Render real GPU-composited frames headlessly (identical pipeline to Studio and the keyboard).
    Preview {
        #[command(subcommand)]
        cmd: PreviewCmd,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum TemplateArg {
    Effect,
    Widget,
    Companion,
    Background,
    HostService,
}

impl From<TemplateArg> for ModuleCategory {
    fn from(v: TemplateArg) -> Self {
        match v {
            TemplateArg::Effect => ModuleCategory::Effect,
            TemplateArg::Widget => ModuleCategory::Widget,
            TemplateArg::Companion => ModuleCategory::Companion,
            TemplateArg::Background => ModuleCategory::Background,
            TemplateArg::HostService => ModuleCategory::HostService,
        }
    }
}

#[derive(Subcommand)]
enum ModulesCmd {
    /// List all discovered modules across bundled, installed, and local tiers.
    List {
        #[arg(long)]
        category: Option<TemplateArg>,
        #[arg(long)]
        enabled_only: bool,
    },
    /// Show detailed manifest, state, and disclosure inspection for a module.
    Info { id: String },
    /// Scaffold a brand-new plain-folder module in the local modules directory.
    Create {
        id: String,
        #[arg(long, value_enum, default_value = "effect")]
        template: TemplateArg,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        enable: bool,
        #[arg(long)]
        slot: Option<u8>,
    },
    /// Duplicate/fork any bundled or installed module into a safe local copy.
    Fork {
        source_id: String,
        new_id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        enable: bool,
    },
    /// Inspect a module folder and print its disclosure report (and update diff if already installed).
    Inspect { path: PathBuf },
    /// Install or update a module from a local directory after reviewing disclosure.
    Install {
        path: PathBuf,
        /// Show disclosure/diff only without installing.
        #[arg(long)]
        inspect_only: bool,
        /// Non-interactively acknowledge the disclosure report and proceed with installation.
        #[arg(long)]
        accept_disclosure: bool,
    },
    /// Enable a module (optionally assigning a top-bar slot 0..3).
    Enable {
        id: String,
        #[arg(long)]
        slot: Option<u8>,
    },
    /// Disable a module.
    Disable { id: String },
    /// Assign or clear a top-bar slot (0..3) for a slot-capable module.
    Slot {
        id: String,
        #[arg(long)]
        slot: Option<u8>,
        #[arg(long)]
        clear: bool,
    },
    /// Get or set a module's configuration values.
    Config {
        id: String,
        /// Set key=json_or_string pairs (e.g. --set strength=1.5 --set color=#ff6600).
        #[arg(long = "set")]
        sets: Vec<String>,
    },
    /// Validate a module directory's manifest and Lua script syntax.
    Validate { path: PathBuf },
}

#[derive(Subcommand)]
enum ListsCmd {
    /// List subscribed curated module lists.
    List,
    /// Add a curated list subscription (local JSON file path or URL).
    Add {
        id: String,
        name: String,
        source: String,
    },
    /// Remove a curated list subscription by id.
    Remove { id: String },
    /// Browse entries inside a subscribed curated list (or direct file path).
    Browse { id_or_path: String },
}

#[derive(Subcommand)]
enum PresetCmd {
    /// Export the current enabled modules, slots, and non-secret settings to a JSON preset file.
    Export {
        out: PathBuf,
        #[arg(long, default_value = "My OpenFlux Setup")]
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Import a preset JSON file into local module state (preserving any local secret fields).
    Import { path: PathBuf },
}

#[derive(Subcommand)]
enum BusCmd {
    /// Publish a JSON payload to a topic on openfluxd.
    Pub {
        topic: String,
        payload: String,
        #[arg(long)]
        retain: bool,
    },
    /// Query retained messages matching an MQTT wildcard pattern on openfluxd.
    Get {
        #[arg(default_value = "#")]
        pattern: String,
    },
}

#[derive(Subcommand)]
enum SimulateCmd {
    /// Send a synthetic key press/release or analog depth/velocity sample to openfluxd.
    Key {
        key_index: u16,
        #[arg(long, default_value_t = 2200)]
        depth_um: u16,
        #[arg(long, default_value_t = 12000)]
        velocity_um_s: i32,
        #[arg(long)]
        release: bool,
    },
    /// Publish a spatial surface impulse onto `fx/surface/impulse`.
    Impulse {
        #[arg(long)]
        x: f32,
        #[arg(long)]
        y: f32,
        #[arg(long, default_value_t = 65.0)]
        radius: f32,
        #[arg(long, default_value_t = 1.1)]
        force: f32,
    },
}

#[derive(Subcommand)]
enum PreviewCmd {
    /// Render GPU frames headlessly with current (or specified) modules and synthetic test events.
    Render {
        /// Output PNG path.
        #[arg(long, short)]
        out: PathBuf,
        /// Optional explicit list of module IDs to enable for this render (overrides saved state).
        #[arg(long = "module")]
        modules: Vec<String>,
        /// Optional slot assignments as `id=slot` (e.g. `--slot widget-clock=0`).
        #[arg(long = "slot")]
        slots: Vec<String>,
        /// Number of frames to simulate before saving the final PNG.
        #[arg(long, default_value_t = 45)]
        frames: u32,
        /// Frame step in milliseconds.
        #[arg(long, default_value_t = 16.667)]
        step_ms: f32,
        /// Output width in pixels.
        #[arg(long, default_value_t = 1920)]
        width: u32,
        /// Output height in pixels.
        #[arg(long, default_value_t = 1080)]
        height: u32,
        /// Keyboard physical layout (`ansi` or `iso`).
        #[arg(long, default_value = "ansi")]
        layout: String,
        /// Demo scenario to inject while stepping (`none`, `typing`, `analog`, `showcase`, `idle-pet`).
        #[arg(long, default_value = "showcase")]
        scenario: String,
        /// Optional JSON events script file (`[{ "frame": 10, "topic": "...", "payload": {...} }]`).
        #[arg(long)]
        events_file: Option<PathBuf>,
        /// Hide default keycap legend overlay.
        #[arg(long)]
        no_legends: bool,
    },
}

fn default_data_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local/share/openflux")
    } else {
        PathBuf::from(".openflux")
    }
}

fn resolve_bundled_dir(cli_dir: Option<&PathBuf>) -> PathBuf {
    if let Some(d) = cli_dir {
        return d.clone();
    }
    let candidates = [
        PathBuf::from("modules"),
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.join("modules")))
            .unwrap_or_default(),
    ];
    for c in candidates {
        if c.is_dir() {
            return c;
        }
    }
    PathBuf::from("modules")
}

fn load_bundled_font() -> Option<Vec<u8>> {
    let candidates = [
        "assets/bundled_fonts/Montserrat-VariableFont_wght.ttf",
        "dist/assets/fonts/Montserrat-VariableFont_wght.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    for p in candidates {
        if let Ok(bytes) = std::fs::read(p) {
            return Some(bytes);
        }
    }
    None
}

fn make_registry(cli: &Cli) -> ModuleRegistry {
    let data = cli.data_dir.clone().unwrap_or_else(default_data_dir);
    let bundled = resolve_bundled_dir(cli.bundled_dir.as_ref());
    ModuleRegistry::new(
        bundled,
        data.join("v2/modules/installed"),
        data.join("v2/modules/local"),
        data.join("v2/modules_state.json"),
    )
}

fn print_out(json_mode: bool, value: &Value, human: &str) {
    if json_mode {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
    } else {
        println!("{human}");
    }
}

fn parse_setting_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn daemon_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())
}

fn daemon_base(cli: &Cli) -> String {
    cli.daemon
        .clone()
        .unwrap_or_else(|| "https://127.0.0.1:52323".to_string())
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let reg = make_registry(&cli);
    match &cli.command {
        Commands::Status => {
            let discovered = reg.discover();
            let state = reg.load_state();
            let enabled_count = state.modules.values().filter(|m| m.enabled).count();
            let geom = KeyboardGeometry::build(FrameLayout::Ansi);
            let gpu_info = Compositor::new_headless(
                geom,
                None,
                CompositorOptions {
                    width: 320,
                    height: 180,
                    draw_default_legends: false,
                },
            )
            .map(|c| c.adapter_summary())
            .unwrap_or_else(|e| format!("unavailable ({e})"));
            let val = serde_json::json!({
                "bundled_dir": reg.bundled_dir,
                "installed_dir": reg.installed_dir,
                "local_dir": reg.local_dir,
                "state_path": reg.state_path,
                "discovered_modules": discovered.len(),
                "enabled_modules": enabled_count,
                "gpu_adapter": gpu_info,
            });
            print_out(
                cli.json,
                &val,
                &format!(
                    "OpenFlux v2 CLI\n  Bundled modules:   {}\n  Installed modules: {}\n  Local modules:     {}\n  Discovered: {} ({} enabled)\n  GPU Compositor:    {}",
                    reg.bundled_dir.display(),
                    reg.installed_dir.display(),
                    reg.local_dir.display(),
                    discovered.len(),
                    enabled_count,
                    gpu_info
                ),
            );
            Ok(())
        }

        Commands::Modules { cmd } => handle_modules(&cli, &reg, cmd),
        Commands::Lists { cmd } => handle_lists(&cli, &reg, cmd),
        Commands::Preset { cmd } => handle_preset(&cli, &reg, cmd),
        Commands::Bus { cmd } => handle_bus(&cli, cmd),
        Commands::Simulate { cmd } => handle_simulate(&cli, cmd),
        Commands::Preview { cmd } => handle_preview(&cli, &reg, cmd),
    }
}

fn handle_modules(cli: &Cli, reg: &ModuleRegistry, cmd: &ModulesCmd) -> Result<(), String> {
    match cmd {
        ModulesCmd::List {
            category,
            enabled_only,
        } => {
            let discovered = reg.discover();
            let state = reg.load_state();
            let filter_cat = category.map(ModuleCategory::from);
            let mut items = Vec::new();
            let mut lines = Vec::new();
            for (id, dm) in &discovered {
                if let Some(cat) = filter_cat {
                    if dm.manifest.category != cat {
                        continue;
                    }
                }
                let inst = state.modules.get(id);
                let enabled = inst.is_some_and(|s| s.enabled);
                if *enabled_only && !enabled {
                    continue;
                }
                let slot = inst.and_then(|s| s.slot);
                items.push(serde_json::json!({
                    "id": id,
                    "name": dm.manifest.name,
                    "version": dm.manifest.version,
                    "category": dm.manifest.category,
                    "tier": dm.tier,
                    "enabled": enabled,
                    "slot": slot,
                    "host_requirement": dm.manifest.host_requirement,
                    "stackable": dm.manifest.stackable,
                    "path": dm.path,
                }));
                let status = if enabled { "ON " } else { "off" };
                let slot_str = slot.map(|s| format!(" [slot {s}]")).unwrap_or_default();
                lines.push(format!(
                    "[{status}] {:24} {:10} {:9} v{}{}",
                    id,
                    format!("{:?}", dm.manifest.category).to_lowercase(),
                    format!("{:?}", dm.tier).to_lowercase(),
                    dm.manifest.version,
                    slot_str
                ));
            }
            print_out(
                cli.json,
                &serde_json::json!({ "modules": items }),
                &lines.join("\n"),
            );
            Ok(())
        }
        ModulesCmd::Info { id } => {
            let discovered = reg.discover();
            let dm = discovered
                .get(id)
                .ok_or_else(|| format!("module {id:?} not found"))?;
            let state = reg.load_state();
            let disclosure = inspect_module_dir(&dm.path)?;
            let val = serde_json::json!({
                "module": dm,
                "state": state.modules.get(id),
                "disclosure": disclosure,
            });
            print_out(cli.json, &val, &disclosure.summary_lines.join("\n"));
            Ok(())
        }
        ModulesCmd::Create {
            id,
            template,
            name,
            enable,
            slot,
        } => {
            let dm = reg.scaffold_local(id, (*template).into(), name.as_deref())?;
            if *enable || slot.is_some() {
                let mut state = reg.load_state();
                let entry = state.modules.entry(id.clone()).or_default();
                entry.enabled = *enable || entry.enabled;
                if let Some(s) = slot {
                    entry.slot = Some(*s);
                }
                reg.save_state(&state)?;
            }
            let val = serde_json::json!({ "created": dm });
            print_out(
                cli.json,
                &val,
                &format!("Created local module {} at {}", dm.manifest.id, dm.path.display()),
            );
            Ok(())
        }
        ModulesCmd::Fork {
            source_id,
            new_id,
            name,
            enable,
        } => {
            let dm = reg.fork_module(source_id, new_id, name.as_deref())?;
            if *enable {
                let mut state = reg.load_state();
                let entry = state.modules.entry(new_id.clone()).or_default();
                entry.enabled = true;
                reg.save_state(&state)?;
            }
            let val = serde_json::json!({ "forked": dm });
            print_out(
                cli.json,
                &val,
                &format!(
                    "Forked {} -> local module {} at {}",
                    source_id,
                    dm.manifest.id,
                    dm.path.display()
                ),
            );
            Ok(())
        }
        ModulesCmd::Inspect { path } => {
            let preview = reg.preview_install(path)?;
            let val = serde_json::to_value(&preview).map_err(|e| e.to_string())?;
            let mut lines = preview.disclosure.summary_lines.clone();
            for w in &preview.disclosure.warnings {
                lines.push(format!("  WARNING: {w}"));
            }
            if let Some(diff) = &preview.update_diff {
                lines.push("--- Update Diff ---".into());
                lines.extend(diff.summary_lines.clone());
            }
            print_out(cli.json, &val, &lines.join("\n"));
            Ok(())
        }
        ModulesCmd::Install {
            path,
            inspect_only,
            accept_disclosure,
        } => {
            let preview = reg.preview_install(path)?;
            if *inspect_only || !*accept_disclosure {
                let val = serde_json::json!({
                    "installed": false,
                    "requires_acceptance": !*inspect_only,
                    "preview": preview,
                });
                let mut lines = preview.disclosure.summary_lines.clone();
                for w in &preview.disclosure.warnings {
                    lines.push(format!("  WARNING: {w}"));
                }
                if let Some(diff) = &preview.update_diff {
                    lines.push("--- Update Diff ---".into());
                    lines.extend(diff.summary_lines.clone());
                }
                if !*inspect_only {
                    lines.push(
                        "\nPass --accept-disclosure to confirm and install this module.".into(),
                    );
                }
                print_out(cli.json, &val, &lines.join("\n"));
                return Ok(());
            }
            let dm = reg.install_from_dir(path)?;
            let val = serde_json::json!({
                "installed": true,
                "module": dm,
                "preview": preview,
            });
            print_out(
                cli.json,
                &val,
                &format!("Installed {} v{} -> {}", dm.manifest.id, dm.manifest.version, dm.path.display()),
            );
            Ok(())
        }
        ModulesCmd::Enable { id, slot } => {
            let discovered = reg.discover();
            if !discovered.contains_key(id) {
                return Err(format!("module {id:?} not found"));
            }
            let mut state = reg.load_state();
            let entry = state.modules.entry(id.clone()).or_default();
            entry.enabled = true;
            if let Some(s) = slot {
                if *s > 3 {
                    return Err("slot must be between 0 and 3".into());
                }
                entry.slot = Some(*s);
            }
            reg.save_state(&state)?;
            print_out(
                cli.json,
                &serde_json::json!({ "id": id, "state": state.modules.get(id) }),
                &format!("Enabled {id}"),
            );
            Ok(())
        }
        ModulesCmd::Disable { id } => {
            let mut state = reg.load_state();
            let entry = state.modules.entry(id.clone()).or_default();
            entry.enabled = false;
            reg.save_state(&state)?;
            print_out(
                cli.json,
                &serde_json::json!({ "id": id, "state": state.modules.get(id) }),
                &format!("Disabled {id}"),
            );
            Ok(())
        }
        ModulesCmd::Slot { id, slot, clear } => {
            let mut state = reg.load_state();
            let entry = state.modules.entry(id.clone()).or_default();
            if *clear {
                entry.slot = None;
            } else if let Some(s) = slot {
                if *s > 3 {
                    return Err("slot must be between 0 and 3".into());
                }
                entry.slot = Some(*s);
            }
            reg.save_state(&state)?;
            print_out(
                cli.json,
                &serde_json::json!({ "id": id, "state": state.modules.get(id) }),
                &format!("Updated slot for {id}: {:?}", state.modules[id].slot),
            );
            Ok(())
        }
        ModulesCmd::Config { id, sets } => {
            let mut state = reg.load_state();
            let cfg_clone = {
                let entry = state.modules.entry(id.clone()).or_default();
                for kv in sets {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| format!("expected key=value, got {kv:?}"))?;
                    entry.config.insert(k.to_string(), parse_setting_value(v));
                }
                entry.config.clone()
            };
            if !sets.is_empty() {
                reg.save_state(&state)?;
            }
            let val = serde_json::json!({ "id": id, "config": cfg_clone });
            print_out(
                cli.json,
                &val,
                &serde_json::to_string_pretty(&cfg_clone).unwrap(),
            );
            Ok(())
        }
        ModulesCmd::Validate { path } => {
            let report = inspect_module_dir(path)?;
            // Dry-load inside Lua runtime to verify zero syntax/init errors!
            let dm = openflux_modules::DiscoveredModule {
                manifest: openflux_modules::ModuleManifest::load_from_dir(path)
                    .map_err(|e| e.to_string())?,
                tier: openflux_modules::ModuleTier::Local,
                path: path.clone(),
            };
            let broker = Broker::new("validate");
            let mut rt = ModuleRuntime::new(broker, KeyboardGeometry::build(FrameLayout::Ansi));
            let mut disc = std::collections::BTreeMap::new();
            let mut states = std::collections::BTreeMap::new();
            let id = dm.manifest.id.clone();
            disc.insert(id.clone(), dm);
            states.insert(
                id.clone(),
                ModuleInstanceState {
                    enabled: true,
                    slot: Some(0),
                    config: Default::default(),
                },
            );
            rt.sync_modules(&disc, &states);
            let _ = rt.step_frame(0.016, 16);
            let status = rt.statuses().into_iter().next();
            let healthy = status.as_ref().is_some_and(|s| s.healthy);
            let err = status.and_then(|s| s.last_error);
            let val = serde_json::json!({
                "valid": healthy,
                "error": err,
                "disclosure": report,
            });
            print_out(
                cli.json,
                &val,
                &if healthy {
                    format!("Module {} validated cleanly.", report.module_id)
                } else {
                    format!(
                        "Module {} failed validation: {}",
                        report.module_id,
                        err.as_deref().unwrap_or("unknown")
                    )
                },
            );
            if !healthy {
                return Err("module validation failed".into());
            }
            Ok(())
        }
    }
}

fn handle_lists(cli: &Cli, reg: &ModuleRegistry, cmd: &ListsCmd) -> Result<(), String> {
    match cmd {
        ListsCmd::List => {
            let state = reg.load_state();
            let val = serde_json::json!({ "lists": state.curated_lists });
            let lines: Vec<String> = state
                .curated_lists
                .iter()
                .map(|l| format!("{} — {} ({})", l.id, l.name, l.source))
                .collect();
            let human = if lines.is_empty() {
                "No curated lists subscribed yet. Use `fluxctl lists add ...`.".to_string()
            } else {
                lines.join("\n")
            };
            print_out(cli.json, &val, &human);
            Ok(())
        }
        ListsCmd::Add { id, name, source } => {
            let mut state = reg.load_state();
            state.curated_lists.retain(|l| l.id != *id);
            state.curated_lists.push(CuratedListSubscription {
                id: id.clone(),
                name: name.clone(),
                source: source.clone(),
                official: false,
            });
            reg.save_state(&state)?;
            print_out(
                cli.json,
                &serde_json::json!({ "lists": state.curated_lists }),
                &format!("Added curated list {id} ({source})"),
            );
            Ok(())
        }
        ListsCmd::Remove { id } => {
            let mut state = reg.load_state();
            state.curated_lists.retain(|l| l.id != *id);
            reg.save_state(&state)?;
            print_out(
                cli.json,
                &serde_json::json!({ "lists": state.curated_lists }),
                &format!("Removed curated list {id}"),
            );
            Ok(())
        }
        ListsCmd::Browse { id_or_path } => {
            let state = reg.load_state();
            let source = state
                .curated_lists
                .iter()
                .find(|l| l.id == *id_or_path)
                .map(|l| l.source.clone())
                .unwrap_or_else(|| id_or_path.clone());
            let text = if source.starts_with("http://") || source.starts_with("https://") {
                daemon_client()?
                    .get(&source)
                    .send()
                    .and_then(|r| r.text())
                    .map_err(|e| e.to_string())?
            } else {
                std::fs::read_to_string(&source).map_err(|e| e.to_string())?
            };
            let doc: CuratedListDocument =
                serde_json::from_str(&text).map_err(|e| format!("invalid list JSON: {e}"))?;
            let val = serde_json::to_value(&doc).map_err(|e| e.to_string())?;
            let lines: Vec<String> = doc
                .entries
                .iter()
                .map(|e| format!("{:24} v{:7} {} — {}", e.id, e.version, e.name, e.description))
                .collect();
            print_out(cli.json, &val, &lines.join("\n"));
            Ok(())
        }
    }
}

fn handle_preset(cli: &Cli, reg: &ModuleRegistry, cmd: &PresetCmd) -> Result<(), String> {
    match cmd {
        PresetCmd::Export {
            out,
            name,
            description,
        } => {
            let preset = reg.export_preset(name, description);
            let text = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
            std::fs::write(out, text).map_err(|e| e.to_string())?;
            print_out(
                cli.json,
                &serde_json::json!({ "exported": out, "preset": preset }),
                &format!("Exported secret-free preset to {}", out.display()),
            );
            Ok(())
        }
        PresetCmd::Import { path } => {
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let preset: SetupPreset =
                serde_json::from_str(&text).map_err(|e| format!("invalid preset JSON: {e}"))?;
            let state = reg.import_preset(&preset)?;
            print_out(
                cli.json,
                &serde_json::json!({ "imported": preset.name, "modules": state.modules.len() }),
                &format!(
                    "Imported preset {:?} ({} modules configured)",
                    preset.name,
                    state.modules.len()
                ),
            );
            Ok(())
        }
    }
}

fn handle_bus(cli: &Cli, cmd: &BusCmd) -> Result<(), String> {
    let base = daemon_base(cli);
    let client = daemon_client()?;
    match cmd {
        BusCmd::Pub {
            topic,
            payload,
            retain,
        } => {
            let val: Value = serde_json::from_str(payload)
                .map_err(|e| format!("payload must be valid JSON: {e}"))?;
            let resp = client
                .post(format!("{base}/v2/bus/pub"))
                .json(&serde_json::json!({
                    "topic": topic,
                    "payload": val,
                    "retain": retain,
                }))
                .send()
                .map_err(|e| format!("daemon request failed: {e}"))?;
            let body: Value = resp.json().map_err(|e| e.to_string())?;
            print_out(cli.json, &body, &format!("Published to {topic}"));
            Ok(())
        }
        BusCmd::Get { pattern } => {
            let resp = client
                .get(format!("{base}/v2/bus/retained"))
                .query(&[("pattern", pattern)])
                .send()
                .map_err(|e| format!("daemon request failed: {e}"))?;
            let body: Value = resp.json().map_err(|e| e.to_string())?;
            print_out(
                cli.json,
                &body,
                &serde_json::to_string_pretty(&body).unwrap(),
            );
            Ok(())
        }
    }
}

fn handle_simulate(cli: &Cli, cmd: &SimulateCmd) -> Result<(), String> {
    let base = daemon_base(cli);
    let client = daemon_client()?;
    let geom = KeyboardGeometry::build(FrameLayout::Ansi);
    match cmd {
        SimulateCmd::Key {
            key_index,
            depth_um,
            velocity_um_s,
            release,
        } => {
            let key = geom
                .key(*key_index)
                .ok_or_else(|| format!("key_index {key_index} out of range"))?;
            let ev = KeyEvent {
                key_index: *key_index,
                kind: if *release {
                    KeyEventKind::Release
                } else {
                    KeyEventKind::Press
                },
                timestamp_ms: openflux_bus::now_unix_ms(),
                x_px: key.center_x,
                y_px: key.center_y,
                w_px: key.rect.w,
                h_px: key.rect.h,
                uv_x: key.uv_x,
                uv_y: key.uv_y,
                row: key.row,
                col: key.col,
                depth_um: *depth_um,
                velocity_um_s: *velocity_um_s,
                color: None,
            };
            let resp = client
                .post(format!("{base}/v2/bus/pub"))
                .json(&serde_json::json!({
                    "topic": topics::INPUT_KEYS_EVENT,
                    "payload": ev,
                    "retain": false,
                }))
                .send()
                .map_err(|e| format!("daemon request failed: {e}"))?;
            let body: Value = resp.json().map_err(|e| e.to_string())?;
            print_out(cli.json, &body, &format!("Simulated key {key_index}"));
            Ok(())
        }
        SimulateCmd::Impulse {
            x,
            y,
            radius,
            force,
        } => {
            let imp = SurfaceImpulse {
                x: *x,
                y: *y,
                radius: *radius,
                force: *force,
                color: None,
                kind: "shockwave".into(),
                source: Some("cli".into()),
                key_index: None,
            };
            let resp = client
                .post(format!("{base}/v2/bus/pub"))
                .json(&serde_json::json!({
                    "topic": topics::FX_SURFACE_IMPULSE,
                    "payload": imp,
                    "retain": false,
                }))
                .send()
                .map_err(|e| format!("daemon request failed: {e}"))?;
            let body: Value = resp.json().map_err(|e| e.to_string())?;
            print_out(
                cli.json,
                &body,
                &format!("Simulated surface impulse at ({x}, {y}) r={radius} force={force}"),
            );
            Ok(())
        }
    }
}

fn handle_preview(cli: &Cli, reg: &ModuleRegistry, cmd: &PreviewCmd) -> Result<(), String> {
    match cmd {
        PreviewCmd::Render {
            out,
            modules,
            slots,
            frames,
            step_ms,
            width,
            height,
            layout,
            scenario,
            events_file,
            no_legends,
        } => {
            let fl = if layout.eq_ignore_ascii_case("iso") {
                FrameLayout::Iso
            } else {
                FrameLayout::Ansi
            };
            let geom = KeyboardGeometry::build(fl);
            let broker = Broker::new("fluxctl-preview");

            // Seed retained geometry and realistic host state so hybrid widgets render richly.
            let _ = broker.publish_typed(
                topics::KEYBOARD_GEOMETRY,
                &geom,
                PublishOptions::retained(),
            );
            let _ = broker.publish_typed(
                topics::HOST_CONNECTION,
                &HostConnectionState {
                    connected: true,
                    host_asleep: false,
                    last_seen_ms: openflux_bus::now_unix_ms(),
                    host_platform: Some("macos".into()),
                },
                PublishOptions::retained(),
            );
            let _ = broker.publish_typed(
                topics::HOST_SYSTEM_METRICS,
                &SystemMetrics {
                    cpu_percent: 38.4,
                    memory_used_gb: 21.6,
                    memory_total_gb: 64.0,
                    gpu_percent: 24.0,
                    gpu_unified: true,
                    vram_used_gb: None,
                    vram_total_gb: None,
                    net_down_kbps: 1420.0,
                    net_up_kbps: 280.0,
                },
                PublishOptions::retained(),
            );
            let _ = broker.publish_typed(
                topics::HOST_MEDIA_NOW_PLAYING,
                &MediaNowPlaying {
                    title: "Midnight City".into(),
                    artist: "M83".into(),
                    album: "Hurry Up, We're Dreaming".into(),
                    playing: true,
                    length_sec: 243.0,
                    position_sec: 112.0,
                    track_id: "demo-1".into(),
                    art_base64: String::new(),
                },
                PublishOptions::retained(),
            );
            let _ = broker.publish_typed(
                topics::HOST_WEATHER_CURRENT,
                &WeatherState {
                    city: "Seattle".into(),
                    temp: 64.0,
                    high: 68.0,
                    low: 52.0,
                    condition: "Partly Cloudy".into(),
                    is_day: true,
                    celsius: false,
                    ready: true,
                    aqi: Some(28),
                },
                PublishOptions::retained(),
            );

            let discovered = reg.discover();
            let mut states = if modules.is_empty() {
                let saved = reg.load_state().modules;
                if saved.values().any(|s| s.enabled) {
                    saved
                } else {
                    // Default v2 showcase stack when user has no saved state yet!
                    default_showcase_states()
                }
            } else {
                let mut m = std::collections::BTreeMap::new();
                for id in modules {
                    m.insert(
                        id.clone(),
                        ModuleInstanceState {
                            enabled: true,
                            slot: None,
                            config: Default::default(),
                        },
                    );
                }
                m
            };
            for pair in slots {
                if let Some((id, s)) = pair.split_once('=') {
                    if let Ok(slot_num) = s.parse::<u8>() {
                        let entry = states.entry(id.to_string()).or_default();
                        entry.enabled = true;
                        entry.slot = Some(slot_num);
                    }
                }
            }

            let script_events: Vec<Value> = if let Some(ef) = events_file {
                let text = std::fs::read_to_string(ef).map_err(|e| e.to_string())?;
                serde_json::from_str(&text).map_err(|e| e.to_string())?
            } else {
                Vec::new()
            };

            let mut runtime = ModuleRuntime::new(broker.clone(), geom.clone());
            runtime.sync_modules(&discovered, &states);

            let font_bytes = load_bundled_font();
            let mut compositor = Compositor::new_headless(
                geom.clone(),
                font_bytes.as_deref(),
                CompositorOptions {
                    width: *width,
                    height: *height,
                    draw_default_legends: !*no_legends,
                },
            )?;

            let dt_sec = (*step_ms / 1000.0).max(0.001);
            for f in 0..*frames {
                let sim_ms = (f as f32 * *step_ms) as u64;
                inject_scenario_events(&broker, &geom, scenario, f, sim_ms);
                for ev in &script_events {
                    if ev.get("frame").and_then(|v| v.as_u64()) == Some(f as u64) {
                        if let (Some(topic), Some(payload)) = (
                            ev.get("topic").and_then(|v| v.as_str()),
                            ev.get("payload").cloned(),
                        ) {
                            let retain =
                                ev.get("retain").and_then(|v| v.as_bool()).unwrap_or(false);
                            let _ = broker.publish(
                                topic,
                                payload,
                                PublishOptions {
                                    retain,
                                    ..Default::default()
                                },
                            );
                        }
                    }
                }
                let scene = runtime.step_frame(dt_sec, sim_ms);
                compositor.render_scene(&scene);
            }

            compositor.save_png(out)?;
            let statuses = runtime.statuses();
            let val = serde_json::json!({
                "output": out,
                "width": width,
                "height": height,
                "frames": frames,
                "scenario": scenario,
                "gpu_adapter": compositor.adapter_summary(),
                "modules": statuses,
            });
            print_out(
                cli.json,
                &val,
                &format!(
                    "Rendered {} ({}x{}, {} frames, GPU: {})",
                    out.display(),
                    width,
                    height,
                    frames,
                    compositor.adapter_summary()
                ),
            );
            Ok(())
        }
    }
}

fn default_showcase_states() -> std::collections::BTreeMap<String, ModuleInstanceState> {
    let mut m = std::collections::BTreeMap::new();
    let enable = |m: &mut std::collections::BTreeMap<String, ModuleInstanceState>,
                  id: &str,
                  slot: Option<u8>| {
        m.insert(
            id.to_string(),
            ModuleInstanceState {
                enabled: true,
                slot,
                config: Default::default(),
            },
        );
    };
    enable(&mut m, "bg-aurora-shader", None);
    enable(&mut m, "fx-water-ripple-3d", None);
    enable(&mut m, "fx-analog-depth-glow", None);
    enable(&mut m, "fx-key-particles", None);
    enable(&mut m, "widget-clock", Some(0));
    enable(&mut m, "widget-system-load", Some(1));
    enable(&mut m, "widget-media", Some(2));
    enable(&mut m, "companion-idle-pet", Some(3));
    m
}

fn publish_key_press(broker: &Broker, geom: &KeyboardGeometry, key_index: u16, now_ms: u64, depth_um: u16, vel: i32) {
    if let Some(k) = geom.key(key_index) {
        let ev = KeyEvent {
            key_index,
            kind: KeyEventKind::Press,
            timestamp_ms: now_ms,
            x_px: k.center_x,
            y_px: k.center_y,
            w_px: k.rect.w,
            h_px: k.rect.h,
            uv_x: k.uv_x,
            uv_y: k.uv_y,
            row: k.row,
            col: k.col,
            depth_um,
            velocity_um_s: vel,
            color: None,
        };
        let _ = broker.publish_typed(topics::INPUT_KEYS_EVENT, &ev, PublishOptions::default());
    }
}

fn inject_scenario_events(
    broker: &Broker,
    geom: &KeyboardGeometry,
    scenario: &str,
    frame: u32,
    now_ms: u64,
) {
    match scenario {
        "showcase" => {
            // Multiple keys pressed at staggered times + analog depth ramp down/up + pet roaming!
            if frame == 5 {
                publish_key_press(broker, geom, 35, now_ms, 2400, 16000); // R
            }
            if frame == 14 {
                publish_key_press(broker, geom, 51, now_ms, 2600, 17500); // G
            }
            if frame == 24 {
                publish_key_press(broker, geom, 54, now_ms, 2300, 15000); // K
            }
            if frame == 32 {
                publish_key_press(broker, geom, 66, now_ms, 2500, 16500); // B
            }
            // Continuous analog depth demo: key 49 (D) pressing DOWN strongly, key 52 (H) lifting UP gently!
            let mut samples = Vec::new();
            if let Some(k_down) = geom.key(49) {
                let d = ((frame as f32 / 44.0).clamp(0.0, 1.0) * 2650.0) as u16;
                samples.push(AnalogKeySample::from_raw(
                    49,
                    d,
                    15500,
                    k_down.center_x,
                    k_down.center_y,
                ));
            }
            if let Some(k_up) = geom.key(52) {
                let d = ((1.0 - (frame as f32 / 44.0).clamp(0.0, 0.7)) * 2200.0) as u16;
                samples.push(AnalogKeySample::from_raw(
                    52,
                    d,
                    -11500,
                    k_up.center_x,
                    k_up.center_y,
                ));
            }
            let _ = broker.publish_typed(
                topics::INPUT_KEYS_ANALOG,
                &AnalogKeyFrame {
                    timestamp_ms: now_ms,
                    samples,
                },
                PublishOptions::default(),
            );
            // Tell keyboard/state we've been idle enough for the pet to roam on showcase!
            if frame == 2 {
                let _ = broker.publish(
                    "modules/companion-idle-pet/command",
                    serde_json::json!({ "action": "roam", "target_key": 38 }),
                    PublishOptions::default(),
                );
            }
        }
        "analog" => {
            // Side-by-side comparison: left key pressing DOWN vs right key releasing UP at same depth!
            let mut samples = Vec::new();
            if let Some(k_l) = geom.key(49) {
                samples.push(AnalogKeySample::from_raw(49, 2100, 16500, k_l.center_x, k_l.center_y));
            }
            if let Some(k_r) = geom.key(53) {
                samples.push(AnalogKeySample::from_raw(53, 2100, -14500, k_r.center_x, k_r.center_y));
            }
            let _ = broker.publish_typed(
                topics::INPUT_KEYS_ANALOG,
                &AnalogKeyFrame {
                    timestamp_ms: now_ms,
                    samples,
                },
                PublishOptions::default(),
            );
        }
        "idle-pet" => {
            if frame == 1 {
                let _ = broker.publish(
                    "modules/companion-idle-pet/command",
                    serde_json::json!({ "action": "roam", "target_key": 51 }),
                    PublishOptions::default(),
                );
            }
        }
        "typing" => {
            let keys = [48, 50, 51, 35, 36, 52, 53, 66];
            if frame % 5 == 0 {
                let idx = keys[(frame as usize / 5) % keys.len()];
                publish_key_press(broker, geom, idx, now_ms, 2200, 14000);
            }
        }
        _ => {}
    }
}
