//! Whether the daemon for a data directory has exited.
//!
//! A host update replaces the daemon's files, so "the daemon is gone" is decided only on positive
//! evidence, by the same rule as `openfluxd --request-stop`: there is no discovery file, or the
//! port it records refuses connections, and in both cases no process holds the daemon's
//! instance lock. A discovery file that can't be read, a port that accepts connections but fails
//! TLS or authentication, a timeout, or a lock that can't be checked all mean "not known", which
//! blocks the install.

use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// Data-directory file the running daemon holds an exclusive lock on for its whole life.
pub const INSTANCE_LOCK_FILE: &str = "config/openfluxd.lock";
/// The daemon's own discovery file.
pub const DISCOVERY_FILE: &str = "config/keys.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    /// No daemon runs for the data directory. `evidence` says why that is known.
    Exited { evidence: String },
    /// Something accepts connections on the recorded port.
    Listening,
    /// The daemon's state couldn't be established.
    Unknown { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockProbe {
    Free,
    Held,
    Unknown(String),
}

/// Checks, without keeping it, whether a process holds the instance lock.
pub fn probe_lock(data_dir: &Path) -> LockProbe {
    let path = data_dir.join(INSTANCE_LOCK_FILE);
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LockProbe::Free,
        Err(e) => return LockProbe::Unknown(format!("open {}: {e}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => LockProbe::Free,
        Err(std::fs::TryLockError::WouldBlock) => LockProbe::Held,
        Err(std::fs::TryLockError::Error(e)) => {
            LockProbe::Unknown(format!("lock {}: {e}", path.display()))
        }
    }
}

fn gone_if_unlocked(data_dir: &Path, observed: &str) -> Presence {
    match probe_lock(data_dir) {
        LockProbe::Free => Presence::Exited {
            evidence: format!("{observed}, and no process holds the instance lock"),
        },
        LockProbe::Held => Presence::Unknown {
            reason: format!(
                "{observed}, but a daemon process still holds the instance lock; it may be starting or not responding"
            ),
        },
        LockProbe::Unknown(e) => Presence::Unknown {
            reason: format!("{observed}, and the instance lock couldn't be checked: {e}"),
        },
    }
}

/// The recorded address from the discovery file, or why it couldn't be read.
fn recorded_addr(data_dir: &Path) -> Result<Option<SocketAddr>, String> {
    let path = data_dir.join(DISCOVERY_FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let doc: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    let port = doc
        .get("openPort")
        .and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .ok_or_else(|| format!("{} has no openPort", path.display()))?;
    let host: IpAddr = doc
        .get("host")
        .and_then(|v| v.as_str())
        .and_then(|h| h.parse().ok())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]));
    Ok(Some(SocketAddr::new(host, port)))
}

/// Default TCP connect timeout when probing whether a recorded loopback port still listens.
/// Windows retries loopback SYNs for ~2s before reporting `ConnectionRefused`, so a 2s timeout
/// can report `TimedOut` instead of `ConnectionRefused` on a stopped daemon.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(4);

/// Establishes whether the daemon for `data_dir` has exited. Blocks for at most `timeout` on
/// the connection attempt.
pub fn probe(data_dir: &Path, timeout: Duration) -> Presence {
    let addr = match recorded_addr(data_dir) {
        Ok(Some(a)) => a,
        Ok(None) => return gone_if_unlocked(data_dir, "there is no discovery file"),
        Err(reason) => return Presence::Unknown { reason },
    };
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => Presence::Listening,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            gone_if_unlocked(data_dir, &format!("{addr} refuses connections"))
        }
        Err(e) => Presence::Unknown {
            reason: format!("connecting to {addr} failed: {e}"),
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    pub(crate) fn data_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("openflux_presence_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("config")).unwrap();
        d
    }

    /// Holds the instance lock the way a running daemon does.
    pub(crate) fn hold_lock(dir: &Path) -> std::fs::File {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(INSTANCE_LOCK_FILE))
            .unwrap();
        f.try_lock().unwrap();
        f
    }

    /// A port nothing listens on.
    pub(crate) fn refused_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    pub(crate) fn write_keys(dir: &Path, port: u16) {
        std::fs::write(
            dir.join(DISCOVERY_FILE),
            format!(r#"{{"openPort": {port}, "host": "127.0.0.1"}}"#),
        )
        .unwrap();
    }

    const T: Duration = DEFAULT_PROBE_TIMEOUT;

    #[test]
    fn a_missing_discovery_file_with_a_held_lock_is_not_proof_of_exit() {
        let d = data_dir("held");
        let _lock = hold_lock(&d);
        assert!(
            matches!(probe(&d, T), Presence::Unknown { reason } if reason.contains("holds the instance lock"))
        );
    }

    #[test]
    fn an_unreadable_discovery_file_is_not_proof_of_exit() {
        let d = data_dir("unreadable");
        std::fs::create_dir_all(d.join(DISCOVERY_FILE)).unwrap();
        assert!(
            matches!(probe(&d, T), Presence::Unknown { reason } if reason.contains("keys.json"))
        );
        let d = data_dir("garbled");
        std::fs::write(d.join(DISCOVERY_FILE), "{").unwrap();
        assert!(matches!(probe(&d, T), Presence::Unknown { .. }));
    }

    #[test]
    fn a_refused_port_is_proof_of_exit_only_with_a_free_lock() {
        let d = data_dir("refused");
        write_keys(&d, refused_port());
        // A lock file left behind by an exited daemon is free.
        std::fs::write(d.join(INSTANCE_LOCK_FILE), "").unwrap();
        assert!(
            matches!(probe(&d, T), Presence::Exited { evidence } if evidence.contains("refuses connections"))
        );
        let _lock = hold_lock(&d);
        assert!(matches!(probe(&d, T), Presence::Unknown { .. }));
    }

    #[test]
    fn no_discovery_file_and_no_lock_holder_is_proof_of_exit() {
        let d = data_dir("absent");
        assert!(matches!(probe(&d, T), Presence::Exited { .. }));
    }

    #[test]
    fn a_listening_port_is_not_an_exit() {
        let d = data_dir("listening");
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        write_keys(&d, l.local_addr().unwrap().port());
        assert_eq!(probe(&d, T), Presence::Listening);
    }
}
