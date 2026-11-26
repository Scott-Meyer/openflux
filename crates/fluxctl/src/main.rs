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

    /// Live keyboard & host latency/performance profiler (FPS, p50/p95/p99, jank hitches, RKMPP queue, per-module Lua timings).
    Perf {
        /// Refresh continuously once per second.
        #[arg(long)]
        watch: bool,
    },

    /// Run a standard JSON-RPC 2.0 stdio MCP server exposing live KeyGlass diagnostics, performance, module logs, bus state, and keyboard sync for AI tools & module developers.
    Mcp,

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

    /// Sync v2 modules, upload background videos, and sideload/flash local builds to the connected keyboard.
    Keyboard {
        #[command(subcommand)]
        cmd: KeyboardCmd,
    },

    /// Inspect and edit QMK/Vial keymaps, sync dynamic module actions, and query live Vial Raw HID state.
    Vial {
        #[command(subcommand)]
        cmd: VialCmd,
    },

    /// Key faces: explain why a key looks the way it does, flash a key, and edit your Key Rules.
    Keys {
        #[command(subcommand)]
        cmd: KeysCmd,
    },
}

#[derive(Subcommand)]
enum KeysCmd {
    /// Explain a key's face step by step: every rule that applies, in precedence order, and why
    /// skipped rules didn't match. KEY is a key index or name (e.g. 3, F2, KC_A, ESC).
    Explain { key: String },
    /// Flash keys with a momentary glow or outline (published on `keys/signal`).
    Signal {
        /// Keys: names/indices, a group (fkeys, home_row, all, ...), or action:<id>; comma-separated.
        keys: String,
        /// Outline instead of glow.
        #[arg(long)]
        outline: bool,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        pulse: bool,
        #[arg(long, default_value_t = 3000)]
        duration_ms: u64,
        /// Signal id; re-sending it refreshes the signal, and --clear ends it.
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        clear: bool,
    },
    /// Your Key Rules (the `keys-custom` module's `rules` list). Later rules win.
    Rules {
        #[command(subcommand)]
        cmd: KeyRulesCmd,
    },
}

#[derive(Subcommand)]
enum KeyRulesCmd {
    /// List rules in order.
    List,
    /// Append a rule (or insert it with --at). Fields match the Key Rules list in Studio.
    Add {
        /// e.g. "all", "F1-F4", "home_row", "Esc, a", "action:mqtt-bridge.scene_on"
        #[arg(long)]
        keys: String,
        #[arg(long)]
        name: Option<String>,
        /// Only on this layer (0-15).
        #[arg(long)]
        layer: Option<u8>,
        /// Only in these apps (comma-separated).
        #[arg(long)]
        app: Option<String>,
        /// held | not_held
        #[arg(long)]
        held: Option<String>,
        /// on | off
        #[arg(long)]
        caps: Option<String>,
        /// Only when this retained bus topic...
        #[arg(long)]
        when_topic: Option<String>,
        /// ...at this dot path...
        #[arg(long)]
        when_path: Option<String>,
        /// ...is (eq, ne, in, gt, lt, contains, exists, missing)...
        #[arg(long, default_value = "eq")]
        when_op: String,
        /// ...this value.
        #[arg(long)]
        when_value: Option<String>,
        /// legend | all
        #[arg(long)]
        hide: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        icon: Option<String>,
        #[arg(long)]
        color: Option<String>,
        /// soft | subtle | pulse
        #[arg(long)]
        glow: Option<String>,
        #[arg(long)]
        glow_color: Option<String>,
        /// ring | window | edge
        #[arg(long)]
        outline: Option<String>,
        #[arg(long)]
        outline_color: Option<String>,
        #[arg(long)]
        backdrop: Option<String>,
        /// Corner content as SLOT=VALUE (tl/tr/bl/br; text, dot, icon:<name>, none); repeatable.
        #[arg(long = "corner")]
        corners: Vec<String>,
        /// Insert at this position (1-based) instead of appending.
        #[arg(long)]
        at: Option<usize>,
    },
    /// Remove a rule by position (1-based) or name.
    Remove { rule: String },
    /// Enable a rule by position or name.
    Enable { rule: String },
    /// Disable a rule by position or name (kept in the list).
    Disable { rule: String },
    /// Move a rule to a new position (1-based); later positions win.
    Move { rule: String, to: usize },
}

#[derive(Subcommand)]
enum VialCmd {
    /// Show Vial Raw HID connection status, protocol version, and keyboard UID.
    Status,
    /// List sticky QK_KB_n custom action slots mapped from enabled modules.
    Actions,
    /// Rebuild dynamic vial.json + module_actions.json and upload to RP2040 + keyboard SoC.
    Sync,
    /// Read the 16-bit QMK keycode at (layer, row, col) directly from live Vial EEPROM.
    Get { layer: u8, row: u8, col: u8 },
    /// Write a 16-bit QMK keycode (hex `0x2209` or decimal) at (layer, row, col) and verify readback.
    Set {
        layer: u8,
        row: u8,
        col: u8,
        keycode: String,
    },
    /// Dump all 6x16 keycodes for a layer from live Vial EEPROM.
    Layer { layer: u8 },
    /// Export the live Vial keymap setup (all 8 layers + Tap Dances + Combos + Key Overrides) to a JSON file.
    Export { path: PathBuf },
    /// Import and apply a saved Vial keymap setup JSON file onto the live keyboard.
    Import { path: PathBuf },
}

#[derive(Subcommand)]
enum KeyboardCmd {
    /// Push enabled v2 modules, modules_state.json, and referenced background media to the connected keyboard.
    Sync,
    /// Upload a video or image straight to `/home/root/openflux/media/` on the connected keyboard.
    UploadMedia {
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    /// Sideload a local `.ipk` package (`openflux-bridge` or `openflux-display`) to the connected keyboard.
    Sideload { ipk: PathBuf },
    /// Flash a local RP2040 `.uf2` firmware image to the connected keyboard.
    FlashUf2 { uf2: PathBuf },
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
        tag: Option<String>,
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
    /// Inspect any module source (local folder, Git/GitHub URL, archive, or curated list ID).
    Inspect { source: String },
    /// Install or update a module from a local folder, Git/GitHub URL, archive, or curated list ID.
    Install {
        source: String,
        /// Show disclosure/diff only without installing.
        #[arg(long)]
        inspect_only: bool,
        /// Non-interactively acknowledge the disclosure report and proceed with installation.
        #[arg(long)]
        accept_disclosure: bool,
    },
    /// Uninstall an installed or local module (bundled defaults are protected).
    Uninstall { id: String },
    /// Update an installed module from its recorded upstream source after reviewing disclosure diff.
    Update {
        id: String,
        #[arg(long)]
        inspect_only: bool,
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
    /// Inspect a module's imported bindings, or replace them from an export file.
    BindingLayout {
        id: String,
        #[arg(long, value_name = "FILE")]
        import: Option<PathBuf>,
    },
    /// Hot-reload a module inside the running openfluxd daemon immediately.
    Reload { id: String },
    /// Trigger a declared action button on a module (publishes its bus event).
    Action { id: String, action_id: String },
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
    Import {
        path: PathBuf,
        /// Automatically install missing modules that include a source reference in the preset.
        #[arg(long)]
        install_missing: bool,
        /// Acknowledge disclosure for any missing modules installed via --install-missing.
        #[arg(long)]
        accept_disclosure: bool,
    },
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
    let mut candidates = vec![PathBuf::from("modules")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            candidates.push(p.join("modules"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            PathBuf::from(&home)
                .join("Applications/OpenFlux Studio.app/Contents/Resources/modules"),
        );
        candidates.push(PathBuf::from(&home).join("git/flux/modules"));
    }
    for c in candidates {
        if c.is_dir() {
            return c;
        }
    }
    PathBuf::from("modules")
}

fn load_bundled_font() -> Option<Vec<u8>> {
    let mut candidates = vec![
        PathBuf::from("assets/bundled_fonts/Montserrat-Medium.ttf"),
        PathBuf::from("assets/bundled_fonts/Montserrat-VariableFont_wght.ttf"),
        PathBuf::from("dist/assets/fonts/Montserrat-Medium.ttf"),
        PathBuf::from("dist/assets/fonts/Montserrat-VariableFont_wght.ttf"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            PathBuf::from(&home)
                .join("Applications/OpenFlux Studio.app/Contents/Resources/assets/bundled_fonts/Montserrat-Medium.ttf"),
        );
        candidates.push(
            PathBuf::from(&home)
                .join("Applications/OpenFlux Studio.app/Contents/Resources/assets/bundled_fonts/Montserrat-VariableFont_wght.ttf"),
        );
    }
    candidates.push(PathBuf::from("/System/Library/Fonts/Helvetica.ttc"));
    candidates.push(PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"));
    for p in candidates {
        if let Ok(bytes) = std::fs::read(&p) {
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

fn load_daemon_token() -> Option<String> {
    if let Ok(t) = std::env::var("OPENFLUX_BUS_TOKEN") {
        let trimmed = t.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    let data = std::env::var("OPENFLUX_DATA_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir);
    std::fs::read_to_string(data.join("config/desktop_token.txt"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn daemon_client(timeout_ms: u64) -> Result<reqwest::blocking::Client, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(tok) = load_daemon_token() {
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {tok}")) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
    }
    reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .default_headers(headers)
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
        Commands::Perf { watch } => {
            let base = live_daemon
                .ok_or_else(|| "openfluxd is not running; start KeyGlass to inspect live performance".to_string())?;
            loop {
                let val = daemon_json(reqwest::Method::GET, &format!("{base}/v2/perf"), None, 3000)?;
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
                } else {
                    if *watch {
                        print!("\x1b[2J\x1b[H");
                    }
                    println!("{}", format_perf_report(&val));
                }
                if !*watch {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Ok(())
        }
        Commands::Mcp => run_mcp_stdio_server(&cli),
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
        Commands::Keyboard { cmd } => handle_keyboard(&cli, cmd),
        Commands::Vial { cmd } => handle_vial(&cli, cmd),
        Commands::Keys { cmd } => handle_keys(&cli, cmd),
    }
}

fn vial_xfer(base: &str, pkt: &[u8; 32]) -> Result<[u8; 32], String> {
    let val = daemon_json(
        reqwest::Method::POST,
        &format!("{base}/v2/vial/xfer"),
        Some(&json!({ "data": pkt.to_vec() })),
        10_000,
    )?;
    let arr = val["data"]
        .as_array()
        .ok_or_else(|| "Malformed Vial xfer response".to_string())?;
    let mut out = [0u8; 32];
    for (i, v) in arr.iter().take(32).enumerate() {
        out[i] = v.as_u64().unwrap_or(0) as u8;
    }
    Ok(out)
}

fn parse_u16_keycode(s: &str) -> Result<u16, String> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).map_err(|e| format!("Invalid hex keycode '{s}': {e}"))
    } else {
        t.parse::<u16>()
            .map_err(|e| format!("Invalid keycode '{s}' (use hex like 0x2209 or decimal): {e}"))
    }
}

fn handle_vial(cli: &Cli, cmd: &VialCmd) -> Result<(), String> {
    let base = require_live_daemon(cli)?;
    match cmd {
        VialCmd::Status => {
            let mut val = daemon_json(
                reqwest::Method::GET,
                &format!("{base}/v2/vial/status"),
                None,
                5_000,
            )?;
            let connected = val["connected"].as_bool().unwrap_or(false);
            let mut summary = format!(
                "Vial Raw HID: {} ({})",
                if connected { "CONNECTED" } else { "disconnected" },
                val["product_name"].as_str().unwrap_or("OpenFlux")
            );
            if connected {
                let mut id_req = [0u8; 32];
                id_req[0] = 0xFE;
                id_req[1] = 0x00;
                if let Ok(r) = vial_xfer(&base, &id_req) {
                    let proto = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
                    let uid: String = r[4..12].iter().map(|b| format!("{b:02x}")).collect();
                    val["protocol"] = json!(proto);
                    val["uid"] = json!(uid);
                    summary.push_str(&format!(" · protocol={proto} · uid={uid}"));
                }
            }
            print_out(cli.json, &val, &summary);
            Ok(())
        }
        VialCmd::Actions => {
            let val = daemon_json(
                reqwest::Method::GET,
                &format!("{base}/v2/vial/actions"),
                None,
                5_000,
            )?;
            let mut lines = vec!["OpenFlux QMK/Vial Custom Action Slots (QK_KB_n):".to_string()];
            if let Some(entries) = val["entries"].as_object() {
                let mut sorted: Vec<(u16, &Value)> = entries
                    .iter()
                    .filter_map(|(k, v)| k.parse::<u16>().ok().map(|n| (n, v)))
                    .collect();
                sorted.sort_by_key(|(n, _)| *n);
                for (slot, e) in sorted {
                    let short = e["short_name"].as_str().unwrap_or("");
                    let title = e["title"].as_str().unwrap_or("");
                    let active = e["active"].as_bool().unwrap_or(true);
                    lines.push(format!(
                        "  QK_KB_{:>2} ({}): {:<10} {} {}",
                        slot,
                        if active { "active" } else { "disabled" },
                        short,
                        e["action_id"].as_str().unwrap_or(""),
                        if !title.is_empty() { format!("— {title}") } else { String::new() }
                    ));
                }
            }
            print_out(cli.json, &val, &lines.join("\n"));
            Ok(())
        }
        VialCmd::Sync => {
            let val = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/vial/sync"),
                Some(&json!({})),
                15_000,
            )?;
            print_out(
                cli.json,
                &val,
                &format!(
                    "Synced {} action slots ({} B XZ) · firmware={} · soc={}",
                    val["slot_count"].as_u64().unwrap_or(0),
                    val["xz_bytes"].as_u64().unwrap_or(0),
                    val["firmware_synced"].as_bool().unwrap_or(false),
                    val["soc_synced"].as_bool().unwrap_or(false)
                ),
            );
            Ok(())
        }
        VialCmd::Get { layer, row, col } => {
            let mut req = [0u8; 32];
            req[0] = 0x04;
            req[1] = *layer;
            req[2] = *row;
            req[3] = *col;
            let r = vial_xfer(&base, &req)?;
            let kc = ((r[4] as u16) << 8) | (r[5] as u16);
            let val = json!({
                "layer": layer,
                "row": row,
                "col": col,
                "keycode": kc,
                "hex": format!("0x{kc:04X}"),
            });
            print_out(
                cli.json,
                &val,
                &format!("L{layer} ({row},{col}) = 0x{kc:04X} ({kc})"),
            );
            Ok(())
        }
        VialCmd::Set {
            layer,
            row,
            col,
            keycode,
        } => {
            let kc = parse_u16_keycode(keycode)?;
            let mut set_req = [0u8; 32];
            set_req[0] = 0x05;
            set_req[1] = *layer;
            set_req[2] = *row;
            set_req[3] = *col;
            set_req[4] = (kc >> 8) as u8;
            set_req[5] = (kc & 0xFF) as u8;
            let _ = vial_xfer(&base, &set_req)?;

            let mut get_req = [0u8; 32];
            get_req[0] = 0x04;
            get_req[1] = *layer;
            get_req[2] = *row;
            get_req[3] = *col;
            let r = vial_xfer(&base, &get_req)?;
            let got = ((r[4] as u16) << 8) | (r[5] as u16);
            if got != kc {
                return Err(format!(
                    "Readback mismatch at L{layer} ({row},{col}): wrote 0x{kc:04X}, got 0x{got:04X}"
                ));
            }
            let val = json!({
                "ok": true,
                "layer": layer,
                "row": row,
                "col": col,
                "keycode": got,
                "hex": format!("0x{got:04X}"),
            });
            print_out(
                cli.json,
                &val,
                &format!("Set L{layer} ({row},{col}) = 0x{got:04X} (verified)"),
            );
            Ok(())
        }
        VialCmd::Layer { layer } => {
            let mut rows = Vec::new();
            let mut lines = vec![format!("Layer {layer} (6x16 QMK keycodes):")];
            for row in 0..6u8 {
                let mut cols = Vec::new();
                let mut row_str = format!("  R{row}:");
                for col in 0..16u8 {
                    let mut req = [0u8; 32];
                    req[0] = 0x04;
                    req[1] = *layer;
                    req[2] = row;
                    req[3] = col;
                    let r = vial_xfer(&base, &req)?;
                    let kc = ((r[4] as u16) << 8) | (r[5] as u16);
                    cols.push(format!("0x{kc:04X}"));
                    row_str.push_str(&format!(" {kc:04X}"));
                }
                rows.push(cols);
                lines.push(row_str);
            }
            print_out(
                cli.json,
                &json!({ "layer": layer, "rows": rows }),
                &lines.join("\n"),
            );
            Ok(())
        }
        VialCmd::Export { path } => {
            let mut layers = Vec::new();
            for layer in 0..8u8 {
                let mut rows = Vec::new();
                for row in 0..6u8 {
                    let mut cols = Vec::new();
                    for col in 0..16u8 {
                        let mut req = [0u8; 32];
                        req[0] = 0x04;
                        req[1] = layer;
                        req[2] = row;
                        req[3] = col;
                        let r = vial_xfer(&base, &req)?;
                        let kc = ((r[4] as u16) << 8) | (r[5] as u16);
                        cols.push(kc);
                    }
                    rows.push(cols);
                }
                layers.push(rows);
            }
            // Read Tap Dance entries (dynamic_vial_tap_dance_get = [0xFE, 0x0D, 0x01, idx])
            let mut tap_dances = Vec::new();
            for idx in 0..32u8 {
                let mut req = [0u8; 32];
                req[0] = 0xFE;
                req[1] = 0x0D;
                req[2] = 0x01;
                req[3] = idx;
                if let Ok(r) = vial_xfer(&base, &req) {
                    if r[0] == 0 {
                        let on_tap = u16::from_le_bytes([r[1], r[2]]);
                        let on_hold = u16::from_le_bytes([r[3], r[4]]);
                        let on_double_tap = u16::from_le_bytes([r[5], r[6]]);
                        let on_tap_hold = u16::from_le_bytes([r[7], r[8]]);
                        let tapping_term = u16::from_le_bytes([r[9], r[10]]);
                        if on_tap != 0 || on_hold != 0 || on_double_tap != 0 || on_tap_hold != 0 {
                            tap_dances.push(json!({
                                "index": idx,
                                "on_tap": on_tap,
                                "on_hold": on_hold,
                                "on_double_tap": on_double_tap,
                                "on_tap_hold": on_tap_hold,
                                "tapping_term": tapping_term,
                            }));
                        }
                    }
                }
            }
            let doc = json!({
                "format": "openflux-vial-setup-v1",
                "layers": layers,
                "tap_dances": tap_dances,
            });
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
            let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
            std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
            print_out(
                cli.json,
                &json!({ "ok": true, "path": path, "layers": 8, "tap_dances": tap_dances.len() }),
                &format!(
                    "Exported 8 layers + {} tap dances -> {}",
                    tap_dances.len(),
                    path.display()
                ),
            );
            Ok(())
        }
        VialCmd::Import { path } => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            let doc: Value = serde_json::from_str(&text)
                .map_err(|e| format!("parse {}: {e}", path.display()))?;
            let layers = doc["layers"]
                .as_array()
                .ok_or_else(|| "Missing 'layers' array".to_string())?;
            let mut written_keys = 0usize;
            for (l_idx, layer_val) in layers.iter().take(8).enumerate() {
                let Some(rows) = layer_val.as_array() else { continue };
                for (r_idx, row_val) in rows.iter().take(6).enumerate() {
                    let Some(cols) = row_val.as_array() else { continue };
                    for (c_idx, kc_val) in cols.iter().take(16).enumerate() {
                        let kc = if let Some(n) = kc_val.as_u64() {
                            n as u16
                        } else if let Some(s) = kc_val.as_str() {
                            parse_u16_keycode(s)?
                        } else {
                            continue;
                        };
                        // Only write keys that differ from live EEPROM so imports finish fast!
                        let mut get_req = [0u8; 32];
                        get_req[0] = 0x04;
                        get_req[1] = l_idx as u8;
                        get_req[2] = r_idx as u8;
                        get_req[3] = c_idx as u8;
                        let cur = vial_xfer(&base, &get_req)?;
                        let cur_kc = ((cur[4] as u16) << 8) | (cur[5] as u16);
                        if cur_kc != kc {
                            let mut set_req = [0u8; 32];
                            set_req[0] = 0x05;
                            set_req[1] = l_idx as u8;
                            set_req[2] = r_idx as u8;
                            set_req[3] = c_idx as u8;
                            set_req[4] = (kc >> 8) as u8;
                            set_req[5] = (kc & 0xFF) as u8;
                            let _ = vial_xfer(&base, &set_req)?;
                            written_keys += 1;
                        }
                    }
                }
            }
            let mut written_tds = 0usize;
            if let Some(tds) = doc["tap_dances"].as_array() {
                for td in tds {
                    let idx = td["index"].as_u64().unwrap_or(0) as u8;
                    if idx >= 32 {
                        continue;
                    }
                    let on_tap = td["on_tap"].as_u64().unwrap_or(0) as u16;
                    let on_hold = td["on_hold"].as_u64().unwrap_or(0) as u16;
                    let on_dt = td["on_double_tap"].as_u64().unwrap_or(0) as u16;
                    let on_th = td["on_tap_hold"].as_u64().unwrap_or(0) as u16;
                    let term = td["tapping_term"].as_u64().unwrap_or(200) as u16;
                    let mut req = [0u8; 32];
                    req[0] = 0xFE;
                    req[1] = 0x0D;
                    req[2] = 0x02; // dynamic_vial_tap_dance_set
                    req[3] = idx;
                    req[4..6].copy_from_slice(&on_tap.to_le_bytes());
                    req[6..8].copy_from_slice(&on_hold.to_le_bytes());
                    req[8..10].copy_from_slice(&on_dt.to_le_bytes());
                    req[10..12].copy_from_slice(&on_th.to_le_bytes());
                    req[12..14].copy_from_slice(&term.to_le_bytes());
                    let _ = vial_xfer(&base, &req)?;
                    written_tds += 1;
                }
            }
            print_out(
                cli.json,
                &json!({ "ok": true, "path": path, "updated_keys": written_keys, "tap_dances": written_tds }),
                &format!(
                    "Imported {} ({written_keys} keycodes updated, {written_tds} tap dances written)",
                    path.display()
                ),
            );
            Ok(())
        }
    }
}

fn handle_keyboard(cli: &Cli, cmd: &KeyboardCmd) -> Result<(), String> {
    let base = require_live_daemon(cli)?;
    match cmd {
        KeyboardCmd::Sync => {
            let val = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/keyboard/sync"),
                Some(&json!({})),
                120_000,
            )?;
            let count = val["uploaded_files"].as_array().map(|a| a.len()).unwrap_or(0);
            let skipped = val["skipped_unchanged"].as_u64().unwrap_or(0);
            let bytes = val["uploaded_bytes"].as_u64().unwrap_or(0);
            let verified = val["persisted_verified"].as_bool().unwrap_or(false);
            let rt_applied = match val["runtime_applied"].as_bool() {
                Some(true) => "applied",
                Some(false) => "pending",
                None => "unconfirmed",
            };
            let err_note = val["persisted_error"]
                .as_str()
                .map(|e| format!(" · ERROR: {e}"))
                .unwrap_or_default();
            print_out(
                cli.json,
                &val,
                &format!(
                    "Synced v2 setup to keyboard ({count} uploaded, {skipped} unchanged, {bytes} bytes · persisted_verified={verified} · runtime_applied={rt_applied}{err_note})"
                ),
            );
            Ok(())
        }
        KeyboardCmd::UploadMedia { path, name } => {
            let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let val = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/keyboard/upload-media"),
                Some(&json!({ "local_path": abs.to_string_lossy(), "remote_name": name })),
                180_000,
            )?;
            print_out(
                cli.json,
                &val,
                &format!(
                    "Uploaded {} -> {} ({} bytes)",
                    abs.display(),
                    val["remote_path"].as_str().unwrap_or(""),
                    val["bytes"].as_u64().unwrap_or(0)
                ),
            );
            Ok(())
        }
        KeyboardCmd::Sideload { ipk } => {
            let abs = std::fs::canonicalize(ipk).unwrap_or_else(|_| ipk.clone());
            let val = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/keyboard/sideload"),
                Some(&json!({ "ipk_path": abs.to_string_lossy() })),
                180_000,
            )?;
            print_out(
                cli.json,
                &val,
                &format!(
                    "Sideloaded {}: {}",
                    abs.display(),
                    val["message"].as_str().unwrap_or("ok")
                ),
            );
            Ok(())
        }
        KeyboardCmd::FlashUf2 { uf2 } => {
            let abs = std::fs::canonicalize(uf2).unwrap_or_else(|_| uf2.clone());
            let val = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/keyboard/flash-uf2"),
                Some(&json!({ "uf2_path": abs.to_string_lossy() })),
                180_000,
            )?;
            print_out(
                cli.json,
                &val,
                &format!(
                    "Flashed {}: {}",
                    abs.display(),
                    val["message"].as_str().unwrap_or("ok")
                ),
            );
            Ok(())
        }
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
            tag,
            enabled_only,
        } => {
            if let Some(base) = live {
                let val = daemon_json(reqwest::Method::GET, &format!("{base}/v2/modules"), None, 5000)?;
                let filter_cat = category.map(|c| serde_json::to_value(ModuleCategory::from(c)).unwrap());
                let filter_tag = tag.as_deref().map(|t| t.to_ascii_lowercase());
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
                        if let Some(ref ft) = filter_tag {
                            let has_tag = m["manifest"]["tags"]
                                .as_array()
                                .is_some_and(|ts| ts.iter().any(|x| x.as_str().is_some_and(|s| s.eq_ignore_ascii_case(ft))));
                            if !has_tag {
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
            let filter_tag = tag.as_deref().map(|t| t.to_ascii_lowercase());
            let mut items = Vec::new();
            let mut lines = Vec::new();
            for (id, dm) in &discovered {
                if let Some(cat) = filter_cat {
                    if dm.manifest.category != cat {
                        continue;
                    }
                }
                if let Some(ref ft) = filter_tag {
                    if !dm.manifest.tags.iter().any(|t| t.eq_ignore_ascii_case(ft)) {
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
        ModulesCmd::Inspect { source } => {
            let preview = reg.preview_install_source(source)?;
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
            source,
            inspect_only,
            accept_disclosure,
        } => {
            let preview = reg.preview_install_source(source)?;
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
                let resolved_src = {
                    let p = PathBuf::from(source);
                    if p.is_dir() || p.is_file() {
                        std::fs::canonicalize(&p)
                            .unwrap_or(p)
                            .to_string_lossy()
                            .into_owned()
                    } else {
                        source.clone()
                    }
                };
                let body = json!({ "source": resolved_src, "accept_disclosure": true });
                let val = daemon_json(
                    reqwest::Method::POST,
                    &format!("{base}/v2/modules/install"),
                    Some(&body),
                    60_000,
                )?;
                print_out(cli.json, &val, &format!("Installed module live from {resolved_src}"));
                return Ok(());
            }
            let dm = reg.install_from_source(source)?;
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
        ModulesCmd::Uninstall { id } => {
            if let Some(base) = live {
                let val = daemon_json(
                    reqwest::Method::DELETE,
                    &format!("{base}/v2/modules/{id}"),
                    None,
                    5000,
                )?;
                print_out(cli.json, &val, &format!("Uninstalled module {id:?} (live)"));
                return Ok(());
            }
            let tier = reg.uninstall_module(id)?;
            print_out(
                cli.json,
                &json!({ "uninstalled": true, "id": id, "removed_tier": tier }),
                &format!("Uninstalled module {id:?} (tier: {tier:?})"),
            );
            Ok(())
        }
        ModulesCmd::Update {
            id,
            inspect_only,
            accept_disclosure,
        } => {
            let (origin, preview) = reg.preview_update(id)?;
            if *inspect_only || !*accept_disclosure {
                let val = json!({
                    "updated": false,
                    "requires_acceptance": !*inspect_only,
                    "origin": origin,
                    "preview": preview,
                });
                let mut lines = vec![format!("Updating {id} from {}", origin.source)];
                lines.extend(preview.disclosure.summary_lines.clone());
                if let Some(diff) = &preview.update_diff {
                    lines.push("--- Update Diff ---".into());
                    lines.extend(diff.summary_lines.clone());
                }
                if !*inspect_only {
                    lines.push("\nPass --accept-disclosure to apply this update.".into());
                }
                print_out(cli.json, &val, &lines.join("\n"));
                return Ok(());
            }
            if let Some(base) = live {
                let val = daemon_json(
                    reqwest::Method::POST,
                    &format!("{base}/v2/modules/{id}/update"),
                    Some(&json!({ "accept_disclosure": true })),
                    60_000,
                )?;
                print_out(cli.json, &val, &format!("Updated module {id:?} live from {}", origin.source));
                return Ok(());
            }
            let dm = reg.update_module_from_source(id)?;
            print_out(
                cli.json,
                &json!({ "updated": true, "module": dm, "origin": origin }),
                &format!("Updated {} -> v{}", dm.manifest.id, dm.manifest.version),
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
        ModulesCmd::BindingLayout { id, import } => {
            use std::io::Read;
            let base = require_live_daemon(cli)?;
            let path = format!("{base}/v2/modules/{id}/binding-layout");
            let val = if let Some(file) = import {
                const MAX_BYTES: u64 = 512 * 1024;
                let mut bytes = Vec::new();
                std::fs::File::open(&file)
                    .and_then(|f| f.take(MAX_BYTES + 1).read_to_end(&mut bytes))
                    .map_err(|e| format!("read {}: {e}", file.display()))?;
                if bytes.len() as u64 > MAX_BYTES {
                    return Err("Binding export exceeds 512 KiB".into());
                }
                let contents = String::from_utf8(bytes).map_err(|_| "Binding export must be UTF-8")?;
                daemon_json(reqwest::Method::POST, &path, Some(&json!({ "contents": contents })), 120_000)?
            } else {
                daemon_json(reqwest::Method::GET, &path, None, 5000)?
            };
            let text = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
            print_out(cli.json, &val, &text);
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
        ModulesCmd::Action { id, action_id } => {
            let base = require_live_daemon(cli)?;
            let val = daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/modules/{id}/action"),
                Some(&json!({ "action_id": action_id })),
                5000,
            )?;
            print_out(cli.json, &val, &format!("Triggered action {action_id} on {id}"));
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
                    slot_pos: Some(0.0),
                    slot_span: None,
                    pin_tab: None,
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
                // Never use `daemon_client` (which attaches the local first-party bearer token and
                // disables TLS verification for loopback self-signed certs!) for remote catalog URLs.
                let catalog_http = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_millis(8000))
                    .build()
                    .map_err(|e| e.to_string())?;
                catalog_http
                    .get(&source)
                    .send()
                    .and_then(|r| r.error_for_status())
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
        PresetCmd::Import {
            path,
            install_missing,
            accept_disclosure,
        } => {
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let preset: SetupPreset =
                serde_json::from_str(&text).map_err(|e| format!("invalid preset JSON: {e}"))?;
            let mut auto_installed = Vec::new();
            if *install_missing && *accept_disclosure {
                let discovered = reg.discover();
                for (id, src_ref) in &preset.module_sources {
                    if !discovered.contains_key(id) {
                        if let Some(src) = &src_ref.source {
                            if let Ok(dm) = reg.install_from_source(src) {
                                auto_installed.push(dm.manifest.id);
                            }
                        }
                    }
                }
            }
            if let Some(base) = live {
                let mut val = daemon_json(
                    reqwest::Method::POST,
                    &format!("{base}/v2/preset/import"),
                    Some(&serde_json::to_value(&preset).unwrap()),
                    5000,
                )?;
                if !auto_installed.is_empty() {
                    val["auto_installed"] = json!(auto_installed);
                }
                let missing_cnt = val["missing_modules"].as_array().map(|a| a.len()).unwrap_or(0);
                let mut msg = format!("Imported preset {:?} (live)", preset.name);
                if missing_cnt > 0 {
                    msg.push_str(&format!(
                        " — {missing_cnt} missing module(s) reported (re-run with --install-missing --accept-disclosure to fetch from source)"
                    ));
                }
                print_out(cli.json, &val, &msg);
                return Ok(());
            }
            let report = reg.import_preset_detailed(&preset)?;
            let mut msg = format!(
                "Imported preset {:?} ({} modules configured)",
                preset.name, report.imported_count
            );
            if !report.missing_modules.is_empty() {
                msg.push_str(&format!(
                    "\nMissing {} module(s):",
                    report.missing_modules.len()
                ));
                for m in &report.missing_modules {
                    msg.push_str(&format!(
                        "\n  - {} (v{}, source: {})",
                        m.id,
                        m.version.as_deref().unwrap_or("?"),
                        m.source.as_deref().unwrap_or("none")
                    ));
                }
            }
            print_out(
                cli.json,
                &json!({
                    "imported": preset.name,
                    "modules": report.imported_count,
                    "missing_modules": report.missing_modules,
                    "auto_installed": auto_installed,
                }),
                &msg,
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

/// Splits a Key Rules `keys` spec into a `keys/signal` selector (`group`, `action` or `keys`).
fn signal_selector(spec: &str) -> Value {
    let groups = ["all", "fkeys", "numbers", "letters", "home_row", "mods", "arrows", "nav"];
    let t = spec.trim();
    let low = t.to_ascii_lowercase();
    if groups.contains(&low.as_str()) || low.starts_with("row:") {
        return json!({ "group": low });
    }
    if let Some(a) = t.strip_prefix("action:") {
        return json!({ "action": a.trim() });
    }
    let keys: Vec<Value> = t
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| k.parse::<u64>().map(Value::from).unwrap_or_else(|_| Value::from(k)))
        .collect();
    json!({ "keys": keys })
}

fn key_rules_state(base: &str) -> Result<(serde_json::Map<String, Value>, Vec<Value>), String> {
    let val = daemon_json(reqwest::Method::GET, &format!("{base}/v2/modules/keys-custom"), None, 5000)?;
    let cfg = val["state"]["config"].as_object().cloned().unwrap_or_default();
    let rules = cfg.get("rules").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    Ok((cfg, rules))
}

fn save_key_rules(base: &str, rules: Vec<Value>) -> Result<Value, String> {
    daemon_json(
        reqwest::Method::PUT,
        &format!("{base}/v2/modules/keys-custom/state"),
        Some(&json!({ "config": { "rules": rules } })),
        5000,
    )
}

fn find_rule(rules: &[Value], which: &str) -> Result<usize, String> {
    if let Ok(n) = which.parse::<usize>() {
        if n >= 1 && n <= rules.len() {
            return Ok(n - 1);
        }
        return Err(format!("no rule #{n} (there are {})", rules.len()));
    }
    rules
        .iter()
        .position(|r| r["name"].as_str() == Some(which))
        .ok_or_else(|| format!("no rule named {which:?}"))
}

fn describe_rule(i: usize, r: &Value) -> String {
    let mut look = Vec::new();
    for (k, word) in [("hide", "hide"), ("label", "label"), ("icon", "icon"), ("glow", "glow"), ("outline", "outline"), ("backdrop", "backdrop")] {
        if let Some(v) = r[k].as_str().filter(|v| !v.is_empty() && *v != "none" && *v != "nothing") {
            look.push(format!("{word} {v}"));
        }
    }
    let mut when = Vec::new();
    if let Some(l) = r["layer"].as_str().filter(|l| *l != "any" && !l.is_empty()) {
        when.push(format!("layer {l}"));
    }
    if let Some(a) = r["app"].as_str().filter(|a| !a.is_empty()) {
        when.push(format!("in {a}"));
    }
    if let Some(t) = r["condition_topic"].as_str().filter(|t| !t.is_empty()) {
        when.push(format!(
            "{}{} {} {}",
            t,
            r["condition_path"].as_str().filter(|p| !p.is_empty()).map(|p| format!(":{p}")).unwrap_or_default(),
            r["condition_op"].as_str().unwrap_or("eq"),
            r["condition_value"].as_str().unwrap_or("")
        ));
    }
    format!(
        "{:>2}. {}{} on {}{}: {}",
        i + 1,
        if r["enabled"] == Value::Bool(false) { "(off) " } else { "" },
        r["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("rule"),
        r["keys"].as_str().unwrap_or("?"),
        if when.is_empty() { String::new() } else { format!(" when {}", when.join(", ")) },
        if look.is_empty() { "no change".into() } else { look.join(", ") }
    )
}

fn handle_keys(cli: &Cli, cmd: &KeysCmd) -> Result<(), String> {
    let base = require_live_daemon(cli)?;
    match cmd {
        KeysCmd::Explain { key } => {
            let key_val = key.parse::<u64>().map(Value::from).unwrap_or_else(|_| Value::from(key.as_str()));
            let status = daemon_json(reqwest::Method::GET, &format!("{base}/v2/status"), None, 3000)?;
            let since = status["latest_bus_seq"].as_u64().unwrap_or(0);
            let request_id = format!("fluxctl-{}", std::process::id());
            daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/bus/pub"),
                Some(&json!({ "topic": "keys/inspect/request", "payload": { "key": key_val, "request_id": request_id } })),
                5000,
            )?;
            let client = daemon_client(6000)?;
            let started = Instant::now();
            let mut since_seq = since;
            while started.elapsed() < Duration::from_millis(4000) {
                let resp = client
                    .get(format!("{base}/v2/bus/events"))
                    .query(&[("pattern", "keys/inspect/result"), ("since_seq", &since_seq.to_string()), ("wait_ms", "1500")])
                    .send()
                    .map_err(|e| format!("bus poll failed: {e}"))?;
                let body: Value = resp.json().map_err(|e| e.to_string())?;
                since_seq = body["latest_seq"].as_u64().unwrap_or(since_seq);
                for ev in body["events"].as_array().cloned().unwrap_or_default() {
                    let p = &ev["payload"];
                    if p["request_id"].as_str() != Some(request_id.as_str()) {
                        continue;
                    }
                    let mut lines = vec![format!(
                        "Key {} (layer {}, mods {}): rules from lowest to highest precedence",
                        p["key"], p["layer"], p["mods"]
                    )];
                    for st in p["steps"].as_array().cloned().unwrap_or_default() {
                        lines.push(format!(
                            "  {} {} #{} prio {} {}{}",
                            if st["matched"] == Value::Bool(true) { "✓" } else { "·" },
                            st["provider"].as_str().unwrap_or("?"),
                            st["rule"],
                            st["priority"],
                            st["mode"].as_str().unwrap_or(""),
                            st["skipped"].as_str().map(|w| format!("  (skipped: {w})")).unwrap_or_default()
                        ));
                    }
                    lines.push(format!("  face: {}", p["face"]));
                    if !p["signal"].is_null() {
                        lines.push(format!("  signal active: {}", p["signal"]));
                    }
                    print_out(cli.json, p, &lines.join("\n"));
                    return Ok(());
                }
            }
            Err("no answer from keys-renderer (is it enabled?)".into())
        }
        KeysCmd::Signal { keys, outline, color, pulse, duration_ms, id, clear } => {
            let mut payload = signal_selector(keys);
            let part = if *outline { "outline" } else { "glow" };
            let mut look = serde_json::Map::new();
            look.insert("pulse".into(), Value::Bool(*pulse));
            if let Some(c) = color {
                look.insert("color".into(), Value::from(c.as_str()));
            }
            payload["set"] = json!({ part: Value::Object(look) });
            payload["duration_ms"] = Value::from(*duration_ms);
            payload["id"] = Value::from(id.clone().unwrap_or_else(|| "fluxctl".into()));
            if *clear {
                payload["clear"] = Value::Bool(true);
            }
            daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/bus/pub"),
                Some(&json!({ "topic": "keys/signal", "payload": payload })),
                5000,
            )?;
            print_out(cli.json, &payload, &format!("Signal sent to {keys}"));
            Ok(())
        }
        KeysCmd::Rules { cmd } => {
            let (_cfg, mut rules) = key_rules_state(&base)?;
            match cmd {
                KeyRulesCmd::List => {}
                KeyRulesCmd::Add {
                    keys, name, layer, app, held, caps, when_topic, when_path, when_op, when_value, hide, label, icon,
                    color, glow, glow_color, outline, outline_color, backdrop, corners, at,
                } => {
                    let mut row = serde_json::Map::new();
                    row.insert("keys".into(), Value::from(keys.as_str()));
                    row.insert("enabled".into(), Value::Bool(true));
                    let mut put = |k: &str, v: &Option<String>| {
                        if let Some(v) = v {
                            row.insert(k.into(), Value::from(v.as_str()));
                        }
                    };
                    put("name", name);
                    put("app", app);
                    put("held", held);
                    put("caps", caps);
                    put("condition_topic", when_topic);
                    put("condition_path", when_path);
                    put("condition_value", when_value);
                    put("hide", hide);
                    put("label", label);
                    put("icon", icon);
                    put("color", color);
                    put("glow", glow);
                    put("glow_color", glow_color);
                    put("outline", outline);
                    put("outline_color", outline_color);
                    put("backdrop", backdrop);
                    if when_topic.is_some() {
                        row.insert("condition_op".into(), Value::from(when_op.as_str()));
                    }
                    if let Some(l) = layer {
                        row.insert("layer".into(), Value::from(l.to_string()));
                    }
                    for c in corners {
                        let (slot, v) = c.split_once('=').ok_or_else(|| format!("--corner expects SLOT=VALUE, got {c:?}"))?;
                        if !["tl", "tr", "bl", "br"].contains(&slot) {
                            return Err(format!("corner slot must be tl, tr, bl or br, got {slot:?}"));
                        }
                        row.insert(format!("corner_{slot}"), Value::from(v));
                    }
                    let pos = at.map(|a| a.saturating_sub(1).min(rules.len())).unwrap_or(rules.len());
                    rules.insert(pos, Value::Object(row));
                }
                KeyRulesCmd::Remove { rule } => {
                    let i = find_rule(&rules, rule)?;
                    rules.remove(i);
                }
                KeyRulesCmd::Enable { rule } | KeyRulesCmd::Disable { rule } => {
                    let i = find_rule(&rules, rule)?;
                    rules[i]["enabled"] = Value::Bool(matches!(cmd, KeyRulesCmd::Enable { .. }));
                }
                KeyRulesCmd::Move { rule, to } => {
                    let i = find_rule(&rules, rule)?;
                    let r = rules.remove(i);
                    rules.insert(to.saturating_sub(1).min(rules.len()), r);
                }
            }
            if !matches!(cmd, KeyRulesCmd::List) {
                save_key_rules(&base, rules.clone())?;
            }
            let human = if rules.is_empty() {
                "No Key Rules.".to_string()
            } else {
                rules.iter().enumerate().map(|(i, r)| describe_rule(i, r)).collect::<Vec<_>>().join("\n")
            };
            print_out(cli.json, &json!({ "rules": rules }), &human);
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
            net_down: "1.4 MB/s".into(),
            net_up: "280 KB/s".into(),
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
            city: "Denver".into(),
            temp: 56.4,
            high: 62.0,
            low: 39.0,
            condition: "Partly Cloudy".into(),
            is_day: true,
            celsius: false,
            ready: true,
            aqi: Some(24),
            sunrise: "6:54 AM".into(),
            sunset: "4:41 PM".into(),
            utc_offset_sec: Some(-25200),
            hourly: vec![
                openflux_bus::WeatherHourly {
                    hour: "3PM".into(),
                    temp: 58.0,
                    condition: "Partly Cloudy".into(),
                    is_day: true,
                },
                openflux_bus::WeatherHourly {
                    hour: "4PM".into(),
                    temp: 55.0,
                    condition: "Clear".into(),
                    is_day: true,
                },
                openflux_bus::WeatherHourly {
                    hour: "5PM".into(),
                    temp: 49.0,
                    condition: "Clear".into(),
                    is_day: false,
                },
            ],
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
                            slot_pos: prev.slot_pos,
                            slot_span: prev.slot_span,
                            pin_tab: prev.pin_tab,
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
                                slot_pos: None,
                                slot_span: None,
                                pin_tab: None,
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
                slot_pos: slot.map(|s| s as f32),
                slot_span: None,
                pin_tab: None,
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
            x_px: k.visual_x,
            y_px: k.visual_y,
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

fn format_perf_report(val: &Value) -> String {
    let mut out = String::new();
    let host = &val["host"];
    let kb = &val["keyboard_perf"];
    out.push_str(&format!(
        "Keyglass Live Performance Profiler (usb={} · enabled={} · bus_seq={})\n",
        host["usb_protocol"].as_str().unwrap_or("offline"),
        host["enabled_modules"].as_u64().unwrap_or(0),
        host["bus_latest_seq"].as_u64().unwrap_or(0),
    ));
    if kb.is_null() {
        out.push_str("  Keyboard display/perf: waiting for keyboard (requires openflux-display >= 2.3.12)\n");
        return out;
    }
    let iv = &kb["interval_ms"];
    let ru = &kb["render_us"];
    let st = &kb["stages_us"];
    let vid = &kb["video"];
    out.push_str(&format!(
        "  FPS: {:.1}  |  Frame interval: p50={}ms  p95={}ms  p99={}ms  max={}ms\n",
        kb["fps"].as_f64().unwrap_or(0.0),
        iv["p50"], iv["p95"], iv["p99"], iv["max"],
    ));
    out.push_str(&format!(
        "  Render work: p50={}us  p95={}us  p99={}us  max={}us  |  Jank (>22ms): {}/s ({} total)\n",
        ru["p50"], ru["p95"], ru["p99"], ru["max"],
        kb["jank_frames_1s"], kb["jank_frames_total"],
    ));
    out.push_str(&format!(
        "  Stages (avg/max us): ipc_mux={}/{}  lua={}/{}  physics={}/{}  video_up={}/{}  gpu_swap={}/{}\n",
        st["ipc_mux_avg"], st["ipc_mux_max"],
        st["lua_avg"], st["lua_max"],
        st["physics_avg"], st["physics_max"],
        st["video_upload_avg"], st["video_upload_max"],
        st["gpu_swap_avg"], st["gpu_swap_max"],
    ));
    out.push_str(&format!(
        "  Video RKMPP: {:.2} fps  queue={}  underruns={}  uploads={}  |  overlay_uploads/s={}\n",
        vid["fps"].as_f64().unwrap_or(0.0),
        vid["rkmpp_queue"], vid["rkmpp_underruns"], vid["rkmpp_uploads"],
        kb["overlay_uploads_1s"],
    ));
    if let Some(mods) = kb["modules"].as_array() {
        if !mods.is_empty() {
            out.push_str("  Top Lua modules (avg / max us):\n");
            for m in mods.iter().take(8) {
                out.push_str(&format!(
                    "    - {:24} avg={:>5}us  max={:>6}us  events={}\n",
                    m["id"].as_str().unwrap_or("?"),
                    m["avg_us"].as_u64().unwrap_or(0),
                    m["max_us"].as_u64().unwrap_or(0),
                    m["events_last"].as_u64().unwrap_or(0),
                ));
            }
        }
    }
    if let Some(hitches) = kb["recent_hitches"].as_array() {
        if !hitches.is_empty() {
            out.push_str("  Recent hitches (>22ms interval):\n");
            for h in hitches.iter().take(5) {
                out.push_str(&format!(
                    "    - t={}ms  interval={}ms  render={}us  top_stage={}  rkmpp_q={}\n",
                    h["mono_ms"], h["interval_ms"], h["render_us"],
                    h["top_stage"].as_str().unwrap_or("?"), h["rkmpp_queue"],
                ));
            }
        }
    }
    out
}

/// Built-in MCP (Model Context Protocol) JSON-RPC 2.0 stdio server for KeyGlass module developers.
fn run_mcp_stdio_server(cli: &Cli) -> Result<(), String> {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    let tools_schema = json!([
        {
            "name": "keyglass_status",
            "description": "Show live KeyGlass daemon status, USB OFV2 protocol state, connected keyboard versions, and module counts.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "keyglass_perf",
            "description": "Inspect live keyboard & host latency/performance: FPS, frame interval p50/p95/p99/max ms, jank hitches (>22ms), stage breakdown (IPC/Mux, Lua, Physics, Video Upload, GPU Swap), RKMPP video queue/underruns, and per-module Lua execution timings (avg/max us).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "keyglass_modules_list",
            "description": "List all discovered KeyGlass modules with tier (bundled/installed/local), enabled state, health, host helper status, and last error.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "keyglass_module_logs",
            "description": "Fetch recent Lua and host-helper stdout/stderr logs for a specific KeyGlass module ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module_id": { "type": "string", "description": "Module ID (e.g. 'keys-renderer', 'streamdeck-bridge')" }
                },
                "required": ["module_id"]
            }
        },
        {
            "name": "keyglass_bus_retained",
            "description": "Query retained messages on the live KeyGlass hierarchical pub/sub bus (supports MQTT-style wildcards like '#' or 'keys/providers/#').",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Topic pattern (default '#')" }
                }
            }
        },
        {
            "name": "keyglass_bus_publish",
            "description": "Publish a JSON payload onto a KeyGlass bus topic for testing or triggering module behaviors.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": { "type": "string" },
                    "payload": { "type": "object" },
                    "retain": { "type": "boolean" }
                },
                "required": ["topic", "payload"]
            }
        },
        {
            "name": "keyglass_keyboard_sync",
            "description": "Sync local/installed module edits and config from the host directly to the connected Flux keyboard over OFV2.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }
    ]);

    for line_res in stdin.lock().lines() {
        let Ok(line) = line_res else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

        // Notifications have no `id` and expect no response.
        if id.is_none() {
            continue;
        }

        let resp = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "keyglass-mcp", "version": env!("CARGO_PKG_VERSION") }
                }
            }),
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tools_schema }
            }),
            "tools/call" => {
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let call_res = handle_mcp_tool_call(cli, tool_name, &args);
                match call_res {
                    Ok(val) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&val).unwrap_or_default() }],
                            "isError": false
                        }
                    }),
                    Err(e) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": e }],
                            "isError": true
                        }
                    }),
                }
            }
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            }),
        };

        let _ = writeln!(stdout, "{}", resp);
        let _ = stdout.flush();
    }
    Ok(())
}

fn handle_mcp_tool_call(cli: &Cli, name: &str, args: &Value) -> Result<Value, String> {
    let base = detect_live_daemon(cli)
        .ok_or_else(|| "openfluxd is not running; start KeyGlass first".to_string())?;
    match name {
        "keyglass_status" => {
            let st = daemon_json(reqwest::Method::GET, &format!("{base}/v2/status"), None, 3000)?;
            let sw = daemon_json(reqwest::Method::GET, &format!("{base}/v2/keyboard/software-status"), None, 3000).ok();
            Ok(json!({ "daemon": st, "keyboard_software": sw }))
        }
        "keyglass_perf" => daemon_json(reqwest::Method::GET, &format!("{base}/v2/perf"), None, 3000),
        "keyglass_modules_list" => {
            let mut val = daemon_json(reqwest::Method::GET, &format!("{base}/v2/modules"), None, 3000)?;
            // Redact settings marked `kind == "secret"` before returning diagnostics to MCP clients.
            if let Some(arr) = val.get_mut("modules").and_then(|v| v.as_array_mut()) {
                for m in arr {
                    let secret_keys: Vec<String> = m
                        .pointer("/manifest/settings")
                        .and_then(|s| s.as_array())
                        .map(|list| {
                            list.iter()
                                .filter(|f| {
                                    f.get("secret").and_then(|v| v.as_bool()) == Some(true)
                                        || f.get("kind").and_then(|k| k.as_str()) == Some("secret")
                                        || f.get("kind").and_then(|k| k.as_str()) == Some("password")
                                })
                                .filter_map(|f| f.get("key").and_then(|k| k.as_str()).map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Some(cfg) = m.pointer_mut("/state/config").and_then(|c| c.as_object_mut()) {
                        let all_keys: Vec<String> = cfg.keys().cloned().collect();
                        for k in all_keys {
                            let low = k.to_ascii_lowercase();
                            let looks_secret = secret_keys.contains(&k)
                                || low.contains("token")
                                || low.contains("secret")
                                || low.contains("password")
                                || low.contains("api_key")
                                || low.contains("apikey");
                            if looks_secret {
                                if let Some(slot) = cfg.get_mut(&k) {
                                    if slot.as_str().is_some_and(|s| !s.is_empty()) {
                                        *slot = json!("[REDACTED]");
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(val)
        }
        "keyglass_module_logs" => {
            let mid = args
                .get("module_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing module_id".to_string())?;
            if mid.is_empty()
                || !mid
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(format!("invalid module_id {mid:?}"));
            }
            daemon_json(reqwest::Method::GET, &format!("{base}/v2/modules/{mid}/logs"), None, 3000)
        }
        "keyglass_bus_retained" => {
            let pat = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("#");
            let enc = pat.replace('#', "%23").replace('+', "%2B");
            daemon_json(reqwest::Method::GET, &format!("{base}/v2/bus/retained?pattern={enc}"), None, 3000)
        }
        "keyglass_bus_publish" => {
            let topic = args
                .get("topic")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing topic".to_string())?;
            let payload = args.get("payload").cloned().unwrap_or_else(|| json!({}));
            let retain = args.get("retain").and_then(|v| v.as_bool()).unwrap_or(false);
            let body = json!({ "topic": topic, "payload": payload, "retain": retain });
            daemon_json(
                reqwest::Method::POST,
                &format!("{base}/v2/bus/pub"),
                Some(&body),
                3000,
            )
        }
        "keyglass_keyboard_sync" => {
            let body = json!({});
            daemon_json(reqwest::Method::POST, &format!("{base}/v2/keyboard/sync"), Some(&body), 30000)
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}
