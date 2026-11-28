//! When the daemon may exit.
//!
//! Work on the keyboard (flashing, saving, installing, backing up) must not be interrupted by a
//! restart or stop, so each such operation holds a [`BusyLease`]. A restart or stop request that
//! arrives while any lease is held is deferred and takes effect when the last lease is released.
//! Once an exit is due, new leases are refused so a stream of new work can't postpone it forever.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// How the daemon leaves when it exits on request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExitKind {
    /// Exit so the service manager starts the daemon again, e.g. from an updated binary.
    Restart,
    /// Exit and stay stopped.
    Stop,
}

impl ExitKind {
    /// Process exit status. Service definitions restart the daemon after any non-zero exit and
    /// leave it stopped after exit 0.
    pub fn exit_code(self) -> i32 {
        match self {
            ExitKind::Restart => 75,
            ExitKind::Stop => 0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ExitKind::Restart => "restart",
            ExitKind::Stop => "stop",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "restart" => Some(ExitKind::Restart),
            "stop" => Some(ExitKind::Stop),
            _ => None,
        }
    }
}

/// Result of asking the daemon to exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitDecision {
    /// No keyboard work is running; the daemon exits now.
    Exiting(ExitKind),
    /// Keyboard work is running; the daemon exits on its own when it finishes.
    Deferred {
        kind: ExitKind,
        busy: Vec<&'static str>,
    },
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    holders: BTreeMap<u64, &'static str>,
    /// Strongest exit requested so far (`Stop` wins over `Restart`).
    requested: Option<ExitKind>,
}

#[derive(Default)]
pub struct Lifecycle {
    inner: Mutex<Inner>,
    changed: Condvar,
}

/// Held for the duration of one keyboard operation. Dropping it releases the lease.
#[must_use = "the lease is released when dropped"]
pub struct BusyLease {
    lifecycle: Arc<Lifecycle>,
    id: u64,
    activity: &'static str,
}

impl BusyLease {
    pub fn activity(&self) -> &'static str {
        self.activity
    }
}

impl Drop for BusyLease {
    fn drop(&mut self) {
        let mut g = self.lifecycle.inner.lock().unwrap();
        g.holders.remove(&self.id);
        drop(g);
        self.lifecycle.changed.notify_all();
    }
}

/// A lease was refused because the daemon is about to exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitPending(pub ExitKind);

impl std::fmt::Display for ExitPending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            ExitKind::Restart => f.write_str("Keyglass is restarting; try again in a moment"),
            ExitKind::Stop => f.write_str("Keyglass is stopping"),
        }
    }
}

impl Lifecycle {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Takes a lease for `activity` (shown to whoever asks why an exit is deferred).
    pub fn hold(self: &Arc<Self>, activity: &'static str) -> Result<BusyLease, ExitPending> {
        let mut g = self.inner.lock().unwrap();
        if let Some(kind) = g.requested {
            return Err(ExitPending(kind));
        }
        g.next_id += 1;
        let id = g.next_id;
        g.holders.insert(id, activity);
        Ok(BusyLease {
            lifecycle: self.clone(),
            id,
            activity,
        })
    }

    /// Asks the daemon to exit. The exit happens now if nothing holds a lease, otherwise as soon as
    /// the last lease is released.
    pub fn request_exit(&self, kind: ExitKind) -> ExitDecision {
        let mut g = self.inner.lock().unwrap();
        let effective = g.requested.map_or(kind, |prev| prev.max(kind));
        g.requested = Some(effective);
        let busy: Vec<&'static str> = g.holders.values().copied().collect();
        drop(g);
        self.changed.notify_all();
        if busy.is_empty() {
            ExitDecision::Exiting(effective)
        } else {
            ExitDecision::Deferred {
                kind: effective,
                busy,
            }
        }
    }

    /// The exit that has been requested, if any. Once one is, no new keyboard work starts.
    pub fn exit_requested(&self) -> Option<ExitKind> {
        self.inner.lock().unwrap().requested
    }

    /// Activities holding a lease.
    pub fn busy(&self) -> Vec<&'static str> {
        self.inner
            .lock()
            .unwrap()
            .holders
            .values()
            .copied()
            .collect()
    }

    /// The exit to perform now, if one was requested and no lease is held.
    pub fn exit_due(&self) -> Option<ExitKind> {
        let g = self.inner.lock().unwrap();
        if g.holders.is_empty() {
            g.requested
        } else {
            None
        }
    }

    /// Waits up to `timeout` for an exit to become due.
    pub fn wait_exit_due(&self, timeout: Duration) -> Option<ExitKind> {
        let g = self.inner.lock().unwrap();
        let (g, _) = self
            .changed
            .wait_timeout_while(g, timeout, |s| {
                !(s.holders.is_empty() && s.requested.is_some())
            })
            .unwrap();
        if g.holders.is_empty() {
            g.requested
        } else {
            None
        }
    }
}

/// Runs until the daemon should exit, then clears `running`.
///
/// A signal (`signalled`) is a stop request like `--request-stop`: keyboard work that holds a
/// lease, including every remaining step of an admitted job, finishes first. Exit requests
/// arriving through the API or the tray work the same way.
pub fn watch_exit(
    lifecycle: &Lifecycle,
    signalled: &std::sync::atomic::AtomicBool,
    running: &std::sync::atomic::AtomicBool,
) {
    use std::sync::atomic::Ordering;
    let mut stop_requested = false;
    loop {
        if !stop_requested && signalled.load(Ordering::SeqCst) {
            stop_requested = true;
            lifecycle.request_exit(ExitKind::Stop);
        }
        if lifecycle.wait_exit_due(Duration::from_millis(50)).is_some() {
            break;
        }
    }
    running.store(false, Ordering::SeqCst);
}

/// Data-directory file the running daemon holds an exclusive lock on for its whole life.
pub const INSTANCE_LOCK_FILE: &str = "config/keyglassd.lock";

/// Proof that this process is the daemon for a data directory. The lock is released when the
/// process exits, however it exits.
pub struct InstanceLock {
    _file: std::fs::File,
}

/// What the instance lock says about a data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceProbe {
    /// No process holds the lock: no daemon runs for this data directory.
    Free,
    /// A daemon holds the lock.
    Held,
    /// The lock couldn't be checked.
    Unknown(String),
}

impl InstanceLock {
    /// Takes the lock for `data_dir`. `Ok(None)` when another daemon holds it.
    pub fn acquire(data_dir: &std::path::Path) -> Result<Option<Self>, String> {
        let path = data_dir.join(INSTANCE_LOCK_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(format!("lock {}: {e}", path.display())),
        }
    }

    /// Checks, without keeping it, whether a daemon holds the lock for `data_dir`.
    pub fn probe(data_dir: &std::path::Path) -> InstanceProbe {
        let path = data_dir.join(INSTANCE_LOCK_FILE);
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return InstanceProbe::Free,
            Err(e) => return InstanceProbe::Unknown(format!("open {}: {e}", path.display())),
        };
        match file.try_lock() {
            Ok(()) => InstanceProbe::Free,
            Err(std::fs::TryLockError::WouldBlock) => InstanceProbe::Held,
            Err(std::fs::TryLockError::Error(e)) => {
                InstanceProbe::Unknown(format!("lock {}: {e}", path.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_request_exits_immediately() {
        let lc = Lifecycle::new();
        assert_eq!(
            lc.request_exit(ExitKind::Restart),
            ExitDecision::Exiting(ExitKind::Restart)
        );
        assert_eq!(lc.exit_due(), Some(ExitKind::Restart));
    }

    #[test]
    fn request_during_lease_is_deferred_until_release() {
        let lc = Lifecycle::new();
        let lease = lc.hold("flashing firmware").unwrap();
        assert_eq!(
            lc.request_exit(ExitKind::Restart),
            ExitDecision::Deferred {
                kind: ExitKind::Restart,
                busy: vec!["flashing firmware"]
            }
        );
        assert_eq!(lc.exit_due(), None);
        assert!(
            lc.hold("saving").is_err(),
            "new work is refused once an exit is pending"
        );
        drop(lease);
        assert_eq!(
            lc.wait_exit_due(Duration::from_secs(1)),
            Some(ExitKind::Restart)
        );
    }

    #[test]
    fn stop_takes_precedence_over_restart() {
        let lc = Lifecycle::new();
        let _lease = lc.hold("saving").unwrap();
        lc.request_exit(ExitKind::Stop);
        assert!(matches!(
            lc.request_exit(ExitKind::Restart),
            ExitDecision::Deferred {
                kind: ExitKind::Stop,
                ..
            }
        ));
    }

    #[test]
    fn waiting_wakes_when_the_last_lease_is_released() {
        let lc = Lifecycle::new();
        let a = lc.hold("a").unwrap();
        let b = lc.hold("b").unwrap();
        lc.request_exit(ExitKind::Stop);
        let waiter = {
            let lc = lc.clone();
            std::thread::spawn(move || lc.wait_exit_due(Duration::from_secs(5)))
        };
        drop(a);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(lc.exit_due(), None);
        drop(b);
        assert_eq!(waiter.join().unwrap(), Some(ExitKind::Stop));
    }
}
