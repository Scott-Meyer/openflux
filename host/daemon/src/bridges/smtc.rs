//! Now playing on Windows, from the system media session that players publish to the volume
//! flyout and lock screen (`GlobalSystemMediaTransportControlsSessionManager`).

use super::MediaSnapshot;

/// 100 ns ticks between 1601-01-01 (Windows time) and 1970-01-01 (Unix time).
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
const TICKS_PER_SEC: i64 = 10_000_000;

/// The current time in Windows ticks (100 ns since 1601), as `DateTime.UniversalTime` counts.
pub fn now_ticks() -> i64 {
    let since_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_nanos() / 100).min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    UNIX_EPOCH_TICKS.saturating_add(since_unix)
}

/// A session's timeline, in ticks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timeline {
    pub start: i64,
    pub end: i64,
    pub position: i64,
    /// When the player last reported its position.
    pub last_updated: i64,
}

/// The track length and position in seconds. A player reports its position only now and then,
/// so while it plays the time since that report is added.
pub fn track_seconds(t: &Timeline, playing: bool, now: i64) -> (u32, u32) {
    let secs = |ticks: i64| (ticks.max(0) / TICKS_PER_SEC).min(u32::MAX as i64) as u32;
    let length = secs(t.end.saturating_sub(t.start));
    let mut position = t.position.saturating_sub(t.start);
    if playing && t.last_updated > 0 && now > t.last_updated {
        position = position.saturating_add(now - t.last_updated);
    }
    let played = secs(position);
    (
        length,
        if length > 0 {
            played.min(length)
        } else {
            played
        },
    )
}

/// The key the artwork cache remembers a session's art by: the app and the track.
pub fn art_key(app_id: &str, title: &str, artist: &str, album: &str) -> String {
    if title.is_empty() {
        return String::new();
    }
    format!("{app_id}\u{1f}{title}\u{1f}{artist}\u{1f}{album}")
}

/// `PlaybackStatus` value of a playing session.
pub const STATUS_PLAYING: i32 = 4;

/// Puts the parts of a session's state together.
pub fn snapshot_from(
    title: &str,
    artist: &str,
    album: &str,
    status: i32,
    timeline: &Timeline,
    now: i64,
) -> MediaSnapshot {
    if title.is_empty() {
        return MediaSnapshot::default();
    }
    let playing = status == STATUS_PLAYING;
    let (length_s, played_s) = track_seconds(timeline, playing, now);
    MediaSnapshot {
        title: title.to_string(),
        artists: artist.to_string(),
        album: album.to_string(),
        length_s,
        played_s,
        playing,
        artwork: None,
    }
}

#[cfg(target_os = "windows")]
pub use imp::SmtcState;

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use crate::bridges::artwork::{ArtworkCache, MAX_SOURCE_BYTES};
    use std::time::{Duration, Instant};
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
    };
    use windows::Storage::Streams::{Buffer, DataReader, InputStreamOptions};
    use windows_future::{AsyncStatus, IAsyncOperation, IAsyncOperationWithProgress};

    /// How long one call into the media session may take. A player that doesn't answer
    /// (hung, or busy) costs at most this.
    const CALL_TIMEOUT: Duration = Duration::from_millis(1500);

    trait Pending {
        type Output;
        fn status(&self) -> windows::core::Result<AsyncStatus>;
        fn results(&self) -> windows::core::Result<Self::Output>;
        fn cancel(&self);
    }

    impl<T: windows::core::RuntimeType + 'static> Pending for IAsyncOperation<T> {
        type Output = T;
        fn status(&self) -> windows::core::Result<AsyncStatus> {
            self.Status()
        }
        fn results(&self) -> windows::core::Result<T> {
            self.GetResults()
        }
        fn cancel(&self) {
            let _ = self.Cancel();
        }
    }

    impl<T: windows::core::RuntimeType + 'static, P: windows::core::RuntimeType + 'static> Pending
        for IAsyncOperationWithProgress<T, P>
    {
        type Output = T;
        fn status(&self) -> windows::core::Result<AsyncStatus> {
            self.Status()
        }
        fn results(&self) -> windows::core::Result<T> {
            self.GetResults()
        }
        fn cancel(&self) {
            let _ = self.Cancel();
        }
    }

    /// Waits up to [`CALL_TIMEOUT`] for an operation, and cancels it when it runs longer.
    fn wait<P: Pending>(op: windows::core::Result<P>) -> Option<P::Output> {
        let op = op.ok()?;
        let deadline = Instant::now() + CALL_TIMEOUT;
        loop {
            match op.status().ok()? {
                AsyncStatus::Completed => return op.results().ok(),
                AsyncStatus::Started if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                AsyncStatus::Started => {
                    op.cancel();
                    return None;
                }
                _ => return None,
            }
        }
    }

    /// What the media sampler keeps between samples. It lives on the sampler's thread, which
    /// joins the multithreaded COM apartment the media session calls need.
    pub struct SmtcState {
        manager: Option<Manager>,
        tried_at: Option<Instant>,
        art: ArtworkCache,
    }

    impl Default for SmtcState {
        fn default() -> Self {
            // SAFETY: initializes this thread's apartment once, before any WinRT call on it.
            unsafe {
                let _ = windows::Win32::System::WinRT::RoInitialize(
                    windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
                );
            }
            Self {
                manager: None,
                tried_at: None,
                art: ArtworkCache::default(),
            }
        }
    }

    impl SmtcState {
        pub fn sample(&mut self) -> MediaSnapshot {
            if self.manager.is_none()
                && self
                    .tried_at
                    .is_none_or(|t| t.elapsed() >= Duration::from_secs(10))
            {
                self.tried_at = Some(Instant::now());
                self.manager = wait(Manager::RequestAsync());
            }
            let Some(session) = self
                .manager
                .as_ref()
                .and_then(|m| m.GetCurrentSession().ok())
            else {
                self.art.resolve("", || None);
                return MediaSnapshot::default();
            };
            let mut snap = Self::session_snapshot(&session);
            let key = art_key(
                &session
                    .SourceAppUserModelId()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                &snap.title,
                &snap.artists,
                &snap.album,
            );
            snap.artwork = self.art.resolve(&key, || Self::thumbnail(&session));
            snap
        }

        fn session_snapshot(session: &Session) -> MediaSnapshot {
            let Some(props) = wait(session.TryGetMediaPropertiesAsync()) else {
                return MediaSnapshot::default();
            };
            let text = |r: windows::core::Result<windows::core::HSTRING>| {
                r.map(|s| s.to_string()).unwrap_or_default()
            };
            let status = session
                .GetPlaybackInfo()
                .and_then(|i| i.PlaybackStatus())
                .map(|s| s.0)
                .unwrap_or(0);
            let timeline = session
                .GetTimelineProperties()
                .map(|t| Timeline {
                    start: t.StartTime().map(|v| v.Duration).unwrap_or(0),
                    end: t.EndTime().map(|v| v.Duration).unwrap_or(0),
                    position: t.Position().map(|v| v.Duration).unwrap_or(0),
                    last_updated: t.LastUpdatedTime().map(|v| v.UniversalTime).unwrap_or(0),
                })
                .unwrap_or_default();
            snapshot_from(
                &text(props.Title()),
                &text(props.Artist()),
                &text(props.AlbumTitle()),
                status,
                &timeline,
                now_ticks(),
            )
        }

        /// The session's thumbnail image, read from its stream.
        fn thumbnail(session: &Session) -> Option<Vec<u8>> {
            let props = wait(session.TryGetMediaPropertiesAsync())?;
            let stream = wait(props.Thumbnail().ok()?.OpenReadAsync())?;
            let size = stream.Size().ok()?;
            if size == 0 || size > MAX_SOURCE_BYTES as u64 {
                return None;
            }
            let buffer = Buffer::Create(size as u32).ok()?;
            let filled =
                wait(stream.ReadAsync(&buffer, size as u32, InputStreamOptions::ReadAhead))?;
            let len = filled.Length().ok()? as usize;
            let mut bytes = vec![0u8; len];
            DataReader::FromBuffer(&filled)
                .ok()?
                .ReadBytes(&mut bytes)
                .ok()?;
            Some(bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_track_position_moves_on_from_the_players_last_report() {
        let now = now_ticks();
        let t = Timeline {
            start: 0,
            end: 200 * TICKS_PER_SEC,
            position: 40 * TICKS_PER_SEC,
            last_updated: now - 5 * TICKS_PER_SEC,
        };
        assert_eq!(track_seconds(&t, true, now), (200, 45));
        assert_eq!(track_seconds(&t, false, now), (200, 40));
        // Never past the end, and a player without a timeline reports none.
        let long_ago = Timeline {
            last_updated: now - 1_000 * TICKS_PER_SEC,
            ..t
        };
        assert_eq!(track_seconds(&long_ago, true, now), (200, 200));
        assert_eq!(track_seconds(&Timeline::default(), true, now), (0, 0));

        let snap = snapshot_from("Song", "Artist", "Album", STATUS_PLAYING, &t, now);
        assert!(snap.playing);
        assert_eq!((snap.length_s, snap.played_s), (200, 45));
        assert!(!snapshot_from("Song", "", "", 5, &t, now).playing);
        assert_eq!(
            snapshot_from("", "Artist", "", STATUS_PLAYING, &t, now),
            MediaSnapshot::default()
        );
    }

    #[test]
    fn windows_ticks_count_from_1601() {
        let ticks = now_ticks();
        let unix_secs = (ticks - UNIX_EPOCH_TICKS) / TICKS_PER_SEC;
        let expected = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!((unix_secs - expected).abs() <= 1);
        assert_eq!(art_key("Spotify.exe", "", "a", "b"), "");
        assert_ne!(
            art_key("Spotify.exe", "Song", "a", "b"),
            art_key("Spotify.exe", "Song", "a", "c")
        );
    }
}
