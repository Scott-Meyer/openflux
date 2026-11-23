//! Updates of Keyglass itself (Studio, the daemon and helpers ship as one bundle), through the
//! Tauri updater.
//!
//! The commands are single steps: check, download (which verifies the signature), install and
//! relaunch. The order, and stopping the daemon before the bundle is replaced, belong to the
//! interface's host update flow, which tells the user what it is waiting for.

use crate::daemon_api::CommandError;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Feed used when the configuration names none.
pub const DEFAULT_FEED: &str = "https://keyglass-dev.github.io/keyglass/updates/stable/latest.json";

#[derive(Default)]
pub struct HostUpdateState {
    found: Mutex<Option<Update>>,
    downloaded: Mutex<Option<(String, Vec<u8>)>>,
}

/// The feed for `channel`. Feeds are published per channel as `updates/<channel>/latest.json`
/// next to each other, so the beta feed is derived from the configured stable one.
pub fn feed_url(configured: &str, channel: &str) -> String {
    let channel = if channel == "beta" { "beta" } else { "stable" };
    match configured.rfind("/updates/stable/") {
        Some(i) => format!(
            "{}/updates/{channel}/{}",
            &configured[..i],
            &configured[i + "/updates/stable/".len()..]
        ),
        None => configured.to_string(),
    }
}

fn configured_feed<R: Runtime>(app: &AppHandle<R>) -> String {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("endpoints"))
        .and_then(|e| e.get(0))
        .and_then(|e| e.as_str())
        .unwrap_or(DEFAULT_FEED)
        .to_string()
}

/// How this installation receives host updates.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Support {
    /// `inApp`: Studio downloads and installs updates. `notify`: Studio links to the release.
    pub mode: &'static str,
    pub reason: Option<String>,
    /// Whether installing replaces the running process (the Windows setup program does).
    pub installer_exits_app: bool,
}

pub fn support_for(os: &str, appimage: bool) -> Support {
    match os {
        "linux" if !appimage => Support {
            mode: "notify",
            reason: Some(
                "Keyglass was installed with the system package manager. Install the updated package from the release page."
                    .into(),
            ),
            installer_exits_app: false,
        },
        "macos" | "windows" | "linux" => Support {
            mode: "inApp",
            reason: None,
            installer_exits_app: os == "windows",
        },
        _ => Support {
            mode: "notify",
            reason: Some("Automatic updates aren't available on this platform.".into()),
            installer_exits_app: false,
        },
    }
}

fn current_support() -> Support {
    support_for(
        std::env::consts::OS,
        std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty()),
    )
}

fn err(e: impl std::fmt::Display) -> CommandError {
    CommandError::local(e.to_string())
}

#[tauri::command]
pub fn host_update_support() -> Support {
    current_support()
}

#[tauri::command]
pub async fn host_update_check<R: Runtime>(
    app: AppHandle<R>,
    st: State<'_, HostUpdateState>,
    channel: String,
) -> Result<Value, CommandError> {
    let url = feed_url(&configured_feed(&app), &channel);
    let url = url.parse().map_err(err)?;
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .map_err(err)?
        .build()
        .map_err(err)?;
    let current = app.package_info().version.to_string();
    let found = updater.check().await.map_err(err)?;
    let reply = match &found {
        Some(u) => json!({
            "available": true,
            "currentVersion": current,
            "version": u.version,
            "notes": u.body,
            "date": u.date.map(|d| d.to_string()),
        }),
        None => json!({ "available": false, "currentVersion": current }),
    };
    *st.found.lock().unwrap() = found;
    *st.downloaded.lock().unwrap() = None;
    Ok(reply)
}

/// Downloads the update found by the last check and verifies its signature. Progress is sent
/// as `{"downloaded": bytes, "total": bytes | null}`.
#[tauri::command]
pub async fn host_update_download(
    st: State<'_, HostUpdateState>,
    on_progress: Channel<Value>,
) -> Result<Value, CommandError> {
    let update = st
        .found
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| CommandError::local("Check for updates first"))?;
    let mut downloaded: u64 = 0;
    let mut last_sent: u64 = 0;
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                if downloaded - last_sent >= 256 * 1024 || Some(downloaded) == total {
                    last_sent = downloaded;
                    let _ = on_progress.send(json!({ "downloaded": downloaded, "total": total }));
                }
            },
            || {},
        )
        .await
        .map_err(err)?;
    let size = bytes.len();
    *st.downloaded.lock().unwrap() = Some((update.version.clone(), bytes));
    Ok(json!({ "version": update.version, "bytes": size }))
}

/// Installs the downloaded update. On Windows the setup program takes over and Studio exits.
#[tauri::command]
pub async fn host_update_install(st: State<'_, HostUpdateState>) -> Result<(), CommandError> {
    let update = st
        .found
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| CommandError::local("Check for updates first"))?;
    let (version, bytes) = st
        .downloaded
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| CommandError::local("Download the update first"))?;
    if version != update.version {
        return Err(CommandError::local(
            "The downloaded update doesn't match the last check",
        ));
    }
    tokio::task::spawn_blocking(move || update.install(bytes))
        .await
        .map_err(err)?
        .map_err(err)
}

/// Forgets a downloaded update that won't be installed.
#[tauri::command]
pub fn host_update_discard(st: State<'_, HostUpdateState>) {
    *st.downloaded.lock().unwrap() = None;
}

#[tauri::command]
pub fn relaunch_studio<R: Runtime>(app: AppHandle<R>) {
    if let Some(st) = app.try_state::<crate::DesktopState>() {
        st.closing.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The release overlay (`host/packaging/scripts/tauri-release-config.sh`) sets
    /// `plugins.updater.requireSignedVersion`, and `.github/scripts/test-tauri-release-config.sh`
    /// checks that it does. This shows the updater plugin reads that key: it ignores unknown keys,
    /// so a misspelled or renamed one would leave the check off without an error.
    #[test]
    fn the_updater_reads_require_signed_version() {
        let with: tauri_plugin_updater::Config = serde_json::from_value(json!({
            "pubkey": "k",
            "endpoints": ["https://example.invalid/updates/stable/latest.json"],
            "requireSignedVersion": true
        }))
        .unwrap();
        assert!(with.require_signed_version);
        let without: tauri_plugin_updater::Config =
            serde_json::from_value(json!({ "pubkey": "k", "endpoints": [] })).unwrap();
        assert!(!without.require_signed_version);
    }

    #[test]
    fn the_beta_feed_sits_next_to_the_configured_stable_feed() {
        assert_eq!(
            feed_url(DEFAULT_FEED, "beta"),
            "https://keyglass-dev.github.io/keyglass/updates/beta/latest.json"
        );
        assert_eq!(feed_url(DEFAULT_FEED, "stable"), DEFAULT_FEED);
        assert_eq!(
            feed_url(
                "https://updates.example.org/updates/stable/latest.json",
                "beta"
            ),
            "https://updates.example.org/updates/beta/latest.json"
        );
        // Unknown channels use stable; feeds without the channel layout are used as they are.
        assert_eq!(feed_url(DEFAULT_FEED, "nightly"), DEFAULT_FEED);
        assert_eq!(
            feed_url("https://x.test/latest.json", "beta"),
            "https://x.test/latest.json"
        );
    }

    #[test]
    fn the_base_configuration_names_the_stable_feed_and_parses_as_updater_config() {
        let conf: Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let updater = conf["plugins"]["updater"].clone();
        let parsed: tauri_plugin_updater::Config =
            serde_json::from_value(updater).expect("updater configuration");
        assert_eq!(parsed.endpoints.len(), 1);
        assert_eq!(parsed.endpoints[0].as_str(), DEFAULT_FEED);
        assert!(!parsed.dangerous_insecure_transport_protocol);
    }

    #[test]
    fn packages_from_the_system_package_manager_are_announced_not_installed() {
        assert_eq!(support_for("linux", false).mode, "notify");
        assert_eq!(support_for("linux", true).mode, "inApp");
        assert_eq!(support_for("macos", false).mode, "inApp");
        assert!(support_for("windows", false).installer_exits_app);
        assert!(!support_for("macos", false).installer_exits_app);
        assert_eq!(support_for("freebsd", false).mode, "notify");
    }
}
