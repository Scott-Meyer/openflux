//! The factory reference: the keyboard's factory software, for resetting a keyboard to it.
//!
//! The signed index records where the factory software is published (a ZIP64 container), which
//! members of it hold the firmware, bridge and display, and the size and SHA-256 of each. OpenFlux
//! doesn't host these files. The daemon reads only the members it needs, with HTTP range
//! requests (the central directory, then each member), and downloads the whole container when the
//! server doesn't support ranges. Every file is checked against the signed index before use.

use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `User-Agent` of the daemon's requests for factory files.
pub fn user_agent() -> String {
    format!("OpenFlux/{} (openfluxd)", env!("CARGO_PKG_VERSION"))
}

pub use openflux_update::{FactoryFile, FactoryReference};

/// A factory file's name without its path in the container.
pub fn file_name(file: &FactoryFile) -> &str {
    file.member.rsplit('/').next().unwrap_or(&file.member)
}

fn safe_name(file: &FactoryFile) -> bool {
    let n = file_name(file);
    !n.is_empty() && !n.contains('\\') && n != "." && n != ".."
}

/// Why a factory file couldn't be obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactoryError {
    Offline(String),
    Unavailable(String),
    Verification(String),
    Storage(String),
}

impl std::fmt::Display for FactoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FactoryError::Offline(m)
            | FactoryError::Unavailable(m)
            | FactoryError::Verification(m)
            | FactoryError::Storage(m) => f.write_str(m),
        }
    }
}

fn client() -> Result<reqwest::blocking::Client, FactoryError> {
    reqwest::blocking::Client::builder()
        .user_agent(user_agent())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(|e| FactoryError::Unavailable(format!("the HTTP client couldn't be set up: {e}")))
}

fn request_error(url: &str, e: reqwest::Error) -> FactoryError {
    if e.is_connect() || e.is_timeout() {
        FactoryError::Offline(format!("couldn't reach {url}: {e}"))
    } else {
        FactoryError::Unavailable(format!("{url}: {e}"))
    }
}

/// The server doesn't answer range requests with partial content.
const NO_RANGES: &str = "the server doesn't support range requests";

/// Reads a remote file through HTTP range requests, in blocks.
struct RangeReader {
    client: reqwest::blocking::Client,
    url: String,
    size: u64,
    pos: u64,
    block: Option<(u64, Vec<u8>)>,
}

const BLOCK: u64 = 256 * 1024;

impl RangeReader {
    fn fetch(&self, start: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let end = (start + len).min(self.size) - 1;
        let resp = self
            .client
            .get(&self.url)
            .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
            .send()
            .map_err(|e| std::io::Error::other(request_error(&self.url, e).to_string()))?;
        if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(std::io::Error::other(NO_RANGES));
        }
        let total = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|t| t.parse::<u64>().ok());
        if total != Some(self.size) {
            return Err(std::io::Error::other(
                "the factory container's size differs from the signed index",
            ));
        }
        let body = resp
            .bytes()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        if body.len() as u64 != end - start + 1 {
            return Err(std::io::Error::other("short range response"));
        }
        Ok(body.to_vec())
    }
}

impl Read for RangeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let cached = self
            .block
            .as_ref()
            .is_some_and(|(s, b)| self.pos >= *s && self.pos < s + b.len() as u64);
        if !cached {
            let want = (buf.len() as u64).max(BLOCK);
            let data = self.fetch(self.pos, want)?;
            self.block = Some((self.pos, data));
        }
        let (start, data) = self.block.as_ref().expect("filled above");
        let off = (self.pos - start) as usize;
        let n = buf.len().min(data.len() - off);
        buf[..n].copy_from_slice(&data[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for RangeReader {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.size as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if p < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

/// Copies `member` out of the archive into `dest`, checking its size and SHA-256.
fn extract<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    file: &FactoryFile,
    dest: &Path,
) -> Result<(), FactoryError> {
    let mut entry = archive.by_name(&file.member).map_err(|e| {
        FactoryError::Verification(format!(
            "{} isn't in the factory container: {e}",
            file.member
        ))
    })?;
    if entry.size() != file.size {
        return Err(FactoryError::Verification(format!(
            "{} is {} bytes, the signed index says {}",
            file.member,
            entry.size(),
            file.size
        )));
    }
    let part = dest.with_extension("part");
    let mut out = std::fs::File::create(&part)
        .map_err(|e| FactoryError::Storage(format!("create {}: {e}", part.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = entry.read(&mut buf).map_err(|e| {
            if e.to_string().contains(NO_RANGES) {
                FactoryError::Unavailable(NO_RANGES.into())
            } else if matches!(
                e.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
            ) {
                // The container's data is corrupt (bad compressed data or checksum).
                FactoryError::Verification(format!("{} is corrupt: {e}", file.member))
            } else {
                FactoryError::Offline(format!("reading {} failed: {e}", file.member))
            }
        })?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > file.size {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])
            .map_err(|e| FactoryError::Storage(e.to_string()))?;
    }
    drop(out);
    let sha = format!("{:x}", hasher.finalize());
    if total != file.size || sha != file.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(FactoryError::Verification(format!(
            "{} doesn't match the signed index (size {total}, sha256 {sha})",
            file.member
        )));
    }
    std::fs::rename(&part, dest).map_err(|e| FactoryError::Storage(e.to_string()))
}

/// Checks a cached factory file against the signed index.
pub fn verify_file(path: &Path, file: &FactoryFile) -> Result<(), FactoryError> {
    let f = std::fs::File::open(path)
        .map_err(|e| FactoryError::Storage(format!("read {}: {e}", path.display())))?;
    let (sha, size) = openflux_update::trust::sha256_hex(std::io::BufReader::new(f))
        .map_err(|e| FactoryError::Storage(e.to_string()))?;
    if size != file.size || sha != file.sha256 {
        return Err(FactoryError::Verification(format!(
            "{} doesn't match the signed index",
            file_name(file)
        )));
    }
    Ok(())
}

/// Where a factory file is kept once verified.
pub fn cached_path(cache: &Path, file: &FactoryFile) -> PathBuf {
    cache
        .join("factory")
        .join(&file.sha256)
        .join(file_name(file))
}

/// A published ZIP container to read files from.
#[derive(Debug, Clone, Copy)]
pub struct Container<'a> {
    pub url: &'a str,
    pub size: u64,
    /// SHA-256 of the whole container, checked when it is downloaded whole.
    pub sha256: Option<&'a str>,
}

/// Places a verified copy of `file`, a member of `container`, under `dir` and returns its path
/// ([`cached_path`]). A copy already there is checked again and reused.
pub fn obtain_member(
    container: Container<'_>,
    file: &FactoryFile,
    dir: &Path,
) -> Result<PathBuf, FactoryError> {
    if !safe_name(file) {
        return Err(FactoryError::Verification(format!(
            "{} isn't a usable file name",
            file.member
        )));
    }
    let path = cached_path(dir, file);
    if path.is_file() && verify_file(&path, file).is_ok() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| FactoryError::Storage(format!("create {}: {e}", parent.display())))?;
    }
    let files = std::slice::from_ref(file);
    match fetch_with_ranges(container, files, dir) {
        Err(FactoryError::Unavailable(m)) if m == NO_RANGES => fetch_whole(container, files, dir)?,
        other => other?,
    }
    Ok(path)
}

/// Copies the member `file` out of the ZIP file at `zip_path` into `dest`, checking its size and
/// SHA-256.
pub fn extract_from_zip_file(
    zip_path: &Path,
    file: &FactoryFile,
    dest: &Path,
) -> Result<(), FactoryError> {
    let f = std::fs::File::open(zip_path)
        .map_err(|e| FactoryError::Storage(format!("read {}: {e}", zip_path.display())))?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(|e| {
        FactoryError::Verification(format!(
            "{} isn't a readable package: {e}",
            zip_path.display()
        ))
    })?;
    extract(&mut archive, file, dest)
}

/// Places verified copies of every factory file in `cache` and returns their paths, in the
/// reference's order. Cached copies are checked again and reused.
///
/// `container_url` replaces the reference's URL, for a mirror; the files are still checked
/// against the signed index.
pub fn obtain_all(
    reference: &FactoryReference,
    container_url: Option<&str>,
    cache: &Path,
) -> Result<Vec<PathBuf>, FactoryError> {
    if let Some(bad) = reference.files.iter().find(|f| !safe_name(f)) {
        return Err(FactoryError::Verification(format!(
            "{} isn't a usable file name",
            bad.member
        )));
    }
    let mut reference = reference.clone();
    if let Some(url) = container_url {
        reference.container_url = url.to_string();
    }
    let reference = &reference;
    let mut missing = Vec::new();
    for f in &reference.files {
        let p = cached_path(cache, f);
        if !(p.is_file() && verify_file(&p, f).is_ok()) {
            missing.push(f.clone());
        }
    }
    if !missing.is_empty() {
        for f in &missing {
            let dir = cached_path(cache, f)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            std::fs::create_dir_all(&dir)
                .map_err(|e| FactoryError::Storage(format!("create {}: {e}", dir.display())))?;
        }
        let container = Container {
            url: &reference.container_url,
            size: reference.container_size,
            sha256: None,
        };
        match fetch_with_ranges(container, &missing, cache) {
            Err(FactoryError::Unavailable(m)) if m == NO_RANGES => {
                fetch_whole(container, &missing, cache)?
            }
            other => other?,
        }
    }
    Ok(reference
        .files
        .iter()
        .map(|f| cached_path(cache, f))
        .collect())
}

/// An error with every underlying cause.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        text.push_str(": ");
        text.push_str(&c.to_string());
        cur = c.source();
    }
    text
}

fn fetch_with_ranges(
    container: Container<'_>,
    files: &[FactoryFile],
    cache: &Path,
) -> Result<(), FactoryError> {
    let reader = RangeReader {
        client: client()?,
        url: container.url.to_string(),
        size: container.size,
        pos: 0,
        block: None,
    };
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| {
        let text = error_chain(&e);
        if text.contains(NO_RANGES) {
            FactoryError::Unavailable(NO_RANGES.into())
        } else if text.contains("couldn't reach") {
            FactoryError::Offline(text)
        } else {
            FactoryError::Verification(format!("the factory container couldn't be read: {text}"))
        }
    })?;
    for f in files {
        extract(&mut archive, f, &cached_path(cache, f))?;
    }
    Ok(())
}

/// Hashes what passes through it.
struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn fetch_whole(
    container: Container<'_>,
    files: &[FactoryFile],
    cache: &Path,
) -> Result<(), FactoryError> {
    let url = container.url;
    let resp = client()?
        .get(url)
        .send()
        .map_err(|e| request_error(url, e))?;
    if !resp.status().is_success() {
        return Err(FactoryError::Unavailable(format!(
            "{url} answered {}",
            resp.status()
        )));
    }
    let tmp = cache.join("factory").join("container.part");
    if let Some(d) = tmp.parent() {
        std::fs::create_dir_all(d).map_err(|e| FactoryError::Storage(e.to_string()))?;
    }
    let out = std::fs::File::create(&tmp)
        .map_err(|e| FactoryError::Storage(format!("create {}: {e}", tmp.display())))?;
    let mut out = HashingWriter {
        inner: out,
        hasher: Sha256::new(),
    };
    let copied = std::io::copy(&mut resp.take(container.size + 1), &mut out)
        .map_err(|e| FactoryError::Offline(format!("download of {url} failed: {e}")))?;
    let sha = format!("{:x}", out.hasher.finalize());
    drop(out.inner);
    let result = if copied != container.size {
        Err(FactoryError::Verification(
            "the factory container's size differs from the signed index".into(),
        ))
    } else if container.sha256.is_some_and(|want| want != sha) {
        Err(FactoryError::Verification(format!(
            "the downloaded container has SHA-256 {sha}, the signed index says {}",
            container.sha256.unwrap_or_default()
        )))
    } else {
        let f = std::fs::File::open(&tmp).map_err(|e| FactoryError::Storage(e.to_string()))?;
        zip::ZipArchive::new(std::io::BufReader::new(f))
            .map_err(|e| {
                FactoryError::Verification(format!("the factory container couldn't be read: {e}"))
            })
            .and_then(|mut archive| {
                files
                    .iter()
                    .try_for_each(|f| extract(&mut archive, f, &cached_path(cache, f)))
            })
    };
    let _ = std::fs::remove_file(&tmp);
    result
}
