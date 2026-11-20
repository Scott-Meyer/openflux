//! A sampler that runs on its own thread, for sensors whose calls can block (system media
//! sessions, D-Bus, network fetches) or cost enough CPU that they shouldn't run on every worker
//! tick. Readers never wait for a sample: they get the latest one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A sample and when it was taken.
#[derive(Debug, Clone)]
pub struct Sampled<T> {
    pub value: T,
    pub at: Instant,
}

struct Shared<T> {
    latest: Mutex<Option<Sampled<T>>>,
    stop: AtomicBool,
}

/// Runs `sample` every `interval` on a background thread until dropped. The thread starts with
/// `init` (for example opening a connection), and `sample` receives what `init` returned.
pub struct Poller<T> {
    shared: Arc<Shared<T>>,
}

impl<T: Clone + Send + 'static> Poller<T> {
    pub fn spawn<S, I, F>(name: &str, interval: Duration, init: I, mut sample: F) -> Self
    where
        I: FnOnce() -> S + Send + 'static,
        F: FnMut(&mut S) -> T + Send + 'static,
    {
        let shared = Arc::new(Shared {
            latest: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        let thread_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let mut state = init();
                while !thread_shared.stop.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    let value = sample(&mut state);
                    *thread_shared.latest.lock().unwrap() = Some(Sampled {
                        value,
                        at: Instant::now(),
                    });
                    // Sleep in short steps so a dropped poller's thread ends promptly.
                    let wake = started + interval;
                    while !thread_shared.stop.load(Ordering::Relaxed) {
                        let now = Instant::now();
                        if now >= wake {
                            break;
                        }
                        std::thread::sleep((wake - now).min(Duration::from_millis(100)));
                    }
                }
            });
        if spawned.is_err() {
            shared.stop.store(true, Ordering::Relaxed);
        }
        Self { shared }
    }

    /// The latest sample, or `None` before the first one finishes.
    pub fn latest(&self) -> Option<Sampled<T>> {
        self.shared.latest.lock().unwrap().clone()
    }
}

impl<T> Drop for Poller<T> {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn readers_get_the_latest_sample_without_waiting_and_the_thread_ends_when_dropped() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = Arc::clone(&calls);
        let poller = Poller::spawn(
            "test-poller",
            Duration::from_millis(20),
            || 10usize,
            move |base| {
                // A slow sample doesn't hold up readers.
                std::thread::sleep(Duration::from_millis(5));
                *base + calls_in.fetch_add(1, Ordering::SeqCst)
            },
        );
        let started = Instant::now();
        let first = poller.latest();
        assert!(started.elapsed() < Duration::from_millis(5));
        assert!(first.is_none() || first.unwrap().value >= 10);
        let deadline = Instant::now() + Duration::from_secs(5);
        while calls.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(poller.latest().unwrap().value >= 11);
        drop(poller);
        std::thread::sleep(Duration::from_millis(250));
        let after_drop = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(calls.load(Ordering::SeqCst), after_drop);
    }
}
