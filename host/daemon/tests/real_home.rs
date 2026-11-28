//! A daemon given its own data directory leaves the user's real locations alone.

use keyglassd::api::third_party;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

fn snapshot(root: &Path, out: &mut BTreeMap<PathBuf, Option<(SystemTime, u64)>>) {
    let meta = std::fs::symlink_metadata(root).ok();
    out.insert(
        root.to_path_buf(),
        meta.as_ref()
            .map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len())),
    );
    if meta.is_some_and(|m| m.is_dir()) {
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                snapshot(&e.path(), out);
            }
        }
    }
}

/// Kills and reaps the daemon if the test ends before it has exited.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Where the daemon could write in the user's home: everywhere it publishes discovery files, and
/// the data directories of the factory software around them.
fn user_locations() -> Vec<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let mut out = Vec::new();
    if let Some(home) = var("HOME").or_else(|| var("USERPROFILE")) {
        for rel in [
            "Documents/flux",
            "flux",
            "Library/Application Support/com.flux.polymath",
            ".local/share/flux",
            ".local/share/com.fluxkeyboard.polymath",
            "AppData/Roaming/com.fluxkeyboard",
            "AppData/Local/flux",
        ] {
            out.push(home.join(rel));
        }
    }
    if let Some(x) = var("XDG_DATA_HOME") {
        out.push(x.join("flux"));
        out.push(x.join("com.fluxkeyboard.polymath"));
    }
    if let Some(a) = var("APPDATA") {
        out.push(a.join("com.fluxkeyboard"));
    }
    if let Some(l) = var("LOCALAPPDATA") {
        out.push(l.join("flux"));
    }
    let env = third_party::DiscoveryEnv::from_process();
    let locations =
        third_party::discovery_locations(third_party::DiscoveryPlatform::current(), &env);
    out.extend(
        locations
            .keys_files
            .iter()
            .filter_map(|f| f.parent().map(Path::to_path_buf)),
    );
    out.extend(locations.cert_dirs);
    out.extend(third_party::shared_token_store(&env));
    out.sort();
    out.dedup();
    out
}

#[test]
fn a_daemon_with_its_own_data_directory_writes_nothing_in_the_users_home() {
    let bin = env!("CARGO_BIN_EXE_keyglassd");
    let tmp = std::env::temp_dir().join(format!("keyglassd_real_home_{}", std::process::id()));
    let data = tmp.join("data");
    let mut before = BTreeMap::new();
    for p in user_locations() {
        snapshot(&p, &mut before);
    }

    let command = |args: &[&str]| {
        let mut c = Command::new(bin);
        c.args(args)
            .env("KEYGLASS_DATA_DIR", &data)
            .env_remove("KEYGLASS_HOME_OVERRIDE")
            .env_remove("KEYGLASS_ENROLLMENT_SECRET")
            .env("KEYGLASS_API_ADDR", "127.0.0.1:0")
            .env(
                "KEYGLASS_UPDATE_INDEX_URL",
                format!("file://{}/no-index.json", tmp.display()),
            );
        c
    };
    let mut daemon = KillOnDrop(
        command(&["--no-tray", "--no-usb"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Some(ep)) = keyglassd::api::lifecycle_client::DaemonEndpoint::discover(&data) {
            if std::net::TcpStream::connect(ep.addr).is_ok() {
                break;
            }
        }
        assert!(Instant::now() < deadline, "the daemon didn't start");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Give startup work (discovery, adopting earlier state) time to run.
    std::thread::sleep(Duration::from_millis(500));
    let out = command(&["--request-stop"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = daemon.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the daemon didn't exit");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0));
    let mut err = String::new();
    std::io::Read::read_to_string(&mut daemon.0.stderr.take().unwrap(), &mut err).unwrap();
    assert!(err.contains("third-party discovery is off"), "{err}");

    let mut after = BTreeMap::new();
    for p in user_locations() {
        snapshot(&p, &mut after);
    }
    assert_eq!(before, after, "the user's locations are unchanged");
    let _ = std::fs::remove_dir_all(&tmp);
}
