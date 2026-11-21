//! Press-refinement data: the keyboard maker's press-refinement coefficients. The OpenFlux
//! firmware uses them to refine Hall readings; a build without press refinement only stores
//! them.
//!
//! The coefficients belong to the keyboard maker, so OpenFlux never hosts or ships them. The
//! user's own daemon reads them out of the maker's software, following the press-refinement source
//! the signed index records ([`PressRefinementSource`], from `factory-reference.toml`):
//!
//! 1. the source member (a firmware UF2) comes from one of its locations: the maker's published container,
//!    read with HTTP range requests (or downloaded whole and checked against its SHA-256), or a
//!    copy the user already has, either the package or the bare UF2;
//! 2. the member is checked against its size and SHA-256;
//! 3. its blocks are placed at their RP2040 addresses into one contiguous image, checked against
//!    its size and SHA-256;
//! 4. the payload is the image's payload ranges in order, checked against its SHA-256.
//!
//! The verified payload is kept in the daemon's data directory under its SHA-256, so installing
//! again works offline. It is installed with the firmware's press-refinement commands (see
//! [`crate::usb::press_refinement`]) and confirmed by reading the status and the record back.

use super::factory::{self, Container, FactoryError, FactoryFile};
use crate::api::state::DeviceSink;
use crate::usb::press_refinement::{self as record, State, Status};
pub use openflux_update::{PressRefinementSource, SourceLocation, SourceRange};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

/// Why press-refinement data couldn't be obtained or installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The release index doesn't record where the data is found.
    NotAvailable(String),
    /// The maker's published software couldn't be reached.
    Offline(String),
    /// The maker's server answered, but not with the published software.
    Unavailable(String),
    /// A file, the image or the payload doesn't match its recorded SHA-256 or size.
    HashMismatch(String),
    /// A file the user chose can't be read, or is neither a package nor a UF2.
    File(String),
    /// The data directory couldn't be read or written.
    Storage(String),
    /// The keyboard firmware isn't connected.
    KeyboardDisconnected(String),
    /// The keyboard firmware refused a press-refinement command or didn't answer it.
    KeyboardRefused(String),
    /// The keyboard reports that its press-refinement storage can't be used.
    StorageUnavailable(String),
    /// The keyboard accepted the data but its status or read-back doesn't match.
    VerifyFailed(String),
}

impl Error {
    /// The API error code.
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotAvailable(_) => "notAvailable",
            Error::Offline(_) => "offline",
            Error::Unavailable(_) => "unavailable",
            Error::HashMismatch(_) => "hashMismatch",
            Error::File(_) => "fileUnusable",
            Error::Storage(_) => "storage",
            Error::KeyboardDisconnected(_) => "keyboardDisconnected",
            Error::KeyboardRefused(_) => "keyboardRefused",
            Error::StorageUnavailable(_) => "storageUnavailable",
            Error::VerifyFailed(_) => "verifyFailed",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Error::NotAvailable(m)
            | Error::Offline(m)
            | Error::Unavailable(m)
            | Error::HashMismatch(m)
            | Error::File(m)
            | Error::Storage(m)
            | Error::KeyboardDisconnected(m)
            | Error::KeyboardRefused(m)
            | Error::StorageUnavailable(m)
            | Error::VerifyFailed(m) => m,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl From<FactoryError> for Error {
    fn from(e: FactoryError) -> Self {
        match e {
            FactoryError::Offline(m) => Error::Offline(m),
            FactoryError::Unavailable(m) => Error::Unavailable(m),
            FactoryError::Verification(m) => Error::HashMismatch(m),
            FactoryError::Storage(m) => Error::Storage(m),
        }
    }
}

const UF2_MAGIC_START0: u32 = 0x0A32_4655;
const UF2_MAGIC_START1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
const UF2_FLAG_NOT_MAIN_FLASH: u32 = 0x0000_0001;
const UF2_FLAG_FAMILY: u32 = 0x0000_2000;
const RP2040_FAMILY_ID: u32 = 0xE48B_FF56;

/// Places a UF2's blocks at their RP2040 target addresses and returns the contiguous image that
/// starts at `base`. Blocks marked as not for main flash are skipped. The blocks must cover the
/// image without gaps or overlaps.
pub fn assemble_uf2(uf2: &[u8], base: u32) -> Result<Vec<u8>, String> {
    if uf2.is_empty() || !uf2.len().is_multiple_of(512) {
        return Err(format!(
            "the UF2 is {} bytes, not a whole number of 512-byte blocks",
            uf2.len()
        ));
    }
    let le = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let mut pieces: Vec<(u32, &[u8])> = Vec::new();
    for (i, blk) in uf2.chunks_exact(512).enumerate() {
        if le(blk, 0) != UF2_MAGIC_START0
            || le(blk, 4) != UF2_MAGIC_START1
            || le(blk, 508) != UF2_MAGIC_END
        {
            return Err(format!("UF2 block {i} has no UF2 magic"));
        }
        let flags = le(blk, 8);
        if flags & UF2_FLAG_NOT_MAIN_FLASH != 0 {
            continue;
        }
        if flags & UF2_FLAG_FAMILY != 0 && le(blk, 28) != RP2040_FAMILY_ID {
            return Err(format!("UF2 block {i} isn't for the RP2040"));
        }
        let addr = le(blk, 12);
        let size = le(blk, 16) as usize;
        if size == 0 || size > 476 {
            return Err(format!("UF2 block {i} has a payload of {size} bytes"));
        }
        let offset = addr
            .checked_sub(base)
            .ok_or_else(|| format!("UF2 block {i} at 0x{addr:08x} is below 0x{base:08x}"))?;
        pieces.push((offset, &blk[32..32 + size]));
    }
    pieces.sort_by_key(|(o, _)| *o);
    let mut image = Vec::new();
    for (offset, data) in pieces {
        let at = offset as usize;
        if at != image.len() {
            return Err(if at < image.len() {
                format!("UF2 blocks overlap at 0x{:08x}", base as u64 + at as u64)
            } else {
                format!(
                    "the UF2 has no data at 0x{:08x}",
                    base as u64 + image.len() as u64
                )
            });
        }
        image.extend_from_slice(data);
    }
    if image.is_empty() {
        return Err("the UF2 has no flash blocks".into());
    }
    Ok(image)
}

/// Assembles the source UF2, checks the image, and returns the verified payload.
pub fn payload_from_uf2(uf2: &[u8], refinement: &PressRefinementSource) -> Result<Vec<u8>, Error> {
    let image = assemble_uf2(uf2, refinement.image_base).map_err(Error::HashMismatch)?;
    let image_sha = sha256_hex(&image);
    if image.len() as u64 != refinement.image_size || image_sha != refinement.image_sha256 {
        return Err(Error::HashMismatch(format!(
            "the firmware image is {} bytes with SHA-256 {image_sha}; {} bytes with {} expected",
            image.len(),
            refinement.image_size,
            refinement.image_sha256
        )));
    }
    let mut payload = Vec::with_capacity(refinement.payload_length as usize);
    for r in &refinement.payload_ranges {
        let start = r.offset as usize;
        let end = start + r.length as usize;
        let part = image.get(start..end).ok_or_else(|| {
            Error::HashMismatch(format!(
                "range 0x{:x}+{} is outside the image",
                r.offset, r.length
            ))
        })?;
        payload.extend_from_slice(part);
    }
    let sha = sha256_hex(&payload);
    if payload.len() as u64 != refinement.payload_length || sha != refinement.payload_sha256 {
        return Err(Error::HashMismatch(format!(
            "the press-refinement data has SHA-256 {sha}; {} expected",
            refinement.payload_sha256
        )));
    }
    Ok(payload)
}

/// Where the verified payload is kept.
pub fn cached_path(store: &Path, refinement: &PressRefinementSource) -> PathBuf {
    store.join(format!("{}.bin", refinement.payload_sha256))
}

/// The verified payload and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obtained {
    pub payload: Vec<u8>,
    /// Read from the data directory rather than downloaded or read from a chosen file.
    pub from_cache: bool,
}

fn kept(refinement: &PressRefinementSource, store: &Path) -> Option<Vec<u8>> {
    let cached = cached_path(store, refinement);
    let raw = std::fs::read(&cached).ok()?;
    if raw.len() as u64 == refinement.payload_length
        && sha256_hex(&raw) == refinement.payload_sha256
    {
        return Some(raw);
    }
    let _ = std::fs::remove_file(&cached);
    None
}

fn keep(
    refinement: &PressRefinementSource,
    store: &Path,
    payload: Vec<u8>,
) -> Result<Obtained, Error> {
    crate::updates::source::write_durably(&cached_path(store, refinement), &payload)
        .map_err(Error::Storage)?;
    Ok(Obtained {
        payload,
        from_cache: false,
    })
}

/// The source member as a factory file, for the container reader.
fn member_file(refinement: &PressRefinementSource) -> FactoryFile {
    FactoryFile {
        component: openflux_update::Component::Firmware,
        member: refinement.member.clone(),
        version: refinement.description.clone(),
        size: refinement.member_size,
        sha256: refinement.member_sha256.clone(),
    }
}

/// The verified payload: the kept copy when it still matches, otherwise read from the source's
/// locations in order, verified and kept. `mirror` replaces every container location's URL; the
/// hashes are checked either way. Locations of kinds this version doesn't know are skipped.
pub fn obtain_payload(
    refinement: &PressRefinementSource,
    mirror: Option<&str>,
    store: &Path,
) -> Result<Obtained, Error> {
    if let Some(payload) = kept(refinement, store) {
        return Ok(Obtained {
            payload,
            from_cache: true,
        });
    }
    let mut last = Error::NotAvailable(
        "The release index lists no source of press-refinement data this OpenFlux can use".into(),
    );
    for src in &refinement.sources {
        let SourceLocation::Container { url, size, sha256 } = src else {
            continue;
        };
        let container = Container {
            url: mirror.unwrap_or(url),
            size: *size,
            sha256: Some(sha256),
        };
        let work = store.join(format!("download-{}", std::process::id()));
        let result = factory::obtain_member(container, &member_file(refinement), &work)
            .map_err(Error::from)
            .and_then(|path| {
                std::fs::read(&path)
                    .map_err(|e| Error::Storage(format!("read {}: {e}", path.display())))
            })
            .and_then(|uf2| payload_from_uf2(&uf2, refinement));
        let _ = std::fs::remove_dir_all(&work);
        match result {
            Ok(payload) => return keep(refinement, store, payload),
            // A file that doesn't match is final; another source can't make it match.
            Err(e @ Error::HashMismatch(_)) | Err(e @ Error::Storage(_)) => return Err(e),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// The UF2 magic numbers at the start of every block.
const UF2_START: [u8; 8] = [0x55, 0x46, 0x32, 0x0A, 0x57, 0x51, 0x5D, 0x9E];

/// The verified payload from a file the user chose: the maker's package (a ZIP container such as
/// the MSIX) holding the source member, or the bare UF2. The member, image and payload hashes are
/// checked as for a download, and the payload is kept.
pub fn payload_from_file(
    refinement: &PressRefinementSource,
    path: &Path,
    store: &Path,
) -> Result<Obtained, Error> {
    let mut head = [0u8; 8];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(path)
            .map_err(|e| Error::File(format!("{} can't be read: {e}", path.display())))?;
        f.read_exact(&mut head).map_err(|_| {
            Error::File(format!(
                "{} is too short to be a package or a UF2",
                path.display()
            ))
        })?;
    }
    let uf2 = if head.starts_with(b"PK\x03\x04") {
        let work = store.join(format!("file-{}", std::process::id()));
        std::fs::create_dir_all(&work)
            .map_err(|e| Error::Storage(format!("create {}: {e}", work.display())))?;
        let dest = work.join("member.uf2");
        let result = factory::extract_from_zip_file(path, &member_file(refinement), &dest)
            .map_err(Error::from)
            .and_then(|()| {
                std::fs::read(&dest)
                    .map_err(|e| Error::Storage(format!("read {}: {e}", dest.display())))
            });
        let _ = std::fs::remove_dir_all(&work);
        result?
    } else if head == UF2_START {
        let uf2 = std::fs::read(path)
            .map_err(|e| Error::File(format!("{} can't be read: {e}", path.display())))?;
        let sha = sha256_hex(&uf2);
        if uf2.len() as u64 != refinement.member_size || sha != refinement.member_sha256 {
            return Err(Error::HashMismatch(format!(
                "{} is {} bytes with SHA-256 {sha}; {} bytes with {} expected",
                path.display(),
                uf2.len(),
                refinement.member_size,
                refinement.member_sha256
            )));
        }
        uf2
    } else {
        return Err(Error::File(format!(
            "{} is neither a software package nor a UF2 firmware file",
            path.display()
        )));
    };
    let payload = payload_from_uf2(&uf2, refinement)?;
    keep(refinement, store, payload)
}

/// What an install did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    /// CRC-32 of the installed payload, as the keyboard reports it.
    pub crc32: u32,
    /// The firmware applies the data; a firmware build that only stores it reports `false`.
    pub active: bool,
    /// The keyboard already had exactly this data, so nothing was written.
    pub already_installed: bool,
    /// The payload came from the data directory rather than a download.
    pub from_cache: bool,
}

fn keyboard_error(device: &dyn DeviceSink, what: &str, e: String) -> Error {
    if !device.is_connected().1 {
        Error::KeyboardDisconnected("The keyboard firmware isn't connected".into())
    } else {
        Error::KeyboardRefused(format!("{what}: {e}"))
    }
}

/// Reads the keyboard's press-refinement status.
pub fn status(device: &dyn DeviceSink) -> Result<Status, Error> {
    device.press_refinement_status().map_err(|e| {
        keyboard_error(
            device,
            "The keyboard firmware didn't answer the press-refinement status",
            e,
        )
    })
}

/// Writes `payload` to the keyboard and confirms it: the status must report it installed with the
/// payload's CRC-32, and the record must read back byte for byte. A keyboard that already holds
/// exactly this record isn't written.
pub fn install_payload(device: &dyn DeviceSink, payload: &[u8]) -> Result<Installed, Error> {
    let rec = record::build_record(payload).map_err(Error::HashMismatch)?;
    let crc = record::crc32(payload);
    let before = status(device)?;
    if before.state == State::StorageUnavailable {
        return Err(Error::StorageUnavailable(
            "The keyboard's press-refinement storage is unavailable".into(),
        ));
    }
    let read_back = |device: &dyn DeviceSink| device.get_press_refinement();
    if before.state.is_installed()
        && before.payload_crc32 == crc
        && read_back(device).is_ok_and(|r| r == rec)
    {
        return Ok(Installed {
            crc32: crc,
            active: before.state == State::Installed,
            already_installed: true,
            from_cache: false,
        });
    }
    device
        .put_press_refinement(&rec)
        .map_err(|e| keyboard_error(device, "The keyboard refused the press-refinement data", e))?;
    let after = status(device)?;
    match after.state {
        state if state.is_installed() && after.payload_crc32 == crc => {}
        State::StorageUnavailable => {
            return Err(Error::StorageUnavailable(
                "The keyboard's press-refinement storage became unavailable while it was written"
                    .into(),
            ))
        }
        state => {
            return Err(Error::VerifyFailed(format!(
            "After the write the keyboard reports {state:?} with CRC-32 {:08x}; {crc:08x} expected",
            after.payload_crc32
        )))
        }
    }
    let back = read_back(device).map_err(|e| {
        Error::VerifyFailed(format!(
            "The keyboard's press-refinement data couldn't be read back: {e}"
        ))
    })?;
    if back != rec {
        return Err(Error::VerifyFailed(
            "The keyboard's press-refinement data reads back differently".into(),
        ));
    }
    Ok(Installed {
        crc32: crc,
        active: after.state == State::Installed,
        already_installed: false,
        from_cache: false,
    })
}

/// Installs an obtained payload (see [`install_payload`]).
pub fn install_obtained(device: &dyn DeviceSink, obtained: Obtained) -> Result<Installed, Error> {
    let mut done = install_payload(device, &obtained.payload)?;
    done.from_cache = obtained.from_cache;
    Ok(done)
}

/// Where to obtain the payload from.
#[derive(Debug, Clone, Copy)]
pub enum Source<'a> {
    /// The kept copy, otherwise the source's published locations.
    Published,
    /// A file the user chose: the maker's package or the bare UF2.
    File(&'a Path),
}

/// The verified payload for the press-refinement source the verified index records, from `source`.
pub fn obtain(svc: &crate::updates::UpdateService, source: Source<'_>) -> Result<Obtained, Error> {
    let refinement = svc.press_refinement_source().ok_or_else(|| {
        Error::NotAvailable(
            "The release index doesn't say where press-refinement data is found; check for updates"
                .into(),
        )
    })?;
    let store = svc.press_refinement_dir();
    match source {
        Source::Published => {
            obtain_payload(&refinement, svc.press_refinement_container_url(), &store)
        }
        Source::File(path) => payload_from_file(&refinement, path, &store),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::usb::press_refinement::FirmwareStore;
    use crate::usb::press_refinement::PAYLOAD_LEN;
    use std::sync::Mutex;

    pub(crate) fn uf2_block(addr: u32, data: &[u8], flags: u32) -> [u8; 512] {
        let mut b = [0u8; 512];
        b[0..4].copy_from_slice(&UF2_MAGIC_START0.to_le_bytes());
        b[4..8].copy_from_slice(&UF2_MAGIC_START1.to_le_bytes());
        b[8..12].copy_from_slice(&(flags | UF2_FLAG_FAMILY).to_le_bytes());
        b[12..16].copy_from_slice(&addr.to_le_bytes());
        b[16..20].copy_from_slice(&(data.len() as u32).to_le_bytes());
        b[28..32].copy_from_slice(&RP2040_FAMILY_ID.to_le_bytes());
        b[32..32 + data.len()].copy_from_slice(data);
        b[508..512].copy_from_slice(&UF2_MAGIC_END.to_le_bytes());
        b
    }

    /// A synthetic firmware image, its UF2 with the blocks out of order, and a press-refinement
    /// source that describes them.
    pub(crate) fn synthetic() -> (Vec<u8>, Vec<u8>, PressRefinementSource) {
        let image: Vec<u8> = (0..16 * 256u32)
            .map(|i| (i.wrapping_mul(31) ^ (i >> 8)) as u8)
            .collect();
        let mut uf2 = Vec::new();
        for page in (0..16u32).rev() {
            let at = page as usize * 256;
            uf2.extend_from_slice(&uf2_block(
                0x1000_0000 + page * 256,
                &image[at..at + 256],
                0,
            ));
        }
        let mut payload = image[0x200..0x200 + 1452].to_vec();
        payload.extend_from_slice(&image[0x10..0x14]);
        let refinement = PressRefinementSource {
            description: "synthetic".into(),
            sources: vec![SourceLocation::Container {
                url: "https://127.0.0.1:9/none.zip".into(),
                size: 1_000_000,
                sha256: "0".repeat(64),
            }],
            member: "assets/firmware/synthetic.uf2".into(),
            member_size: uf2.len() as u64,
            member_sha256: sha256_hex(&uf2),
            image_base: 0x1000_0000,
            image_size: image.len() as u64,
            image_sha256: sha256_hex(&image),
            payload_ranges: vec![
                SourceRange {
                    offset: 0x200,
                    length: 1452,
                },
                SourceRange {
                    offset: 0x10,
                    length: 4,
                },
            ],
            payload_length: PAYLOAD_LEN as u64,
            payload_sha256: sha256_hex(&payload),
        };
        refinement.validate().unwrap();
        (uf2, payload, refinement)
    }

    #[test]
    fn uf2_blocks_are_placed_by_address() {
        let (uf2, _, refinement) = synthetic();
        let image = assemble_uf2(&uf2, refinement.image_base).unwrap();
        assert_eq!(image.len() as u64, refinement.image_size);
        assert_eq!(sha256_hex(&image), refinement.image_sha256);

        // Blocks for other memories are skipped.
        let mut with_extra = uf2.clone();
        with_extra.extend_from_slice(&uf2_block(0x2000_0000, &[9u8; 256], 1));
        assert_eq!(assemble_uf2(&with_extra, 0x1000_0000).unwrap(), image);

        // A missing page, an overlap, foreign families and stray bytes are refused.
        assert!(assemble_uf2(&uf2[512..], 0x1000_0000).is_ok());
        let mut gap = uf2[..512].to_vec();
        gap.extend_from_slice(&uf2[1024..]);
        assert!(assemble_uf2(&gap, 0x1000_0000)
            .unwrap_err()
            .contains("no data"));
        let mut overlap = uf2.clone();
        overlap.extend_from_slice(&uf2[..512]);
        assert!(assemble_uf2(&overlap, 0x1000_0000)
            .unwrap_err()
            .contains("overlap"));
        let mut foreign = uf2.clone();
        foreign[28] ^= 1;
        assert!(assemble_uf2(&foreign, 0x1000_0000)
            .unwrap_err()
            .contains("RP2040"));
        assert!(assemble_uf2(&uf2[..511], 0x1000_0000).is_err());
        assert!(assemble_uf2(&uf2, 0x1000_1000).is_err());
    }

    #[test]
    fn payload_is_the_payload_ranges_in_order_and_every_hash_is_checked() {
        let (uf2, payload, refinement) = synthetic();
        assert_eq!(payload_from_uf2(&uf2, &refinement).unwrap(), payload);

        let mut other_image = uf2.clone();
        other_image[40] ^= 1;
        assert!(matches!(
            payload_from_uf2(&other_image, &refinement),
            Err(Error::HashMismatch(m)) if m.contains("firmware image")
        ));
        let mut wrong_payload = refinement.clone();
        wrong_payload.payload_sha256 = sha256_hex(b"other");
        assert!(matches!(
            payload_from_uf2(&uf2, &wrong_payload),
            Err(Error::HashMismatch(m)) if m.contains("press-refinement data")
        ));
        // Missing the first page makes the image too short.
        assert!(matches!(
            payload_from_uf2(&uf2[..uf2.len() - 512], &refinement),
            Err(Error::HashMismatch(_))
        ));
    }

    fn temp_store(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "openfluxd_press_refinement_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_kept_payload_is_used_offline_and_a_damaged_one_is_not() {
        let (_, payload, refinement) = synthetic();
        let store = temp_store("cache");
        // Nothing listens at the container URL, so a download fails as offline.
        assert!(matches!(
            obtain_payload(&refinement, None, &store),
            Err(Error::Offline(_) | Error::Unavailable(_))
        ));
        let no_sources = PressRefinementSource {
            sources: vec![SourceLocation::Unrecognized],
            ..refinement.clone()
        };
        assert!(matches!(
            obtain_payload(&no_sources, None, &store),
            Err(Error::NotAvailable(_))
        ));
        std::fs::write(cached_path(&store, &refinement), &payload).unwrap();
        let got = obtain_payload(&refinement, None, &store).unwrap();
        assert!(got.from_cache);
        assert_eq!(got.payload, payload);

        let mut damaged = payload.clone();
        damaged[0] ^= 1;
        std::fs::write(cached_path(&store, &refinement), &damaged).unwrap();
        assert!(obtain_payload(&refinement, None, &store).is_err());
        assert!(!cached_path(&store, &refinement).exists());
        let _ = std::fs::remove_dir_all(&store);
    }

    /// A keyboard with simulated press-refinement storage, which can misbehave after a write.
    #[derive(Default)]
    struct Keyboard {
        fw: Mutex<FirmwareStore>,
        connected: bool,
        /// After a write, the status reports this instead.
        status_after_put: Option<Status>,
        /// After a write, the stored record has one byte changed (the status still matches).
        corrupt_read_back: bool,
        refuse_put: bool,
        puts: Mutex<usize>,
    }

    impl DeviceSink for Keyboard {
        fn is_connected(&self) -> (bool, bool) {
            (false, self.connected)
        }
        fn versions(&self) -> serde_json::Value {
            serde_json::json!({})
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
            Ok(())
        }
        fn press_refinement_status(&self) -> Result<Status, String> {
            if !self.connected {
                return Err("not found".into());
            }
            if *self.puts.lock().unwrap() > 0 {
                if let Some(s) = self.status_after_put {
                    return Ok(s);
                }
            }
            Ok(self.fw.lock().unwrap().status())
        }
        fn put_press_refinement(&self, rec: &[u8]) -> Result<(), String> {
            if self.refuse_put {
                return Err("Device returned NAK".into());
            }
            *self.puts.lock().unwrap() += 1;
            self.fw.lock().unwrap().put(rec).map_err(|n| n.0)
        }
        fn get_press_refinement(&self) -> Result<Vec<u8>, String> {
            let mut r = self.fw.lock().unwrap().get().map_err(|n| n.0)?;
            if self.corrupt_read_back {
                r[100] ^= 1;
            }
            Ok(r)
        }
    }

    fn keyboard() -> Keyboard {
        Keyboard {
            connected: true,
            ..Default::default()
        }
    }

    #[test]
    fn install_writes_then_confirms_status_and_read_back() {
        let (_, payload, _) = synthetic();
        let kb = keyboard();
        let done = install_payload(&kb, &payload).unwrap();
        assert!(!done.already_installed);
        assert!(
            !done.active,
            "a firmware build without press refinement stores the data without applying it"
        );
        assert_eq!(done.crc32, record::crc32(&payload));
        assert_eq!(
            kb.fw.lock().unwrap().status().state,
            State::InstalledInactive
        );
        kb.fw.lock().unwrap().applies = true;
        // Installing the same data again writes nothing.
        let again = install_payload(&kb, &payload).unwrap();
        assert!(again.already_installed);
        assert!(again.active);
        assert_eq!(*kb.puts.lock().unwrap(), 1);
    }

    #[test]
    fn a_successful_put_is_not_trusted_alone() {
        let (_, payload, _) = synthetic();
        let wrong_crc = Keyboard {
            status_after_put: Some(Status {
                state: State::Installed,
                payload_length: 1456,
                payload_crc32: 1,
            }),
            ..keyboard()
        };
        assert!(matches!(
            install_payload(&wrong_crc, &payload),
            Err(Error::VerifyFailed(_))
        ));
        let still_absent = Keyboard {
            status_after_put: Some(Status::empty(State::Absent)),
            ..keyboard()
        };
        assert!(matches!(
            install_payload(&still_absent, &payload),
            Err(Error::VerifyFailed(_))
        ));
        let read_back_differs = Keyboard {
            corrupt_read_back: true,
            ..keyboard()
        };
        assert!(matches!(
            install_payload(&read_back_differs, &payload),
            Err(Error::VerifyFailed(m)) if m.contains("reads back")
        ));
        let lost_storage = Keyboard {
            status_after_put: Some(Status::empty(State::StorageUnavailable)),
            ..keyboard()
        };
        assert!(matches!(
            install_payload(&lost_storage, &payload),
            Err(Error::StorageUnavailable(_))
        ));
    }

    #[test]
    fn refusals_and_unavailable_storage_are_reported() {
        let (_, payload, _) = synthetic();
        let refused = Keyboard {
            refuse_put: true,
            ..keyboard()
        };
        assert!(matches!(
            install_payload(&refused, &payload),
            Err(Error::KeyboardRefused(_))
        ));
        let no_storage = keyboard();
        no_storage.fw.lock().unwrap().storage_unavailable = true;
        assert!(matches!(
            install_payload(&no_storage, &payload),
            Err(Error::StorageUnavailable(_))
        ));
        assert_eq!(*no_storage.puts.lock().unwrap(), 0);
        assert!(matches!(
            install_payload(&Keyboard::default(), &payload),
            Err(Error::KeyboardDisconnected(_))
        ));
    }

    #[test]
    fn reinstalling_offline_uses_the_kept_payload() {
        let (_, payload, refinement) = synthetic();
        let store = temp_store("offline_install");
        std::fs::write(cached_path(&store, &refinement), &payload).unwrap();
        let kb = keyboard();
        let obtained = obtain_payload(&refinement, None, &store).unwrap();
        let done = install_obtained(&kb, obtained).unwrap();
        assert!(done.from_cache);
        assert_eq!(kb.fw.lock().unwrap().get().unwrap()[16..], payload[..]);
        let _ = std::fs::remove_dir_all(&store);
    }

    fn zip_with(member: &str, data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("AppxManifest.xml", opts).unwrap();
            zip.write_all(b"<Package/>").unwrap();
            zip.start_file(member, opts).unwrap();
            zip.write_all(data).unwrap();
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn a_chosen_package_or_bare_uf2_gives_the_same_verified_payload() {
        let (uf2, payload, refinement) = synthetic();
        let store = temp_store("chosen");
        let bare = store.join("firmware.uf2");
        std::fs::write(&bare, &uf2).unwrap();
        let got = payload_from_file(&refinement, &bare, &store).unwrap();
        assert_eq!(got.payload, payload);
        assert!(!got.from_cache);
        assert_eq!(
            std::fs::read(cached_path(&store, &refinement)).unwrap(),
            payload
        );

        let package = store.join("software.msix");
        std::fs::write(&package, zip_with(&refinement.member, &uf2)).unwrap();
        assert_eq!(
            payload_from_file(&refinement, &package, &store)
                .unwrap()
                .payload,
            payload
        );
    }

    #[test]
    fn a_chosen_file_that_isnt_the_source_member_is_refused() {
        let (uf2, _, refinement) = synthetic();
        let store = temp_store("chosen_bad");
        let mut other = uf2.clone();
        other[100] ^= 1;
        let other_uf2 = store.join("other.uf2");
        std::fs::write(&other_uf2, &other).unwrap();
        assert!(matches!(
            payload_from_file(&refinement, &other_uf2, &store),
            Err(Error::HashMismatch(_))
        ));
        let wrong_member = store.join("other.msix");
        std::fs::write(
            &wrong_member,
            zip_with("assets/firmware/elsewhere.uf2", &uf2),
        )
        .unwrap();
        assert!(matches!(
            payload_from_file(&refinement, &wrong_member, &store),
            Err(Error::HashMismatch(m)) if m.contains("isn't in")
        ));
        let text = store.join("notes.txt");
        std::fs::write(&text, b"just some notes here").unwrap();
        assert!(matches!(
            payload_from_file(&refinement, &text, &store),
            Err(Error::File(m)) if m.contains("neither")
        ));
        assert!(matches!(
            payload_from_file(&refinement, &store.join("missing.uf2"), &store),
            Err(Error::File(_))
        ));
        assert!(!cached_path(&store, &refinement).exists());
    }

    /// The repository's press-refinement source against the real published software: a range
    /// read from its first location, and the file at `OPENFLUX_PRESS_REFINEMENT_FILE` when set. Needs
    /// the network, so it runs only on request (`cargo test -- --ignored real_press_refinement_source`).
    #[test]
    #[ignore]
    fn real_press_refinement_source() {
        let file = openflux_update::factory::ReferenceFile::parse_toml(include_str!(
            "../../../../factory-reference.toml"
        ))
        .unwrap();
        let refinement = file.press_refinement_source.unwrap();
        let store = temp_store("real");
        let got = obtain_payload(&refinement, None, &store).unwrap();
        assert!(!got.from_cache);
        assert_eq!(sha256_hex(&got.payload), refinement.payload_sha256);
        if let Some(path) = std::env::var_os("OPENFLUX_PRESS_REFINEMENT_FILE") {
            let local = payload_from_file(&refinement, Path::new(&path), &store).unwrap();
            assert_eq!(local.payload, got.payload);
        }
        let _ = std::fs::remove_dir_all(&store);
    }
}
