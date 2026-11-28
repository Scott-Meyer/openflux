//! `keyglassd --request-restart` / `--request-stop` and the busy lease that defers them.

use keyglassd::api::build_router;
use keyglassd::api::state::{ApiState, NoopDeviceSink};
use keyglassd::lifecycle::ExitKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "keyglassd_lifecycle_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn post(addr: std::net::SocketAddr, path: &str, body: &str) -> (u16, serde_json::Value) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).await.unwrap();
    let txt = String::from_utf8_lossy(&resp).to_string();
    let (head, body) = txt.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (
        status,
        serde_json::from_str(body).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn a_request_during_keyboard_work_is_deferred_until_the_work_ends() {
    let tmp = temp_dir("deferred");
    let state = ApiState::with_dirs(
        Arc::new(NoopDeviceSink),
        None,
        Duration::from_secs(20),
        tmp.join("icons"),
    );
    state.set_registration_secret("lifecycle_secret");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let (code, _) = post(addr, "/v1/daemon/lifecycle", r#"{"action":"restart"}"#).await;
    assert_eq!(
        code, 401,
        "the request needs the enrollment secret or a first-party token"
    );
    let (code, body) = post(
        addr,
        "/v1/daemon/lifecycle",
        r#"{"action":"status","enrollmentSecret":"lifecycle_secret"}"#,
    )
    .await;
    assert_eq!(
        (code, body["outcome"].clone()),
        (200, serde_json::json!("running"))
    );

    let lease = state
        .lifecycle
        .hold("flashing the keyboard firmware")
        .unwrap();
    let (code, body) = post(
        addr,
        "/v1/daemon/lifecycle",
        r#"{"action":"restart","enrollmentSecret":"lifecycle_secret"}"#,
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(body["outcome"], "deferred");
    assert_eq!(body["exit"], "restart");
    assert_eq!(
        body["busy"],
        serde_json::json!(["flashing the keyboard firmware"])
    );
    assert_eq!(
        state.lifecycle.exit_due(),
        None,
        "the daemon keeps running while the work runs"
    );
    assert!(
        state.lifecycle.hold("saving to the keyboard").is_err(),
        "no new keyboard work starts once an exit is pending"
    );

    drop(lease);
    assert_eq!(
        state.lifecycle.wait_exit_due(Duration::from_secs(1)),
        Some(ExitKind::Restart)
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

struct Daemon {
    child: std::process::Child,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn daemon_command(bin: &str, tmp: &Path) -> Command {
    let mut cmd = Command::new(bin);
    cmd.env("KEYGLASS_DATA_DIR", tmp.join("data"))
        .env("KEYGLASS_HOME_OVERRIDE", tmp.join("home"))
        .env(
            "KEYGLASS_UPDATE_INDEX_URL",
            format!("file://{}/no-index.json", tmp.display()),
        )
        .env("HOME", tmp.join("home"))
        .env("USERPROFILE", tmp.join("home"))
        .env("APPDATA", tmp.join("home/AppData/Roaming"))
        .env("LOCALAPPDATA", tmp.join("home/AppData/Local"))
        .env_remove("XDG_DATA_HOME")
        .env_remove("KEYGLASS_ENROLLMENT_SECRET")
        .env("KEYGLASS_API_ADDR", "127.0.0.1:0");
    cmd
}

/// Starts the daemon and waits until its local API answers.
fn start_daemon(bin: &str, tmp: &Path) -> Daemon {
    let keys = tmp.join("data/config/keys.json");
    let _ = std::fs::remove_file(&keys);
    let daemon = Daemon {
        child: daemon_command(bin, tmp)
            .args(["--no-tray", "--no-usb"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(ep)) =
            keyglassd::api::lifecycle_client::DaemonEndpoint::discover(&tmp.join("data"))
        {
            if std::net::TcpStream::connect(ep.addr).is_ok() {
                return daemon;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("daemon did not start");
}

fn request(bin: &str, tmp: &Path, flag: &str) -> (i32, String) {
    let out = daemon_command(bin, tmp).arg(flag).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

fn wait_exit(d: &mut Daemon) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(status) = d.child.try_wait().unwrap() {
            return status.code().unwrap_or(-1);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("daemon did not exit");
}

#[test]
fn an_idle_daemon_exits_at_once_and_reports_how() {
    let bin = env!("CARGO_BIN_EXE_keyglassd");
    let tmp = temp_dir("binary");

    assert_eq!(
        request(bin, &tmp, "--request-stop"),
        (4, "not-running".into())
    );

    let mut d = start_daemon(bin, &tmp);
    assert_eq!(
        request(bin, &tmp, "--request-restart"),
        (0, "restarted".into())
    );
    assert_eq!(
        wait_exit(&mut d),
        ExitKind::Restart.exit_code(),
        "a restart exits non-zero so the service manager starts it again"
    );
    assert_eq!(ExitKind::Restart.exit_code(), 75);

    // The discovery file is left behind; the daemon behind it is gone.
    assert_eq!(
        request(bin, &tmp, "--request-restart"),
        (4, "not-running".into())
    );

    let mut d = start_daemon(bin, &tmp);
    assert_eq!(request(bin, &tmp, "--request-stop"), (0, "stopped".into()));
    assert_eq!(
        wait_exit(&mut d),
        0,
        "a stop exits 0 so the service manager leaves it stopped"
    );

    // Third-party discovery says the API is off while the daemon is stopped.
    let platform_keys = keyglassd::api::third_party::discovery_locations(
        keyglassd::api::third_party::DiscoveryPlatform::current(),
        &keyglassd::api::third_party::DiscoveryEnv {
            home: Some(tmp.join("home")),
            xdg_data_home: None,
            appdata: None,
            local_appdata: None,
        },
    )
    .keys_files;
    for k in platform_keys {
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&k).unwrap()).unwrap();
        assert_eq!(doc["apiActive"], false, "{}", k.display());
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------------------------
// "not running" is reported only on positive evidence
// ---------------------------------------------------------------------------------------------

fn plaintext_discovery(data: &Path, addr: std::net::SocketAddr) {
    std::fs::create_dir_all(data.join("config")).unwrap();
    std::fs::write(
        data.join("config/keys.json"),
        serde_json::json!({"openPort": addr.port(), "host": addr.ip().to_string(), "scheme": "http"}).to_string(),
    )
    .unwrap();
    std::fs::write(data.join("config/enrollment.secret"), "client_test_secret").unwrap();
}

fn refused_addr() -> std::net::SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    addr
}

fn stop_with(data: &Path) -> keyglassd::api::lifecycle_client::RequestOutcome {
    stop_with_connect_timeout(data, Duration::from_millis(300))
}

fn stop_with_connect_timeout(
    data: &Path,
    timeout: Duration,
) -> keyglassd::api::lifecycle_client::RequestOutcome {
    keyglassd::api::lifecycle_client::request_exit_with_timeout(
        data,
        ExitKind::Stop,
        Duration::from_secs(1),
        timeout,
    )
}

#[test]
fn a_missing_discovery_file_means_not_running() {
    let tmp = temp_dir("client_missing");
    assert_eq!(stop_with(&tmp.join("data")).exit_code(), 4);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn an_unreadable_discovery_file_is_an_error() {
    let tmp = temp_dir("client_unreadable");
    let data = tmp.join("data");
    // A directory where the file should be can't be read as one, on every platform.
    std::fs::create_dir_all(data.join("config/keys.json")).unwrap();
    let outcome = stop_with(&data);
    assert_eq!(outcome.exit_code(), 1, "{outcome:?}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn a_daemon_that_does_not_answer_in_time_is_an_error() {
    let tmp = temp_dir("client_timeout");
    let data = tmp.join("data");
    // Accepts the connection and never answers.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    plaintext_discovery(&data, silent.local_addr().unwrap());
    let outcome = stop_with(&data);
    assert_eq!(outcome.exit_code(), 1, "{outcome:?}");

    // An address that can't be reached at all (TEST-NET-1) times out or fails; never "not running".
    plaintext_discovery(&data, "192.0.2.1:9".parse().unwrap());
    let outcome = stop_with(&data);
    assert_eq!(outcome.exit_code(), 1, "{outcome:?}");
    drop(silent);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn a_refused_connection_means_not_running_only_without_a_daemon_process() {
    let tmp = temp_dir("client_refused");
    let data = tmp.join("data");
    plaintext_discovery(&data, refused_addr());
    // Windows retries loopback SYNs for ~2s before returning ConnectionRefused.
    let refused_timeout = Duration::from_secs(4);
    assert_eq!(
        stop_with_connect_timeout(&data, refused_timeout).exit_code(),
        4,
        "nothing listens and nothing holds the instance lock"
    );

    let lock = keyglassd::lifecycle::InstanceLock::acquire(&data)
        .unwrap()
        .expect("lock is free");
    let outcome = stop_with_connect_timeout(&data, refused_timeout);
    assert_eq!(
        outcome.exit_code(),
        1,
        "a process holds the instance lock: {outcome:?}"
    );
    assert!(
        keyglassd::lifecycle::InstanceLock::acquire(&data)
            .unwrap()
            .is_none(),
        "a second daemon can't take the lock"
    );
    drop(lock);
    assert_eq!(
        stop_with_connect_timeout(&data, refused_timeout).exit_code(),
        4
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn a_held_lock_without_a_discovery_file_is_an_error() {
    let tmp = temp_dir("client_lock_no_discovery");
    let data = tmp.join("data");
    let _lock = keyglassd::lifecycle::InstanceLock::acquire(&data)
        .unwrap()
        .unwrap();
    let outcome = stop_with(&data);
    assert_eq!(
        outcome.exit_code(),
        1,
        "a daemon may be starting: {outcome:?}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A daemon stand-in answering each lifecycle request with the next canned response.
fn canned_daemon(responses: Vec<&'static str>) -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut responses = responses.into_iter();
        let mut last = "";
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let resp = responses.next().unwrap_or(last);
            last = resp;
            let _ = s.write_all(resp.as_bytes());
        }
    });
    addr
}

fn http_ok(body: &str) -> &'static str {
    Box::leak(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_boxed_str(),
    )
}

#[test]
fn malformed_or_failed_answers_are_errors() {
    let tmp = temp_dir("client_malformed");
    let data = tmp.join("data");
    let lock = keyglassd::lifecycle::InstanceLock::acquire(&data)
        .unwrap()
        .unwrap();
    for resp in [
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        http_ok("not json"),
        http_ok(r#"{"outcome":"exiting"}"#),
        "garbage",
    ] {
        plaintext_discovery(&data, canned_daemon(vec![resp]));
        let outcome = stop_with(&data);
        assert_eq!(outcome.exit_code(), 1, "{resp:?}: {outcome:?}");
    }
    drop(lock);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn another_daemon_appearing_during_a_stop_is_an_error() {
    let tmp = temp_dir("client_new_instance");
    let data = tmp.join("data");
    let lock = keyglassd::lifecycle::InstanceLock::acquire(&data)
        .unwrap()
        .unwrap();
    let addr = canned_daemon(vec![
        http_ok(r#"{"outcome":"exiting","exit":"stop","instanceId":"first"}"#),
        http_ok(r#"{"outcome":"running","instanceId":"second","busy":[]}"#),
    ]);
    plaintext_discovery(&data, addr);
    let outcome = stop_with(&data);
    assert_eq!(outcome.exit_code(), 1, "{outcome:?}");
    drop(lock);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn a_second_daemon_for_the_same_data_directory_touches_nothing() {
    let bin = env!("CARGO_BIN_EXE_keyglassd");
    let tmp = temp_dir("second_instance");
    let data = tmp.join("data");
    let lock = keyglassd::lifecycle::InstanceLock::acquire(&data)
        .unwrap()
        .unwrap();
    for args in [vec!["--no-tray", "--no-usb"], vec!["--once", "--no-usb"]] {
        let out = daemon_command(bin, &tmp).args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("already running"),
            "{args:?}"
        );
    }
    let created: Vec<String> = walk(&tmp)
        .into_iter()
        .filter(|p| !p.ends_with("keyglassd.lock"))
        .collect();
    assert!(created.is_empty(), "nothing is written: {created:?}");
    drop(lock);
    let _ = std::fs::remove_dir_all(&tmp);
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p.to_string_lossy().to_string());
            }
        }
    }
    out
}

#[test]
fn of_two_daemons_started_together_one_runs_and_a_crash_releases_the_lock() {
    let bin = env!("CARGO_BIN_EXE_keyglassd");
    let tmp = temp_dir("concurrent");
    let spawn = || {
        daemon_command(bin, &tmp)
            .args(["--no-tray", "--no-usb"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut a = Daemon { child: spawn() };
    let mut b = Daemon { child: spawn() };
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut exited, mut running) = (None, None);
    while Instant::now() < deadline && exited.is_none() {
        for (name, d) in [("a", &mut a), ("b", &mut b)] {
            if let Some(st) = d.child.try_wait().unwrap() {
                exited = Some((name, st.code()));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let (name, code) = exited.expect("one of the two exits");
    assert_eq!(code, Some(0));
    let (mut loser, mut winner) = if name == "a" { (a, b) } else { (b, a) };
    let mut err = String::new();
    use std::io::Read;
    loser
        .child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    drop(loser);
    assert!(err.contains("already running"), "{err}");
    assert!(
        winner.child.try_wait().unwrap().is_none(),
        "the other keeps running"
    );
    // The discovery file names the running daemon.
    while Instant::now() < deadline && running.is_none() {
        if let Ok(Some(ep)) =
            keyglassd::api::lifecycle_client::DaemonEndpoint::discover(&tmp.join("data"))
        {
            if std::net::TcpStream::connect(ep.addr).is_ok() {
                running = Some(ep.addr);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(running.is_some());

    // A crash leaves the lock to the next daemon.
    winner.child.kill().unwrap();
    winner.child.wait().unwrap();
    let freed = Instant::now() + Duration::from_secs(3);
    while keyglassd::lifecycle::InstanceLock::probe(&tmp.join("data"))
        != keyglassd::lifecycle::InstanceProbe::Free
    {
        let holders = std::process::Command::new("lsof")
            .arg(tmp.join("data/config/keyglassd.lock"))
            .output();
        assert!(
            Instant::now() < freed,
            "the lock is released: {:?}",
            holders.map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut next = start_daemon(bin, &tmp);
    assert_eq!(
        keyglassd::lifecycle::InstanceLock::probe(&tmp.join("data")),
        keyglassd::lifecycle::InstanceProbe::Held
    );
    assert_eq!(request(bin, &tmp, "--request-stop"), (0, "stopped".into()));
    assert_eq!(wait_exit(&mut next), 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[cfg(unix)]
#[test]
fn a_signal_stops_an_idle_daemon_with_exit_0() {
    let bin = env!("CARGO_BIN_EXE_keyglassd");
    let tmp = temp_dir("sigterm");
    let mut d = start_daemon(bin, &tmp);
    unsafe {
        libc::kill(d.child.id() as i32, libc::SIGTERM);
    }
    assert_eq!(wait_exit(&mut d), 0, "a stopped daemon stays stopped");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Keyboard work requested between an idle status check and a stop either finishes before the
/// daemon may exit or is refused; it is never cut off.
#[tokio::test]
async fn work_racing_a_stop_finishes_first_or_is_refused() {
    use std::sync::Mutex;
    struct SlowSave {
        calls: Mutex<Vec<(Instant, Instant)>>,
    }
    impl keyglassd::api::state::DeviceSink for SlowSave {
        fn is_connected(&self) -> (bool, bool) {
            (true, true)
        }
        fn versions(&self) -> serde_json::Value {
            keyglassd::api::state::empty_versions()
        }
        fn send_live_json(&self, _: &serde_json::Value) -> Result<(), String> {
            Ok(())
        }
        fn set_active_profile(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_language(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_platform(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn save_config_dir(&self, _: &serde_json::Value) -> Result<(), String> {
            let start = Instant::now();
            std::thread::sleep(Duration::from_millis(150));
            self.calls.lock().unwrap().push((start, Instant::now()));
            Ok(())
        }
    }
    for delay_ms in [0u64, 1, 3, 10, 30, 80] {
        let tmp = temp_dir("race");
        let device = Arc::new(SlowSave {
            calls: Mutex::new(Vec::new()),
        });
        let state = ApiState::with_dirs(
            device.clone(),
            Some("race_token".into()),
            Duration::from_secs(20),
            tmp.join("icons"),
        );
        state.set_registration_secret("race_secret");
        state.inner.lock().unwrap().cached_connected = (true, true);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = build_router(state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (_, status) = post(
            addr,
            "/v1/daemon/lifecycle",
            r#"{"action":"status","enrollmentSecret":"race_secret"}"#,
        )
        .await;
        assert_eq!(status["busy"], serde_json::json!([]));
        // The installer's stop and a save arrive together.
        let save = tokio::spawn(async move {
            let mut s = TcpStream::connect(addr).await.unwrap();
            let req = "POST /v1/config/save HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer race_token\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
            s.write_all(req.as_bytes()).await.unwrap();
            let mut resp = Vec::new();
            s.read_to_end(&mut resp).await.unwrap();
            String::from_utf8_lossy(&resp)
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u16>()
                .unwrap()
        });
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        let (code, _) = post(
            addr,
            "/v1/daemon/lifecycle",
            r#"{"action":"stop","enrollmentSecret":"race_secret"}"#,
        )
        .await;
        assert_eq!(code, 200);
        let lc = state.lifecycle.clone();
        let due_at = tokio::task::spawn_blocking(move || {
            lc.wait_exit_due(Duration::from_secs(5))
                .expect("the exit becomes due");
            Instant::now()
        })
        .await
        .unwrap();
        let code = save.await.unwrap();
        let calls = device.calls.lock().unwrap().clone();
        match code {
            200 => {
                assert_eq!(calls.len(), 1);
                assert!(
                    calls[0].1 <= due_at,
                    "the save finished before the exit was due ({delay_ms} ms)"
                );
            }
            409 => assert!(
                calls.is_empty(),
                "a refused save never reaches the keyboard ({delay_ms} ms)"
            ),
            other => panic!("unexpected save status {other} ({delay_ms} ms)"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
