//! Confirming a newly installed bridge.
//!
//! Installing a bridge package arms a rollback on the keyboard: unless the host confirms the new
//! bridge, the keyboard restores the previous one. The package records each install as an
//! attempt (`rollback/bridge/attempt`), and the new bridge writes `trial-<attempt id>` once it is
//! running. The host confirms only an install it started, only once that attempt's trial file
//! exists, and only when both the trial file and the live bridge report the version it
//! installed. A bridge that merely answers proves nothing: the previous bridge keeps running
//! while the package is being unpacked.

use crate::api::state::DeviceSink;
use crate::device_paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Version string a bridge of `version` reports.
pub fn bridge_version_string(version: &str) -> String {
    format!("openflux-bridge {version}")
}

/// The bridge version a package file installs, from its name (`openflux-bridge_<version>_<arch>.ipk`).
pub fn package_target(path: &str) -> Option<String> {
    let name = std::path::Path::new(path).file_name()?.to_str()?;
    let rest = name
        .strip_prefix("openflux-bridge_")?
        .strip_suffix(".ipk")?;
    let (version, _arch) = rest.rsplit_once('_')?;
    (!version.is_empty()).then(|| bridge_version_string(version))
}

fn parse_kv(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn read_kv(
    device: &dyn DeviceSink,
    path: &str,
) -> Result<Option<BTreeMap<String, String>>, String> {
    if !device.stat_file(path)? {
        return Ok(None);
    }
    let raw = device.get_file(path)?;
    Ok(Some(parse_kv(&String::from_utf8_lossy(&raw))))
}

/// The install attempt the keyboard has recorded: `(attempt id, target version string)`.
fn read_attempt(device: &dyn DeviceSink) -> Result<Option<(String, String)>, String> {
    let Some(kv) = read_kv(device, device_paths::BRIDGE_ROLLBACK_ATTEMPT)? else {
        return Ok(None);
    };
    match (kv.get("attempt_id"), kv.get("version")) {
        (Some(id), Some(v)) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(Some((id.clone(), v.clone())))
        }
        _ => Ok(None),
    }
}

/// Where a host-started bridge install stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// The package hasn't recorded this install yet.
    AwaitingAttempt,
    /// The install is recorded; the new bridge hasn't reported in yet.
    AwaitingTrial { attempt: String },
    /// The new bridge runs the target version and was confirmed.
    Confirmed { attempt: String },
}

/// One bridge install the host started. Kept durably by its owner so a restart or reconnect
/// in the middle of the install doesn't lose it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingInstall {
    /// Version string the new bridge reports.
    pub target: String,
    /// Attempt recorded on the keyboard before the upload; the install's own attempt differs.
    pub previous_attempt: Option<String>,
    /// This install's attempt, once the keyboard has recorded it.
    pub attempt: Option<String>,
}

impl PendingInstall {
    /// Call before uploading the package.
    pub fn begin(device: &dyn DeviceSink, target: String) -> Self {
        let previous_attempt = read_attempt(device).ok().flatten().map(|(id, _)| id);
        Self {
            target,
            previous_attempt,
            attempt: None,
        }
    }

    /// Checks the install and confirms the new bridge when it is running.
    pub fn poll(&mut self, device: &dyn DeviceSink) -> Result<Progress, String> {
        if self.attempt.is_none() {
            match read_attempt(device)? {
                Some((id, version))
                    if Some(&id) != self.previous_attempt.as_ref() && version == self.target =>
                {
                    self.attempt = Some(id);
                }
                _ => return Ok(Progress::AwaitingAttempt),
            }
        }
        let attempt = self.attempt.clone().expect("set above");
        let Some(trial) = read_kv(device, &device_paths::bridge_trial_path(&attempt))? else {
            return Ok(Progress::AwaitingTrial { attempt });
        };
        if trial.get("version") != Some(&self.target) {
            return Ok(Progress::AwaitingTrial { attempt });
        }
        let live = device
            .versions()
            .get("bridge")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .map(String::from);
        if live.as_deref() != Some(self.target.as_str()) {
            return Ok(Progress::AwaitingTrial { attempt });
        }
        device.confirm_bridge_install()?;
        Ok(Progress::Confirmed { attempt })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_comes_from_the_package_name() {
        assert_eq!(
            package_target("/c/openflux-bridge_1.0.1_aarch64.ipk").as_deref(),
            Some("openflux-bridge 1.0.1")
        );
        assert_eq!(
            package_target("/c/openflux-bridge_1.1.0-beta.2_aarch64.ipk").as_deref(),
            Some("openflux-bridge 1.1.0-beta.2")
        );
        assert_eq!(
            package_target("/c/openflux-display_1.0.1_aarch64.ipk"),
            None
        );
        assert_eq!(package_target("/c/bridge.ipk"), None);
    }

    #[test]
    fn key_value_files_are_read_line_by_line() {
        let kv = parse_kv("attempt_id=ab12\nversion=openflux-bridge 1.0.1\n");
        assert_eq!(kv["attempt_id"], "ab12");
        assert_eq!(kv["version"], "openflux-bridge 1.0.1");
    }
}
