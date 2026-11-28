//! Now playing on Linux, from the media players on the session bus (the MPRIS D-Bus interface).

use super::artwork::{ArtworkCache, MAX_SOURCE_BYTES};
use super::MediaSnapshot;
use base64::Engine as _;
use std::collections::HashMap;
use std::path::PathBuf;

/// A property value, in the shapes MPRIS players send.
#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    Str(String),
    StrList(Vec<String>),
    Int(i64),
    Float(f64),
    Map(HashMap<String, Prop>),
    Other,
}

impl Prop {
    fn as_str(&self) -> Option<&str> {
        match self {
            Prop::Str(s) => Some(s),
            Prop::StrList(v) => v.first().map(String::as_str),
            _ => None,
        }
    }

    fn as_list(&self) -> Vec<String> {
        match self {
            Prop::Str(s) if !s.is_empty() => vec![s.clone()],
            Prop::StrList(v) => v.iter().filter(|s| !s.is_empty()).cloned().collect(),
            _ => Vec::new(),
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            Prop::Int(n) => Some(*n),
            Prop::Float(f) if f.is_finite() => Some(*f as i64),
            _ => None,
        }
    }
}

/// One player's state, from its `org.mpris.MediaPlayer2.Player` properties.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MprisPlayer {
    pub bus_name: String,
    /// `Playing`, `Paused` or `Stopped`.
    pub status: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    pub length_us: i64,
    pub position_us: i64,
    pub art_url: String,
}

impl MprisPlayer {
    pub fn from_props(bus_name: &str, props: &HashMap<String, Prop>) -> Self {
        let empty = HashMap::new();
        let meta = match props.get("Metadata") {
            Some(Prop::Map(m)) => m,
            _ => &empty,
        };
        let text = |m: &HashMap<String, Prop>, k: &str| {
            m.get(k)
                .and_then(Prop::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Self {
            bus_name: bus_name.to_string(),
            status: text(props, "PlaybackStatus"),
            title: text(meta, "xesam:title"),
            artists: meta
                .get("xesam:artist")
                .map(Prop::as_list)
                .unwrap_or_default(),
            album: text(meta, "xesam:album"),
            length_us: meta.get("mpris:length").and_then(Prop::as_i64).unwrap_or(0),
            position_us: props.get("Position").and_then(Prop::as_i64).unwrap_or(0),
            art_url: text(meta, "mpris:artUrl"),
        }
    }

    pub fn is_playing(&self) -> bool {
        self.status == "Playing"
    }

    /// The track as the keyboard shows it, without art.
    pub fn snapshot(&self) -> MediaSnapshot {
        let secs = |us: i64| (us.max(0) / 1_000_000).min(u32::MAX as i64) as u32;
        let length_s = secs(self.length_us);
        let played_s = secs(self.position_us);
        MediaSnapshot {
            title: self.title.clone(),
            artists: self.artists.join(", "),
            album: self.album.clone(),
            length_s,
            played_s: if length_s > 0 {
                played_s.min(length_s)
            } else {
                played_s
            },
            playing: self.is_playing(),
            artwork: None,
        }
    }
}

/// The player to show: one that's playing, else one that's paused on a track. Among equals the
/// one shown last stays, so two players don't take turns.
pub fn pick_player<'a>(players: &'a [MprisPlayer], previous: &str) -> Option<&'a MprisPlayer> {
    let best_of = |want: fn(&MprisPlayer) -> bool| {
        let matching: Vec<&'a MprisPlayer> = players
            .iter()
            .filter(|p| !p.title.is_empty() && want(p))
            .collect();
        matching
            .iter()
            .find(|p| p.bus_name == previous)
            .or(matching.first())
            .copied()
    };
    best_of(|p| p.is_playing()).or_else(|| best_of(|p| p.status == "Paused"))
}

/// Where a player's `mpris:artUrl` points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtSource {
    File(PathBuf),
    Http(String),
    Inline(Vec<u8>),
}

/// Reads an `mpris:artUrl`: a `file://` path, an `http(s)://` URL, or a base64 `data:` URI.
pub fn parse_art_url(url: &str) -> Option<ArtSource> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("file://") {
        // `file:///path` or `file://localhost/path`.
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        if !path.starts_with('/') {
            return None;
        }
        return Some(ArtSource::File(PathBuf::from(percent_decode(path)?)));
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return Some(ArtSource::Http(url.to_string()));
    }
    if let Some(rest) = url.strip_prefix("data:") {
        let (header, data) = rest.split_once(',')?;
        if !header.ends_with(";base64") {
            return None;
        }
        return base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .ok()
            .map(ArtSource::Inline);
    }
    None
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Loads the image an art URL points at, at most [`MAX_SOURCE_BYTES`] of it.
pub fn load_art(source: &ArtSource) -> Option<Vec<u8>> {
    match source {
        ArtSource::Inline(bytes) => Some(bytes.clone()),
        ArtSource::File(path) => {
            let meta = std::fs::metadata(path).ok()?;
            if !meta.is_file() || meta.len() > MAX_SOURCE_BYTES as u64 {
                return None;
            }
            std::fs::read(path).ok()
        }
        ArtSource::Http(url) => {
            use std::io::Read;
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .ok()?;
            let resp = client.get(url).send().ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let mut body = Vec::new();
            resp.take(MAX_SOURCE_BYTES as u64 + 1)
                .read_to_end(&mut body)
                .ok()?;
            (body.len() <= MAX_SOURCE_BYTES).then_some(body)
        }
    }
}

/// The key the artwork cache remembers a source by. A file that's rewritten in place (some
/// players reuse one path) gets a new key from its new size or modification time.
pub fn art_cache_key(url: &str, source: &ArtSource) -> String {
    match source {
        ArtSource::File(path) => {
            let stamp = std::fs::metadata(path)
                .ok()
                .map(|m| {
                    let modified = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos())
                        .unwrap_or(0);
                    format!("{}:{modified}", m.len())
                })
                .unwrap_or_default();
            format!("{url}#{stamp}")
        }
        _ => url.to_string(),
    }
}

/// What the media sampler keeps between samples.
#[derive(Default)]
pub struct MprisState {
    #[cfg(target_os = "linux")]
    client: Option<client::MprisClient>,
    #[cfg(target_os = "linux")]
    connect_tried_at: Option<std::time::Instant>,
    previous: String,
    art: ArtworkCache,
}

impl MprisState {
    /// The track to show from these players, with its art.
    pub fn snapshot_of(&mut self, players: &[MprisPlayer]) -> MediaSnapshot {
        let Some(player) = pick_player(players, &self.previous) else {
            self.previous.clear();
            self.art.resolve("", || None);
            return MediaSnapshot::default();
        };
        self.previous = player.bus_name.clone();
        let mut snap = player.snapshot();
        snap.artwork = match parse_art_url(&player.art_url) {
            Some(source) => {
                let key = art_cache_key(&player.art_url, &source);
                self.art.resolve(&key, || load_art(&source))
            }
            None => self.art.resolve("", || None),
        };
        snap
    }

    /// Samples the session bus's players.
    #[cfg(target_os = "linux")]
    pub fn sample(&mut self) -> MediaSnapshot {
        if self.client.is_none()
            && self
                .connect_tried_at
                .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(10))
        {
            self.connect_tried_at = Some(std::time::Instant::now());
            self.client = client::MprisClient::connect();
        }
        let players = match &self.client {
            Some(c) => match c.players() {
                Some(p) => p,
                None => {
                    // The bus went away; connect again later.
                    self.client = None;
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        self.snapshot_of(&players)
    }
}

#[cfg(target_os = "linux")]
mod client {
    use super::{MprisPlayer, Prop};
    use std::collections::HashMap;
    use std::time::Duration;
    use zbus::zvariant::{OwnedValue, Value};

    const PREFIX: &str = "org.mpris.MediaPlayer2.";

    pub struct MprisClient {
        conn: zbus::blocking::Connection,
    }

    impl MprisClient {
        pub fn connect() -> Option<Self> {
            let conn = zbus::blocking::connection::Builder::session()
                .ok()?
                .method_timeout(Duration::from_secs(1))
                .build()
                .ok()?;
            Some(Self { conn })
        }

        /// Every player on the bus. `None` when the bus itself didn't answer.
        pub fn players(&self) -> Option<Vec<MprisPlayer>> {
            let names: Vec<String> = self
                .conn
                .call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "ListNames",
                    &(),
                )
                .ok()?
                .body()
                .deserialize()
                .ok()?;
            let players = names
                .iter()
                .filter(|n| n.starts_with(PREFIX))
                .take(16)
                .filter_map(|name| {
                    let reply = self
                        .conn
                        .call_method(
                            Some(name.as_str()),
                            "/org/mpris/MediaPlayer2",
                            Some("org.freedesktop.DBus.Properties"),
                            "GetAll",
                            &("org.mpris.MediaPlayer2.Player",),
                        )
                        .ok()?;
                    let props: HashMap<String, OwnedValue> = reply.body().deserialize().ok()?;
                    let props: HashMap<String, Prop> =
                        props.iter().map(|(k, v)| (k.clone(), to_prop(v))).collect();
                    Some(MprisPlayer::from_props(name, &props))
                })
                .collect();
            Some(players)
        }
    }

    fn to_prop(v: &Value<'_>) -> Prop {
        match v {
            Value::Str(s) => Prop::Str(s.to_string()),
            Value::ObjectPath(p) => Prop::Str(p.as_str().to_string()),
            Value::I16(n) => Prop::Int(*n as i64),
            Value::U16(n) => Prop::Int(*n as i64),
            Value::I32(n) => Prop::Int(*n as i64),
            Value::U32(n) => Prop::Int(*n as i64),
            Value::I64(n) => Prop::Int(*n),
            Value::U64(n) => Prop::Int((*n).min(i64::MAX as u64) as i64),
            Value::U8(n) => Prop::Int(*n as i64),
            Value::F64(f) => Prop::Float(*f),
            Value::Value(inner) => to_prop(inner),
            Value::Array(a) => Prop::StrList(
                a.iter()
                    .filter_map(|e| match to_prop(e) {
                        Prop::Str(s) => Some(s),
                        _ => None,
                    })
                    .collect(),
            ),
            Value::Dict(d) => Prop::Map(
                d.iter()
                    .filter_map(|(k, v)| match k {
                        Value::Str(k) => Some((k.to_string(), to_prop(v))),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => Prop::Other,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn dbus_values_become_props() {
            let mut meta = HashMap::<String, Value>::new();
            meta.insert("xesam:title".into(), Value::from("Song"));
            meta.insert("xesam:artist".into(), Value::from(vec!["A", "B"]));
            meta.insert("mpris:length".into(), Value::from(240_000_000i64));
            let v = Value::from(meta);
            let Prop::Map(m) = to_prop(&v) else {
                panic!("not a map");
            };
            assert_eq!(m["xesam:title"], Prop::Str("Song".into()));
            assert_eq!(
                m["xesam:artist"],
                Prop::StrList(vec!["A".into(), "B".into()])
            );
            assert_eq!(m["mpris:length"], Prop::Int(240_000_000));
            assert_eq!(
                to_prop(&Value::Value(Box::new(Value::from(5u64)))),
                Prop::Int(5)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(bus: &str, status: &str, title: &str) -> MprisPlayer {
        MprisPlayer {
            bus_name: bus.into(),
            status: status.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn mpris_properties_become_the_keyboard_track() {
        let mut meta = HashMap::new();
        meta.insert("xesam:title".into(), Prop::Str("Song".into()));
        meta.insert(
            "xesam:artist".into(),
            Prop::StrList(vec!["Ann".into(), "Bo".into()]),
        );
        meta.insert("xesam:album".into(), Prop::Str("Album".into()));
        meta.insert("mpris:length".into(), Prop::Int(200_000_000));
        meta.insert(
            "mpris:artUrl".into(),
            Prop::Str("https://i.scdn.co/image/ab67".into()),
        );
        let mut props = HashMap::new();
        props.insert("Metadata".into(), Prop::Map(meta));
        props.insert("PlaybackStatus".into(), Prop::Str("Playing".into()));
        props.insert("Position".into(), Prop::Int(45_500_000));
        let p = MprisPlayer::from_props("org.mpris.MediaPlayer2.spotify", &props);
        assert_eq!(p.art_url, "https://i.scdn.co/image/ab67");
        let snap = p.snapshot();
        assert_eq!(
            snap,
            MediaSnapshot {
                title: "Song".into(),
                artists: "Ann, Bo".into(),
                album: "Album".into(),
                length_s: 200,
                played_s: 45,
                playing: true,
                artwork: None,
            }
        );
        // A player that sends one artist as a string, and a position past the end.
        let mut meta = HashMap::new();
        meta.insert("xesam:title".into(), Prop::Str("Live".into()));
        meta.insert("xesam:artist".into(), Prop::Str("Solo".into()));
        meta.insert("mpris:length".into(), Prop::Int(10_000_000));
        let mut props = HashMap::new();
        props.insert("Metadata".into(), Prop::Map(meta));
        props.insert("PlaybackStatus".into(), Prop::Str("Paused".into()));
        props.insert("Position".into(), Prop::Int(99_000_000));
        let snap = MprisPlayer::from_props("org.mpris.MediaPlayer2.vlc", &props).snapshot();
        assert_eq!(snap.artists, "Solo");
        assert_eq!(snap.played_s, 10);
        assert!(!snap.playing);
    }

    #[test]
    fn a_playing_player_wins_and_the_shown_one_stays_among_equals() {
        let players = vec![
            player("org.mpris.MediaPlayer2.firefox", "Paused", "Video"),
            player("org.mpris.MediaPlayer2.spotify", "Playing", "Song"),
            player("org.mpris.MediaPlayer2.vlc", "Playing", "Film"),
            player("org.mpris.MediaPlayer2.idle", "Stopped", ""),
        ];
        assert_eq!(
            pick_player(&players, "").unwrap().bus_name,
            "org.mpris.MediaPlayer2.spotify"
        );
        assert_eq!(
            pick_player(&players, "org.mpris.MediaPlayer2.vlc")
                .unwrap()
                .bus_name,
            "org.mpris.MediaPlayer2.vlc"
        );
        let paused_only = vec![
            player("org.mpris.MediaPlayer2.idle", "Stopped", "Old"),
            player("org.mpris.MediaPlayer2.firefox", "Paused", "Video"),
        ];
        assert_eq!(
            pick_player(&paused_only, "").unwrap().bus_name,
            "org.mpris.MediaPlayer2.firefox"
        );
        assert!(pick_player(&[player("x", "Stopped", "Old")], "").is_none());
    }

    #[test]
    fn art_urls_name_files_web_images_and_inline_data() {
        assert_eq!(
            parse_art_url("file:///home/me/Music/My%20Album/cover.jpg"),
            Some(ArtSource::File("/home/me/Music/My Album/cover.jpg".into()))
        );
        assert_eq!(
            parse_art_url("file://localhost/tmp/a.png"),
            Some(ArtSource::File("/tmp/a.png".into()))
        );
        assert_eq!(
            parse_art_url("https://i.scdn.co/image/ab67"),
            Some(ArtSource::Http("https://i.scdn.co/image/ab67".into()))
        );
        assert_eq!(
            parse_art_url("data:image/png;base64,AAEC"),
            Some(ArtSource::Inline(vec![0, 1, 2]))
        );
        assert_eq!(parse_art_url(""), None);
        assert_eq!(parse_art_url("file://relative/x.png"), None);
        assert_eq!(parse_art_url("ftp://host/x.png"), None);
        assert_eq!(parse_art_url("file:///bad%zz"), None);
    }

    #[test]
    #[cfg(unix)]
    fn the_shown_track_carries_its_art_from_a_file() {
        let dir = std::env::temp_dir().join(format!("keyglass-mpris-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cover = dir.join("cover art.png");
        std::fs::write(&cover, super::super::artwork::test_png(300, 300)).unwrap();
        let mut p = player("org.mpris.MediaPlayer2.mpv", "Playing", "Song");
        p.art_url = format!("file://{}", cover.display()).replace(' ', "%20");
        let mut state = MprisState::default();
        let snap = state.snapshot_of(std::slice::from_ref(&p));
        let art = snap.artwork.expect("artwork");
        assert!(!art.id.is_empty());
        assert_eq!(state.snapshot_of(&[p]).artwork.unwrap().id, art.id);
        assert_eq!(state.snapshot_of(&[]), MediaSnapshot::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
