use openflux_bus::{Broker, FrameLayout, KeyboardGeometry, PublishOptions};
use openflux_modules::{
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
    let tmp = std::env::temp_dir().join(format!("openflux-mod-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let reg = ModuleRegistry::new(
        repo_bundled_modules(),
        tmp.join("installed"),
        tmp.join("local"),
        tmp.join("state.json"),
    );

    // 1. Forking a bundled module creates a Local tier override with forked_from metadata.
    let forked = reg
        .fork_module("fx-water-ripple-3d", "my-water", Some("My Custom Water"))
        .unwrap();
    assert_eq!(forked.tier, ModuleTier::Local);
    assert_eq!(
        forked.manifest.forked_from.as_deref(),
        Some("fx-water-ripple-3d")
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

    // 3. Preset export strips secret fields (`api_key` on widget-weather).
    let mut st = reg.load_state();
    st.modules.insert(
        "widget-weather".into(),
        ModuleInstanceState {
            enabled: true,
            slot: Some(1),
            slot_pos: Some(1.0),
            slot_span: None,
            pin_tab: None,
            config: BTreeMap::from([("api_key".into(), json!("secret-xyz"))]),
        },
    );
    reg.save_state(&st).unwrap();
    let preset = reg.export_preset("Test", "");
    assert!(
        !preset.modules["widget-weather"]
            .config
            .contains_key("api_key"),
        "secret field must be stripped from exported preset"
    );

    // 4. Cross-module composition: Idle Pet lands on a key -> publishes `fx/surface/impulse`
    //    -> 3D Water Ripple receives it and emits a WaveImpulseCmd without knowing about the Pet!
    let broker = Broker::new("test");
    let geom = KeyboardGeometry::build(FrameLayout::Ansi);
    let mut rt = ModuleRuntime::new(broker.clone(), geom);
    let discovered = reg.discover();
    let states = BTreeMap::from([
        (
            "companion-idle-pet".into(),
            ModuleInstanceState {
                enabled: true,
                slot: Some(3),
                slot_pos: Some(3.0),
                slot_span: None,
                pin_tab: None,
                config: BTreeMap::new(),
            },
        ),
        (
            "fx-water-ripple-3d".into(),
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
            "modules/companion-idle-pet/command",
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
    let tmp = std::env::temp_dir().join(format!("openflux-v2-audit-{}", std::process::id()));
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
    let mut missing_state = openflux_modules::ModulesStateFile::default();
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
        "openflux-bundled-validate-{}",
        openflux_bus::now_unix_ms()
    ));
    let reg = ModuleRegistry::new(
        bundled_dir,
        tmp.join("installed"),
        tmp.join("local"),
        tmp.join("state.json"),
    );
    let discovered = reg.discover();
    assert!(
        discovered.len() >= 20,
        "expected bundled modules in repo, found {}",
        discovered.len()
    );

    let mut failures = Vec::new();
    for (id, dm) in &discovered {
        if let Err(errs) = openflux_modules::validate_module_behavior(dm) {
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
        "openflux-host-sup-{}",
        openflux_bus::now_unix_ms()
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
    let manifest = openflux_modules::ModuleManifest::load_from_dir(&mod_dir).unwrap();
    let dm = openflux_modules::DiscoveredModule {
        manifest,
        path: mod_dir.clone(),
        tier: ModuleTier::Local,
    };
    let inst = ModuleInstanceState {
        enabled: true,
        ..Default::default()
    };

    let mut mgr = openflux_modules::HostProcessManager::new("http://127.0.0.1:52324");
    assert!(mgr.start_module(&dm, &inst).is_err());
    let st = mgr.status("mod-host-test", true);
    assert!(!st.is_healthy(openflux_modules::HostRequirement::Required));
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
        if let openflux_modules::HostProcessStatus::Exited { code, last_stderr } =
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
        openflux_modules::HostProcessStatus::Running { pid } => pid,
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
