//! Scanning and validating unsigned local developer builds (`firmware`, `bridge`, `display`, and
//! optional host installers) from a user-chosen folder.

use crate::updates::jobs::now_rfc3339;
use openflux_update::Component;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One discovered local build artifact for a keyboard component.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalArtifact {
    pub component: &'static str,
    pub name: String,
    pub path: String,
    pub version: Option<String>,
    pub size_bytes: u64,
    pub modified_at: String,
    #[serde(skip)]
    pub modified_secs: u64,
    #[serde(skip)]
    pub path_buf: PathBuf,
}

/// One discovered local desktop installer (informational; users run it directly).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalHostInstaller {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub modified_at: String,
    #[serde(skip)]
    pub modified_secs: u64,
}

/// Result of scanning a developer's local build folder.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalScan {
    pub dir: String,
    pub exists: bool,
    pub error: Option<String>,
    pub firmware: Option<LocalArtifact>,
    pub bridge: Option<LocalArtifact>,
    pub display: Option<LocalArtifact>,
    pub host: Option<LocalHostInstaller>,
}

/// Subdirectories checked relative to the chosen folder (bounded, shallow scan).
const SEARCH_SUBDIRS: &[&[&str]] = &[
    &[],
    &["build"],
    &["build", "firmware"],
    &["build", "bridge"],
    &["build", "display"],
    &["keyboard", "firmware", "build"],
    &["keyboard", "bridge", "build"],
    &["keyboard", "display", "build"],
    &["target", "release", "bundle", "nsis"],
    &["target", "release", "bundle", "dmg"],
    &["target", "release", "bundle", "macos"],
    &["target", "release", "bundle", "deb"],
    &["target", "release", "bundle", "appimage"],
];

/// Scans `dir` (and common repo build subdirectories) for the newest local artifact of each
/// component.
pub fn scan_dir(dir: &Path) -> LocalScan {
    let dir_str = dir.to_string_lossy().to_string();
    if !dir.is_dir() {
        return LocalScan {
            dir: dir_str,
            exists: false,
            error: Some("Folder doesn't exist or isn't a directory".into()),
            firmware: None,
            bridge: None,
            display: None,
            host: None,
        };
    }

    let mut firmware: Option<LocalArtifact> = None;
    let mut bridge: Option<LocalArtifact> = None;
    let mut display: Option<LocalArtifact> = None;
    let mut host: Option<LocalHostInstaller> = None;

    for rel in SEARCH_SUBDIRS {
        let mut sub = dir.to_path_buf();
        for seg in *rel {
            sub.push(seg);
        }
        let Ok(entries) = std::fs::read_dir(&sub) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() || meta.len() == 0 {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
            let mtime_secs = mtime
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let modified_at = format_system_time(mtime);

            if lower.ends_with(".uf2") {
                let cand = LocalArtifact {
                    component: Component::Firmware.as_str(),
                    version: infer_firmware_version(&name),
                    name,
                    path: path.to_string_lossy().to_string(),
                    size_bytes: meta.len(),
                    modified_at,
                    modified_secs: mtime_secs,
                    path_buf: path,
                };
                if should_replace_artifact(firmware.as_ref(), &cand) {
                    firmware = Some(cand);
                }
            } else if lower.ends_with(".ipk") && !lower.contains("factory-restore") {
                let comp = if lower.starts_with("openflux-bridge") {
                    Some(Component::Bridge)
                } else if lower.starts_with("openflux-display") {
                    Some(Component::Display)
                } else {
                    None
                };
                if let Some(c) = comp {
                    let cand = LocalArtifact {
                        component: c.as_str(),
                        version: infer_ipk_version(&name, c),
                        name,
                        path: path.to_string_lossy().to_string(),
                        size_bytes: meta.len(),
                        modified_at,
                        modified_secs: mtime_secs,
                        path_buf: path,
                    };
                    let slot = match c {
                        Component::Bridge => &mut bridge,
                        Component::Display => &mut display,
                        _ => continue,
                    };
                    if should_replace_artifact(slot.as_ref(), &cand) {
                        *slot = Some(cand);
                    }
                }
            } else if is_host_installer_for_platform(&lower) {
                let cand = LocalHostInstaller {
                    name,
                    path: path.to_string_lossy().to_string(),
                    size_bytes: meta.len(),
                    modified_at,
                    modified_secs: mtime_secs,
                };
                if host
                    .as_ref()
                    .is_none_or(|cur| cand.modified_secs > cur.modified_secs)
                {
                    host = Some(cand);
                }
            }
        }
    }

    LocalScan {
        dir: dir_str,
        exists: true,
        error: None,
        firmware,
        bridge,
        display,
        host,
    }
}

impl LocalScan {
    pub fn artifact_for(&self, c: Component) -> Option<&LocalArtifact> {
        match c {
            Component::Firmware => self.firmware.as_ref(),
            Component::Bridge => self.bridge.as_ref(),
            Component::Display => self.display.as_ref(),
            Component::Host => None,
        }
    }
}

fn should_replace_artifact(current: Option<&LocalArtifact>, cand: &LocalArtifact) -> bool {
    let Some(cur) = current else {
        return true;
    };
    if cand.component == Component::Firmware.as_str() {
        let cur_fw = cur.name.to_ascii_lowercase().contains("firmware");
        let cand_fw = cand.name.to_ascii_lowercase().contains("firmware");
        if cand_fw != cur_fw {
            return cand_fw;
        }
    }
    cand.modified_secs > cur.modified_secs
}

fn is_host_installer_for_platform(lower_name: &str) -> bool {
    if !lower_name.contains("openflux") {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        lower_name.ends_with("-setup.exe") || lower_name.ends_with(".msi")
    }
    #[cfg(target_os = "macos")]
    {
        lower_name.ends_with(".dmg")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        lower_name.ends_with(".appimage") || lower_name.ends_with(".deb")
    }
}

/// Parses `<version>` from `openflux-bridge_<version>_<arch>.ipk` or `openflux-display_<version>_<arch>.ipk`.
pub fn infer_ipk_version(filename: &str, component: Component) -> Option<String> {
    let prefix = format!("openflux-{}_", component.as_str());
    let rest = filename.strip_prefix(&prefix)?.strip_suffix(".ipk")?;
    let (ver, _arch) = rest.rsplit_once('_')?;
    (!ver.is_empty()).then(|| ver.to_string())
}

/// Parses `<version>` from `openflux-firmware-<version>.uf2` when present.
pub fn infer_firmware_version(filename: &str) -> Option<String> {
    let rest = filename
        .strip_prefix("openflux-firmware-")
        .or_else(|| filename.strip_prefix("openflux_firmware_"))?
        .strip_suffix(".uf2")?;
    (!rest.is_empty() && rest.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .then(|| rest.to_string())
}

/// Validates that `path` sits inside `root_dir` and is a structurally valid artifact for `component`.
pub fn validate_local_artifact(
    component: Component,
    path: &Path,
    root_dir: &Path,
) -> Result<(), String> {
    let canon_root = std::fs::canonicalize(root_dir)
        .map_err(|e| format!("Local build folder isn't readable: {e}"))?;
    let canon_file = std::fs::canonicalize(path)
        .map_err(|e| format!("Local build file {} isn't readable: {e}", path.display()))?;
    if !canon_file.starts_with(&canon_root) {
        return Err("Local build file must be inside the configured local build folder".into());
    }
    match component {
        Component::Firmware => {
            let bytes = std::fs::read(&canon_file)
                .map_err(|e| format!("Couldn't read {}: {e}", canon_file.display()))?;
            crate::usb::picoboot::parse_uf2_image(&bytes)?;
            Ok(())
        }
        Component::Bridge | Component::Display => {
            let name = canon_file
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let expected_prefix = format!("openflux-{}", component.as_str());
            if !name.starts_with(&expected_prefix)
                || !name.ends_with(".ipk")
                || name.contains("factory-restore")
            {
                return Err(format!(
                    "Expected an {expected_prefix}*.ipk package, got {name}"
                ));
            }
            let mut f = std::fs::File::open(&canon_file)
                .map_err(|e| format!("Couldn't open {}: {e}", canon_file.display()))?;
            let mut magic = [0u8; 8];
            use std::io::Read;
            let n = f
                .read(&mut magic)
                .map_err(|e| format!("Couldn't read {}: {e}", canon_file.display()))?;
            let is_ar = n == 8 && &magic == b"!<arch>\n";
            let is_gzip = n >= 2 && magic[0] == 0x1f && magic[1] == 0x8b;
            if !is_ar && !is_gzip {
                return Err(format!(
                    "{name} doesn't look like a valid .ipk archive (expected ar or gzip header)"
                ));
            }
            Ok(())
        }
        Component::Host => Err("Host installers aren't flashed to the keyboard".into()),
    }
}

fn format_system_time(t: SystemTime) -> String {
    let Ok(dur) = t.duration_since(UNIX_EPOCH) else {
        return now_rfc3339();
    };
    let secs = dur.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_repo_subdirs_and_infers_versions_excluding_factory_restore() {
        let dir = std::env::temp_dir().join(format!("openflux_local_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("build/firmware")).unwrap();
        std::fs::create_dir_all(dir.join("build/bridge")).unwrap();
        std::fs::create_dir_all(dir.join("build/display")).unwrap();

        std::fs::write(dir.join("build/firmware/openflux_firmware.uf2"), b"UF2\n").unwrap();
        std::fs::write(
            dir.join("build/bridge/openflux-bridge_1.0.2-dev_aarch64.ipk"),
            b"!<arch>\nfake",
        )
        .unwrap();
        std::fs::write(
            dir.join("build/bridge/openflux-factory-restore_1.0.2_aarch64.ipk"),
            b"!<arch>\nfake",
        )
        .unwrap();
        std::fs::write(
            dir.join("build/display/openflux-display_1.0.2-dev_aarch64.ipk"),
            b"\x1f\x8bfake",
        )
        .unwrap();

        let scan = scan_dir(&dir);
        assert!(scan.exists);
        assert!(scan.error.is_none());
        assert_eq!(
            scan.firmware.as_ref().unwrap().name,
            "openflux_firmware.uf2"
        );
        assert_eq!(scan.firmware.as_ref().unwrap().version, None);
        assert_eq!(
            scan.bridge.as_ref().unwrap().name,
            "openflux-bridge_1.0.2-dev_aarch64.ipk"
        );
        assert_eq!(
            scan.bridge.as_ref().unwrap().version.as_deref(),
            Some("1.0.2-dev")
        );
        assert_eq!(
            scan.display.as_ref().unwrap().version.as_deref(),
            Some("1.0.2-dev")
        );

        assert!(validate_local_artifact(
            Component::Bridge,
            &scan.bridge.as_ref().unwrap().path_buf,
            &dir
        )
        .is_ok());
        assert!(validate_local_artifact(
            Component::Display,
            &scan.display.as_ref().unwrap().path_buf,
            &dir
        )
        .is_ok());
        // Rejects corrupt UF2
        assert!(validate_local_artifact(
            Component::Firmware,
            &scan.firmware.as_ref().unwrap().path_buf,
            &dir
        )
        .is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
