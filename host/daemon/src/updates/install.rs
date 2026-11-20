//! Installing keyboard components: bridge and display packages through the keyboard's package
//! drop folder, firmware through its boot loader. Every step reads the versions again and fails
//! with a recovery message when the keyboard doesn't report what it should.
//!
//! Steps are written so a job resumed after a restart can run a step again: a component that
//! already reports its target is not installed twice, and a started bridge install is followed
//! through its recorded attempt.

use super::jobs::{JobError, JobRecord};
use super::source::{FetchError, ObtainError};
use super::UpdateService;
use crate::bridge_install::{PendingInstall, Progress};
use openflux_update::manifest::{Artifact, Release};
use openflux_update::Component;
use std::path::PathBuf;
use std::time::Instant;

/// The file of `release` that installs its component.
pub fn artifact_for(release: &Release) -> Option<Artifact> {
    let prefix = match release.component {
        Component::Firmware => "openflux-firmware",
        Component::Bridge => "openflux-bridge",
        Component::Display => "openflux-display",
        Component::Host => return None,
    };
    let ext = if release.component == Component::Firmware {
        ".uf2"
    } else {
        ".ipk"
    };
    release
        .artifacts
        .iter()
        .find(|a| a.name.starts_with(prefix) && a.name.ends_with(ext))
        .cloned()
}

/// The keyboard-owned package that starts a factory restore, from a bridge release.
pub fn factory_restore_trigger(release: &Release) -> Option<Artifact> {
    (release.component == Component::Bridge)
        .then(|| {
            release
                .artifacts
                .iter()
                .find(|a| {
                    a.name.starts_with("openflux-factory-restore_") && a.name.ends_with(".ipk")
                })
                .cloned()
        })
        .flatten()
}

pub(crate) fn obtain_error(e: ObtainError) -> JobError {
    const RECOVERY: &str = "Nothing was installed. Check for updates and try again.";
    match e {
        ObtainError::Verification(e) => JobError::new(
            "verification",
            format!("A downloaded file failed verification: {e}"),
            RECOVERY,
        ),
        ObtainError::Fetch(FetchError::Offline(m)) => JobError::new(
            "offline",
            m,
            "Nothing was installed. Connect to the internet and try again.",
        ),
        ObtainError::Fetch(FetchError::Unavailable(m)) => JobError::new("unavailable", m, RECOVERY),
        ObtainError::Fetch(FetchError::Io(m)) => JobError::new("storage", m, RECOVERY),
    }
}

/// A verified local copy of `artifact`, checked again right before it is used.
pub(crate) fn verified_file(svc: &UpdateService, artifact: &Artifact) -> Result<PathBuf, JobError> {
    svc.sources()
        .obtain(&svc.cache_dir(), artifact)
        .map_err(obtain_error)
}

/// Downloads and verifies every file of the job before anything is written to the keyboard.
pub fn prepare(svc: &UpdateService, job: &mut JobRecord) -> Result<(), JobError> {
    for s in job.steps.clone() {
        if let Some(a) = &s.artifact {
            verified_file(svc, a)?;
            job.log.push(format!("Verified {}", a.name));
        }
    }
    crate::conversion::jobs::prepare_factory_files(svc, job)
}

/// The version string the keyboard reports for `component` now.
pub(crate) fn reported(svc: &UpdateService, component: &str) -> Option<String> {
    svc.device
        .versions()
        .get(component)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
}

fn describe(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "nothing".into())
}

/// Waits until `cond` holds, polling at the job's interval.
pub(crate) fn wait_until(
    svc: &UpdateService,
    timeout: std::time::Duration,
    mut cond: impl FnMut() -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(svc.timings().poll);
    }
}

fn step_flag(job: &JobRecord, i: usize, key: &str) -> bool {
    job.steps[i]
        .state
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn set_step(
    svc: &UpdateService,
    job: &mut JobRecord,
    i: usize,
    key: &str,
    value: serde_json::Value,
) {
    job.steps[i].state.insert(key.into(), value);
    svc.save_job(job);
}

/// Installs a bridge or display package and waits for the new version. A bridge is confirmed
/// only through its own install attempt (see [`crate::bridge_install`]).
pub fn install_package(svc: &UpdateService, job: &mut JobRecord, i: usize) -> Result<(), JobError> {
    let step = job.steps[i].clone();
    let component = step.component.clone().unwrap_or_default();
    let expect = step.expect.clone().unwrap_or_default();
    let artifact = step
        .artifact
        .clone()
        .ok_or_else(|| JobError::new("malformed", "The step has no package", ""))?;
    let path = verified_file(svc, &artifact)?;
    let t = svc.timings();
    if !wait_until(svc, t.reconnect_wait, || svc.device.is_connected().0) {
        return Err(JobError::new(
            "keyboardDisconnected",
            "The keyboard bridge isn't connected",
            "Connect the keyboard and start the update again.",
        ));
    }
    let is_bridge = component == "bridge";
    let mut pending: Option<PendingInstall> = step
        .state
        .get("pending")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    // A restart between starting a bridge install and recording the upload: the keyboard's attempt
    // record tells whether the package arrived.
    if is_bridge && !step_flag(job, i, "uploaded") {
        if let Some(p) = pending.as_mut() {
            let _ = p.poll(&*svc.device);
            if p.attempt.is_some() {
                let p = p.clone();
                set_step(
                    svc,
                    job,
                    i,
                    "pending",
                    serde_json::to_value(&p).unwrap_or_default(),
                );
                set_step(svc, job, i, "uploaded", serde_json::json!(true));
            }
        }
    }
    if !step_flag(job, i, "uploaded") {
        let now = reported(svc, &component);
        if now.as_deref() == Some(expect.as_str()) {
            job.log
                .push(format!("The {component} already reports {expect}"));
            return Ok(());
        }
        if step.from.is_some() && now != step.from {
            return Err(JobError::new(
                "versionMismatch",
                format!(
                    "The {component} reports {}, expected {}",
                    describe(&now),
                    describe(&step.from)
                ),
                "Nothing was installed. Check for updates and try again.",
            ));
        }
        if is_bridge {
            let p = PendingInstall::begin(&*svc.device, expect.clone());
            set_step(
                svc,
                job,
                i,
                "pending",
                serde_json::to_value(&p).unwrap_or_default(),
            );
            pending = Some(p);
        }
        svc.device
            .sideload_ipk(&path.to_string_lossy())
            .map_err(|e| {
                JobError::new(
                    "installFailed",
                    format!("Uploading {} failed: {e}", artifact.name),
                    "Nothing was installed. Reconnect the keyboard and try again.",
                )
            })?;
        set_step(svc, job, i, "uploaded", serde_json::json!(true));
        job.log.push(format!("Uploaded {}", artifact.name));
    }
    let done = if is_bridge {
        let mut p = pending.unwrap_or_else(|| PendingInstall {
            target: expect.clone(),
            previous_attempt: None,
            attempt: None,
        });
        let mut learned = p.attempt.is_some();
        let ok = wait_until(svc, t.install_wait, || {
            if !svc.device.is_connected().0 {
                return false;
            }
            let r = p.poll(&*svc.device);
            if !learned && p.attempt.is_some() {
                learned = true;
                job.steps[i].state.insert(
                    "pending".into(),
                    serde_json::to_value(&p).unwrap_or_default(),
                );
                svc.save_job(job);
            }
            matches!(r, Ok(Progress::Confirmed { .. }))
        });
        if ok {
            job.log.push(format!("Confirmed the new bridge ({expect})"));
        }
        ok
    } else {
        wait_until(svc, t.install_wait, || {
            reported(svc, &component).as_deref() == Some(expect.as_str())
        })
    };
    if !done {
        let now = reported(svc, &component);
        let recovery = if is_bridge {
            "The keyboard restores its previous bridge by itself when a new one isn't confirmed. Leave it connected for a few minutes, then check for updates and try again."
        } else {
            "Reconnect the keyboard, check for updates and install again."
        };
        return Err(JobError::new(
            "versionMismatch",
            format!(
                "The {component} didn't come back as {expect}; it reports {}",
                describe(&now)
            ),
            recovery,
        ));
    }
    Ok(())
}

/// Flashes firmware through the boot loader. The flasher never writes the calibration region and
/// checks its hash before and after; the image is read back after writing.
pub fn flash_firmware(svc: &UpdateService, job: &mut JobRecord, i: usize) -> Result<(), JobError> {
    let step = job.steps[i].clone();
    let expect = step.expect.clone().unwrap_or_default();
    let path = match crate::conversion::jobs::factory_firmware(svc, job, i) {
        Some(p) => p?,
        None => {
            let artifact = step
                .artifact
                .clone()
                .ok_or_else(|| JobError::new("malformed", "The step has no firmware", ""))?;
            verified_file(svc, &artifact)?
        }
    };
    flash_verified_uf2(svc, job, i, &path, &expect, &step.from)
}

/// Flashes a verified UF2 file and waits for the firmware to report `expect`.
pub(crate) fn flash_verified_uf2(
    svc: &UpdateService,
    job: &mut JobRecord,
    i: usize,
    path: &std::path::Path,
    expect: &str,
    from: &Option<String>,
) -> Result<(), JobError> {
    let t = svc.timings();
    if !wait_until(svc, t.reconnect_wait, || svc.device.is_connected().1) {
        let (code, recovery) = if step_flag(job, i, "flashing") {
            (
                "interrupted",
                "The firmware was being written when OpenFlux stopped and doesn't answer. Keep the keyboard connected. If it doesn't come back, copy the firmware .uf2 file from the OpenFlux release onto the RPI-RP2 drive that appears, then check for updates.",
            )
        } else {
            (
                "keyboardDisconnected",
                "Connect the keyboard and start again.",
            )
        };
        return Err(JobError::new(
            code,
            "The keyboard firmware isn't connected",
            recovery,
        ));
    }
    let now = reported(svc, "firmware");
    if now.as_deref() == Some(expect) {
        job.log
            .push(format!("The firmware already reports {expect}"));
        return Ok(());
    }
    if !step_flag(job, i, "flashing") && from.is_some() && &now != from {
        return Err(JobError::new(
            "versionMismatch",
            format!(
                "The firmware reports {}, expected {}",
                describe(&now),
                describe(from)
            ),
            "Nothing was installed. Check for updates and try again.",
        ));
    }
    set_step(svc, job, i, "flashing", serde_json::json!(true));
    match svc.device.flash_uf2(&path.to_string_lossy()) {
        Ok(msg) => job.log.push(msg),
        Err(e) => {
            let calibration = e.contains(crate::usb::picoboot::CALIBRATION_CHANGED);
            return Err(if calibration {
                JobError::new(
                    "calibrationChanged",
                    format!("The keyboard's calibration region changed while flashing: {e}"),
                    concat!(
                        "Stop using the keyboard's analog features and open an issue at ",
                        env!("CARGO_PKG_REPOSITORY"),
                        "/issues with this message; the calibration may need to be redone."
                    ),
                )
            } else {
                JobError::new(
                    "installFailed",
                    format!("Flashing the firmware failed: {e}"),
                    "Keep the keyboard connected and install the firmware again. If the keyboard doesn't come back, copy the firmware .uf2 file from the OpenFlux release onto the RPI-RP2 drive that appears, then check for updates.",
                )
            });
        }
    }
    if !wait_until(svc, t.install_wait, || {
        reported(svc, "firmware").as_deref() == Some(expect)
    }) {
        return Err(JobError::new(
            "versionMismatch",
            format!(
                "The firmware didn't come back as {expect}; it reports {}",
                describe(&reported(svc, "firmware"))
            ),
            "Reconnect the keyboard and install the firmware again.",
        ));
    }
    Ok(())
}

/// Checks that every component reports what the job expects.
pub fn verify(svc: &UpdateService, job: &mut JobRecord, i: usize) -> Result<(), JobError> {
    let expect = job.steps[i]
        .state
        .get("expect")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let ok = wait_until(svc, svc.timings().reconnect_wait, || {
        let v = svc.device.versions();
        expect
            .iter()
            .all(|(c, want)| v.get(c).and_then(|x| x.as_str()).map(str::trim) == want.as_str())
    });
    if !ok {
        let v = svc.device.versions();
        let wrong: Vec<String> = expect
            .iter()
            .filter(|(c, want)| {
                v.get(c.as_str()).and_then(|x| x.as_str()).map(str::trim) != want.as_str()
            })
            .map(|(c, want)| {
                format!(
                    "{c} reports {}, expected {}",
                    v.get(c.as_str())
                        .and_then(|x| x.as_str())
                        .unwrap_or("nothing"),
                    want.as_str().unwrap_or("nothing")
                )
            })
            .collect();
        return Err(JobError::new(
            "versionMismatch",
            wrong.join("; "),
            "Reconnect the keyboard and check for updates; the update can be installed again.",
        ));
    }
    Ok(())
}
