//! Sandboxed behavioral module validator.
//!
//! Static TOML parsing is not enough to prevent runtime regressions (like calling a nonexistent
//! `ctx:*` method, referencing an undefined Lua global, or shipping a partial module copy missing
//! its declared `entrypoints.host` script).
//!
//! [`validate_module_behavior`] verifies a module against the real [`ModuleRuntime`] and canonical
//! `openflux-bus` fixtures across every lifecycle hook (`on_init`, `on_event`, `on_tick`, and
//! `on_render` for every declared stage).

use crate::lua_runtime::ModuleRuntime;
use crate::registry::{DiscoveredModule, ModuleInstanceState};
use openflux_bus::{
    Broker, FrameLayout, HostConnectionState, KeyEvent, KeyEventKind, KeyboardGeometry,
    KeyboardState, PublishOptions, SystemMetrics, topics,
};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ModuleValidationReport {
    pub id: String,
    pub stages_checked: usize,
    pub events_delivered: usize,
}

/// Runs a complete contract validation on a discovered module:
/// 1. Manifest schema validation.
/// 2. Filesystem existence check for every declared entrypoint (`script`, `host`, `shader`).
/// 3. Live Lua execution inside an isolated [`ModuleRuntime`] with canonical bus fixtures,
///    verifying `on_init`, `on_event` (for canonical topics), `on_tick`, and `on_render` across
///    every declared render stage with zero Lua runtime errors.
pub fn validate_module_behavior(
    discovered: &DiscoveredModule,
) -> Result<ModuleValidationReport, Vec<String>> {
    let mut errors = Vec::new();
    let m = &discovered.manifest;

    if let Err(e) = m.validate() {
        errors.push(format!("manifest validation failed: {e}"));
    }

    if let Some(script_rel) = &m.entrypoints.script {
        let p = discovered.path.join(script_rel);
        if !p.is_file() {
            errors.push(format!("declared script entrypoint missing: {}", p.display()));
        }
    }
    if let Some(host_rel) = &m.entrypoints.host {
        let p = discovered.path.join(host_rel);
        if !p.is_file() {
            errors.push(format!("declared host entrypoint missing: {}", p.display()));
        }
    }
    if let Some(shader_rel) = &m.entrypoints.shader {
        let p = discovered.path.join(shader_rel);
        if !p.is_file() {
            errors.push(format!("declared shader entrypoint missing: {}", p.display()));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    // Exercise the module inside a real isolated ModuleRuntime with canonical typed bus fixtures.
    let broker = Broker::new("validator");
    let geometry = KeyboardGeometry::build(FrameLayout::Ansi);
    seed_canonical_fixtures(&broker, &geometry);

    let mut runtime = ModuleRuntime::new(broker.clone(), geometry.clone());
    let mut discovered_map = BTreeMap::new();
    discovered_map.insert(m.id.clone(), discovered.clone());

    let mut state_map = BTreeMap::new();
    let mut config = BTreeMap::new();
    for field in &m.settings {
        config.insert(field.key.clone(), field.default.clone());
    }
    state_map.insert(
        m.id.clone(),
        ModuleInstanceState {
            enabled: true,
            slot: if m.slot_capable { Some(0) } else { None },
            slot_pos: if m.slot_capable { Some(0.0) } else { None },
            slot_span: m.top_bar.as_ref().map(|tb| tb.default_span),
            pin_tab: None,
            config,
        },
    );

    runtime.sync_modules(&discovered_map, &state_map);

    // Deliver a realistic sequence of live events matching the real bus contracts.
    let events_delivered = publish_canonical_events(&broker, &geometry);

    // Run two frames: base state + active overlay/peek state (e.g. Prusa popup / OSD toast).
    let _scene1 = runtime.step_frame(0.016, 1_700_000_000_016);
    let _ = broker.publish(
        "modules/prusa-connect/peek",
        json!({ "active": true }),
        PublishOptions::retained(),
    );
    let _ = broker.publish(
        "modules/streams/prusa-cam",
        json!({ "active": true, "codec": "h264", "name": "prusa-cam" }),
        PublishOptions::retained(),
    );
    let _scene2 = runtime.step_frame(0.016, 1_700_000_000_032);

    for st in runtime.statuses() {
        if st.id == m.id && !st.healthy {
            errors.push(format!(
                "runtime error in module '{}': {}",
                st.id,
                st.last_error.unwrap_or_else(|| "unknown error".into())
            ));
        }
    }

    if errors.is_empty() {
        Ok(ModuleValidationReport {
            id: m.id.clone(),
            stages_checked: m.stages.len(),
            events_delivered,
        })
    } else {
        Err(errors)
    }
}

fn seed_canonical_fixtures(broker: &Broker, geometry: &KeyboardGeometry) {
    let _ = broker.publish_typed(topics::KEYBOARD_GEOMETRY, geometry, PublishOptions::retained());
    let _ = broker.publish_typed(
        topics::KEYBOARD_STATE,
        &KeyboardState::default(),
        PublishOptions::retained(),
    );
    let _ = broker.publish_typed(
        topics::HOST_CONNECTION,
        &HostConnectionState {
            connected: true,
            host_asleep: false,
            last_seen_ms: 1_700_000_000_000,
            host_platform: Some("macos".into()),
        },
        PublishOptions::retained(),
    );
    let _ = broker.publish_typed(
        topics::HOST_SYSTEM_METRICS,
        &SystemMetrics {
            cpu_percent: 24.5,
            memory_used_gb: 14.2,
            memory_total_gb: 36.0,
            gpu_percent: 18.0,
            gpu_unified: true,
            vram_used_gb: None,
            vram_total_gb: None,
            net_down_kbps: 120.0,
            net_up_kbps: 40.0,
            net_down: "120 KB/s".into(),
            net_up: "40 KB/s".into(),
        },
        PublishOptions::retained(),
    );
    let _ = broker.publish(
        "host/time",
        json!({ "unix_sec": 1795411200, "tz_offset_sec": -25200, "zone": "America/Denver" }),
        PublishOptions::retained(),
    );
    let _ = broker.publish(
        "host/app/active",
        json!({ "app_id": "com.apple.Safari", "process_name": "Safari", "window_title": "OpenFlux" }),
        PublishOptions::retained(),
    );
    let _ = broker.publish(
        "host/media/now_playing",
        json!({ "title": "Track", "artist": "Artist", "playing": true, "progress_sec": 42, "duration_sec": 180 }),
        PublishOptions::retained(),
    );
    let _ = broker.publish(
        "host/weather/current",
        json!({ "temp_c": 18.5, "condition": "Clear", "city": "Denver", "high_c": 22.0, "low_c": 9.0, "hourly": [] }),
        PublishOptions::retained(),
    );
    let _ = broker.publish(
        "modules/prusa-connect/status",
        json!({
            "state": "PRINTING",
            "name": "prusa-core-one",
            "progress": 42.0,
            "nozzle_temp": 230.0,
            "nozzle_target": 230.0,
            "bed_temp": 60.0,
            "bed_target": 60.0,
            "time_remaining_min": 25.0,
            "job_name": "test.bgcode",
            "camera_mode": "live",
            "camera_path": "",
            "camera_kb_path": ""
        }),
        PublishOptions::retained(),
    );
    let _ = broker.publish(
        "keyboard/resolved_keys",
        json!({
            "active_layer": 0,
            "keys": [
                { "keycode": 0x29, "source_layer": 0 },
                { "keycode": 0x04, "source_layer": 0 },
                { "keycode": 0x5221, "source_layer": 0 }
            ]
        }),
        PublishOptions::retained(),
    );
}

fn make_key_event(key_index: u16, kind: KeyEventKind, timestamp_ms: u64, geometry: &KeyboardGeometry) -> KeyEvent {
    let k = geometry.keys.get(key_index as usize);
    KeyEvent {
        key_index,
        kind,
        timestamp_ms,
        x_px: k.map(|x| x.center_x).unwrap_or(960.0),
        y_px: k.map(|x| x.center_y).unwrap_or(540.0),
        w_px: k.map(|x| x.rect.w).unwrap_or(96.0),
        h_px: k.map(|x| x.rect.h).unwrap_or(96.0),
        uv_x: k.map(|x| x.uv_x).unwrap_or(0.5),
        uv_y: k.map(|x| x.uv_y).unwrap_or(0.5),
        row: k.map(|x| x.row).unwrap_or(0),
        col: k.map(|x| x.col).unwrap_or(0),
        depth_um: if kind == KeyEventKind::Press { 2000 } else { 0 },
        velocity_um_s: if kind == KeyEventKind::Press { 12000 } else { -8000 },
        color: None,
    }
}

fn publish_canonical_events(broker: &Broker, geometry: &KeyboardGeometry) -> usize {
    let mut n = 0;
    let press = make_key_event(13, KeyEventKind::Press, 1_700_000_000_001, geometry);
    let _ = broker.publish_typed(topics::INPUT_KEYS_EVENT, &press, PublishOptions::default());
    n += 1;
    let release = make_key_event(13, KeyEventKind::Release, 1_700_000_000_120, geometry);
    let _ = broker.publish_typed(topics::INPUT_KEYS_EVENT, &release, PublishOptions::default());
    n += 1;
    let _ = broker.publish(
        "ui/toast",
        json!({
            "id": "volume",
            "icon": "volume",
            "title": "Volume",
            "level": 0.5,
            "value_text": "50%",
            "duration_ms": 1500
        }),
        PublishOptions::default(),
    );
    n += 1;
    n
}
