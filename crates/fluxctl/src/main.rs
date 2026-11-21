//! `fluxctl`: 100% CLI parity with OpenFlux Studio for humans and AI coding agents.
//!
//! When `openfluxd` (the running app daemon) is reachable—auto-discovered from
//! `<data_dir>/config/keys.json` or `--daemon`—`fluxctl` operates live against it so Studio
//! updates instantaneously as an AI or script creates, edits, enables, configures, reloads,
//! inspects logs, or streams bus events. When offline (or with `--offline`), it works directly
//! against local files and the headless `wgpu` GPU compositor.

use base64::Engine;
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
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

    /// Optional openfluxd base URL (auto-discovered from keys.json when omitted).
    #[arg(long, global = true, env = "OPENFLUX_DAEMON_URL")]
    daemon: Option<String>,

    /// Force local/offline filesystem mode even if openfluxd is running.
    #[arg(long, global = true)]
    offline: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show live daemon connection (if running), paths, discovered module counts, and GPU info.
    Status,

    /// Manage modules: list, info, create, fork, inspect, install, enable, disable, slot, config, reload, logs, validate.
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

    /// Inject synthetic key presses, analog travel/velocity samples, impulses, or physics bursts.
    Simulate {
        #[command(subcommand)]
        cmd: SimulateCmd,
    },

    /// Render, capture live, or benchmark the unified GPU compositor.
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
    /// Show detailed manifest, state, disclosure report, and recent logs for a module.
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
        /// Set key=json_or_string pairs (e.g. --set refraction=68 --set tint=#ff66cc).
        #[arg(long = "set")]
        sets: Vec<String>,
    },
    /// Hot-reload a module inside the running openfluxd daemon immediately.
    Reload { id: String },
    /// Read live Lua and host-process logs from the running app (one module or all modules).
    Logs {
        /// Optional module ID; omit to show logs across all active modules.
        id: Option<String>,
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
    /// Stream live bus events matching an MQTT wildcard pattern from the running openfluxd daemon.
    Sub {
        #[arg(default_value = "#")]
        pattern: String,
        /// Stop after receiving N events (0 = stream until timeout/Ctrl-C).
        #[arg(long, default_value_t = 0)]
        count: usize,
        /// Maximum total duration in milliseconds (default 10000; 0 = unlimited).
        #[arg(long, default_value_t = 10_000)]
        timeout_ms: u64,
    },
}

#[derive(Subcommand)]
enum SimulateCmd {
    /// Send a synthetic key press/release + analog depth/velocity sample to openfluxd.
    Key {
        key_index: u16,
        #[arg(long, default_value_t = 2200)]
        depth_um: u16,
        #[arg(long, default_value_t = 14000)]
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
        #[arg(long, default_value_t = 1.15)]
        force: f32,
    },
    /// Trigger a dramatic multi-key 3D water + colliding/refracting particle + analog burst.
    Burst,
}

#[derive(Subcommand)]
enum PreviewCmd {
    /// Render GPU frames headlessly with current (or specified) modules and synthetic test events.
    Render {
        #[arg(long, short)]
        out: PathBuf,
        #[arg(long = "module")]
        modules: Vec<String>,
        #[arg(long = "slot")]
        slots: Vec<String>,
        #[arg(long, default_value_t = 45)]
        frames: u32,
        #[arg(long, default_value_t = 16.667)]
        step_ms: f32,
        #[arg(long, default_value_t = 1920)]
        width: u32,
        #[arg(long, default_value_t = 1080)]
        height: u32,
        #[arg(long, default_value = "ansi")]
        layout: String,
        #[arg(long, default_value = "showcase")]
        scenario: String,
        #[arg(long)]
        events_file: Option<PathBuf>,
        #[arg(long)]
        no_legends: bool,
    },
    /// Capture the live rendered frame directly from the running openfluxd app compositor.
    Capture {
        #[arg(long, short)]
        out: PathBuf,
        #[arg(long, default_value_t = 1)]
        steps: u32,
        #[arg(long, default_value_t = 1920)]
        width: u32,
        #[arg(long, default_value_t = 1080)]
        height: u32,
    },
    /// Benchmark full v2 performance (3D water wave + colliding/SDF-refracting particles + analog glow + Idle Pet + 4 widgets).
    Bench {
        #[arg(long, default_value_t = 180)]
        frames: u32,
        #[arg(long, default_value_t = 1920)]
        width: u32,
        #[arg(long, default_value_t = 1080)]
        height: u32,
        /// Optional PNG output of the final peak-stress frame for visual inspection.
        #[arg(long)]
        out: Option<PathBuf>,
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

fn daemon_client(timeout_ms: u64) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|e| e.to_string())
}

/// Auto-discovers a live v2-capable `openfluxd` daemon from `--daemon` or `<data_dir>/config/keys.json`.
fn detect_live_daemon(cli: &Cli) -> Option<String> {
    if cli.offline {
        return None;
    }
    let mut candidates = Vec::new();
    if let Some(url) = &cli.daemon {
        candidates.push(url.trim_end_matches('/').to_string());
    } else {
        let data = cli.data_dir.clone().unwrap_or_else(default_data_dir);
        let keys_path = data.join("config/keys.json");
        if let Ok(txt) = std::fs::read_to_string(keys_path) {
            if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                if let Some(port) = v.get("openPort").and_then(|p| p.as_u64()) {
                    let host = v
                        .get("host")
                        .and_then(|h| h.as_str())
                        .unwrap_or("127.0.0.1");
                    candidates.push(format!("https://{host}:{port}"));
                    candidates.push(format!("http://{host}:{port}"));
                }
            }
        }
        candidates.push("https://127.0.0.1:52323".to_string());
    }

    let client = daemon_client(450).ok()?;
    for base in candidates {
        if let Ok(resp) = client.get(format!("{base}/v2/status")).send() {
            if resp.status().is_success() {
                return Some(base);
            }
        }
    }
    None
}

fn require_live_daemon(cli: &Cli) -> Result<String, String> {
    detect_live_daemon(cli).ok_or_else(|| {
        "no running v2 openfluxd daemon found (start openfluxd or pass --daemon URL)".to_string()
    })
}

fn daemon_json(
    method: reqwest::Method,
    url: &str,
    body: Option<&Value>,
    timeout_ms: u64,
) -> Result<Value, String> {
    let client = daemon_client(timeout_ms)?;
    let mut req = client.request(method, url);
    if let Some(b) = body {
        req = req.json(b);
    }
    let resp = req.send().map_err(|e| format!("daemon request failed: {e}"))?;
    let status = resp.status();
    let val: Value = resp.json().map_err(|e| format!("invalid JSON from daemon: {e}"))?;
    if !status.is_success() {
        let msg = val
            .get("error")
            .or_else(|| val.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("daemon error");
        return Err(format!("HTTP {}: {msg}", status.as_u16()));
    }
    Ok(val)
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
    let live_daemon = detect_live_daemon(&cli);

    match &cli.command {
        Commands::Status => {
            if let Some(base) = live_daemon {
                let mut val = daemon_json(reqwest::Method::GET, &format!("{base}/v2/status"), None, 3000)?;
                val["mode"] = json!("live_daemon");
                val["daemon_url"] = json!(base);
                let human = format!(
                    "OpenFlux v2 CLI (CONNECTED to live openfluxd at {base})\n  Daemon PID:        {}\n  Modules rev:       {}\n  Latest bus seq:    {}\n  Discovered:        {} ({} enabled)\n  Local modules:     {}\n  GPU Compositor:    {}",
                    val["pid"],
                    val["modules_rev"],
                    val["latest_bus_seq"],
                    val["discovered_modules"],
                    val["enabled_modules"],
                    val["local_dir"].as_str().unwrap_or(""),
                    val["gpu_adapter"].as_str().unwrap_or("")
                );
                print_out(cli.json, &val, &human);
                return Ok(());
            }

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
            let val = json!({
                "mode": "offline",
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
                    "OpenFlux v2 CLI (offline mode — no v2 openfluxd detected)\n  Bundled modules:   {}\n  Installed modules: {}\n  Local modules:     {}\n  Discovered:        {} ({} enabled)\n  GPU Compositor:    {}",
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

        Commands::Modules { cmd } => handle_modules(&cli, &reg, live_daemon.as_deref(), cmd),
        Commands::Lists { cmd } => handle_lists(&cli, &reg, live_daemon.as_deref(), cmd),
        Commands::Preset { cmd } => handle_preset(&cli, &reg, live_daemon.as_deref(), cmd),
        Commands::Bus { cmd } => handle_bus(&cli, cmd),
        Commands::Simulate { cmd } => handle_simulate(&cli, cmd),
        Commands::Preview { cmd } => handle_preview(&cli, &reg, cmd),
    }
}

fn handle_modules(
    cli: &Cli,
    reg: &ModuleRegistry,
    live: Option<&str>,
    cmd: &ModulesCmd,
) -> Result<(), String> {
    match cmd {
        ModulesCmd::List {
            category,
            enabled_only,
        } => {
            if let Some(base) = live {
                let val = daemon_json(reqwest::Method::GET, &format!("{base}/v2/modules"), None, 5000)?;
                let filter_cat = category.map(|c| serde_json::to_value(ModuleCategory::from(c)).unwrap());
                let mut filtered = Vec::new();
                let mut lines = Vec::new();
                if let Some(arr) = val.get("modules").and_then(|v| v.as_array()) {
                    for m in arr {
                        let cat_v = &m["manifest"]["category"];
                        if let Some(ref fc) = filter_cat {
                            if cat_v != fc {
                                continue;
                            }
                        }
                        let enabled = m["state"]["enabled"].as_bool().unwrap_or(false);
                        if *enabled_only && !enabled {
                            continue;
                        }
                        filtered.push(m.clone());
                        let status = if enabled { "ON " } else { "off" };
                        let health = if m["healthy"].as_bool().unwrap_or(true) {
                            ""
                        } else {
                            " [ERR]"
                        };
                        let slot_str = m["state"]["slot"]
                            .as_u64()
                            .map(|s| format!(" [slot {s}]"))
                            .unwrap_or_default();
                        lines.push(format!(
                            "[{status}] {:24} {:10} {:9} v{}{}{}",
                            m["id"].as_str().unwrap_or(""),
                            cat_v.as_str().unwrap_or(""),
                            m["tier"].as_str().unwrap_or(""),
                            m["manifest"]["version"].as_str().unwrap_or(""),
                            slot_str,
                            health
                        ));
                    }
                }
                print_out(
                    cli.json,
                    &json!({ "modules": filtered, "modules_rev": val["modules_rev"], "live": true }),
                    &lines.join("\n"),
                );
                return Ok(());
            }

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
                items.push(json!({
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
            print_out(cli.json, &json!({ "modules": items }), &lines.join("\n"));
            Ok(())
        }
        ModulesCmd::Info { id } => {
            if let Some(base) = live {
                let val = daemon_json(
                    reqwest::Method::GET,
                    &format!("{base}/v2/modules/{id}"),
                    None,
                    5000,
                )?;
                let summary = val["disclosure"]["summary_lines"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_else(|| serde_json::to_string_pretty(&val).unwrap());
                print_out(cli.json, &val, &summary);
                return Ok(());
            }
            let discovered = reg.discover();
            let dm = discovered
                .get(id)
                .ok_or_else(|| format!("module {id:?} not found"))?;
            let state = reg.load_state();
            let disclosure = inspect_module_dir(&dm.path)?;
            let val = json!({
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
            if let Some(base) = live {
                let body = json!({
                    "id": id,
                    "category": ModuleCategory::from(*template),
                    "name": name,
                    "enable": enable,
                    "slot": slot,
                });
                let val = daemon_json(
                    reqwest::Method::POST,
                    &format!("{base}/v2/modules/create"),
                    Some(&body),
                    5000,
                )?;
                print_out(
                    cli.json,
                    &val,
                    &format!(
                        "Created local module {id} live at {}",
                        val["module"]["path"].as_str().unwrap_or("")
                    ),
                );
                return Ok(());
            }
            let dm = reg.scaffold_local(id, (*template).into(), name.as_deref())?;
            if *enable || slot.is_some() {
                let mut state = reg.load_state();
                if let Some(s) = slot {
                    for (other_id, other) in state.modules.iter_mut() {
                        if other_id != id && other.slot == Some(*s) {
                            other.slot = None;
                        }
                    }
                }
                let entry = state.modules.entry(id.clone()).or_default();
                entry.enabled = *enable || entry.enabled;
                if let Some(s) = slot {
                    entry.slot = Some(*s);
                }
                reg.save_state(&state)?;
            }
            let val = json!({ "created": dm });
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
            if let Some(base) = live {
                let body = json!({
                    "source_id": source_id,
                    "new_id": new_id,
                    "name": name,
                    "enable": enable,
                });
                let val = daemon_json(
                    reqwest::Method::POST,
                    &format!("{base}/v2/modules/fork"),
                    Some(&body),
                    5000,
                )?;
                print_out(
                    cli.json,
                    &val,
                    &format!(
                        "Forked {source_id} -> local module {new_id} live at {}",
                        val["module"]["path"].as_str().unwrap_or("")
                    ),
                );
                return Ok(());
            }
            let dm = reg.fork_module(source_id, new_id, name.as_deref())?;
            if *enable {
                let mut state = reg.load_state();
                let entry = state.modules.entry(new_id.clone()).or_default();
                entry.enabled = true;
                reg.save_state(&state)?;
            }
            let val = json!({ "forked": dm });
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
                let val = json!({
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
            if let Some(base) = live {
                let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                let body = json!({ "path": abs.to_string_lossy(), "accept_disclosure": true });
                let val = daemon_json(
                    reqwest::Method::POST,
                    &format!("{base}/v2/modules/install"),
                    Some(&body),
                    5000,
                )?;
                print_out(cli.json, &val, &format!("Installed module live from {}", abs.display()));
                return Ok(());
            }
            let dm = reg.install_from_dir(path)?;
            let val = json!({ "installed": true, "module": dm, "preview": preview });
            print_out(
                cli.json,
                &val,
                &format!(
                    "Installed {} v{} -> {}",
                    dm.manifest.id,
                    dm.manifest.version,
                    dm.path.display()
                ),
            );
            Ok(())
        }
        ModulesCmd::Enable { id, slot } => {
            if let Some(base) = live {
                let mut body = json!({ "enabled": true });
                if let Some(s) = slot {
                    body["slot"] = json!(s);
                }
                let val = daemon_json(
                    reqwest::Method::PUT,
                    &format!("{base}/v2/modules/{id}/state"),
                    Some(&body),
                    5000,
                )?;
                print_out(cli.json, &val, &format!("Enabled {id} (live)"));
                return Ok(());
            }
            let discovered = reg.discover();
            if !discovered.contains_key(id) {
                return Err(format!("module {id:?} not found"));
            }
            let mut state = reg.load_state();
            if let Some(s) = slot {
                if *s > 3 {
                    return Err("slot must be between 0 and 3".into());
                }
                for (other_id, other) in state.modules.iter_mut() {
                    if other_id != id && other.slot == Some(*s) {
                        other.slot = None;
                    }
                }
            }
            let entry = state.modules.entry(id.clone()).or_default();
            entry.enabled = true;
            if let Some(s) = slot {
                entry.slot = Some(*s);
            }
            reg.save_state(&state)?;
            print_out(
                cli.json,
                &json!({ "id": id, "state": state.modules.get(id) }),
                &format!("Enabled {id}"),
            );
            Ok(())
        }
        ModulesCmd::Disable { id } => {
            if let Some(base) = live {
                let body = json!({ "enabled": false });
                let val = daemon_json(
                    reqwest::Method::PUT,
                    &format!("{base}/v2/modules/{id}/state"),
                    Some(&body),
                    5000,
                )?;
                print_out(cli.json, &val, &format!("Disabled {id} (live)"));
                return Ok(());
            }
            let mut state = reg.load_state();
            let entry = state.modules.entry(id.clone()).or_default();
            entry.enabled = false;
            reg.save_state(&state)?;
            print_out(
                cli.json,
                &json!({ "id": id, "state": state.modules.get(id) }),
                &format!("Disabled {id}"),
            );
            Ok(())
        }
        ModulesCmd::Slot { id, slot, clear } => {
            if let Some(base) = live {
                let slot_val = if *clear { json!(null) } else { json!(slot) };
                let body = json!({ "slot": slot_val });
                let val = daemon_json(
                    reqwest::Method::PUT,
                    &format!("{base}/v2/modules/{id}/state"),
                    Some(&body),
                    5000,
                )?;
                print_out(cli.json, &val, &format!("Updated slot for {id} (live)"));
                return Ok(());
            }
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
                &json!({ "id": id, "state": state.modules.get(id) }),
                &format!("Updated slot for {id}: {:?}", state.modules[id].slot),
            );
            Ok(())
        }
        ModulesCmd::Config { id, sets } => {
            if let Some(base) = live {
                if !sets.is_empty() {
                    let mut cfg = serde_json::Map::new();
                    for kv in sets {
                        let (k, v) = kv
                            .split_once('=')
                            .ok_or_else(|| format!("expected key=value, got {kv:?}"))?;
                        cfg.insert(k.to_string(), parse_setting_value(v));
                    }
                    let val = daemon_json(
                        reqwest::Method::PUT,
                        &format!("{base}/v2/modules/{id}/state"),
                        Some(&json!({ "config": cfg })),
                        5000,
                    )?;
                    print_out(
                        cli.json,
                        &val,
                        &serde_json::to_string_pretty(&val["state"]["config"]).unwrap(),
                    );
                    return Ok(());
                }
                let val = daemon_json(
                    reqwest::Method::GET,
                    &format!("{base}/v2/modules/{id}"),
                    None,
                    5000,
                )?;
                let cfg = &val["state"]["config"];
                print_out(cli.json, &json!({ "id": id, "config": cfg }), &serde_json::to_string_pretty(cfg).unwrap());
                return Ok(());
            }
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
            let val = json!({ "id": id, "config": cfg_clone });
            print_out(
                cli.json,
                &val,
                &serde_json::to_string_pretty(&cfg_clone).unwrap(),
            );
            Ok(())
        }
        ModulesCmd::Reload { id } => {
            let base = require_live_daemon(cli)?;
            let val = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/modules/{id}/reload"),
                Some(&json!({})),
                5000,
            )?;
            print_out(cli.json, &val, &format!("Hot-reloaded {id} in live daemon"));
            Ok(())
        }
        ModulesCmd::Logs { id } => {
            let base = require_live_daemon(cli)?;
            let url = match id {
                Some(mid) => format!("{base}/v2/modules/{mid}/logs"),
                None => format!("{base}/v2/logs"),
            };
            let val = daemon_json(reqwest::Method::GET, &url, None, 5000)?;
            let lines: Vec<String> = val["logs"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|e| {
                            let mid = e
                                .get("module_id")
                                .and_then(|v| v.as_str())
                                .or(id.as_deref())
                                .unwrap_or("module");
                            format!(
                                "[{}] [{}] {}: {}",
                                e["timestamp_ms"].as_u64().unwrap_or(0),
                                e["level"].as_str().unwrap_or("info"),
                                mid,
                                e["message"].as_str().unwrap_or("")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let human = if lines.is_empty() {
                "No logs recorded yet.".to_string()
            } else {
                lines.join("\n")
            };
            print_out(cli.json, &val, &human);
            Ok(())
        }
        ModulesCmd::Validate { path } => {
            let report = inspect_module_dir(path)?;
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
            let val = json!({
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

fn handle_lists(
    cli: &Cli,
    reg: &ModuleRegistry,
    live: Option<&str>,
    cmd: &ListsCmd,
) -> Result<(), String> {
    match cmd {
        ListsCmd::List => {
            if let Some(base) = live {
                let val = daemon_json(reqwest::Method::GET, &format!("{base}/v2/lists"), None, 5000)?;
                let lines: Vec<String> = val["lists"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|l| {
                                format!(
                                    "{} — {} ({})",
                                    l["id"].as_str().unwrap_or(""),
                                    l["name"].as_str().unwrap_or(""),
                                    l["source"].as_str().unwrap_or("")
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let human = if lines.is_empty() {
                    "No curated lists subscribed.".to_string()
                } else {
                    lines.join("\n")
                };
                print_out(cli.json, &val, &human);
                return Ok(());
            }
            let state = reg.load_state();
            let val = json!({ "lists": state.curated_lists });
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
            if let Some(base) = live {
                let val = daemon_json(
                    reqwest::Method::POST,
                    &format!("{base}/v2/lists"),
                    Some(&json!({ "id": id, "name": name, "source": source, "official": false })),
                    5000,
                )?;
                print_out(cli.json, &val, &format!("Added curated list {id} ({source})"));
                return Ok(());
            }
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
                &json!({ "lists": state.curated_lists }),
                &format!("Added curated list {id} ({source})"),
            );
            Ok(())
        }
        ListsCmd::Remove { id } => {
            if let Some(base) = live {
                let val = daemon_json(
                    reqwest::Method::DELETE,
                    &format!("{base}/v2/lists/{id}"),
                    None,
                    5000,
                )?;
                print_out(cli.json, &val, &format!("Removed curated list {id}"));
                return Ok(());
            }
            let mut state = reg.load_state();
            state.curated_lists.retain(|l| l.id != *id);
            reg.save_state(&state)?;
            print_out(
                cli.json,
                &json!({ "lists": state.curated_lists }),
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
                daemon_client(5000)?
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

fn handle_preset(
    cli: &Cli,
    reg: &ModuleRegistry,
    live: Option<&str>,
    cmd: &PresetCmd,
) -> Result<(), String> {
    match cmd {
        PresetCmd::Export {
            out,
            name,
            description,
        } => {
            let preset_val = if let Some(base) = live {
                let res = daemon_json(
                    reqwest::Method::POST,
                    &format!("{base}/v2/preset/export"),
                    Some(&json!({ "name": name, "description": description })),
                    5000,
                )?;
                res["preset"].clone()
            } else {
                serde_json::to_value(reg.export_preset(name, description)).map_err(|e| e.to_string())?
            };
            let text = serde_json::to_string_pretty(&preset_val).map_err(|e| e.to_string())?;
            std::fs::write(out, text).map_err(|e| e.to_string())?;
            print_out(
                cli.json,
                &json!({ "exported": out, "preset": preset_val }),
                &format!("Exported secret-free preset to {}", out.display()),
            );
            Ok(())
        }
        PresetCmd::Import { path } => {
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let preset: SetupPreset =
                serde_json::from_str(&text).map_err(|e| format!("invalid preset JSON: {e}"))?;
            if let Some(base) = live {
                let val = daemon_json(
                    reqwest::Method::POST,
                    &format!("{base}/v2/preset/import"),
                    Some(&serde_json::to_value(&preset).unwrap()),
                    5000,
                )?;
                print_out(cli.json, &val, &format!("Imported preset {:?} (live)", preset.name));
                return Ok(());
            }
            let state = reg.import_preset(&preset)?;
            print_out(
                cli.json,
                &json!({ "imported": preset.name, "modules": state.modules.len() }),
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
    let base = require_live_daemon(cli)?;
    match cmd {
        BusCmd::Pub {
            topic,
            payload,
            retain,
        } => {
            let val: Value = serde_json::from_str(payload)
                .map_err(|e| format!("payload must be valid JSON: {e}"))?;
            let body = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/bus/pub"),
                Some(&json!({ "topic": topic, "payload": val, "retain": retain })),
                5000,
            )?;
            print_out(cli.json, &body, &format!("Published to {topic}"));
            Ok(())
        }
        BusCmd::Get { pattern } => {
            let client = daemon_client(5000)?;
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
        BusCmd::Sub {
            pattern,
            count,
            timeout_ms,
        } => {
            let client = daemon_client(12_000)?;
            let status = daemon_json(reqwest::Method::GET, &format!("{base}/v2/status"), None, 3000)?;
            let mut since_seq = status["latest_bus_seq"].as_u64().unwrap_or(0);
            let started = Instant::now();
            let mut seen = 0usize;

            loop {
                if *timeout_ms > 0 && started.elapsed() >= Duration::from_millis(*timeout_ms) {
                    break;
                }
                let rem_ms = if *timeout_ms == 0 {
                    2000u64
                } else {
                    (*timeout_ms)
                        .saturating_sub(started.elapsed().as_millis() as u64)
                        .min(2000)
                        .max(50)
                };
                let resp = client
                    .get(format!("{base}/v2/bus/events"))
                    .query(&[
                        ("pattern", pattern.as_str()),
                        ("since_seq", &since_seq.to_string()),
                        ("wait_ms", &rem_ms.to_string()),
                    ])
                    .send()
                    .map_err(|e| format!("bus sub failed: {e}"))?;
                let body: Value = resp.json().map_err(|e| e.to_string())?;
                if let Some(latest) = body["latest_seq"].as_u64() {
                    since_seq = latest;
                }
                if let Some(events) = body["events"].as_array() {
                    for ev in events {
                        if cli.json {
                            println!("{}", serde_json::to_string(ev).unwrap());
                        } else {
                            println!(
                                "#{} {} {}",
                                ev["seq"].as_u64().unwrap_or(0),
                                ev["topic"].as_str().unwrap_or(""),
                                ev["payload"]
                            );
                        }
                        seen += 1;
                        if *count > 0 && seen >= *count {
                            return Ok(());
                        }
                    }
                }
            }
            Ok(())
        }
    }
}

fn handle_simulate(cli: &Cli, cmd: &SimulateCmd) -> Result<(), String> {
    let base = require_live_daemon(cli)?;
    match cmd {
        SimulateCmd::Key {
            key_index,
            depth_um,
            velocity_um_s,
            release,
        } => {
            let body = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/simulate/key"),
                Some(&json!({
                    "key_index": key_index,
                    "depth_um": depth_um,
                    "velocity_um_s": velocity_um_s,
                    "release": release,
                })),
                5000,
            )?;
            print_out(cli.json, &body, &format!("Simulated key {key_index} (depth={depth_um}um vel={velocity_um_s}um/s)"));
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
            let body = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/simulate/impulse"),
                Some(&serde_json::to_value(&imp).unwrap()),
                5000,
            )?;
            print_out(
                cli.json,
                &body,
                &format!("Simulated surface impulse at ({x}, {y}) r={radius} force={force}"),
            );
            Ok(())
        }
        SimulateCmd::Burst => {
            let body = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/simulate/burst"),
                Some(&json!({})),
                5000,
            )?;
            print_out(cli.json, &body, "Triggered 6-key 3D water + particle + analog burst!");
            Ok(())
        }
    }
}

fn seed_preview_broker(broker: &Broker, geom: &KeyboardGeometry) {
    let _ = broker.publish_typed(topics::KEYBOARD_GEOMETRY, geom, PublishOptions::retained());
    let _ = broker.publish_typed(
        topics::HOST_CONNECTION,
        &HostConnectionState {
            connected: true,
            host_asleep: false,
            last_seen_ms: openflux_bus::now_unix_ms(),
            host_platform: Some(std::env::consts::OS.to_string()),
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
}

fn handle_preview(cli: &Cli, reg: &ModuleRegistry, cmd: &PreviewCmd) -> Result<(), String> {
    match cmd {
        PreviewCmd::Capture {
            out,
            steps,
            width,
            height,
        } => {
            let base = require_live_daemon(cli)?;
            let val = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/preview/frame"),
                Some(&json!({ "steps": steps, "width": width, "height": height })),
                15_000,
            )?;
            let b64 = val["png_base64"]
                .as_str()
                .ok_or_else(|| "daemon response missing png_base64".to_string())?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| e.to_string())?;
            if let Some(p) = out.parent() {
                if !p.as_os_str().is_empty() {
                    std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
                }
            }
            std::fs::write(out, bytes).map_err(|e| e.to_string())?;
            let summary = json!({
                "output": out,
                "width": val["width"],
                "height": val["height"],
                "gpu_adapter": val["gpu_adapter"],
                "modules_rev": val["modules_rev"],
                "timings": val["timings"],
            });
            print_out(
                cli.json,
                &summary,
                &format!(
                    "Captured live app frame -> {} ({}x{}, {:.2} ms/frame, {} particles)",
                    out.display(),
                    val["width"],
                    val["height"],
                    val["timings"]["total_frame_ms"].as_f64().unwrap_or(0.0),
                    val["timings"]["particle_count"].as_u64().unwrap_or(0)
                ),
            );
            Ok(())
        }
        PreviewCmd::Bench {
            frames,
            width,
            height,
            out,
        } => {
            let geom = KeyboardGeometry::build(FrameLayout::Ansi);
            let broker = Broker::new("fluxctl-bench");
            seed_preview_broker(&broker, &geom);

            let discovered = reg.discover();
            let states = default_showcase_states();
            let mut runtime = ModuleRuntime::new(broker.clone(), geom.clone());
            runtime.sync_modules(&discovered, &states);

            let font_bytes = load_bundled_font();
            let mut compositor = Compositor::new_headless(
                geom.clone(),
                font_bytes.as_deref(),
                CompositorOptions {
                    width: *width,
                    height: *height,
                    draw_default_legends: true,
                },
            )?;

            // Warmup 5 frames (bakes cached keycap legend layer + shader pipelines).
            for w in 0..5u32 {
                let s = runtime.step_frame(1.0 / 60.0, w as u64 * 16);
                let _ = compositor.render_scene(&s);
            }

            let mut totals = Vec::with_capacity(*frames as usize);
            let mut lua_times = Vec::with_capacity(*frames as usize);
            let mut phys_times = Vec::with_capacity(*frames as usize);
            let mut c2d_times = Vec::with_capacity(*frames as usize);
            let mut gpu_times = Vec::with_capacity(*frames as usize);
            let mut peak_particles = 0usize;

            let bench_start = Instant::now();
            for f in 0..*frames {
                let sim_ms = (f as u64 + 5) * 16;
                // Heavy stress: rapid typing + analog depth sweeps + water impulses + roaming pet!
                if f % 3 == 0 {
                    let k = 20 + ((f * 7) % 58) as u16;
                    publish_key_press(&broker, &geom, k, sim_ms, 2450, 16800);
                }
                if f == 2 {
                    let _ = broker.publish(
                        "modules/companion-idle-pet/command",
                        json!({ "action": "roam", "target_key": 38 }),
                        PublishOptions::default(),
                    );
                }
                let mut analog = Vec::new();
                for offset in 0..4u16 {
                    let ki = 48 + offset * 2;
                    if let Some(k) = geom.key(ki) {
                        let phase = ((f as f32 * 0.14 + offset as f32).sin() * 0.5 + 0.5) * 2600.0;
                        let vel = ((f as f32 * 0.14 + offset as f32).cos() * 16000.0) as i32;
                        analog.push(AnalogKeySample::from_raw(
                            ki,
                            phase as u16,
                            vel,
                            k.center_x,
                            k.center_y,
                        ));
                    }
                }
                let _ = broker.publish_typed(
                    topics::INPUT_KEYS_ANALOG,
                    &AnalogKeyFrame {
                        timestamp_ms: sim_ms,
                        samples: analog,
                    },
                    PublishOptions::default(),
                );

                let t_lua = Instant::now();
                let scene = runtime.step_frame(1.0 / 60.0, sim_ms);
                let l_ms = t_lua.elapsed().as_secs_f32() * 1000.0;
                let t = compositor.render_scene(&scene);
                peak_particles = peak_particles.max(t.particle_count);
                lua_times.push(l_ms);
                phys_times.push(t.physics_ms);
                c2d_times.push(t.canvas2d_ms);
                gpu_times.push(t.gpu_submit_ms);
                totals.push(l_ms + t.total_ms);
            }
            let wall_ms = bench_start.elapsed().as_secs_f32() * 1000.0;

            if let Some(out_path) = out {
                compositor.save_png(out_path)?;
            }

            let pct = |v: &mut Vec<f32>, p: f32| -> f32 {
                v.sort_by(|a, b| a.total_cmp(b));
                if v.is_empty() {
                    return 0.0;
                }
                let idx = ((v.len() as f32 - 1.0) * p).round() as usize;
                v[idx]
            };

            let p50 = pct(&mut totals, 0.50);
            let p95 = pct(&mut totals, 0.95);
            let max_t = *totals.last().unwrap_or(&0.0);
            let avg_ms = wall_ms / (*frames).max(1) as f32;
            let fps = 1000.0 / avg_ms.max(0.001);

            let val = json!({
                "frames": frames,
                "resolution": format!("{width}x{height}"),
                "gpu_adapter": compositor.adapter_summary(),
                "peak_particles": peak_particles,
                "sustained_fps": fps,
                "avg_frame_ms": avg_ms,
                "p50_frame_ms": p50,
                "p95_frame_ms": p95,
                "max_frame_ms": max_t,
                "p50_lua_ms": pct(&mut lua_times, 0.50),
                "p50_physics_ms": pct(&mut phys_times, 0.50),
                "p50_canvas2d_ms": pct(&mut c2d_times, 0.50),
                "p50_gpu_submit_ms": pct(&mut gpu_times, 0.50),
                "output": out,
            });
            print_out(
                cli.json,
                &val,
                &format!(
                    "OpenFlux v2 Stress Benchmark ({} frames @ {}x{}, GPU: {})\n  Active Stack: 3D Water Ripple + Colliding/SDF Particles + Analog Depth Glow + Idle Pet + 4 Widgets + Aurora Shader\n  Peak active particles: {}\n  Sustained FPS: {:.1} fps (avg {:.2} ms/frame)\n  Frame time:    p50={:.2} ms | p95={:.2} ms | max={:.2} ms\n  Breakdown p50: Lua={:.2} ms | Physics={:.2} ms | Canvas2D={:.2} ms | GPU={:.2} ms",
                    frames,
                    width,
                    height,
                    compositor.adapter_summary(),
                    peak_particles,
                    fps,
                    avg_ms,
                    p50,
                    p95,
                    max_t,
                    val["p50_lua_ms"].as_f64().unwrap_or(0.0),
                    val["p50_physics_ms"].as_f64().unwrap_or(0.0),
                    val["p50_canvas2d_ms"].as_f64().unwrap_or(0.0),
                    val["p50_gpu_submit_ms"].as_f64().unwrap_or(0.0),
                ),
            );
            Ok(())
        }
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
            seed_preview_broker(&broker, &geom);

            let discovered = reg.discover();
            let saved = reg.load_state().modules;
            let mut states = if modules.is_empty() {
                if saved.values().any(|s| s.enabled) {
                    saved.clone()
                } else {
                    default_showcase_states()
                }
            } else {
                let mut m = std::collections::BTreeMap::new();
                for id in modules {
                    let prev = saved.get(id).cloned().unwrap_or_default();
                    m.insert(
                        id.clone(),
                        ModuleInstanceState {
                            enabled: true,
                            slot: prev.slot,
                            config: prev.config,
                        },
                    );
                }
                m
            };
            for pair in slots {
                if let Some((id, s)) = pair.split_once('=') {
                    if let Ok(slot_num) = s.parse::<u8>() {
                        let prev_cfg = saved.get(id).map(|p| p.config.clone()).unwrap_or_default();
                        let entry = states.entry(id.to_string()).or_insert_with(|| {
                            ModuleInstanceState {
                                enabled: true,
                                slot: None,
                                config: prev_cfg,
                            }
                        });
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
            let mut last_timings = openflux_compositor::FrameTimings::default();
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
                last_timings = compositor.render_scene(&scene);
            }

            compositor.save_png(out)?;
            let statuses = runtime.statuses();
            let val = json!({
                "output": out,
                "width": width,
                "height": height,
                "frames": frames,
                "scenario": scenario,
                "gpu_adapter": compositor.adapter_summary(),
                "timings": {
                    "physics_ms": last_timings.physics_ms,
                    "canvas2d_ms": last_timings.canvas2d_ms,
                    "gpu_submit_ms": last_timings.gpu_submit_ms,
                    "total_ms": last_timings.total_ms,
                    "particle_count": last_timings.particle_count,
                },
                "modules": statuses,
            });
            print_out(
                cli.json,
                &val,
                &format!(
                    "Rendered {} ({}x{}, {} frames, {:.2} ms last frame, {} particles, GPU: {})",
                    out.display(),
                    width,
                    height,
                    frames,
                    last_timings.total_ms,
                    last_timings.particle_count,
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

fn publish_key_press(
    broker: &Broker,
    geom: &KeyboardGeometry,
    key_index: u16,
    now_ms: u64,
    depth_um: u16,
    vel: i32,
) {
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
            if frame == 5 {
                publish_key_press(broker, geom, 35, now_ms, 2400, 16000);
            }
            if frame == 14 {
                publish_key_press(broker, geom, 51, now_ms, 2600, 17500);
            }
            if frame == 24 {
                publish_key_press(broker, geom, 54, now_ms, 2300, 15000);
            }
            if frame == 32 {
                publish_key_press(broker, geom, 66, now_ms, 2500, 16500);
            }
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
            if frame == 2 {
                let _ = broker.publish(
                    "modules/companion-idle-pet/command",
                    json!({ "action": "roam", "target_key": 38 }),
                    PublishOptions::default(),
                );
            }
        }
        "analog" => {
            let mut samples = Vec::new();
            if let Some(k_l) = geom.key(49) {
                samples.push(AnalogKeySample::from_raw(
                    49,
                    2100,
                    16500,
                    k_l.center_x,
                    k_l.center_y,
                ));
            }
            if let Some(k_r) = geom.key(53) {
                samples.push(AnalogKeySample::from_raw(
                    53,
                    2100,
                    -14500,
                    k_r.center_x,
                    k_r.center_y,
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
        }
        "idle-pet" => {
            if frame == 1 {
                let _ = broker.publish(
                    "modules/companion-idle-pet/command",
                    json!({ "action": "roam", "target_key": 51 }),
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
