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
