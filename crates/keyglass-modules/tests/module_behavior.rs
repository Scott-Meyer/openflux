use keyglass_bus::{Broker, FrameLayout, KeyboardGeometry, PublishOptions};
use keyglass_modules::{
    ModuleCategory, ModuleInstanceState, ModuleRegistry, ModuleRuntime, ModuleTier,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn repo_bundled_modules() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules")
}

#[test]
fn fork_disclosure_diff_secret_stripping_and_pet_to_water_cross_module_composition() {
    let tmp = std::env::temp_dir().join(format!("keyglass-mod-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let bundled = tmp.join("bundled");
    std::fs::create_dir_all(bundled.join("fx-water-fixture")).unwrap();
    std::fs::create_dir_all(bundled.join("pet-fixture")).unwrap();
    std::fs::write(
        bundled.join("fx-water-fixture/module.toml"),
        r#"
id = "fx-water-fixture"
name = "Water Fixture"
version = "1.0.0"
author = "KeyGlass"
category = "effect"
description = "Test water receiver"
stages = ["distortion"]
host_requirement = "none"
[entrypoints]
script = "script.lua"
[channels]
subscribes = ["fx/surface/impulse"]
[[settings]]
key = "api_key"
label = "API Key"
kind = "string"
default = ""
secret = true
"#,
    )
    .unwrap();
    std::fs::write(
        bundled.join("fx-water-fixture/script.lua"),
        r#"
local pending = {}
function on_event(topic, payload)
  if topic == "fx/surface/impulse" and type(payload) == "table" then
    table.insert(pending, payload)
  end
end
function on_render(ctx, stage)
  for _, p in ipairs(pending) do
    ctx:wave_impulse(p.x or 100, p.y or 100, p.radius or 20, p.strength or 1.0)
  end
  pending = {}
end
"#,
    )
    .unwrap();
    std::fs::write(
        bundled.join("pet-fixture/module.toml"),
        r#"
id = "pet-fixture"
name = "Pet Fixture"
version = "1.0.0"
author = "KeyGlass"
category = "widget"
description = "Test pet publisher"
stages = ["world_overlay"]
host_requirement = "none"
[entrypoints]
script = "script.lua"
[channels]
subscribes = ["modules/pet-fixture/command"]
publishes = ["fx/surface/impulse"]
"#,
    )
    .unwrap();
    std::fs::write(
        bundled.join("pet-fixture/script.lua"),
        r#"
function on_event(topic, payload)
  if topic == "modules/pet-fixture/command" then
    bus.publish("fx/surface/impulse", { x = 400, y = 500, radius = 24, strength = 0.9 }, false)
  end
end
"#,
    )
    .unwrap();

    let reg = ModuleRegistry::new(
        bundled,
        tmp.join("installed"),
        tmp.join("local"),
        tmp.join("state.json"),
    );

    // 1. Forking a bundled module creates a Local tier override with forked_from metadata.
    let forked = reg
        .fork_module("fx-water-fixture", "my-water", Some("My Custom Water"))
        .unwrap();
    assert_eq!(forked.tier, ModuleTier::Local);
    assert_eq!(
        forked.manifest.forked_from.as_deref(),
        Some("fx-water-fixture")
    );

    // 2. Host-service install disclosure flags host execution, and updating the script flags a diff.
    let svc = reg
        .scaffold_local("demo-host", ModuleCategory::HostService, Some("Demo Host"))
        .unwrap();
    let pre1 = reg.preview_install(&svc.path).unwrap();
    assert!(pre1.disclosure.runs_host_process);
    assert!(pre1.update_diff.is_none());
    reg.install_from_dir(&svc.path).unwrap();

    std::fs::write(
        svc.path.join("host/service.py"),
        "#!/usr/bin/env python3\nprint('changed')\n",
    )
    .unwrap();
    let pre2 = reg.preview_install(&svc.path).unwrap();
    let diff = pre2.update_diff.expect("update diff expected");
    assert!(diff.executable_or_host_changed);
    assert!(diff
        .modified_files
        .iter()
        .any(|f| f.rel_path == "host/service.py"));

    // 3. Preset export strips secret fields (`api_key`).
    let mut st = reg.load_state();
    st.modules.insert(
        "fx-water-fixture".into(),
        ModuleInstanceState {
            enabled: true,
            slot: None,
            slot_pos: None,
            slot_span: None,
            pin_tab: None,
            config: BTreeMap::from([("api_key".into(), json!("secret-xyz"))]),
        },
    );
    reg.save_state(&st).unwrap();
    let preset = reg.export_preset("Test", "");
    assert!(
        !preset.modules["fx-water-fixture"]
            .config
            .contains_key("api_key"),
        "secret field must be stripped from exported preset"
    );

    // 4. Cross-module composition: Pet publishes `fx/surface/impulse`
    //    -> Water receives it and emits a WaveImpulseCmd without knowing about the Pet!
    let broker = Broker::new("test");
    let geom = KeyboardGeometry::build(FrameLayout::Ansi);
    let mut rt = ModuleRuntime::new(broker.clone(), geom);
    let discovered = reg.discover();
    let states = BTreeMap::from([
        (
            "pet-fixture".into(),
            ModuleInstanceState {
                enabled: true,
                slot: None,
                slot_pos: None,
                slot_span: None,
                pin_tab: None,
                config: BTreeMap::new(),
            },
        ),
        (
            "fx-water-fixture".into(),
            ModuleInstanceState {
                enabled: true,
                slot: None,
                slot_pos: None,
                slot_span: None,
                pin_tab: None,
                config: BTreeMap::new(),
            },
        ),
    ]);
    rt.sync_modules(&discovered, &states);
    broker
        .publish(
            "modules/pet-fixture/command",
            json!({ "action": "roam", "target_key": 38 }),
            PublishOptions::default(),
        )
        .unwrap();

    let mut saw_paw_wave = false;
    for f in 0..40 {
        let scene = rt.step_frame(0.02, f * 20);
        if !scene.wave_impulses.is_empty() {
            saw_paw_wave = true;
            break;
        }
    }
    assert!(
        saw_paw_wave,
        "Idle Pet landing on a keycap must trigger a 3D Water Ripple wave impulse via fx/surface/impulse"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn roles_dependencies_lua_require_archive_install_update_uninstall_and_rich_presets() {
    let tmp = std::env::temp_dir().join(format!("keyglass-v2-audit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let reg = ModuleRegistry::new(
        repo_bundled_modules(),
        tmp.join("installed"),
        tmp.join("local"),
        tmp.join("modules_state.json"),
    );

    // 1. Fork `keys-renderer` into a brand-new ID; it MUST still take over `has_custom_keys_renderer`
    //    via `provides = ["keys_renderer"]` with zero hardcoded name check!
    let forked = reg
        .fork_module("keys-renderer", "my-custom-keys", Some("My Custom Keys"))
        .expect("fork keys-renderer");
    assert!(forked.manifest.provides.contains(&"keys_renderer".to_string()));

    let broker = Broker::new("test");
    let geom = KeyboardGeometry::build(FrameLayout::Ansi);
    let mut rt = ModuleRuntime::new(broker.clone(), geom);
    let discovered = reg.discover();
    let mut states = BTreeMap::new();
    states.insert(
        "my-custom-keys".to_string(),
        ModuleInstanceState {
            enabled: true,
            ..Default::default()
        },
    );
    rt.sync_modules(&discovered, &states);
    let scene = rt.step_frame(0.016, 16);
    assert!(
        scene.has_custom_keys_renderer,
        "Forked renderer declaring provides = [\"keys_renderer\"] must own key rendering regardless of ID"
    );

    // 2. Dependency validation catches missing role when no keys_renderer is enabled
    let mut missing_state = keyglass_modules::ModulesStateFile::default();
    missing_state.modules.insert(
        "keys-keymap".into(),
        ModuleInstanceState {
            enabled: true,
            ..Default::default()
        },
    );
    let dep_issues = reg.check_all_dependencies(&discovered, &missing_state);
    assert!(
        dep_issues.contains_key("keys-keymap"),
        "keys-keymap should report missing role keys_renderer when disabled"
    );
    missing_state.modules.insert(
        "my-custom-keys".into(),
        ModuleInstanceState {
            enabled: true,
            ..Default::default()
        },
    );
    let dep_issues_ok = reg.check_all_dependencies(&discovered, &missing_state);
    assert!(
        !dep_issues_ok.contains_key("keys-keymap"),
        "Forked provider my-custom-keys must satisfy role dependency keys_renderer"
    );

    // 3. Multi-file Lua `require("helpers")` works from inside a module's own folder
    let mod_dir = tmp.join("pkg-src/multi-file-mod");
    std::fs::create_dir_all(&mod_dir).unwrap();
    std::fs::write(
        mod_dir.join("module.toml"),
        r#"
id = "multi-file-mod"
name = "Multi File Mod"
version = "1.0.0"
category = "effect"
stages = ["under_key"]
[entrypoints]
script = "script.lua"
"#,
    )
    .unwrap();
    std::fs::write(mod_dir.join("helpers.lua"), "return { answer = 42 }\n").unwrap();
    std::fs::write(
        mod_dir.join("script.lua"),
        r#"
local h = require("helpers")
function on_init()
  bus.publish("modules/multi-file-mod/ready", { value = h.answer }, true)
end
"#,
    )
    .unwrap();

    // 4. Pack as .tar.gz archive -> install via universal `install_from_source` -> update -> rich preset -> uninstall
    let tar_path = tmp.join("multi-file-mod.tar.gz");
    let status = std::process::Command::new("tar")
        .args(["-czf"])
        .arg(&tar_path)
        .args(["-C"])
        .arg(tmp.join("pkg-src"))
        .arg("multi-file-mod")
        .status()
        .expect("tar archive");
    assert!(status.success());

    let installed = reg
        .install_from_source(&tar_path.to_string_lossy())
        .expect("install from tar.gz archive");
    assert_eq!(installed.manifest.version, "1.0.0");
    assert!(reg.module_origin(&installed).is_some());

    // Run installed multi-file module and verify Lua `require("helpers")` published 42!
    let discovered2 = reg.discover();
    let mut st2 = BTreeMap::new();
    st2.insert(
        "multi-file-mod".into(),
        ModuleInstanceState {
            enabled: true,
            ..Default::default()
        },
    );
    rt.sync_modules(&discovered2, &st2);
    let ready_msg = broker
        .get_retained("modules/multi-file-mod/ready")
        .expect("multi-file Lua require should publish ready");
    assert_eq!(ready_msg.payload["value"], 42);

    // Rich preset records source + version and reports missing module after uninstall
    let mut state_file = reg.load_state();
    state_file.modules.insert(
        "multi-file-mod".into(),
        ModuleInstanceState {
            enabled: true,
            ..Default::default()
        },
    );
    reg.save_state(&state_file).unwrap();
    let preset = reg.export_preset("Test", "Rich preset");
    assert_eq!(preset.module_sources["multi-file-mod"].version, "1.0.0");
    assert!(preset.module_sources["multi-file-mod"].source.is_some());

    // Uninstall installed copy -> import preset -> missing_modules reports exact id, version, source!
    reg.uninstall_module("multi-file-mod").expect("uninstall");
    let report = reg.import_preset_detailed(&preset).expect("import preset");
    assert_eq!(report.missing_modules.len(), 1);
    assert_eq!(report.missing_modules[0].id, "multi-file-mod");
    assert_eq!(report.missing_modules[0].version.as_deref(), Some("1.0.0"));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn all_bundled_modules_pass_sandboxed_runtime_contract_validation() {
    let bundled_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules");
    let tmp = std::env::temp_dir().join(format!(
        "keyglass-bundled-validate-{}",
        keyglass_bus::now_unix_ms()
    ));
    let reg = ModuleRegistry::new(
        bundled_dir,
        tmp.join("installed"),
        tmp.join("local"),
        tmp.join("state.json"),
    );
    let discovered = reg.discover();
    assert!(
        discovered.len() >= 10,
        "expected core bundled modules in repo, found {}",
        discovered.len()
    );

    let mut failures = Vec::new();
    for (id, dm) in &discovered {
        if let Err(errs) = keyglass_modules::validate_module_behavior(dm) {
            failures.push(format!("{id}: {}", errs.join("; ")));
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(
        failures.is_empty(),
        "Bundled module contract failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn host_process_manager_reports_missing_crashed_and_cleans_up_on_drop() {
    let tmp = std::env::temp_dir().join(format!(
        "keyglass-host-sup-{}",
        keyglass_bus::now_unix_ms()
    ));
    let mod_dir = tmp.join("mod-host-test");
    std::fs::create_dir_all(&mod_dir).unwrap();

    // 1. Missing host entrypoint MUST fail loudly and mark required module unhealthy.
    let manifest_toml = r#"
id = "mod-host-test"
name = "Host Supervisor Test"
version = "1.0.0"
category = "widget"
host_requirement = "required"
[entrypoints]
host = "missing.sh"
"#;
    std::fs::write(mod_dir.join("module.toml"), manifest_toml).unwrap();
    let manifest = keyglass_modules::ModuleManifest::load_from_dir(&mod_dir).unwrap();
    let dm = keyglass_modules::DiscoveredModule {
        manifest,
        path: mod_dir.clone(),
        tier: ModuleTier::Local,
    };
    let inst = ModuleInstanceState {
        enabled: true,
        ..Default::default()
    };

    let mut mgr = keyglass_modules::HostProcessManager::new("http://127.0.0.1:52324");
    assert!(mgr.start_module(&dm, &inst).is_err());
    let st = mgr.status("mod-host-test", true);
    assert!(!st.is_healthy(keyglass_modules::HostRequirement::Required));
    assert!(st.error_message().unwrap_or_default().contains("not found"));

    // 2. Crashing host script MUST transition to Exited with exit code and captured stderr.
    let crash_sh = mod_dir.join("crash.sh");
    std::fs::write(&crash_sh, "#!/bin/sh\necho 'boom-stderr' >&2\nexit 42\n").unwrap();
    let mut dm_crash = dm.clone();
    dm_crash.manifest.entrypoints.host = Some("crash.sh".into());
    mgr.start_module(&dm_crash, &inst).unwrap();
    let mut saw_exited = false;
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(30));
        if let keyglass_modules::HostProcessStatus::Exited { code, last_stderr } =
            mgr.status("mod-host-test", true)
        {
            assert_eq!(code, Some(42));
            assert_eq!(last_stderr.as_deref(), Some("boom-stderr"));
            saw_exited = true;
            break;
        }
    }
    assert!(saw_exited, "expected crashed host process to report Exited");

    // 3. Long-running child MUST be terminated cleanly when HostProcessManager is dropped (zero orphans!).
    let sleep_sh = mod_dir.join("stay.sh");
    std::fs::write(&sleep_sh, "#!/bin/sh\nsleep 30\n").unwrap();
    let mut dm_stay = dm.clone();
    dm_stay.manifest.entrypoints.host = Some("stay.sh".into());
    mgr.start_module(&dm_stay, &inst).unwrap();
    let pid = match mgr.status("mod-host-test", true) {
        keyglass_modules::HostProcessStatus::Running { pid } => pid,
        other => panic!("expected Running, got {other:?}"),
    };
    drop(mgr);

    #[cfg(unix)]
    {
        let mut alive = true;
        for _ in 0..20 {
            let rc = unsafe { libc::kill(pid as i32, 0) };
            if rc != 0 {
                alive = false;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(!alive, "child process {pid} leaked after HostProcessManager drop");
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn module_registry_blocks_path_traversal_on_uninstall_fork_and_scaffold() {
    let tmp = std::env::temp_dir().join(format!(
        "keyglass-sec-traversal-{}",
        keyglass_bus::now_unix_ms()
    ));
    let sentinel = tmp.join("keep_me.txt");
    std::fs::create_dir_all(tmp.join("v2/modules/local")).unwrap();
    std::fs::write(&sentinel, "do not delete").unwrap();

    let reg = ModuleRegistry::new(
        tmp.join("bundled"),
        tmp.join("v2/modules/installed"),
        tmp.join("v2/modules/local"),
        tmp.join("v2/state.json"),
    );

    for bad_id in ["", ".", "..", "../../..", "../escape", "foo/bar", "a\\b", "BadCaps"] {
        assert!(reg.uninstall_module(bad_id).is_err(), "uninstall({bad_id:?}) must fail");
        assert!(
            reg.scaffold_local(bad_id, keyglass_modules::ModuleCategory::Widget, None)
                .is_err(),
            "scaffold({bad_id:?}) must fail"
        );
        assert!(
            reg.fork_module("widget-clock", bad_id, None).is_err(),
            "fork({bad_id:?}) must fail"
        );
    }
    assert!(sentinel.is_file(), "sentinel outside modules dir must never be deleted");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn lua_sandbox_blocks_os_execute_and_aborts_infinite_loops() {
    let tmp = std::env::temp_dir().join(format!(
        "keyglass-sec-sandbox-{}",
        keyglass_bus::now_unix_ms()
    ));
    let pwn_file = tmp.join("should_not_exist");

    let exec_dir = tmp.join("local/bad-exec");
    std::fs::create_dir_all(&exec_dir).unwrap();
    std::fs::write(
        exec_dir.join("module.toml"),
        r#"
id = "bad-exec"
name = "Bad Exec"
version = "1.0.0"
category = "effect"
host_requirement = "none"
stages = ["under_key"]
[entrypoints]
script = "script.lua"
"#,
    )
    .unwrap();
    std::fs::write(
        exec_dir.join("script.lua"),
        format!("function on_init() os.execute('touch {}') end\n", pwn_file.display()),
    )
    .unwrap();

    let loop_dir = tmp.join("local/bad-loop");
    std::fs::create_dir_all(&loop_dir).unwrap();
    std::fs::write(
        loop_dir.join("module.toml"),
        r#"
id = "bad-loop"
name = "Bad Loop"
version = "1.0.0"
category = "effect"
host_requirement = "none"
stages = ["under_key"]
[entrypoints]
script = "script.lua"
"#,
    )
    .unwrap();
    std::fs::write(
        loop_dir.join("script.lua"),
        "function on_render(ctx, stage) while true do end end\n",
    )
    .unwrap();

    let reg = ModuleRegistry::new(
        tmp.join("bundled"),
        tmp.join("installed"),
        tmp.join("local"),
        tmp.join("state.json"),
    );
    let discovered = reg.discover();
    let mut state = BTreeMap::new();
    for id in ["bad-exec", "bad-loop"] {
        state.insert(
            id.to_string(),
            ModuleInstanceState {
                enabled: true,
                ..Default::default()
            },
        );
    }

    let broker = Broker::new("test");
    let geom = KeyboardGeometry::build(FrameLayout::Ansi);
    let mut rt = ModuleRuntime::new(broker, geom);
    rt.sync_modules(&discovered, &state);
    let t0 = std::time::Instant::now();
    let _ = rt.step_frame(0.016, 1000);
    let elapsed = t0.elapsed();

    assert!(!pwn_file.exists(), "os.execute must be stripped from Lua sandbox");
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "infinite loop must be aborted by watchdog quickly, took {elapsed:?}"
    );
    let statuses = rt.statuses();
    let exec_st = statuses.iter().find(|s| s.id == "bad-exec").unwrap();
    let loop_st = statuses.iter().find(|s| s.id == "bad-loop").unwrap();
    assert!(!exec_st.healthy, "bad-exec must be marked unhealthy");
    assert!(!loop_st.healthy, "bad-loop must be marked unhealthy");
    assert!(
        loop_st
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("execution budget"),
        "expected watchdog error, got: {:?}",
        loop_st.last_error
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn profile_api_merges_overrides_with_fallthrough_and_touch_bar_slot_override() {
    let tmp = std::env::temp_dir().join(format!(
        "keyglass-profile-api-{}",
        keyglass_bus::now_unix_ms()
    ));
    let mod_dir = tmp.join("local/widget-prof-test");
    std::fs::create_dir_all(&mod_dir).unwrap();
    std::fs::write(
        mod_dir.join("module.toml"),
        r##"
id = "widget-prof-test"
name = "Profile Test Widget"
version = "1.0.0"
category = "widget"
slot_capable = true
stages = ["tile"]
[entrypoints]
script = "script.lua"
[channels]
subscribes = ["profiles/active"]
publishes = ["test/profile-observed"]
[[settings]]
key = "dimming"
label = "Dimming"
kind = "number"
default = 0.2
[[settings]]
key = "accent"
label = "Accent"
kind = "color"
default = "#112233"
"##,
    )
    .unwrap();
    std::fs::write(
        mod_dir.join("script.lua"),
        r#"
function on_render(ctx, stage)
  local cfg = module.config()
  local sr = module.slot_rect()
  bus.publish("test/profile-observed", {
    profile_id = profile.id(),
    has_dev_tag = profile.has_tag("dev"),
    dimming = cfg.dimming,
    base_dimming = module.base_config().dimming,
    has_slot = (sr ~= nil),
    slot_x = sr and sr.x or -1,
  }, true)
end
"#,
    )
    .unwrap();

    let reg = ModuleRegistry::new(
        tmp.join("bundled"),
        tmp.join("installed"),
        tmp.join("local"),
        tmp.join("state.json"),
    );
    let discovered = reg.discover();
    let mut state = BTreeMap::new();
    state.insert(
        "widget-prof-test".to_string(),
        ModuleInstanceState {
            enabled: true,
            slot: Some(0),
            slot_pos: Some(0.0),
            slot_span: Some(1.0),
            ..Default::default()
        },
    );

    let broker = Broker::new("test");
    let geom = KeyboardGeometry::build(FrameLayout::Ansi);
    let mut rt = ModuleRuntime::new(broker.clone(), geom);
    rt.sync_modules(&discovered, &state);

    // 1. Default profile: base config dimming=0.2, placed at slot 0
    let _ = rt.step_frame(0.016, 1000);
    let obs1 = broker.get_retained("test/profile-observed").unwrap().payload;
    assert_eq!(obs1["profile_id"], "default");
    assert_eq!(obs1["dimming"], 0.2);
    assert_eq!(obs1["has_slot"], true);
    let x_slot0 = obs1["slot_x"].as_f64().unwrap();

    // 2. Activate "coding" profile overriding dimming=0.65 & moving widget to slot 2 via Touch Bar override!
    let _ = broker.publish(
        "profiles/active",
        serde_json::json!({
            "profile_id": "coding",
            "profile_name": "Coding",
            "chain": ["coding", "default"],
            "tags": ["dev", "base"],
            "module_overrides": {
                "widget-prof-test": { "dimming": 0.65 }
            }
        }),
        PublishOptions::retained(),
    );
    let _ = broker.publish(
        "layout/top_bar/context_override",
        serde_json::json!({
            "active": true,
            "rule_name": "Coding",
            "slots": [{ "module_id": "widget-prof-test", "pos": 2.0, "span": 1.0 }]
        }),
        PublishOptions::retained(),
    );
    let _ = rt.step_frame(0.016, 1016);
    let obs2 = broker.get_retained("test/profile-observed").unwrap().payload;
    assert_eq!(obs2["profile_id"], "coding");
    assert_eq!(obs2["has_dev_tag"], true);
    assert_eq!(obs2["dimming"], 0.65);
    assert_eq!(obs2["base_dimming"], 0.2);
    assert!(obs2["slot_x"].as_f64().unwrap() > x_slot0 + 400.0);

    // 3. Touch Bar override without this widget hides it cleanly (`slot_rect() == nil`)!
    let _ = broker.publish(
        "layout/top_bar/context_override",
        serde_json::json!({
            "active": true,
            "rule_name": "Minimal",
            "slots": [{ "module_id": "widget-clock", "pos": 0.0, "span": 1.0 }]
        }),
        PublishOptions::retained(),
    );
    let _ = rt.step_frame(0.016, 1032);
    let obs3 = broker.get_retained("test/profile-observed").unwrap().payload;
    assert_eq!(obs3["has_slot"], false);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn module_deprecation_rename_migration_and_removed_source_memory() {
    let tmp = std::env::temp_dir().join(format!(
        "keyglass-mod-migrate-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let old_up = tmp.join("upstream-old");
    let new_up = tmp.join("upstream-new");
    std::fs::create_dir_all(&old_up).unwrap();
    std::fs::create_dir_all(&new_up).unwrap();

    // 1. Install v1.0.0 from old upstream and save custom user config + slot placement
    std::fs::write(
        old_up.join("module.toml"),
        r#"
id = "widget-old-name"
name = "Old Widget"
version = "1.0.0"
author = "Author"
category = "widget"
description = "Original widget"
stages = ["tile"]
host_requirement = "none"
slot_capable = true
[entrypoints]
script = "script.lua"
[channels]
subscribes = []
publishes = []
"#,
    )
    .unwrap();
    std::fs::write(old_up.join("script.lua"), "function on_render(ctx, stage) end\n").unwrap();

    std::fs::write(
        new_up.join("module.toml"),
        r#"
id = "widget-new-name"
name = "New Widget"
version = "2.0.0"
author = "Author"
category = "widget"
description = "Renamed replacement widget"
stages = ["tile"]
host_requirement = "none"
slot_capable = true
[entrypoints]
script = "script.lua"
[channels]
subscribes = []
publishes = []
"#,
    )
    .unwrap();
    std::fs::write(new_up.join("script.lua"), "function on_render(ctx, stage) end\n").unwrap();

    let reg = ModuleRegistry::new(
        tmp.join("bundled"),
        tmp.join("installed"),
        tmp.join("local"),
        tmp.join("state.json"),
    );
    reg.install_from_source(&old_up.to_string_lossy()).unwrap();
    let mut st = reg.load_state();
    st.modules.insert(
        "widget-old-name".into(),
        ModuleInstanceState {
            enabled: true,
            slot: Some(2),
            slot_pos: Some(2.0),
            slot_span: Some(1.0),
            pin_tab: None,
            config: BTreeMap::from([("accent".into(), json!("#ff9900"))]),
        },
    );
    reg.save_state(&st).unwrap();

    // 2. Author publishes a deprecation + move/rename tombstone at the old upstream
    std::fs::write(
        old_up.join("module.toml"),
        format!(
            r#"
id = "widget-old-name"
name = "Old Widget"
version = "1.0.1"
author = "Author"
deprecated = true
deprecation_message = "Renamed to New Widget; migrate to keep receiving updates."
moved_to = "{}"
replaced_by = "widget-new-name"
category = "widget"
description = "Original widget"
stages = ["tile"]
host_requirement = "none"
slot_capable = true
[entrypoints]
script = "script.lua"
[channels]
subscribes = []
publishes = []
"#,
            new_up.to_string_lossy().replace('\\', "\\\\")
        ),
    )
    .unwrap();

    let updates = reg.check_available_updates();
    assert_eq!(updates.len(), 1);
    let u = &updates[0];
    assert!(u.deprecated);
    assert_eq!(u.replaced_by.as_deref(), Some("widget-new-name"));
    assert_eq!(u.moved_to.as_deref(), Some(new_up.to_string_lossy().as_ref()));

    // 3. Migrate via staged snapshot -> new module installed, old removed, user config & slot preserved!
    let staged = keyglass_modules::stage_module_source(
        u.moved_to.as_deref().unwrap(),
        &[],
        &tmp.join("bundled"),
    )
    .unwrap();
    let migrated = reg
        .migrate_module_from_staged("widget-old-name", &staged)
        .unwrap();
    assert_eq!(migrated.manifest.id, "widget-new-name");
    assert!(!tmp.join("installed/widget-old-name").exists());
    let after_state = reg.load_state();
    assert!(!after_state.modules.contains_key("widget-old-name"));
    let carried = after_state.modules.get("widget-new-name").unwrap();
    assert!(carried.enabled);
    assert_eq!(carried.slot, Some(2));
    assert_eq!(carried.config["accent"], json!("#ff9900"));

    // 4. Uninstalling remembers direct source; forgetting clears it cleanly.
    reg.uninstall_module("widget-new-name").unwrap();
    let rem = reg.load_state().removed_sources;
    assert_eq!(rem.len(), 1);
    assert_eq!(rem[0].id, "widget-new-name");
    assert!(rem[0].source.ends_with("/upstream-new"));
    reg.forget_removed_source("widget-new-name").unwrap();
    assert!(reg.load_state().removed_sources.is_empty());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn update_geometry_propagates_live_parallax_to_already_loaded_lua_modules() {
    let tmp = std::env::temp_dir().join(format!(
        "keyglass-parallax-live-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let mod_dir = tmp.join("bundled/parallax-probe");
    std::fs::create_dir_all(&mod_dir).unwrap();
    std::fs::write(
        mod_dir.join("module.toml"),
        r#"
id = "parallax-probe"
name = "Parallax Probe"
version = "1.0.0"
author = "Keyglass"
category = "effect"
description = "Verifies keyboard.layout() reflects live update_geometry() parallax shifts"
stages = ["under_key"]
host_requirement = "none"
[entrypoints]
script = "script.lua"
[channels]
subscribes = ["keyboard/geometry"]
publishes = ["test/parallax-observed"]
"#,
    )
    .unwrap();
    std::fs::write(
        mod_dir.join("script.lua"),
        r#"
local function report()
  local g = keyboard.layout()
  local k0 = g.keys[1]
  bus.publish("test/parallax-observed", {
    dx = k0.parallax_dx,
    dy = k0.parallax_dy,
    vx = k0.visual_x,
    vr_x = k0.visual_rect and k0.visual_rect.x or nil,
  }, true)
end
function on_init() report() end
function on_event(topic, payload)
  if topic == "keyboard/geometry" then report() end
end
"#,
    )
    .unwrap();

    let reg = ModuleRegistry::new(
        tmp.join("bundled"),
        tmp.join("installed"),
        tmp.join("local"),
        tmp.join("state.json"),
    );
    let broker = Broker::new("test");
    let mut geom = KeyboardGeometry::build(FrameLayout::Ansi);
    let orig_x = geom.key(0).unwrap().rect.x;
    let mut rt = ModuleRuntime::new(broker.clone(), geom.clone());
    let discovered = reg.discover();
    let states = BTreeMap::from([(
        "parallax-probe".into(),
        ModuleInstanceState {
            enabled: true,
            ..Default::default()
        },
    )]);
    rt.sync_modules(&discovered, &states);
    let obs0 = broker.get_retained("test/parallax-observed").unwrap().payload;
    assert_eq!(obs0["dx"], json!(0.0));

    // Apply live optical parallax WITHOUT reloading the Lua module!
    geom.apply_parallax(false, 0.0, 0.0, 425.0, 8.0, 12.5, -18.25);
    rt.update_geometry(geom);
    let _ = rt.step_frame(0.016, 100);
    let obs1 = broker.get_retained("test/parallax-observed").unwrap().payload;
    assert!((obs1["dx"].as_f64().unwrap() - 12.5).abs() < 0.01);
    assert!((obs1["dy"].as_f64().unwrap() - (-18.25)).abs() < 0.01);
    assert!((obs1["vr_x"].as_f64().unwrap() - (orig_x as f64 + 12.5)).abs() < 0.01);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn lua_sandbox_blocks_c_library_loading_bytecode_and_out_of_root_fs_reads() {
    let tmp = std::env::temp_dir().join(format!(
        "keyglass-lua-sec-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let secret_file = tmp.join("outside_secret.txt");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(&secret_file, "TOP_SECRET_TOKEN").unwrap();

    let bundled = tmp.join("bundled");
    let installed = tmp.join("installed");
    let local = tmp.join("local");
    let mod_dir = local.join("sec-probe");
    std::fs::create_dir_all(&mod_dir).unwrap();
    std::fs::write(mod_dir.join("own.txt"), "ALLOWED_OWN_FILE").unwrap();
    std::fs::write(
        mod_dir.join("module.toml"),
        r#"
id = "sec-probe"
name = "Sec Probe"
version = "1.0.0"
description = "Sandbox security test"
category = "effect"
stages = ["under_key"]
host_requirement = "none"
[entrypoints]
script = "script.lua"
[channels]
publishes = ["test/sec"]
"#,
    )
    .unwrap();
    let secret_escaped = secret_file.to_string_lossy().replace('\\', "\\\\");
    std::fs::write(
        mod_dir.join("script.lua"),
        format!(
            r#"
function on_init()
  local own = fs.read_text(module.dir() .. "/own.txt")
  local outside = fs.read_text("{secret_escaped}")
  local outside_exists = fs.exists("{secret_escaped}")
  local bc_ok = load("\x1bLua\x54\x00") ~= nil
  local has_c_searcher = (package.searchers[3] ~= nil) or (package.searchers[4] ~= nil)
  local has_loadlib = (package.loadlib ~= nil)
  local has_path_env = (os.getenv("PATH") ~= nil)
  bus.publish("test/sec", {{
    own = own,
    outside = outside,
    outside_exists = outside_exists,
    bc_ok = bc_ok,
    has_c_searcher = has_c_searcher,
    has_loadlib = has_loadlib,
    has_path_env = has_path_env,
    cpath = package.cpath,
  }}, true)
end
"#
        ),
    )
    .unwrap();

    let reg = ModuleRegistry::new(bundled, installed, local, tmp.join("state.json"));
    let broker = Broker::new("sec-test");
    let mut rt = ModuleRuntime::new(broker.clone(), KeyboardGeometry::build(FrameLayout::Ansi));
    let discovered = reg.discover();
    let states = BTreeMap::from([(
        "sec-probe".into(),
        ModuleInstanceState {
            enabled: true,
            ..Default::default()
        },
    )]);
    rt.sync_modules(&discovered, &states);
    let res = broker
        .get_retained("test/sec")
        .unwrap_or_else(|| panic!("no test/sec published; discovered={:?} statuses={:?}", discovered.keys().collect::<Vec<_>>(), rt.statuses()))
        .payload;
    assert_eq!(res["own"], json!("ALLOWED_OWN_FILE"));
    assert!(res["outside"].is_null(), "must block fs.read_text outside allowed roots");
    assert_eq!(res["outside_exists"], json!(false), "must block fs.exists outside allowed roots");
    assert_eq!(res["bc_ok"], json!(false), "must block binary Lua bytecode in load()");
    assert_eq!(res["has_c_searcher"], json!(false), "must strip C searchers [3]/[4]");
    assert_eq!(res["has_loadlib"], json!(false), "must strip package.loadlib");
    assert_eq!(res["has_path_env"], json!(false), "must restrict os.getenv");
    assert_eq!(res["cpath"], json!(""), "package.cpath must be empty");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[cfg(feature = "host-net")]
#[test]
fn opt_in_host_network_perm_disclosure_tcp_binary_roundtrip_http_and_unload_cleanup() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let tmp = std::env::temp_dir().join(format!(
        "keyglass-lua-net-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let local = tmp.join("local");

    // 1. Start a tiny loopback TCP echo server + loopback HTTP server
    let tcp_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_port = tcp_listener.local_addr().unwrap().port();
    let tcp_eof_seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let eof_flag = tcp_eof_seen.clone();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = tcp_listener.accept() {
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(3)));
            let mut buf = [0u8; 64];
            if let Ok(n) = s.read(&mut buf) {
                let _ = s.write_all(&buf[..n]);
                let _ = s.flush();
            }
            // Wait for client close on module unload!
            if let Ok(0) = s.read(&mut buf) {
                eof_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    });

    let http_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let http_port = http_listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = http_listener.accept() {
            let mut req_buf = [0u8; 1024];
            let _ = s.read(&mut req_buf);
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 13\r\nConnection: close\r\n\r\n{\"pong\":true}";
            let _ = s.write_all(resp.as_bytes());
        }
    });

    // 2. Module WITHOUT permissions.network must see `net == nil` even on host_network runtime!
    let no_net_dir = local.join("no-net");
    std::fs::create_dir_all(&no_net_dir).unwrap();
    std::fs::write(
        no_net_dir.join("module.toml"),
        r#"
id = "no-net"
name = "No Net"
version = "1.0.0"
description = "Should not receive net table"
category = "effect"
stages = []
[entrypoints]
script = "script.lua"
"#,
    )
    .unwrap();
    std::fs::write(
        no_net_dir.join("script.lua"),
        r#"function on_init() bus.publish("test/no-net", { has_net = (net ~= nil) }, true) end"#,
    )
    .unwrap();

    // 3. Module WITH permissions.network = true gets `net.tcp_*` & `net.http` AND disclosure warning!
    let with_net_dir = local.join("with-net");
    std::fs::create_dir_all(&with_net_dir).unwrap();
    std::fs::write(
        with_net_dir.join("module.toml"),
        r#"
id = "with-net"
name = "With Net"
version = "1.0.0"
description = "Uses host net API"
category = "effect"
stages = []
[permissions]
network = true
[entrypoints]
script = "script.lua"
"#,
    )
    .unwrap();
    std::fs::write(
        with_net_dir.join("script.lua"),
        format!(
            r#"
local sock_id = nil
local sent_n = 0
local http_res = nil
local got = nil
function on_init()
  local id, err = net.tcp_connect("127.0.0.1", {tcp_port}, {{ timeout_ms = 2000 }})
  sock_id = id
  if id then
    sent_n = net.tcp_send(id, "\x00\x01hello\xff") or 0
  end
  http_res = net.http({{ url = "http://127.0.0.1:{http_port}/ping", timeout_ms = 2000 }})
  bus.publish("test/with-net", {{
    connected = (id ~= nil),
    err = err,
    sent = sent_n,
    echo_match = false,
    http_ok = http_res.ok,
    http_status = http_res.status,
    http_body = http_res.body,
  }}, true)
end
function on_update(dt)
  if sock_id and not got then
    local chunk = net.tcp_recv(sock_id, 64)
    if chunk then
      got = chunk
      bus.publish("test/with-net", {{
        connected = true,
        sent = sent_n,
        echo_len = #got,
        echo_match = (got == "\x00\x01hello\xff"),
        http_ok = http_res.ok,
        http_status = http_res.status,
        http_body = http_res.body,
      }}, true)
    end
  end
end
"#
        ),
    )
    .unwrap();

    // Disclosure check
    let disc = keyglass_modules::inspect_module_dir(&with_net_dir).unwrap();
    assert!(
        disc.warnings.iter().any(|w| w.contains("outbound host network")),
        "expected outbound host network disclosure warning, got: {:?}",
        disc.warnings
    );

    let reg = ModuleRegistry::new(tmp.join("b"), tmp.join("i"), local, tmp.join("state.json"));
    let broker = Broker::new("net-test");
    let mut rt = ModuleRuntime::new(broker.clone(), KeyboardGeometry::build(FrameLayout::Ansi))
        .with_host_network(true);
    let discovered = reg.discover();
    let states = BTreeMap::from([
        ("no-net".into(), ModuleInstanceState { enabled: true, ..Default::default() }),
        ("with-net".into(), ModuleInstanceState { enabled: true, ..Default::default() }),
    ]);
    rt.sync_modules(&discovered, &states);

    assert_eq!(broker.get_retained("test/no-net").unwrap().payload["has_net"], json!(false));
    for i in 0..40 {
        rt.step_frame(0.016, 100 + i * 16);
        if broker
            .get_retained("test/with-net")
            .is_some_and(|m| m.payload["echo_match"] == json!(true))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let res = broker.get_retained("test/with-net").unwrap().payload;
    assert_eq!(res["connected"], json!(true), "tcp_connect failed: {res}");
    assert_eq!(res["sent"], json!(8));
    assert_eq!(res["echo_match"], json!(true), "binary TCP roundtrip mismatch: {res}");
    assert_eq!(res["http_ok"], json!(true), "http failed: {res}");
    assert_eq!(res["http_status"], json!(200));
    assert_eq!(res["http_body"], json!("{\"pong\":true}"));

    // Disable module -> open TCP socket MUST close cleanly so server observes EOF (MQTT Last-Will!)
    rt.sync_modules(&discovered, &BTreeMap::new());
    for _ in 0..40 {
        if tcp_eof_seen.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        tcp_eof_seen.load(std::sync::atomic::Ordering::SeqCst),
        "disabling module must cleanly close all open TCP sockets"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
