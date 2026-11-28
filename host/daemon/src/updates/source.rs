//! Where releases come from: the signed online index, the index bundled with the installer, and
//! the artifacts they list.
//!
//! Everything read here is checked with `keyglass_update` before it is used: indexes against the
//! trust anchors and the persisted trust state, artifacts against their size, hash and signature.

use keyglass_update::manifest::Artifact;
use keyglass_update::trust::{ArtifactError, VerifiedArtifact};
use keyglass_update::{
    verify_artifact, verify_bundled_index, verify_index, Catalog, TrustAnchors, TrustState,
    TrustStore, VerifiedIndex,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Largest `index.json` accepted.
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
/// Largest signature file accepted.
const MAX_SIGNATURE_BYTES: u64 = 64 * 1024;

/// [`TrustStore`] kept as a JSON file in the data directory.
pub struct FileTrustStore {
    path: PathBuf,
}

impl FileTrustStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl TrustStore for FileTrustStore {
    type Error = String;

    fn load(&self) -> Result<TrustState, String> {
        match std::fs::read(&self.path) {
            Ok(raw) => serde_json::from_slice(&raw)
                .map_err(|e| format!("{} is unreadable: {e}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TrustState::default()),
            Err(e) => Err(format!("read {}: {e}", self.path.display())),
        }
    }

    fn save(&mut self, state: &TrustState) -> Result<(), String> {
        let raw = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
        write_durably(&self.path, &raw)
    }
}

/// Writes `data` to `path` through a temporary file, flushed to disk before it replaces `path`.
pub fn write_durably(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    let tmp = path.with_extension("tmp");
    {
        let mut f =
            std::fs::File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
        f.write_all(data)
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        f.sync_all()
            .map_err(|e| format!("flush {}: {e}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("replace {}: {e}", path.display()))?;
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Why something couldn't be fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The server couldn't be reached.
    Offline(String),
    /// The server answered, but not with the file.
    Unavailable(String),
    /// A local file couldn't be read or written.
    Io(String),
}

impl FetchError {
    pub fn message(&self) -> &str {
        match self {
            FetchError::Offline(m) | FetchError::Unavailable(m) | FetchError::Io(m) => m,
        }
    }
}

/// Resolves `url` against `base`: absolute URLs are kept, relative ones replace the last path
/// segment of `base`.
pub fn resolve_url(base: &str, url: &str) -> String {
    if url.contains("://") {
        return url.to_string();
    }
    match base.rfind('/') {
        Some(i) => format!("{}/{}", &base[..i], url.trim_start_matches("./")),
        None => url.to_string(),
    }
}

fn file_url_path(url: &str) -> Option<PathBuf> {
    url.strip_prefix("file://").map(PathBuf::from)
}

/// Copies what `url` (`https://`, `http://` or `file://`) points to into `out`, refusing more than
/// `limit` bytes.
pub fn fetch_to(url: &str, limit: u64, out: &mut dyn std::io::Write) -> Result<u64, FetchError> {
    use std::io::Read;
    let mut reader: Box<dyn Read> = if let Some(path) = file_url_path(url) {
        match std::fs::File::open(&path) {
            Ok(f) => Box::new(f),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(FetchError::Unavailable(format!(
                    "{} doesn't exist",
                    path.display()
                )))
            }
            Err(e) => return Err(FetchError::Io(format!("read {}: {e}", path.display()))),
        }
    } else if url.starts_with("https://") || url.starts_with("http://") {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| FetchError::Io(e.to_string()))?;
        let resp = client.get(url).send().map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                FetchError::Offline(format!("couldn't reach {url}: {e}"))
            } else {
                FetchError::Unavailable(format!("{url}: {e}"))
            }
        })?;
        if !resp.status().is_success() {
            return Err(FetchError::Unavailable(format!(
                "{url} answered {}",
                resp.status()
            )));
        }
        if resp.content_length().is_some_and(|n| n > limit) {
            return Err(FetchError::Unavailable(format!(
                "{url} is larger than expected"
            )));
        }
        Box::new(resp)
    } else {
        return Err(FetchError::Unavailable(format!("unsupported URL {url}")));
    };
    let copied = std::io::copy(&mut (&mut reader).take(limit + 1), out)
        .map_err(|e| FetchError::Offline(format!("download of {url} failed: {e}")))?;
    if copied > limit {
        return Err(FetchError::Unavailable(format!(
            "{url} is larger than expected"
        )));
    }
    Ok(copied)
}

pub fn fetch_bytes(url: &str, limit: u64) -> Result<Vec<u8>, FetchError> {
    let mut out = Vec::new();
    fetch_to(url, limit, &mut out)?;
    Ok(out)
}

/// Why an index wasn't accepted, in the codes the status reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub code: &'static str,
    pub message: String,
}

impl SourceError {
    fn from_index(e: &keyglass_update::trust::IndexError) -> Self {
        use keyglass_update::trust::IndexError as E;
        let code = match e {
            E::Expired { .. } => "expired",
            E::Replayed { .. } => "replayed",
            E::Malformed(_) => "malformed",
            E::Store(_) => "storage",
            E::BadSignatureFormat(_)
            | E::InvalidStatement { .. }
            | E::UntrustedKey { .. }
            | E::RevokedKey { .. }
            | E::KeyIdMismatch { .. }
            | E::BadSignature(_) => "untrusted",
        };
        Self {
            code,
            message: e.to_string(),
        }
    }

    fn from_fetch(e: FetchError) -> Self {
        let code = match e {
            FetchError::Offline(_) => "offline",
            FetchError::Unavailable(_) => "unavailable",
            FetchError::Io(_) => "storage",
        };
        Self {
            code,
            message: e.message().to_string(),
        }
    }
}

/// Fetches and verifies the online index. The trust store is updated before this returns.
pub fn fetch_online_index(
    url: &str,
    anchors: &TrustAnchors,
    store: &mut FileTrustStore,
) -> Result<VerifiedIndex, SourceError> {
    let json = fetch_bytes(url, MAX_INDEX_BYTES).map_err(SourceError::from_fetch)?;
    let sig =
        fetch_bytes(&format!("{url}.sig"), MAX_SIGNATURE_BYTES).map_err(SourceError::from_fetch)?;
    let sig = String::from_utf8(sig).map_err(|_| SourceError {
        code: "untrusted",
        message: "index signature is not text".into(),
    })?;
    verify_index(&json, &sig, anchors, store, time::OffsetDateTime::now_utc())
        .map_err(|e| SourceError::from_index(&e))
}

/// Verifies the index bundled with the installer in `dir` (`index.json`, `index.json.sig`).
/// `Ok(None)` when the directory holds no index.
pub fn load_bundled_index(
    dir: &Path,
    anchors: &TrustAnchors,
    state: &TrustState,
) -> Result<Option<VerifiedIndex>, SourceError> {
    let json = match std::fs::read(dir.join("index.json")) {
        Ok(j) => j,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(SourceError {
                code: "storage",
                message: format!("read bundled index: {e}"),
            })
        }
    };
    let sig = std::fs::read_to_string(dir.join("index.json.sig")).map_err(|e| SourceError {
        code: "untrusted",
        message: format!("bundled index signature: {e}"),
    })?;
    verify_bundled_index(&json, &sig, anchors, state)
        .map(Some)
        .map_err(|e| SourceError::from_index(&e))
}

/// The verified indexes the daemon holds.
#[derive(Clone, Default)]
pub struct Sources {
    pub online: Option<(String, VerifiedIndex)>,
    pub bundled: Option<(PathBuf, VerifiedIndex)>,
}

impl Sources {
    /// Every release from both indexes. A release listed by both is withdrawn if either withdrew it.
    pub fn catalog(&self) -> Catalog {
        let mut catalog = Catalog::default();
        if let Some((_, b)) = &self.bundled {
            let _ = catalog.add_index(b);
        }
        if let Some((_, o)) = &self.online {
            let _ = catalog.add_index(o);
        }
        catalog
    }

    pub fn source_name(&self) -> &'static str {
        if self.online.is_some() {
            "online"
        } else if self.bundled.is_some() {
            "bundled"
        } else {
            "none"
        }
    }

    /// Checks `path` against `artifact` with the keys trusted under either index.
    pub fn verify_file(
        &self,
        path: &Path,
        artifact: &Artifact,
    ) -> Result<VerifiedArtifact, ArtifactError> {
        let mut last = None;
        for keys in [
            self.online.as_ref().map(|(_, i)| i.trusted_keys()),
            self.bundled.as_ref().map(|(_, i)| i.trusted_keys()),
        ]
        .into_iter()
        .flatten()
        {
            let file = std::fs::File::open(path).map_err(|e| ArtifactError::Io {
                name: artifact.name.clone(),
                reason: e.to_string(),
            })?;
            match verify_artifact(std::io::BufReader::new(file), artifact, keys) {
                Ok(v) => return Ok(v),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or(ArtifactError::UntrustedKey {
            name: artifact.name.clone(),
            key_ids: Vec::new(),
        }))
    }

    /// Places a verified copy of `artifact` in `cache` and returns its path.
    ///
    /// A cached copy or the installer's bundled copy is used when it verifies; otherwise the file
    /// is downloaded. Nothing that fails verification is kept.
    pub fn obtain(&self, cache: &Path, artifact: &Artifact) -> Result<PathBuf, ObtainError> {
        let dir = cache.join(&artifact.sha256);
        let dest = dir.join(&artifact.name);
        if dest.is_file() {
            if self.verify_file(&dest, artifact).is_ok() {
                return Ok(dest);
            }
            let _ = std::fs::remove_file(&dest);
        }
        std::fs::create_dir_all(&dir).map_err(|e| {
            ObtainError::Fetch(FetchError::Io(format!("create {}: {e}", dir.display())))
        })?;
        let part = dir.join(format!("{}.part", artifact.name));
        let mut fetched = false;
        if let Some((bdir, _)) = &self.bundled {
            let local = bdir.join(&artifact.name);
            if local.is_file() && std::fs::copy(&local, &part).is_ok() {
                fetched = self.verify_file(&part, artifact).is_ok();
            }
        }
        if !fetched {
            let url = self.artifact_url(artifact);
            let mut f = std::fs::File::create(&part).map_err(|e| {
                ObtainError::Fetch(FetchError::Io(format!("create {}: {e}", part.display())))
            })?;
            let res = fetch_to(&url, artifact.size, &mut f);
            drop(f);
            if let Err(e) = res {
                let _ = std::fs::remove_file(&part);
                return Err(ObtainError::Fetch(e));
            }
            if let Err(e) = self.verify_file(&part, artifact) {
                let _ = std::fs::remove_file(&part);
                return Err(ObtainError::Verification(e));
            }
        }
        std::fs::rename(&part, &dest).map_err(|e| {
            ObtainError::Fetch(FetchError::Io(format!("store {}: {e}", dest.display())))
        })?;
        Ok(dest)
    }

    fn artifact_url(&self, artifact: &Artifact) -> String {
        if let Some((url, _)) = &self.online {
            return resolve_url(url, &artifact.url);
        }
        if let Some((dir, _)) = &self.bundled {
            let base = format!("file://{}/index.json", dir.display());
            return resolve_url(&base, &artifact.url);
        }
        artifact.url.clone()
    }
}

/// Why an artifact couldn't be made available.
#[derive(Debug)]
pub enum ObtainError {
    Fetch(FetchError),
    Verification(ArtifactError),
}

impl std::fmt::Display for ObtainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObtainError::Fetch(e) => f.write_str(e.message()),
            ObtainError::Verification(e) => write!(f, "{e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_artifact_urls_resolve_against_the_index() {
        assert_eq!(
            resolve_url("https://h/a/index.json", "x.ipk"),
            "https://h/a/x.ipk"
        );
        assert_eq!(
            resolve_url("https://h/a/index.json", "https://o/x.ipk"),
            "https://o/x.ipk"
        );
        assert_eq!(
            resolve_url("file:///tmp/i/index.json", "./x.uf2"),
            "file:///tmp/i/x.uf2"
        );
    }

    #[test]
    fn the_trust_store_starts_empty_and_persists() {
        let dir = std::env::temp_dir().join(format!("keyglassd_trust_{}", std::process::id()));
        let path = dir.join("trust.json");
        let mut store = FileTrustStore::new(path.clone());
        assert_eq!(store.load().unwrap(), TrustState::default());
        let state = TrustState {
            highest_sequence: 7,
            ..Default::default()
        };
        store.save(&state).unwrap();
        assert_eq!(FileTrustStore::new(path).load().unwrap(), state);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
