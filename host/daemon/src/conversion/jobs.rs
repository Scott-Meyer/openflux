//! Conversion jobs: converting a keyboard to OpenFlux, and resetting it to its factory software.
//!
//! Converting reads the keyboard's existing settings, installs OpenFlux firmware, bridge and
//! display along the engine's conversion proposal, and writes the settings into the OpenFlux
//! configuration. The firmware flash never writes the calibration region and checks its hash
//! before and after. Last, it installs the keyboard maker's press-refinement data (see
//! [`super::press_refinement`]); a keyboard without it still works, so that step never fails the
//! conversion.
//!
//! Resetting to factory downloads the factory files the signed index pins, stages the factory
//! packages on the keyboard, starts the keyboard's restore with the OpenFlux restore package,
//! flashes the factory firmware, writes the keymap back in the factory firmware's encoding and
//! checks every version. Calibration is never written in either direction.

use super::factory::{self, FactoryError, FactoryReference};
use super::Identity;
use crate::device_paths;
use crate::updates::install::{self, wait_until};
use crate::updates::jobs::{JobError, JobKind, JobRecord, PlannedStep, StepKind};
use crate::updates::status::{KeyboardView, ViewMode};
use crate::updates::{Refused, UpdateService};
use openflux_update::{Component, Engine};

/// Where keyboards that don't run OpenFlux keep their display configuration and media.
const EXISTING_CONFIG_DIR: &str = "/home/root/flux/config";
const EXISTING_MEDIA_DIR: &str = "/home/root/flux/media";
const EXISTING_KEY_ICONS_DIR: &str = "/home/root/flux/media/key_icons";

fn step(kind: StepKind, label: impl Into<String>) -> PlannedStep {
    PlannedStep {
        kind,
        label: label.into(),
        component: None,
        from: None,
        expect: None,
        artifact: None,
        state: Default::default(),
    }
}

fn refused(code: &'static str, message: impl Into<String>) -> Refused {
    Refused {
        status: 409,
        code,
        message: message.into(),
    }
}

/// The steps that convert the keyboard in `view`, or why it can't be converted now.
pub fn conversion_steps(
    svc: &UpdateService,
    view: &KeyboardView,
) -> Result<Vec<PlannedStep>, Refused> {
    match view.mode() {
        ViewMode::Disconnected => {
            return Err(refused(
                "keyboardDisconnected",
                "The keyboard isn't connected",
            ))
        }
        ViewMode::OpenFlux => {
            return Err(refused(
                "alreadyOpenFlux",
                "The keyboard already runs OpenFlux",
            ))
        }
        ViewMode::NeedsConversion => {}
    }
    let catalog = svc.sources().catalog();
    let inventory = view.inventory();
    let engine = Engine::new(
        &svc.profile,
        &inventory,
        &catalog,
        openflux_update::Preferences::default(),
    );
    let proposal = engine
        .propose_conversion()
        .map_err(|r| refused("noPlan", r.to_string()))?;
    let release = |c: Component| proposal.install.iter().find(|r| r.component == c);
    let reported = |c: Component| view.get(c).and_then(|x| x.reported.clone());
    let mut steps = vec![
        step(StepKind::Prepare, "Downloading and verifying OpenFlux"),
        step(StepKind::ReadSettings, "Reading the keyboard's settings"),
    ];
    let bridge = release(Component::Bridge)
        .ok_or_else(|| refused("noPlan", "No OpenFlux bridge release is available"))?;
    let mut rescue = step(
        StepKind::StageRescue,
        "Preparing the keyboard's recovery copy of the OpenFlux bridge",
    );
    rescue.artifact = install::artifact_for(bridge);
    steps.push(rescue);
    let mut expect = serde_json::Map::new();
    for c in [Component::Firmware, Component::Bridge, Component::Display] {
        let r = release(c)
            .ok_or_else(|| refused("noPlan", format!("No OpenFlux {c} release is available")))?;
        let artifact = install::artifact_for(r).ok_or_else(|| {
            refused(
                "malformed",
                format!("{c} {} has no installable file", r.version),
            )
        })?;
        let to = format!("openflux-{c} {}", r.version);
        expect.insert(c.as_str().into(), serde_json::json!(to));
        steps.push(PlannedStep {
            kind: if c == Component::Firmware {
                StepKind::FlashFirmware
            } else {
                StepKind::InstallPackage
            },
            label: format!("Installing OpenFlux {c} {}", r.version),
            component: Some(c.as_str().into()),
            // The display is reported through the bridge, so what it reports changes when the
            // bridge is replaced; it has no precondition.
            from: if c == Component::Display {
                None
            } else {
                reported(c)
            },
            expect: Some(to),
            artifact: Some(artifact),
            state: Default::default(),
        });
    }
    steps.push(step(
        StepKind::ImportSettings,
        "Importing the keyboard's settings",
    ));
    let mut verify = step(StepKind::Verify, "Checking the installed versions");
    verify
        .state
        .insert("expect".into(), serde_json::Value::Object(expect));
    steps.push(verify);
    steps.push(step(
        StepKind::InstallPressRefinement,
        "Installing press-refinement data",
    ));
    Ok(steps)
}

/// The steps that reset the keyboard in `view` to its factory software.
pub fn restore_steps(
    svc: &UpdateService,
    view: &KeyboardView,
) -> Result<Vec<PlannedStep>, Refused> {
    let reference = svc.sources().catalog().factory().cloned().ok_or(Refused {
        status: 501,
        code: "notAvailable",
        message: "The release index doesn't list the factory software; check for updates".into(),
    })?;
    if !(view.bridge_connected && view.firmware_connected) {
        return Err(refused(
            "keyboardDisconnected",
            "The keyboard isn't connected",
        ));
    }
    if !matches!(
        view.get(Component::Bridge).map(|b| &b.identity),
        Some(Identity::OpenFlux(_))
    ) {
        return Err(refused("unsupported", "The factory software is restored by the OpenFlux bridge; this keyboard's bridge isn't OpenFlux"));
    }
    let catalog = svc.sources().catalog();
    let trigger = catalog
        .releases()
        .iter()
        .filter(|r| r.component == Component::Bridge && !r.withdrawn)
        .filter_map(|r| install::factory_restore_trigger(r).map(|a| (r.version.clone(), a)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, a)| a)
        .ok_or_else(|| {
            refused(
                "notAvailable",
                "No bridge release carries the factory restore package",
            )
        })?;
    let file = |c: Component| {
        reference
            .file(c)
            .cloned()
            .expect("a verified reference lists every component")
    };
    let mut prepare = step(
        StepKind::Prepare,
        format!("Downloading and verifying {}", reference.label),
    );
    prepare.state.insert(
        "factory".into(),
        serde_json::to_value(&reference).unwrap_or_default(),
    );
    let mut stage = step(
        StepKind::StageFactoryRestore,
        "Starting the keyboard's restore",
    );
    stage.artifact = Some(trigger);
    let mut wait = step(
        StepKind::AwaitFactory,
        "Waiting for the factory bridge and display",
    );
    wait.state.insert(
        "bridge".into(),
        serde_json::json!(file(Component::Bridge).version),
    );
    wait.state.insert(
        "display".into(),
        serde_json::json!(file(Component::Display).version),
    );
    let mut flash = step(
        StepKind::FlashFirmware,
        format!(
            "Installing factory firmware {}",
            file(Component::Firmware).version
        ),
    );
    flash.component = Some("firmware".into());
    flash.expect = Some(file(Component::Firmware).version);
    flash.state.insert(
        "factoryFile".into(),
        serde_json::to_value(file(Component::Firmware)).unwrap_or_default(),
    );
    let mut keymap = step(
        StepKind::FactoryKeymap,
        "Writing the keymap for the factory firmware",
    );
    keymap.expect = Some(file(Component::Firmware).version);
    let mut verify = step(StepKind::Verify, "Checking the factory versions");
    verify.state.insert(
        "expect".into(),
        serde_json::json!({"firmware": file(Component::Firmware).version, "bridge": file(Component::Bridge).version, "display": file(Component::Display).version}),
    );
    Ok(vec![prepare, stage, wait, flash, keymap, verify])
}

fn job_reference(job: &JobRecord) -> Option<FactoryReference> {
    job.steps
        .iter()
        .find_map(|s| s.state.get("factory"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

fn factory_error(e: FactoryError) -> JobError {
    let recovery =
        "Nothing was changed on the keyboard. Check the internet connection and try again.";
    match e {
        FactoryError::Offline(m) => JobError::new("offline", m, recovery),
        FactoryError::Unavailable(m) => JobError::new("unavailable", m, recovery),
        FactoryError::Verification(m) => JobError::new("verification", m, "Nothing was changed on the keyboard. The published factory files don't match the signed index."),
        FactoryError::Storage(m) => JobError::new(
            "storage",
            m,
            "Nothing was changed on the keyboard. Check that the disk has free space and try again.",
        ),
    }
}

/// Downloads and verifies the factory files of a restore job.
pub fn prepare_factory_files(svc: &UpdateService, job: &mut JobRecord) -> Result<(), JobError> {
    if job.kind != JobKind::Restore {
        return Ok(());
    }
    let reference = job_reference(job)
        .ok_or_else(|| JobError::new("malformed", "The job has no factory reference", ""))?;
    factory::obtain_all(&reference, svc.factory_container_url(), &svc.cache_dir())
        .map_err(factory_error)?;
    job.log.push(format!("Verified {}", reference.label));
    Ok(())
}

fn settings_dir(svc: &UpdateService, job: &JobRecord) -> std::path::PathBuf {
    svc.job_dir(&job.id).join("settings")
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Reads the keyboard's existing display configuration and keeps it with the job.
pub fn read_settings(svc: &UpdateService, job: &mut JobRecord, _i: usize) -> Result<(), JobError> {
    let dir = settings_dir(svc, job);
    std::fs::create_dir_all(&dir).map_err(|e| {
        JobError::new(
            "storage",
            e.to_string(),
            "Nothing was changed on the keyboard; try again.",
        )
    })?;
    let mut found = 0;
    for (remote, _) in device_paths::CONFIG_DOCUMENTS {
        let name = file_name(remote);
        let local = dir.join(name);
        if local.is_file() {
            found += 1;
            continue;
        }
        match svc
            .device
            .get_file(&format!("{EXISTING_CONFIG_DIR}/{name}"))
        {
            Ok(raw)
                if serde_json::from_slice::<serde_json::Value>(&raw)
                    .map(|v| v.is_object())
                    .unwrap_or(false) =>
            {
                crate::updates::source::write_durably(&local, &raw).map_err(|e| {
                    JobError::new(
                        "storage",
                        e,
                        "Nothing was changed on the keyboard; try again.",
                    )
                })?;
                found += 1;
            }
            Ok(_) => job.log.push(format!(
                "{name} on the keyboard isn't a settings document; it isn't imported"
            )),
            Err(_) => job.log.push(format!("The keyboard has no {name}")),
        }
    }
    job.log.push(format!("Read {found} settings documents"));
    Ok(())
}

/// Writes the settings read before the conversion into the OpenFlux configuration, and copies
/// media the configuration refers to. A file that can't be copied is reported in the log.
pub fn import_settings(
    svc: &UpdateService,
    job: &mut JobRecord,
    _i: usize,
) -> Result<(), JobError> {
    let dir = settings_dir(svc, job);
    if !wait_until(svc, svc.timings().reconnect_wait, || {
        svc.device.is_connected().0
    }) {
        return Err(JobError::new(
            "keyboardDisconnected",
            "The keyboard bridge isn't connected",
            "OpenFlux is installed. Connect the keyboard; the settings are imported when the job resumes.",
        ));
    }
    let mut imported = 0;
    let keymap_saved = import_keymap(svc, job, &dir);
    if keymap_saved {
        imported += 1;
    }
    for (remote, _) in device_paths::CONFIG_DOCUMENTS {
        if keymap_saved && remote == device_paths::KEYMAP_JSON {
            continue;
        }
        let Ok(raw) = std::fs::read(dir.join(file_name(remote))) else {
            continue;
        };
        let raw = with_openflux_names(job, file_name(remote), raw);
        match svc.device.put_file(remote, &raw) {
            Ok(()) => imported += 1,
            Err(e) => job
                .log
                .push(format!("Couldn't import {}: {e}", file_name(remote))),
        }
    }
    let mut media = 0;
    for (from, to) in [
        (EXISTING_MEDIA_DIR, device_paths::MEDIA_DIR),
        (EXISTING_KEY_ICONS_DIR, device_paths::KEY_ICONS_DIR),
    ] {
        let Ok(files) = svc.device.list_files(from) else {
            continue;
        };
        let present: std::collections::BTreeMap<String, u64> = svc
            .device
            .list_files(to)
            .unwrap_or_default()
            .into_iter()
            .collect();
        for (name, size) in files {
            if present.get(&name) == Some(&size) {
                continue;
            }
            let copied = svc
                .device
                .get_file(&format!("{from}/{name}"))
                .and_then(|data| svc.device.put_file(&format!("{to}/{name}"), &data));
            match copied {
                Ok(()) => media += 1,
                Err(e) => job.log.push(format!("Couldn't copy {name}: {e}")),
            }
        }
    }
    job.log.push(format!(
        "Imported {imported} settings documents and {media} media files"
    ));
    Ok(())
}

/// A settings document with every factory keycode name replaced by its OpenFlux name (see
/// [`crate::codecs::keycode_names`]). A document without any, or that isn't JSON, is returned as
/// it is.
fn with_openflux_names(job: &mut JobRecord, name: &str, raw: Vec<u8>) -> Vec<u8> {
    let Ok(mut doc) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return raw;
    };
    let renamed = crate::codecs::keycode_names::to_openflux(&mut doc);
    if renamed == 0 {
        return raw;
    }
    match serde_json::to_vec_pretty(&doc) {
        Ok(out) => {
            job.log.push(format!(
                "{name}: {renamed} key names translated to OpenFlux's"
            ));
            out
        }
        Err(_) => raw,
    }
}

/// Translates the firmware's keymap to OpenFlux's encoding (see [`super::keymap`]). The firmware
/// keymap survives the firmware flash, so it is read from the keyboard. With an imported keymap
/// document, both are saved together; otherwise the firmware keymap is written through the
/// keymap command and read back. Returns whether the keymap document was saved; when it can't
/// be, the log says why and the document is copied as it is.
///
/// The document's factory keycode names are translated to OpenFlux's first. A document that still
/// disagrees with the firmware keymap is rebuilt from it, keeping its layout and layer names: the
/// firmware keymap is what the keyboard does.
fn import_keymap(svc: &UpdateService, job: &mut JobRecord, dir: &std::path::Path) -> bool {
    let name = file_name(device_paths::KEYMAP_JSON);
    let document = std::fs::read(dir.join(name))
        .ok()
        .map(|raw| with_openflux_names(job, name, raw))
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok());
    let blob = match svc.device.read_firmware_keymap() {
        Ok(b) => b,
        Err(e) => {
            job.log.push(format!("The firmware keymap couldn't be read ({e}); its media keys keep their previous encoding"));
            return false;
        }
    };
    let (translated, t) = match super::keymap::translate_consumer_controls(&blob) {
        Ok(r) => r,
        Err(e) => {
            job.log
                .push(format!("The firmware keymap couldn't be translated: {e}"));
            return false;
        }
    };
    if !t.unknown.is_empty() {
        job.log.push(format!(
            "{} media keys have no OpenFlux equivalent and are left as they are",
            t.unknown.len()
        ));
    }
    let Some(mut keymap) = document else {
        match write_keymap_verified(svc, &translated) {
            Ok(()) => job.log.push(format!(
                "Translated the firmware keymap ({} media keys)",
                t.translated
            )),
            Err(e) => job.log.push(format!(
                "The translated firmware keymap couldn't be written: {e}"
            )),
        }
        return false;
    };
    if let Err(e) = crate::codecs::keymap::verify_keymap_matches_blob(&keymap, &translated) {
        match crate::codecs::keymap::keymap_json_from_blob(&translated, &keymap) {
            Ok(rebuilt) => {
                job.log.push(format!(
                    "keymap.json didn't match the firmware keymap ({e}); it is rebuilt from the firmware keymap"
                ));
                keymap = rebuilt;
            }
            Err(re) => job.log.push(format!(
                "keymap.json doesn't match the firmware keymap ({e}) and can't be rebuilt from it ({re})"
            )),
        }
    }
    let hex = crate::api::state::ApiState::encode_hex_bytes(&translated);
    let plan = serde_json::json!({"keymap": keymap, "keymapBlobHex": hex});
    match svc.device.save_config_dir(&plan) {
        Ok(()) => {
            job.log.push(format!(
                "Imported the keymap ({} media keys translated)",
                t.translated
            ));
            true
        }
        Err(e) => {
            job.log.push(format!(
                "The keymap couldn't be saved with the firmware keymap: {e}"
            ));
            false
        }
    }
}

/// Writes a keymap blob through the firmware's keymap command and checks it reads back the same.
fn write_keymap_verified(svc: &UpdateService, blob: &[u8]) -> Result<(), String> {
    svc.device.write_firmware_keymap(blob)?;
    let back = svc.device.read_firmware_keymap()?;
    if back != blob {
        return Err("the firmware keymap reads back differently".into());
    }
    Ok(())
}

/// Writes the firmware keymap in the factory firmware's encoding once the factory firmware runs.
///
/// Consumer controls are translated back; one with no factory encoding takes the key's default
/// binding, and each such key is listed in the log. Calibration is not touched.
pub fn factory_keymap(svc: &UpdateService, job: &mut JobRecord, i: usize) -> Result<(), JobError> {
    if job.steps[i].state.get("written").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(());
    }
    let expect = job.steps[i].expect.clone();
    let recovery = "The factory software is installed, but its media keys may not work. Reset to factory again, or set the media keys in the factory software.";
    let running = wait_until(svc, svc.timings().reconnect_wait, || {
        svc.device.is_connected().1 && install::reported(svc, "firmware") == expect
    });
    if !running {
        return Err(JobError::new(
            "keyboardDisconnected",
            "The factory firmware isn't running",
            recovery,
        ));
    }
    let blob = svc.device.read_firmware_keymap().map_err(|e| {
        JobError::new(
            "installFailed",
            format!("Reading the firmware keymap failed: {e}"),
            recovery,
        )
    })?;
    let (factory, t) = super::keymap::to_factory_encoding(&blob).map_err(|e| {
        JobError::new(
            "malformed",
            format!("The firmware keymap couldn't be translated: {e}"),
            recovery,
        )
    })?;
    if factory != blob {
        write_keymap_verified(svc, &factory).map_err(|e| {
            JobError::new(
                "installFailed",
                format!("Writing the factory keymap failed: {e}"),
                recovery,
            )
        })?;
    }
    for f in &t.fallbacks {
        let layout = if f.layout_keys == 87 { "ISO" } else { "ANSI" };
        job.log.push(format!(
            "{layout} layer {} key {}: media key 0x{:04X} has no factory equivalent; it takes its default binding (0x{:08X})",
            f.layer + 1,
            f.key + 1,
            f.word & 0xFFFF,
            f.replacement
        ));
    }
    job.log.push(format!(
        "Wrote the keymap for the factory firmware ({} media keys translated)",
        t.translated
    ));
    job.steps[i]
        .state
        .insert("written".into(), serde_json::json!(true));
    svc.save_job(job);
    Ok(())
}

/// Stages the OpenFlux bridge and its launcher, taken from the verified bridge package, as the
/// keyboard's rescue pair with a manifest, and reads everything back. An OpenFlux bridge package
/// installed over a bridge that isn't OpenFlux's refuses to install without this pair, so a
/// failure here stops the conversion before anything is installed.
pub fn stage_rescue(svc: &UpdateService, job: &mut JobRecord, i: usize) -> Result<(), JobError> {
    let recovery = "Nothing was installed. Reconnect the keyboard and convert again.";
    let artifact = job.steps[i]
        .artifact
        .clone()
        .ok_or_else(|| JobError::new("malformed", "The step has no bridge package", recovery))?;
    let path = install::verified_file(svc, &artifact)?;
    let ipk =
        std::fs::read(&path).map_err(|e| JobError::new("storage", e.to_string(), recovery))?;
    let files = super::ipk::read_files(
        &ipk,
        &[
            device_paths::BRIDGE_BINARY_PATH,
            device_paths::BRIDGE_LAUNCHER_PATH,
        ],
    )
    .map_err(|e| JobError::new("malformed", format!("{}: {e}", artifact.name), recovery))?;
    let mut staged = Vec::new();
    let mut manifest = String::new();
    for src in [
        device_paths::BRIDGE_BINARY_PATH,
        device_paths::BRIDGE_LAUNCHER_PATH,
    ] {
        let name = file_name(src);
        let data = &files[src];
        let (sha, _) = openflux_update::trust::sha256_hex(&data[..]).unwrap_or_default();
        manifest.push_str(&format!("{sha}  {name}\n"));
        staged.push((format!("{}/{name}", device_paths::RESCUE_DIR), data.clone()));
    }
    staged.push((
        format!("{}/manifest.sha256", device_paths::RESCUE_DIR),
        manifest.into_bytes(),
    ));
    for (remote, data) in &staged {
        svc.device.put_file(remote, data).map_err(|e| {
            JobError::new(
                "installFailed",
                format!("Staging {remote} failed: {e}"),
                recovery,
            )
        })?;
    }
    for (remote, data) in &staged {
        let back = svc.device.get_file(remote).map_err(|e| {
            JobError::new(
                "installFailed",
                format!("Reading back {remote} failed: {e}"),
                recovery,
            )
        })?;
        if &back != data {
            return Err(JobError::new(
                "verification",
                format!("{remote} doesn't match what was staged"),
                recovery,
            ));
        }
    }
    job.log
        .push("Staged and verified the recovery copy of the OpenFlux bridge".into());
    Ok(())
}

/// Stages the factory packages with their manifest and starts the keyboard's restore.
pub fn stage_factory_restore(
    svc: &UpdateService,
    job: &mut JobRecord,
    i: usize,
) -> Result<(), JobError> {
    let reference = job_reference(job)
        .ok_or_else(|| JobError::new("malformed", "The job has no factory reference", ""))?;
    if job.steps[i]
        .state
        .get("triggered")
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        return Ok(());
    }
    let recovery = "The keyboard still runs OpenFlux. Reconnect it and try again.";
    let mut manifest = String::new();
    for c in [Component::Bridge, Component::Display] {
        let f = reference
            .file(c)
            .expect("a verified reference lists every component");
        let path = factory::cached_path(&svc.cache_dir(), f);
        factory::verify_file(&path, f).map_err(factory_error)?;
        let data =
            std::fs::read(&path).map_err(|e| JobError::new("storage", e.to_string(), recovery))?;
        let remote = format!(
            "{}/{}",
            device_paths::FACTORY_RESTORE_DIR,
            factory::file_name(f)
        );
        svc.device.put_file(&remote, &data).map_err(|e| {
            JobError::new(
                "installFailed",
                format!("Staging {} failed: {e}", factory::file_name(f)),
                recovery,
            )
        })?;
        let back = svc.device.get_file(&remote).map_err(|e| {
            JobError::new(
                "installFailed",
                format!("Reading back {}: {e}", factory::file_name(f)),
                recovery,
            )
        })?;
        let (sha, _) = openflux_update::trust::sha256_hex(&back[..]).unwrap_or_default();
        if sha != f.sha256 {
            return Err(JobError::new(
                "verification",
                format!(
                    "{} changed on its way to the keyboard",
                    factory::file_name(f)
                ),
                recovery,
            ));
        }
        manifest.push_str(&format!("{}  {}\n", f.sha256, factory::file_name(f)));
    }
    svc.device
        .put_file(
            &format!("{}/manifest.sha256", device_paths::FACTORY_RESTORE_DIR),
            manifest.as_bytes(),
        )
        .map_err(|e| {
            JobError::new(
                "installFailed",
                format!("Staging the manifest failed: {e}"),
                recovery,
            )
        })?;
    let trigger = job.steps[i]
        .artifact
        .clone()
        .ok_or_else(|| JobError::new("malformed", "The step has no restore package", ""))?;
    let path = install::verified_file(svc, &trigger)?;
    svc.device
        .sideload_ipk(&path.to_string_lossy())
        .map_err(|e| {
            JobError::new(
                "installFailed",
                format!("Starting the restore failed: {e}"),
                recovery,
            )
        })?;
    job.steps[i]
        .state
        .insert("triggered".into(), serde_json::json!(true));
    svc.save_job(job);
    job.log
        .push("The keyboard is installing its factory software".into());
    Ok(())
}

/// Waits for the factory bridge and display to report their versions.
pub fn await_factory(svc: &UpdateService, job: &mut JobRecord, i: usize) -> Result<(), JobError> {
    let want = |k: &str| {
        job.steps[i]
            .state
            .get(k)
            .and_then(|v| v.as_str())
            .map(String::from)
    };
    let (bridge, display) = (want("bridge"), want("display"));
    let ok = wait_until(svc, svc.timings().install_wait, || {
        let v = svc.device.versions();
        let r = |c: &str| {
            v.get(c)
                .and_then(|x| x.as_str())
                .map(|s| s.trim().to_string())
        };
        r("bridge") == bridge && r("display") == display
    });
    if !ok {
        let v = svc.device.versions();
        return Err(JobError::new(
            "versionMismatch",
            format!(
                "The keyboard reports bridge {} and display {} instead of the factory versions",
                v.get("bridge").and_then(|x| x.as_str()).unwrap_or("nothing"),
                v.get("display").and_then(|x| x.as_str()).unwrap_or("nothing")
            ),
            "The keyboard keeps its OpenFlux bridge when the restore can't finish. Reconnect it and reset to factory again.",
        ));
    }
    Ok(())
}

/// The factory UF2 of a restore job's firmware step, verified.
pub fn factory_firmware(
    svc: &UpdateService,
    job: &JobRecord,
    i: usize,
) -> Option<Result<std::path::PathBuf, JobError>> {
    let f: factory::FactoryFile =
        serde_json::from_value(job.steps[i].state.get("factoryFile")?.clone()).ok()?;
    let path = factory::cached_path(&svc.cache_dir(), &f);
    Some(
        factory::verify_file(&path, &f)
            .map(|_| path)
            .map_err(factory_error),
    )
}

/// Downloads, verifies and installs the press-refinement data. Never fails the job: a keyboard
/// without the data runs the firmware's fallback, so a failure is logged as "press refinement not
/// installed" with its reason. The outcome is kept in the step's state as `pressRefinement`
/// (`installed`, and `active` or `code` and `message`), which the job summary reports.
pub fn install_press_refinement(
    svc: &UpdateService,
    job: &mut JobRecord,
    i: usize,
) -> Result<(), JobError> {
    let result = super::press_refinement::obtain(svc, super::press_refinement::Source::Published)
        .and_then(|obtained| {
            if !wait_until(svc, svc.timings().reconnect_wait, || {
                svc.device.is_connected().1
            }) {
                return Err(super::press_refinement::Error::KeyboardDisconnected(
                    "The keyboard firmware isn't connected".into(),
                ));
            }
            super::press_refinement::install_obtained(&*svc.device, obtained)
        });
    let outcome = match result {
        Ok(done) => {
            job.log.push(format!(
                "Installed press-refinement data (CRC-32 {:08x}{})",
                done.crc32,
                if done.already_installed {
                    ", already on the keyboard"
                } else {
                    ""
                }
            ));
            serde_json::json!({"installed": true, "active": done.active, "crc32": done.crc32})
        }
        Err(e) => {
            job.log
                .push(format!("Press refinement not installed: {}", e.message()));
            serde_json::json!({"installed": false, "code": e.code(), "message": e.message()})
        }
    };
    job.steps[i].state.insert("pressRefinement".into(), outcome);
    Ok(())
}
