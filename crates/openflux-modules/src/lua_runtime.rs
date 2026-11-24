//! Embedded Lua 5.4 runtime with per-module state isolation, live hot-reload, structured logs,
//! pub/sub integration, keyboard geometry queries, and compositor IR emission.

use crate::ir::{
    DrawCommand, FrameScene, KeyGlowCmd, KeyGlowShape, ParticleSpawnCmd, ShaderPassCmd, TextAlign,
    WaveFieldParams, WaveImpulseCmd,
};
use crate::manifest::CompositorStage;
use crate::registry::{DiscoveredModule, ModuleInstanceState};
use mlua::{
    Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, UserData, UserDataMethods,
    Value as LuaValue, VmState,
};
use openflux_bus::{Broker, KeyboardGeometry, PublishOptions, SubscribeOptions, Subscription};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

const MAX_MODULE_LOGS: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleLogEntry {
    pub timestamp_ms: u64,
    pub level: String,
    pub message: String,
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<VecDeque<ModuleLogEntry>>>);

impl LogBuffer {
    fn push(&self, level: impl Into<String>, message: impl Into<String>) {
        let mut g = self.0.lock().unwrap();
        if g.len() >= MAX_MODULE_LOGS {
            g.pop_front();
        }
        g.push_back(ModuleLogEntry {
            timestamp_ms: openflux_bus::now_unix_ms(),
            level: level.into(),
            message: message.into(),
        });
    }

    fn snapshot(&self) -> Vec<ModuleLogEntry> {
        self.0.lock().unwrap().iter().cloned().collect()
    }
}

pub fn parse_hex_color(hex: &str, fallback: [f32; 4]) -> [f32; 4] {
    let s = hex.trim().trim_start_matches('#');
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok();
            let g = u8::from_str_radix(&s[2..4], 16).ok();
            let b = u8::from_str_radix(&s[4..6], 16).ok();
            if let (Some(r), Some(g), Some(b)) = (r, g, b) {
                return [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
            }
            fallback
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok();
            let g = u8::from_str_radix(&s[2..4], 16).ok();
            let b = u8::from_str_radix(&s[4..6], 16).ok();
            let a = u8::from_str_radix(&s[6..8], 16).ok();
            if let (Some(r), Some(g), Some(b), Some(a)) = (r, g, b, a) {
                return [
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    a as f32 / 255.0,
                ];
            }
            fallback
        }
        _ => fallback,
    }
}

fn lua_color(val: Option<LuaValue>, fallback: [f32; 4]) -> [f32; 4] {
    let Some(val) = val else { return fallback };
    match val {
        LuaValue::String(s) => s
            .to_str()
            .map(|st| parse_hex_color(&st, fallback))
            .unwrap_or(fallback),
        LuaValue::Table(t) => {
            let r: f32 = t.get(1).unwrap_or(fallback[0]);
            let g: f32 = t.get(2).unwrap_or(fallback[1]);
            let b: f32 = t.get(3).unwrap_or(fallback[2]);
            let a: f32 = t.get(4).unwrap_or(1.0);
            [r, g, b, a]
        }
        _ => fallback,
    }
}

/// Per-frame render context handed to a Lua module's `on_render(ctx, stage)`.
struct RenderCtxHandle {
    module_id: String,
    module_dir: PathBuf,
    stage: CompositorStage,
    scene: Arc<Mutex<FrameScene>>,
}

impl RenderCtxHandle {
    fn push_draw(&self, cmd: DrawCommand) {
        let mut s = self.scene.lock().unwrap();
        match self.stage {
            CompositorStage::UnderKey => s.under_key_draws.push(cmd),
            CompositorStage::Tile => s.tile_draws.push(cmd),
            CompositorStage::WorldOverlay => s.world_overlay_draws.push(cmd),
            _ => s.world_overlay_draws.push(cmd),
        }
    }
}

impl UserData for RenderCtxHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("clip", |_, this, (x, y, w, h): (f32, f32, f32, f32)| {
            this.push_draw(DrawCommand::Clip { x, y, w, h });
            Ok(())
        });
        methods.add_method("clear_clip", |_, this, ()| {
            this.push_draw(DrawCommand::ClearClip);
            Ok(())
        });
        methods.add_method(
            "rect",
            |_, this, (x, y, w, h, color): (f32, f32, f32, f32, Option<LuaValue>)| {
                this.push_draw(DrawCommand::Rect {
                    x,
                    y,
                    w,
                    h,
                    color: lua_color(color, [1.0, 1.0, 1.0, 1.0]),
                });
                Ok(())
            },
        );
        methods.add_method(
            "round_rect",
            |_,
             this,
             (x, y, w, h, radius, color, stroke_width, stroke_color): (
                f32,
                f32,
                f32,
                f32,
                f32,
                Option<LuaValue>,
                Option<f32>,
                Option<LuaValue>,
            )| {
                this.push_draw(DrawCommand::RoundRect {
                    x,
                    y,
                    w,
                    h,
                    radius,
                    color: lua_color(color, [0.1, 0.1, 0.15, 0.8]),
                    stroke_width: stroke_width.unwrap_or(0.0),
                    stroke_color: lua_color(stroke_color, [0.0, 0.0, 0.0, 0.0]),
                });
                Ok(())
            },
        );
        methods.add_method(
            "circle",
            |_,
             this,
             (cx, cy, r, color, stroke_width, stroke_color): (
                f32,
                f32,
                f32,
                Option<LuaValue>,
                Option<f32>,
                Option<LuaValue>,
            )| {
                this.push_draw(DrawCommand::Circle {
                    cx,
                    cy,
                    r,
                    color: lua_color(color, [1.0, 1.0, 1.0, 1.0]),
                    stroke_width: stroke_width.unwrap_or(0.0),
                    stroke_color: lua_color(stroke_color, [0.0, 0.0, 0.0, 0.0]),
                });
                Ok(())
            },
        );
        methods.add_method(
            "ellipse",
            |_, this, (cx, cy, rx, ry, color): (f32, f32, f32, f32, Option<LuaValue>)| {
                this.push_draw(DrawCommand::Ellipse {
                    cx,
                    cy,
                    rx,
                    ry,
                    color: lua_color(color, [1.0, 1.0, 1.0, 1.0]),
                });
                Ok(())
            },
        );
        methods.add_method(
            "line",
            |_,
             this,
             (x1, y1, x2, y2, width, color): (f32, f32, f32, f32, f32, Option<LuaValue>)| {
                this.push_draw(DrawCommand::Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    width,
                    color: lua_color(color, [1.0, 1.0, 1.0, 1.0]),
                });
                Ok(())
            },
        );
        methods.add_method(
            "arc",
            |_,
             this,
             (cx, cy, r, start_deg, end_deg, width, color): (
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                Option<LuaValue>,
            )| {
                this.push_draw(DrawCommand::Arc {
                    cx,
                    cy,
                    r,
                    start_deg,
                    end_deg,
                    width,
                    color: lua_color(color, [1.0, 1.0, 1.0, 1.0]),
                });
                Ok(())
            },
        );
        methods.add_method(
            "text",
            |_,
             this,
             (x, y, text, size, color, align): (
                f32,
                f32,
                String,
                f32,
                Option<LuaValue>,
                Option<String>,
            )| {
                let align = match align.as_deref() {
                    Some("center") => TextAlign::Center,
                    Some("right") => TextAlign::Right,
                    _ => TextAlign::Left,
                };
                this.push_draw(DrawCommand::Text {
                    x,
                    y,
                    text,
                    size,
                    color: lua_color(color, [1.0, 1.0, 1.0, 1.0]),
                    align,
                });
                Ok(())
            },
        );
        methods.add_method(
            "image",
            |_,
             this,
             (x, y, w, h, path, tint): (f32, f32, f32, f32, String, Option<LuaValue>)| {
                let resolved = if Path::new(&path).is_absolute() {
                    path
                } else {
                    this.module_dir.join(path).to_string_lossy().into_owned()
                };
                this.push_draw(DrawCommand::Image {
                    x,
                    y,
                    w,
                    h,
                    path: resolved,
                    tint: lua_color(tint, [1.0, 1.0, 1.0, 1.0]),
                });
                Ok(())
            },
        );
        methods.add_method(
            "key_glow",
            |_,
             this,
             (shape, cx, cy, w, h, radius_px, thickness_px, intensity, color): (
                String,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                Option<LuaValue>,
            )| {
                let shape = match shape.as_str() {
                    "ring" => KeyGlowShape::Ring,
                    "ripple" => KeyGlowShape::Ripple,
                    "solid" => KeyGlowShape::Solid,
                    _ => KeyGlowShape::Glow,
                };
                this.scene.lock().unwrap().key_glows.push(KeyGlowCmd {
                    shape,
                    cx,
                    cy,
                    w,
                    h,
                    radius_px,
                    thickness_px,
                    intensity,
                    color: lua_color(color, [0.3, 0.85, 1.0, 1.0]),
                });
                Ok(())
            },
        );
        methods.add_method(
            "wave_params",
            |_,
             this,
             (damping, speed, refraction, specular, tint): (
                f32,
                f32,
                f32,
                f32,
                Option<LuaValue>,
            )| {
                let mut s = this.scene.lock().unwrap();
                s.distortion_enabled = true;
                s.wave_params = WaveFieldParams {
                    damping,
                    speed,
                    refraction_strength: refraction,
                    specular_strength: specular,
                    tint: lua_color(tint, [0.65, 0.88, 1.0, 1.0]),
                };
                Ok(())
            },
        );
        methods.add_method(
            "wave_impulse",
            |_, this, (x, y, radius, force): (f32, f32, f32, f32)| {
                let mut s = this.scene.lock().unwrap();
                s.distortion_enabled = true;
                s.wave_impulses.push(WaveImpulseCmd {
                    x,
                    y,
                    radius,
                    force,
                });
                Ok(())
            },
        );
        methods.add_method("spawn_particle", |_, this, tbl: Table| {
            let color = lua_color(tbl.get("color").ok(), [0.4, 0.9, 1.0, 1.0]);
            this.scene
                .lock()
                .unwrap()
                .particle_spawns
                .push(ParticleSpawnCmd {
                    x: tbl.get("x").unwrap_or(0.0),
                    y: tbl.get("y").unwrap_or(0.0),
                    vx: tbl.get("vx").unwrap_or(0.0),
                    vy: tbl.get("vy").unwrap_or(0.0),
                    radius_px: tbl.get("radius").unwrap_or(5.0),
                    life_ms: tbl.get("life_ms").unwrap_or(900.0),
                    color,
                    bounce_chance: tbl.get("bounce_chance").unwrap_or(0.55),
                    lens_refract: tbl.get("lens_refract").unwrap_or(1.45),
                    drag: tbl.get("drag").unwrap_or(0.985),
                });
            Ok(())
        });
        methods.add_method(
            "shader",
            |_, this, (rel_path, p0, p1, p2, p3): (String, Option<Table>, Option<Table>, Option<Table>, Option<Table>)| {
                let full_path = if Path::new(&rel_path).is_absolute() {
                    rel_path
                } else {
                    this.module_dir.join(rel_path).to_string_lossy().into_owned()
                };
                let to_vec4 = |t: Option<Table>| -> [f32; 4] {
                    match t {
                        Some(t) => [
                            t.get(1).unwrap_or(0.0),
                            t.get(2).unwrap_or(0.0),
                            t.get(3).unwrap_or(0.0),
                            t.get(4).unwrap_or(0.0),
                        ],
                        None => [0.0; 4],
                    }
                };
                let cmd = ShaderPassCmd {
                    module_id: this.module_id.clone(),
                    stage: this.stage,
                    shader_path: full_path,
                    params: [to_vec4(p0), to_vec4(p1), to_vec4(p2), to_vec4(p3)],
                };
                let mut s = this.scene.lock().unwrap();
                match this.stage {
                    CompositorStage::Background => s.background_shaders.push(cmd),
                    CompositorStage::Distortion => {
                        s.distortion_enabled = true;
                        s.distortion_shaders.push(cmd);
                    }
                    CompositorStage::UnderKey => s.under_key_shaders.push(cmd),
                    _ => {}
                }
                Ok(())
            },
        );
        methods.add_method("background_media", |_, this, path: String| {
            let full = if Path::new(&path).is_absolute() {
                path
            } else {
                this.module_dir.join(path).to_string_lossy().into_owned()
            };
            this.scene.lock().unwrap().background_media = Some(full);
            Ok(())
        });
        methods.add_method("stream_background", |_, this, stream_name: String| {
            let trimmed = stream_name.trim().to_string();
            if !trimmed.is_empty() {
                this.scene.lock().unwrap().background_stream = Some(trimmed);
            }
            Ok(())
        });
        methods.add_method(
            "stream_rect",
            |_, this, (x, y, w, h, stream_name, radius): (f32, f32, f32, f32, String, Option<f32>)| {
                let trimmed = stream_name.trim().to_string();
                if !trimmed.is_empty() && w > 2.0 && h > 2.0 {
                    this.scene.lock().unwrap().stream_overlays.push(
                        crate::ir::StreamOverlayCmd {
                            stream_name: trimmed,
                            x,
                            y,
                            w,
                            h,
                            radius: radius.unwrap_or(0.0),
                        },
                    );
                }
                Ok(())
            },
        );
        methods.add_method("background_dimming", |_, this, frac: f32| {
            this.scene.lock().unwrap().bg_dimming = frac.clamp(0.0, 0.9);
            Ok(())
        });
        methods.add_method("top_fade", |_, this, frac: f32| {
            this.scene.lock().unwrap().top_fade = frac.clamp(0.0, 0.85);
            Ok(())
        });
        methods.add_method("keep_layer", |_, this, ()| {
            this.scene.lock().unwrap().keep_under_key_layer = true;
            Ok(())
        });
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModulePerfStats {
    pub id: String,
    pub last_us: u64,
    pub avg_us: u64,
    pub max_us: u64,
    pub events_last: u32,
}

struct LoadedLuaModule {
    discovered: DiscoveredModule,
    instance: ModuleInstanceState,
    lua: Lua,
    subscriptions: Arc<Mutex<Vec<Subscription>>>,
    logs: LogBuffer,
    deadline: Arc<Mutex<Option<std::time::Instant>>>,
    script_mtime: Option<SystemTime>,
    manifest_mtime: Option<SystemTime>,
    healthy: bool,
    last_error: Option<String>,
    last_step_us: u64,
    ema_step_us: f64,
    max_step_us: u64,
    events_last: u32,
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

impl LoadedLuaModule {
    fn load(
        discovered: DiscoveredModule,
        instance: ModuleInstanceState,
        broker: Broker,
        geometry: Arc<RwLock<KeyboardGeometry>>,
    ) -> Self {
        let logs = LogBuffer::default();
        let subscriptions: Arc<Mutex<Vec<Subscription>>> = Arc::new(Mutex::new(Vec::new()));
        let deadline: Arc<Mutex<Option<std::time::Instant>>> = Arc::new(Mutex::new(None));
        // Sandboxed Lua 5.4 VM: NO `io` library, NO `debug` library, NO `os.execute`/`os.remove`/
        // `os.rename`/`os.exit`, NO `dofile`/`loadfile`/`package.loadlib`!
        // Ensures `host_requirement = "none"` modules truly cannot execute host commands.
        let stdlib = StdLib::TABLE
            | StdLib::STRING
            | StdLib::MATH
            | StdLib::UTF8
            | StdLib::PACKAGE
            | StdLib::OS;
        let lua = Lua::new_with(stdlib, LuaOptions::default())
            .expect("failed to initialize sandboxed Lua 5.4 VM");

        // Strip dangerous globals & OS functions immediately after VM creation.
        {
            let g = lua.globals();
            let _ = g.set("dofile", LuaValue::Nil);
            let _ = g.set("loadfile", LuaValue::Nil);
            let _ = g.set("io", LuaValue::Nil);
            if let Ok(pkg) = g.get::<Table>("package") {
                let _ = pkg.set("loadlib", LuaValue::Nil);
            }
            if let Ok(os_tbl) = g.get::<Table>("os") {
                for forbidden in ["execute", "remove", "rename", "tmpname", "exit", "setlocale"] {
                    let _ = os_tbl.set(forbidden, LuaValue::Nil);
                }
                // Restrict `os.getenv` strictly to `HOME` so Lua modules cannot exfiltrate secrets/tokens.
                if let Ok(safe_getenv) = lua.create_function(|_, key: String| {
                    if key == "HOME" {
                        Ok(std::env::var("HOME").ok())
                    } else {
                        Ok(None)
                    }
                }) {
                    let _ = os_tbl.set("getenv", safe_getenv);
                }
            }
        }

        // Execution-budget watchdog: aborts runaway loops (`while true do end`) after 150ms so
        // one broken module can never hang the keyboard display or daemon!
        let dl_hook = deadline.clone();
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(100_000),
            move |_, _| {
                if let Some(dl) = *dl_hook.lock().unwrap() {
                    if std::time::Instant::now() > dl {
                        return Err(mlua::Error::RuntimeError(
                            "module exceeded 150ms execution budget (aborted by watchdog)".into(),
                        ));
                    }
                }
                Ok(VmState::Continue)
            },
        );

        let script_path = discovered
            .manifest
            .entrypoints
            .script
            .as_ref()
            .map(|s| discovered.path.join(s));
        let script_mtime = script_path.as_deref().and_then(file_mtime);
        let manifest_mtime = file_mtime(&discovered.path.join("module.toml"));

        let mut loaded = Self {
            discovered,
            instance,
            lua,
            subscriptions,
            logs,
            deadline,
            script_mtime,
            manifest_mtime,
            healthy: true,
            last_error: None,
            last_step_us: 0,
            ema_step_us: 0.0,
            max_step_us: 0,
            events_last: 0,
        };

        if let Err(e) = loaded.init_bindings(broker, geometry) {
            loaded.record_error(format!("init_bindings failed: {e}"));
            return loaded;
        }

        if let Some(sp) = script_path {
            match std::fs::read_to_string(&sp) {
                Ok(code) => {
                    loaded.arm_watchdog(std::time::Duration::from_millis(300));
                    let load_res = loaded.lua.load(&code).set_name(sp.to_string_lossy()).exec();
                    loaded.clear_watchdog();
                    if let Err(e) = load_res {
                        loaded.record_error(format!("lua load error: {e}"));
                        return loaded;
                    }
                    if let Ok(on_init) = loaded.lua.globals().get::<Function>("on_init") {
                        loaded.arm_watchdog(std::time::Duration::from_millis(300));
                        let init_res = on_init.call::<()>(());
                        loaded.clear_watchdog();
                        if let Err(e) = init_res {
                            loaded.record_error(format!("on_init error: {e}"));
                        }
                    }
                }
                Err(e) => {
                    loaded.record_error(format!("failed to read script {}: {e}", sp.display()));
                }
            }
        }

        loaded
    }

    fn arm_watchdog(&self, budget: std::time::Duration) {
        *self.deadline.lock().unwrap() = Some(std::time::Instant::now() + budget);
    }

    fn clear_watchdog(&self) {
        *self.deadline.lock().unwrap() = None;
    }

    fn record_error(&mut self, msg: String) {
        self.healthy = false;
        self.logs.push("error", &msg);
        self.last_error = Some(msg);
    }

    fn init_bindings(
        &self,
        broker: Broker,
        geometry: Arc<RwLock<KeyboardGeometry>>,
    ) -> mlua::Result<()> {
        let globals = self.lua.globals();

        // Pre-subscribe to manifest channels so modules receive events automatically.
        {
            let mut subs = self.subscriptions.lock().unwrap();
            for pat in &self.discovered.manifest.channels.subscribes {
                if let Ok(sub) = broker.subscribe(pat.clone(), SubscribeOptions::with_retained()) {
                    subs.push(sub);
                }
            }
        }

        // `bus` table
        let bus_tbl = self.lua.create_table()?;
        {
            let broker_pub = broker.clone();
            let mod_origin = format!("module:{}", self.discovered.manifest.id);
            bus_tbl.set(
                "publish",
                self.lua.create_function(
                    move |lua, (topic, payload, retain): (String, LuaValue, Option<bool>)| {
                        let json_val: JsonValue = lua.from_value(payload)?;
                        let opts = PublishOptions {
                            retain: retain.unwrap_or(false),
                            origin: Some(mod_origin.clone()),
                            ..Default::default()
                        };
                        broker_pub
                            .publish(topic, json_val, opts)
                            .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
                        Ok(())
                    },
                )?,
            )?;
        }
        {
            let broker_sub = broker.clone();
            let subs_ref = self.subscriptions.clone();
            bus_tbl.set(
                "subscribe",
                self.lua.create_function(move |_, pattern: String| {
                    let sub = broker_sub
                        .subscribe(pattern, SubscribeOptions::with_retained())
                        .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
                    subs_ref.lock().unwrap().push(sub);
                    Ok(())
                })?,
            )?;
        }
        {
            let broker_get = broker.clone();
            bus_tbl.set(
                "get",
                self.lua.create_function(move |lua, topic: String| {
                    match broker_get.get_retained(&topic) {
                        Some(msg) => lua.to_value(&msg.payload),
                        None => Ok(LuaValue::Nil),
                    }
                })?,
            )?;
        }
        {
            let broker_list = broker.clone();
            bus_tbl.set(
                "list",
                self.lua.create_function(move |lua, pattern: String| {
                    let list = broker_list
                        .query_retained(&pattern)
                        .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
                    lua.to_value(&list)
                })?,
            )?;
        }
        globals.set("bus", bus_tbl)?;

        // `keyboard` table — reads through shared `RwLock<KeyboardGeometry>` so live optical
        // parallax updates (`ModuleRuntime::update_geometry`) immediately reflect in every
        // already-loaded Lua module's `keyboard.layout()`, `nearest_key()`, and `sample_sdf()`!
        let kb_tbl = self.lua.create_table()?;
        {
            let geom_ref = geometry.clone();
            kb_tbl.set(
                "layout",
                self.lua.create_function(move |lua, ()| {
                    let g = geom_ref.read().unwrap();
                    lua.to_value(&*g)
                })?,
            )?;
        }
        {
            let geom_ref = geometry.clone();
            kb_tbl.set(
                "nearest_key",
                self.lua
                    .create_function(move |lua, (x, y): (f32, f32)| {
                        let g = geom_ref.read().unwrap();
                        match g.nearest_key(x, y) {
                            Some(k) => lua.to_value(k),
                            None => Ok(LuaValue::Nil),
                        }
                    })?,
            )?;
        }
        {
            let geom_ref = geometry.clone();
            kb_tbl.set(
                "sample_sdf",
                self.lua
                    .create_function(move |lua, (x, y): (f32, f32)| {
                        let g = geom_ref.read().unwrap();
                        let s = g.sample_key_sdf(x, y);
                        let t = lua.create_table()?;
                        t.set("dist", s.signed_distance_px)?;
                        t.set("nx", s.normal_x)?;
                        t.set("ny", s.normal_y)?;
                        t.set("key_index", s.nearest_key_index)?;
                        Ok(t)
                    })?,
            )?;
        }
        globals.set("keyboard", kb_tbl)?;

        // Point Lua `require()` at this module's folder (and sibling `_shared` helpers)
        // so modules can cleanly split logic across multiple `.lua` files and share libraries!
        if let Ok(pkg) = globals.get::<Table>("package") {
            let mod_root = self.discovered.path.to_string_lossy();
            let shared_root = self
                .discovered
                .path
                .parent()
                .map(|p| p.join("_shared").to_string_lossy().into_owned())
                .unwrap_or_default();
            let existing: String = pkg.get("path").unwrap_or_default();
            let mut search = format!(
                "{mod_root}/?.lua;{mod_root}/?/init.lua;{mod_root}/lib/?.lua"
            );
            if !shared_root.is_empty() {
                search.push_str(&format!(
                    ";{shared_root}/?.lua;{shared_root}/?/init.lua"
                ));
            }
            if !existing.is_empty() {
                search.push(';');
                search.push_str(&existing);
            }
            let _ = pkg.set("path", search);
        }

        // `module` table
        let mod_tbl = self.lua.create_table()?;
        let mod_id = self.discovered.manifest.id.clone();
        let mod_dir = self.discovered.path.to_string_lossy().into_owned();
        let slot_pos = self.instance.effective_slot_pos();
        let slot_span = self.instance.slot_span.unwrap_or(1.0).clamp(0.5, 4.0);
        let fallback_hdr = geometry.read().unwrap().header_rect;

        // Merge manifest defaults with instance overrides.
        let mut merged_cfg = self.discovered.manifest.default_config();
        for (k, v) in &self.instance.config {
            merged_cfg.insert(k.clone(), v.clone());
        }
        let cfg_clone = merged_cfg.clone();
        let cfg_base_clone = merged_cfg.clone();

        mod_tbl.set("id", self.lua.create_function(move |_, ()| Ok(mod_id.clone()))?)?;
        mod_tbl.set(
            "dir",
            self.lua.create_function(move |_, ()| Ok(mod_dir.clone()))?,
        )?;
        // Profile-aware `module.config()`: automatically overlays any per-profile setting
        // overrides from `profiles/active.module_overrides[mod_id]` on top of global config
        // (with automatic parent fall-through!), so ANY module supports per-profile config for free!
        {
            let broker_cfg = broker.clone();
            let my_id_cfg = self.discovered.manifest.id.clone();
            mod_tbl.set(
                "config",
                self.lua.create_function(move |lua, ()| {
                    let mut effective = cfg_clone.clone();
                    if let Some(pmsg) = broker_cfg.get_retained("profiles/active") {
                        if let Some(ov) = pmsg
                            .payload
                            .get("module_overrides")
                            .and_then(|m| m.get(&my_id_cfg))
                            .and_then(|v| v.as_object())
                        {
                            for (k, v) in ov {
                                effective.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    lua.to_value(&effective)
                })?,
            )?;
        }
        mod_tbl.set(
            "base_config",
            self.lua.create_function(move |lua, ()| lua.to_value(&cfg_base_clone))?,
        )?;
        let slot_idx = slot_pos.map(|p| p.floor() as u8);
        mod_tbl.set("slot", self.lua.create_function(move |_, ()| Ok(slot_idx))?)?;
        mod_tbl.set("slot_pos", self.lua.create_function(move |_, ()| Ok(slot_pos))?)?;
        mod_tbl.set("slot_span", self.lua.create_function(move |_, ()| Ok(slot_span))?)?;
        {
            let broker_slot = broker.clone();
            let my_mod_id = self.discovered.manifest.id.clone();
            mod_tbl.set(
                "slot_rect",
                self.lua.create_function(move |lua, ()| {
                    // Per-App "Touch Bar" Context Override: if an active App Context rule specifies
                    // custom top-bar slots on `layout/top_bar/context_override`, follow it cleanly
                    // (showing widgets placed by that rule and hiding others) with zero widget changes!
                    let (eff_pos, eff_span) = if let Some(ov_msg) =
                        broker_slot.get_retained("layout/top_bar/context_override")
                    {
                        if ov_msg.payload.get("active").and_then(|v| v.as_bool()) == Some(true) {
                            let found = ov_msg
                                .payload
                                .get("slots")
                                .and_then(|v| v.as_array())
                                .and_then(|arr| {
                                    arr.iter().find_map(|entry| {
                                        let eid = entry.get("module_id").and_then(|v| v.as_str())?;
                                        if eid == my_mod_id {
                                            let p = entry.get("pos").and_then(|v| v.as_f64())? as f32;
                                            let sp = entry
                                                .get("span")
                                                .and_then(|v| v.as_f64())
                                                .unwrap_or(1.0)
                                                .clamp(0.5, 4.0) as f32;
                                            Some((Some(p), sp))
                                        } else {
                                            None
                                        }
                                    })
                                });
                            found.unwrap_or((None, slot_span))
                        } else {
                            (slot_pos, slot_span)
                        }
                    } else {
                        (slot_pos, slot_span)
                    };

                    // Exact pixel rect from `dock-top-bar`'s `layout/slots/top-bar`,
                    // supporting half-slot positioning (`0.0, 0.5, 1.0, ...`) and `0.5..4.0` spans!
                    if let Some(pos) = eff_pos {
                        let compute_rect = |hx: f32, hy: f32, hw: f32, hh: f32, count: f32, gap: f32, sw: f32| -> openflux_bus::RectPx {
                            let start = pos.clamp(0.0, (count - 0.5).max(0.0));
                            let end = (start + eff_span).min(count);
                            let span = (end - start).max(0.5);
                            let edge_x = |u: f32, is_right: bool| -> f32 {
                                let whole = u.floor();
                                let frac = u - whole;
                                if frac < 0.25 {
                                    // Clean integer slot boundary
                                    if is_right && whole >= 1.0 {
                                        hx + (whole - 1.0) * (sw + gap) + sw
                                    } else {
                                        hx + whole * (sw + gap)
                                    }
                                } else {
                                    // Half-slot boundary inside slot `whole` with a 10px breathing gap
                                    let slot_left = hx + whole * (sw + gap);
                                    if is_right {
                                        slot_left + sw * 0.5 - 5.0
                                    } else {
                                        slot_left + sw * 0.5 + 5.0
                                    }
                                }
                            };
                            let x0 = edge_x(start, false);
                            let x1 = edge_x(start + span, true).min(hx + hw);
                            openflux_bus::RectPx {
                                x: x0,
                                y: hy,
                                w: (x1 - x0).max(60.0),
                                h: hh,
                            }
                        };

                        if let Some(msg) = broker_slot.get_retained("layout/slots/top-bar") {
                            let gap = msg.payload.get("gap").and_then(|v| v.as_f64()).unwrap_or(89.4) as f32;
                            let sw = msg.payload.get("slot_w").and_then(|v| v.as_f64()).unwrap_or(408.9) as f32;
                            let count = msg.payload.get("slot_count").and_then(|v| v.as_f64()).unwrap_or(4.0) as f32;
                            if let Some(hr) = msg.payload.get("header_rect") {
                                let hx = hr.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                                let hy = hr.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                                let hw = hr.get("w").and_then(|v| v.as_f64()).unwrap_or(1904.0) as f32;
                                let hh = hr.get("h").and_then(|v| v.as_f64()).unwrap_or(180.0) as f32;
                                return lua.to_value(&compute_rect(hx, hy, hw, hh, count, gap, sw));
                            }
                        }
                        let gap = 20.0_f32;
                        let sw = (fallback_hdr.w - gap * 3.0) / 4.0;
                        return lua.to_value(&compute_rect(
                            fallback_hdr.x,
                            fallback_hdr.y,
                            fallback_hdr.w,
                            fallback_hdr.h,
                            4.0,
                            gap,
                            sw,
                        ));
                    }
                    Ok(LuaValue::Nil)
                })?,
            )?;
        }
        {
            let cfg_for_col = merged_cfg.clone();
            let broker_col = broker.clone();
            let my_id_col = self.discovered.manifest.id.clone();
            mod_tbl.set(
                "color",
                self.lua.create_function(
                    move |lua, (key, fallback_hex): (String, Option<String>)| {
                        let fb = fallback_hex
                            .as_deref()
                            .map(|h| parse_hex_color(h, [1.0, 1.0, 1.0, 1.0]))
                            .unwrap_or([1.0, 1.0, 1.0, 1.0]);
                        let prof_hex = broker_col
                            .get_retained("profiles/active")
                            .and_then(|m| {
                                m.payload
                                    .get("module_overrides")
                                    .and_then(|o| o.get(&my_id_col))
                                    .and_then(|o| o.get(&key))
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string)
                            });
                        let hex_opt = prof_hex
                            .as_deref()
                            .or_else(|| cfg_for_col.get(&key).and_then(|v| v.as_str()));
                        let rgba = hex_opt.map(|h| parse_hex_color(h, fb)).unwrap_or(fb);
                        let t = lua.create_table()?;
                        t.set(1, rgba[0])?;
                        t.set(2, rgba[1])?;
                        t.set(3, rgba[2])?;
                        t.set(4, rgba[3])?;
                        Ok(t)
                    },
                )?,
            )?;
        }
        {
            let logs_ref = self.logs.clone();
            mod_tbl.set(
                "log",
                self.lua
                    .create_function(move |_, (level, msg): (String, String)| {
                        logs_ref.push(level, msg);
                        Ok(())
                    })?,
            )?;
        }
        globals.set("module", mod_tbl)?;

        // `json` + `fs.list_dir` helpers so Lua modules can read declarative data/packs directly!
        let json_tbl = self.lua.create_table()?;
        json_tbl.set(
            "decode",
            self.lua.create_function(|lua, text: String| {
                let val: serde_json::Value =
                    serde_json::from_str(&text).map_err(mlua::Error::external)?;
                lua.to_value(&val)
            })?,
        )?;
        json_tbl.set(
            "encode",
            self.lua.create_function(|lua, val: LuaValue| {
                let jv: serde_json::Value = lua.from_value(val)?;
                serde_json::to_string(&jv).map_err(mlua::Error::external)
            })?,
        )?;
        globals.set("json", json_tbl)?;

        let fs_tbl = self.lua.create_table()?;
        fs_tbl.set(
            "list_dir",
            self.lua.create_function(|lua, dir: String| {
                let t = lua.create_table()?;
                if dir.contains("..") {
                    return Ok(t);
                }
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    let mut names: Vec<String> = rd
                        .flatten()
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect();
                    names.sort();
                    for (i, n) in names.into_iter().enumerate() {
                        t.set(i + 1, n)?;
                    }
                }
                Ok(t)
            })?,
        )?;
        fs_tbl.set(
            "exists",
            self.lua.create_function(|_, path: String| {
                if path.is_empty() || path.contains('\0') || path.contains("..") {
                    return Ok(false);
                }
                Ok(std::path::Path::new(&path).is_file())
            })?,
        )?;
        fs_tbl.set(
            "read_text",
            self.lua.create_function(|_, path: String| {
                if path.is_empty() || path.contains('\0') {
                    return Ok(None);
                }
                // Cap read size at 2 MB to prevent memory exhaustion in Lua modules
                let Ok(meta) = std::fs::metadata(&path) else {
                    return Ok(None);
                };
                if !meta.is_file() || meta.len() > 2 * 1024 * 1024 {
                    return Ok(None);
                }
                Ok(std::fs::read_to_string(&path).ok())
            })?,
        )?;
        globals.set("fs", fs_tbl)?;

        // First-class `profile` API: gives EVERY module clean, read-only access to the active
        // profile, its inheritance fall-through chain, tags, host computer identity, and scoped
        // per-profile data without hardcoding any profile internals!
        let prof_tbl = self.lua.create_table()?;
        {
            let b = broker.clone();
            prof_tbl.set(
                "active",
                self.lua.create_function(move |lua, ()| {
                    match b.get_retained("profiles/active") {
                        Some(msg) => lua.to_value(&msg.payload),
                        None => Ok(LuaValue::Nil),
                    }
                })?,
            )?;
        }
        {
            let b = broker.clone();
            prof_tbl.set(
                "id",
                self.lua.create_function(move |_, ()| {
                    Ok(b.get_retained("profiles/active")
                        .and_then(|m| m.payload.get("profile_id").and_then(|v| v.as_str()).map(str::to_string))
                        .unwrap_or_else(|| "default".to_string()))
                })?,
            )?;
        }
        {
            let b = broker.clone();
            prof_tbl.set(
                "name",
                self.lua.create_function(move |_, ()| {
                    Ok(b.get_retained("profiles/active")
                        .and_then(|m| m.payload.get("profile_name").and_then(|v| v.as_str()).map(str::to_string))
                        .unwrap_or_else(|| "Default".to_string()))
                })?,
            )?;
        }
        {
            let b = broker.clone();
            prof_tbl.set(
                "chain",
                self.lua.create_function(move |lua, ()| {
                    let t = lua.create_table()?;
                    if let Some(msg) = b.get_retained("profiles/active") {
                        if let Some(arr) = msg.payload.get("chain").and_then(|v| v.as_array()) {
                            for (i, v) in arr.iter().enumerate() {
                                if let Some(s) = v.as_str() {
                                    t.set(i + 1, s)?;
                                }
                            }
                            return Ok(t);
                        }
                    }
                    t.set(1, "default")?;
                    Ok(t)
                })?,
            )?;
        }
        {
            let b = broker.clone();
            prof_tbl.set(
                "has_tag",
                self.lua.create_function(move |_, tag: String| {
                    let needle = tag.trim().to_ascii_lowercase();
                    let found = b
                        .get_retained("profiles/active")
                        .and_then(|m| m.payload.get("tags").and_then(|v| v.as_array()).cloned())
                        .map(|arr| {
                            arr.iter().any(|v| {
                                v.as_str()
                                    .is_some_and(|s| s.eq_ignore_ascii_case(&needle))
                            })
                        })
                        .unwrap_or(false);
                    Ok(found)
                })?,
            )?;
        }
        {
            let b = broker.clone();
            prof_tbl.set(
                "host",
                self.lua.create_function(move |lua, ()| {
                    if let Some(msg) = b.get_retained("host/identity") {
                        return lua.to_value(&msg.payload);
                    }
                    if let Some(msg) = b.get_retained("profiles/active") {
                        if let Some(h) = msg.payload.get("host") {
                            return lua.to_value(h);
                        }
                    }
                    Ok(LuaValue::Nil)
                })?,
            )?;
        }
        {
            let b = broker.clone();
            let my_id = self.discovered.manifest.id.clone();
            prof_tbl.set(
                "data",
                self.lua.create_function(move |lua, (key, fallback): (String, Option<LuaValue>)| {
                    if let Some(msg) = b.get_retained("profiles/active") {
                        if let Some(val) = msg
                            .payload
                            .get("module_overrides")
                            .and_then(|m| m.get(&my_id))
                            .and_then(|m| m.get(&key))
                        {
                            return lua.to_value(val);
                        }
                    }
                    Ok(fallback.unwrap_or(LuaValue::Nil))
                })?,
            )?;
        }
        globals.set("profile", prof_tbl)?;

        Ok(())
    }

    fn needs_hot_reload(&self) -> bool {
        let cur_manifest = file_mtime(&self.discovered.path.join("module.toml"));
        if cur_manifest != self.manifest_mtime {
            return true;
        }
        if let Some(script) = &self.discovered.manifest.entrypoints.script {
            let cur_script = file_mtime(&self.discovered.path.join(script));
            if cur_script != self.script_mtime {
                return true;
            }
        }
        false
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeModuleStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    pub healthy: bool,
    pub last_error: Option<String>,
    pub slot: Option<u8>,
    pub stages: Vec<CompositorStage>,
}

/// Manages all enabled Lua visual/reactive modules, pumping bus events, hot-reloading changed
/// scripts, and producing a unified `FrameScene` for the GPU compositor each frame.
pub struct ModuleRuntime {
    broker: Broker,
    geometry: Arc<RwLock<KeyboardGeometry>>,
    modules: BTreeMap<String, LoadedLuaModule>,
    elapsed_sec: f32,
    tiles_dirty: bool,
    last_tile_render_ms: u64,
    last_tile_context_seq: (u64, u64),
    cached_tile_draws: Vec<DrawCommand>,
    cached_tile_top_fade: f32,
    cached_tile_streams: Vec<crate::ir::StreamOverlayCmd>,
}

impl ModuleRuntime {
    pub fn new(broker: Broker, geometry: KeyboardGeometry) -> Self {
        Self {
            broker,
            geometry: Arc::new(RwLock::new(geometry)),
            modules: BTreeMap::new(),
            elapsed_sec: 0.0,
            tiles_dirty: true,
            last_tile_render_ms: 0,
            last_tile_context_seq: (0, 0),
            cached_tile_draws: Vec::new(),
            cached_tile_top_fade: 0.0,
            cached_tile_streams: Vec::new(),
        }
    }

    pub fn geometry(&self) -> KeyboardGeometry {
        self.geometry.read().unwrap().clone()
    }

    /// Returns true if any healthy loaded module declares the given capability role in `provides`.
    pub fn has_role(&self, role: &str) -> bool {
        self.modules.values().any(|m| {
            m.healthy && m.discovered.manifest.provides.iter().any(|r| r == role)
        })
    }

    /// Returns true if a specific module ID is currently loaded and healthy.
    pub fn has_module(&self, id: &str) -> bool {
        self.modules.get(id).is_some_and(|m| m.healthy)
    }

    pub fn update_geometry(&mut self, geometry: KeyboardGeometry) {
        {
            let mut g = self.geometry.write().unwrap();
            *g = geometry.clone();
        }
        let _ = self.broker.publish_typed(
            openflux_bus::topics::KEYBOARD_GEOMETRY,
            &geometry,
            PublishOptions::retained(),
        );
    }

    /// Synchronizes loaded modules with discovered manifests and desired enable/slot/config state.
    pub fn sync_modules(
        &mut self,
        discovered: &BTreeMap<String, DiscoveredModule>,
        states: &BTreeMap<String, ModuleInstanceState>,
    ) {
        // Remove disabled or deleted modules and purge their Erlang/MQTT LWT retained claims!
        let keep_ids: Vec<String> = discovered
            .keys()
            .filter(|id| states.get(*id).is_some_and(|s| s.enabled))
            .cloned()
            .collect();
        let removed_ids: Vec<String> = self
            .modules
            .keys()
            .filter(|id| !keep_ids.contains(id))
            .cloned()
            .collect();
        for rid in &removed_ids {
            self.broker.purge_origin(&format!("module:{rid}"));
        }
        if !removed_ids.is_empty() {
            self.tiles_dirty = true;
        }
        self.modules.retain(|id, _| keep_ids.contains(id));

        for id in keep_ids {
            let dm = &discovered[&id];
            let inst = states.get(&id).cloned().unwrap_or_default();
            let should_reload = match self.modules.get(&id) {
                None => true,
                Some(existing) => {
                    existing.instance != inst
                        || existing.discovered.path != dm.path
                        || existing.needs_hot_reload()
                }
            };
            if should_reload {
                self.tiles_dirty = true;
                self.broker.purge_origin(&format!("module:{id}"));
                let loaded = LoadedLuaModule::load(
                    dm.clone(),
                    inst,
                    self.broker.clone(),
                    self.geometry.clone(),
                );
                self.modules.insert(id, loaded);
            }
        }

        // Publish live top-bar occupancy so `dock-top-bar` draws smart dividers only between distinct widgets!
        let mut occupied: Vec<serde_json::Value> = self
            .modules
            .values()
            .filter_map(|m| {
                let start = m.instance.effective_slot_pos()?;
                let span = m.instance.slot_span.unwrap_or_else(|| {
                    m.discovered
                        .manifest
                        .top_bar
                        .as_ref()
                        .map(|t| t.default_span)
                        .unwrap_or(1.0)
                });
                Some(serde_json::json!({
                    "id": m.discovered.manifest.id,
                    "start": start,
                    "span": span.clamp(0.5, 4.0),
                }))
            })
            .collect();
        occupied.sort_by(|a, b| {
            let sa = a.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
        });
        let _ = self.broker.publish(
            "layout/top_bar/occupancy",
            serde_json::json!({ "items": occupied }),
            PublishOptions::retained().with_origin("runtime"),
        );
    }

    /// Checks on-disk mtimes and hot-reloads any edited module in place.
    pub fn check_hot_reload(&mut self) -> Vec<String> {
        let mut reloaded = Vec::new();
        let ids: Vec<String> = self
            .modules
            .iter()
            .filter(|(_, m)| m.needs_hot_reload())
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(old) = self.modules.remove(&id) {
                let mut dm = old.discovered.clone();
                if let Ok(fresh_manifest) = crate::manifest::ModuleManifest::load_from_dir(&dm.path)
                {
                    dm.manifest = fresh_manifest;
                }
                let loaded = LoadedLuaModule::load(
                    dm,
                    old.instance,
                    self.broker.clone(),
                    self.geometry.clone(),
                );
                self.modules.insert(id.clone(), loaded);
                reloaded.push(id);
            }
        }
        reloaded
    }

    pub fn reload_module(&mut self, id: &str) -> bool {
        if let Some(old) = self.modules.remove(id) {
            let mut dm = old.discovered.clone();
            if let Ok(fresh) = crate::manifest::ModuleManifest::load_from_dir(&dm.path) {
                dm.manifest = fresh;
            }
            let loaded = LoadedLuaModule::load(
                dm,
                old.instance,
                self.broker.clone(),
                self.geometry.clone(),
            );
            self.modules.insert(id.to_string(), loaded);
            true
        } else {
            false
        }
    }

    /// Advances all modules by `dt_sec`, delivers queued bus events, and builds the `FrameScene`.
    pub fn step_frame(&mut self, dt_sec: f32, now_ms: u64) -> FrameScene {
        self.elapsed_sec += dt_sec;

        // 1. Deliver queued pub/sub events to every healthy module (and LWT-purge on crash!).
        let mut newly_crashed = Vec::new();
        for (id, loaded) in self.modules.iter_mut() {
            loaded.last_step_us = 0;
            loaded.events_last = 0;
            if !loaded.healthy {
                continue;
            }
            let t0 = std::time::Instant::now();
            let events: Vec<openflux_bus::BusMessage> = {
                let subs = loaded.subscriptions.lock().unwrap();
                let mut all = Vec::new();
                for sub in subs.iter() {
                    all.extend(sub.drain());
                }
                all
            };
            loaded.events_last = events.len() as u32;
            if !events.is_empty() {
                if loaded.discovered.manifest.stages.contains(&CompositorStage::Tile) {
                    self.tiles_dirty = true;
                }
                if let Ok(on_event) = loaded.lua.globals().get::<Function>("on_event") {
                    for msg in events {
                        let lua_payload = match loaded.lua.to_value(&msg.payload) {
                            Ok(v) => v,
                            Err(e) => {
                                loaded.record_error(format!("event serialize error: {e}"));
                                newly_crashed.push(id.clone());
                                break;
                            }
                        };
                        let meta = match loaded.lua.create_table() {
                            Ok(t) => {
                                let _ = t.set("seq", msg.seq);
                                let _ = t.set("timestamp_ms", msg.timestamp_ms);
                                let _ = t.set("retain", msg.retain);
                                let _ = t.set("origin", msg.origin.clone());
                                t
                            }
                            Err(_) => continue,
                        };
                        loaded.arm_watchdog(std::time::Duration::from_millis(150));
                        let ev_res = on_event.call::<()>((msg.topic, lua_payload, meta));
                        loaded.clear_watchdog();
                        if let Err(e) = ev_res {
                            loaded.record_error(format!("on_event error: {e}"));
                            newly_crashed.push(id.clone());
                            break;
                        }
                    }
                }
            }
            loaded.last_step_us += t0.elapsed().as_micros() as u64;
        }

        // 2. Call `on_update(dt_sec, now_ms)` for every healthy module.
        for (id, loaded) in self.modules.iter_mut() {
            if !loaded.healthy {
                continue;
            }
            if let Ok(on_update) = loaded.lua.globals().get::<Function>("on_update") {
                let t0 = std::time::Instant::now();
                loaded.arm_watchdog(std::time::Duration::from_millis(150));
                let upd_res = on_update.call::<()>((dt_sec, now_ms));
                loaded.clear_watchdog();
                loaded.last_step_us += t0.elapsed().as_micros() as u64;
                if let Err(e) = upd_res {
                    loaded.record_error(format!("on_update error: {e}"));
                    newly_crashed.push(id.clone());
                }
            }
        }
        for cid in newly_crashed {
            self.broker.purge_origin(&format!("module:{cid}"));
        }

        // 3. Collect render commands per stage in deterministic order.
        let scene = Arc::new(Mutex::new(FrameScene {
            time_sec: self.elapsed_sec,
            dt_sec,
            has_custom_keys_renderer: self.has_role("keys_renderer"),
            has_custom_dock: self.has_role("top_bar_dock"),
            ..Default::default()
        }));

        let stage_order = [
            CompositorStage::Background,
            CompositorStage::Distortion,
            CompositorStage::UnderKey,
            CompositorStage::Tile,
            CompositorStage::WorldOverlay,
        ];

        let cur_ctx_seq = (
            self.broker.get_retained("profiles/active").map(|m| m.seq).unwrap_or(0),
            self.broker
                .get_retained("layout/top_bar/context_override")
                .map(|m| m.seq)
                .unwrap_or(0),
        );
        if cur_ctx_seq != self.last_tile_context_seq {
            self.last_tile_context_seq = cur_ctx_seq;
            self.tiles_dirty = true;
        }
        let render_tiles_now = self.tiles_dirty
            || self.last_tile_render_ms == 0
            || now_ms.saturating_sub(self.last_tile_render_ms) >= 200;
        if render_tiles_now {
            self.tiles_dirty = false;
            self.last_tile_render_ms = now_ms;
        }

        for stage in stage_order {
            if stage == CompositorStage::Tile && !render_tiles_now {
                let mut s = scene.lock().unwrap();
                s.tile_draws = self.cached_tile_draws.clone();
                if self.cached_tile_top_fade > s.top_fade {
                    s.top_fade = self.cached_tile_top_fade;
                }
                s.stream_overlays.extend(self.cached_tile_streams.iter().cloned());
                continue;
            }
            let stage_name = match stage {
                CompositorStage::Background => "background",
                CompositorStage::Distortion => "distortion",
                CompositorStage::UnderKey => "under_key",
                CompositorStage::Tile => "tile",
                CompositorStage::WorldOverlay => "world_overlay",
            };
            let prev_streams_len = if stage == CompositorStage::Tile {
                scene.lock().unwrap().stream_overlays.len()
            } else {
                0
            };
            for loaded in self.modules.values_mut() {
                if !loaded.healthy || !loaded.discovered.manifest.stages.contains(&stage) {
                    continue;
                }
                if let Ok(on_render) = loaded.lua.globals().get::<Function>("on_render") {
                    let handle = RenderCtxHandle {
                        module_id: loaded.discovered.manifest.id.clone(),
                        module_dir: loaded.discovered.path.clone(),
                        stage,
                        scene: scene.clone(),
                    };
                    let t0 = std::time::Instant::now();
                    loaded.arm_watchdog(std::time::Duration::from_millis(150));
                    let render_res = on_render.call::<()>((handle, stage_name));
                    loaded.clear_watchdog();
                    loaded.last_step_us += t0.elapsed().as_micros() as u64;
                    if let Err(e) = render_res {
                        loaded.record_error(format!("on_render({stage_name}) error: {e}"));
                    }
                }
            }
            if stage == CompositorStage::Tile {
                let s = scene.lock().unwrap();
                self.cached_tile_draws = s.tile_draws.clone();
                self.cached_tile_top_fade = s.top_fade;
                self.cached_tile_streams = s.stream_overlays[prev_streams_len..].to_vec();
            }
        }

        for loaded in self.modules.values_mut() {
            let us = loaded.last_step_us;
            loaded.ema_step_us = if loaded.ema_step_us == 0.0 {
                us as f64
            } else {
                loaded.ema_step_us * 0.90 + (us as f64) * 0.10
            };
            if us > loaded.max_step_us {
                loaded.max_step_us = us;
            }
        }

        Arc::try_unwrap(scene)
            .map(|m| m.into_inner().unwrap())
            .unwrap_or_else(|arc| arc.lock().unwrap().clone())
    }

    /// Returns per-module Lua execution timing statistics (sorted hottest-first) and resets
    /// the rolling window `max_us` peak so each profiler snapshot exposes fresh spikes.
    pub fn take_perf_snapshot(&mut self) -> Vec<ModulePerfStats> {
        let mut out: Vec<ModulePerfStats> = self
            .modules
            .values_mut()
            .map(|m| {
                let s = ModulePerfStats {
                    id: m.discovered.manifest.id.clone(),
                    last_us: m.last_step_us,
                    avg_us: m.ema_step_us.round() as u64,
                    max_us: m.max_step_us,
                    events_last: m.events_last,
                };
                m.max_step_us = 0;
                s
            })
            .collect();
        out.sort_by(|a, b| b.max_us.cmp(&a.max_us).then_with(|| b.avg_us.cmp(&a.avg_us)));
        out
    }

    pub fn statuses(&self) -> Vec<RuntimeModuleStatus> {
        self.modules
            .values()
            .map(|m| RuntimeModuleStatus {
                id: m.discovered.manifest.id.clone(),
                name: m.discovered.manifest.name.clone(),
                version: m.discovered.manifest.version.clone(),
                healthy: m.healthy,
                last_error: m.last_error.clone(),
                slot: m.instance.slot,
                stages: m.discovered.manifest.stages.clone(),
            })
            .collect()
    }

    pub fn module_logs(&self, id: &str) -> Vec<ModuleLogEntry> {
        self.modules
            .get(id)
            .map(|m| m.logs.snapshot())
            .unwrap_or_default()
    }
}
