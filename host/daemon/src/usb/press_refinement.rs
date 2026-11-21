//! The firmware's press-refinement record and the framing of its USB commands (`0x70`–`0x72`).
//!
//! The firmware can refine its Hall readings with a small set of coefficients it keeps in flash.
//! This module frames those coefficients for the firmware and reads its answers; the payload's
//! contents are opaque here. See `docs/protocol/usb.md`.
//!
//! - `PRESS_REFINEMENT_STATUS` answers a 16-byte [`Status`]. A valid record is reported as
//!   [`State::Installed`] when the firmware applies it and [`State::InstalledInactive`] when this
//!   firmware build only stores it.
//! - `PUT_PRESS_REFINEMENT` takes a [`RECORD_LEN`]-byte record from [`build_record`].
//! - `GET_PRESS_REFINEMENT` answers the stored record, which [`parse_record`] checks.
//!
//! [`FirmwareStore`] is the firmware's side of the contract in memory, for simulated keyboards.

use serde::Serialize;

/// The record and status format this daemon speaks.
pub const FORMAT: u32 = 1;
/// Length of the payload the firmware accepts.
pub const PAYLOAD_LEN: usize = 1456;
/// Length of the record header: magic, format, payload length and payload CRC-32.
pub const HEADER_LEN: usize = 16;
/// Length of a whole record.
pub const RECORD_LEN: usize = HEADER_LEN + PAYLOAD_LEN;
/// Length of the status answer.
pub const STATUS_LEN: usize = 16;
/// The record's first four bytes.
pub const MAGIC: &[u8; 4] = b"OFRM";

/// What the firmware has stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// Nothing is stored; the firmware runs its deterministic fallback.
    Absent,
    /// A valid record is stored and in use.
    Installed,
    /// A valid record is stored, but this firmware build doesn't apply it.
    InstalledInactive,
    /// Something is stored but doesn't validate; the firmware runs its fallback.
    Invalid,
    /// The firmware can't read or write its press-refinement storage.
    StorageUnavailable,
}

impl State {
    fn from_wire(v: u32) -> Option<Self> {
        match v {
            0 => Some(State::Absent),
            1 => Some(State::Installed),
            2 => Some(State::Invalid),
            3 => Some(State::StorageUnavailable),
            4 => Some(State::InstalledInactive),
            _ => None,
        }
    }

    fn to_wire(self) -> u32 {
        match self {
            State::Absent => 0,
            State::Installed => 1,
            State::Invalid => 2,
            State::StorageUnavailable => 3,
            State::InstalledInactive => 4,
        }
    }

    /// A valid record is stored, whether or not the firmware applies it.
    pub fn is_installed(self) -> bool {
        matches!(self, State::Installed | State::InstalledInactive)
    }
}

/// The firmware's answer to `PRESS_REFINEMENT_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: State,
    /// [`PAYLOAD_LEN`] when a record is installed, otherwise 0.
    pub payload_length: u32,
    /// CRC-32 of the installed payload, otherwise 0.
    pub payload_crc32: u32,
}

impl Status {
    /// A status with no stored payload.
    pub fn empty(state: State) -> Self {
        Self {
            state,
            payload_length: 0,
            payload_crc32: 0,
        }
    }

    /// The 16 bytes the firmware answers with.
    pub fn to_bytes(&self) -> [u8; STATUS_LEN] {
        let mut out = [0u8; STATUS_LEN];
        out[0..4].copy_from_slice(&FORMAT.to_le_bytes());
        out[4..8].copy_from_slice(&self.state.to_wire().to_le_bytes());
        out[8..12].copy_from_slice(&self.payload_length.to_le_bytes());
        out[12..16].copy_from_slice(&self.payload_crc32.to_le_bytes());
        out
    }
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// CRC-32/ISO-HDLC, as zlib's `crc32`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = flate2::Crc::new();
    c.update(data);
    c.sum()
}

/// Reads a `PRESS_REFINEMENT_STATUS` answer.
pub fn parse_status(raw: &[u8]) -> Result<Status, String> {
    if raw.len() != STATUS_LEN {
        return Err(format!(
            "the press-refinement status is {} bytes, not {STATUS_LEN}",
            raw.len()
        ));
    }
    let format = le32(raw, 0);
    if format != FORMAT {
        return Err(format!(
            "the press-refinement status has format {format}, not {FORMAT}"
        ));
    }
    let wire_state = le32(raw, 4);
    let state = State::from_wire(wire_state)
        .ok_or_else(|| format!("the press-refinement status has unknown state {wire_state}"))?;
    let status = Status {
        state,
        payload_length: le32(raw, 8),
        payload_crc32: le32(raw, 12),
    };
    let consistent = if state.is_installed() {
        status.payload_length as usize == PAYLOAD_LEN
    } else {
        status.payload_length == 0 && status.payload_crc32 == 0
    };
    if !consistent {
        return Err(format!(
            "the press-refinement status is inconsistent (state {wire_state}, length {}, crc32 {:08x})",
            status.payload_length, status.payload_crc32
        ));
    }
    Ok(status)
}

/// The `PUT_PRESS_REFINEMENT` record for `payload`: `OFRM`, the format, the payload length and
/// the payload's CRC-32 (all little-endian), then the payload.
pub fn build_record(payload: &[u8]) -> Result<Vec<u8>, String> {
    if payload.len() != PAYLOAD_LEN {
        return Err(format!(
            "press-refinement payload is {} bytes, not {PAYLOAD_LEN}",
            payload.len()
        ));
    }
    let mut out = Vec::with_capacity(RECORD_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&(PAYLOAD_LEN as u32).to_le_bytes());
    out.extend_from_slice(&crc32(payload).to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Checks a record's header and CRC and returns its payload.
pub fn parse_record(raw: &[u8]) -> Result<&[u8], String> {
    if raw.len() != RECORD_LEN {
        return Err(format!(
            "the press-refinement record is {} bytes, not {RECORD_LEN}",
            raw.len()
        ));
    }
    if &raw[0..4] != MAGIC {
        return Err("the press-refinement record doesn't start with OFRM".into());
    }
    let format = le32(raw, 4);
    if format != FORMAT {
        return Err(format!(
            "the press-refinement record has format {format}, not {FORMAT}"
        ));
    }
    let len = le32(raw, 8) as usize;
    if len != PAYLOAD_LEN {
        return Err(format!(
            "the press-refinement record declares {len} payload bytes, not {PAYLOAD_LEN}"
        ));
    }
    let payload = &raw[HEADER_LEN..];
    let want = le32(raw, 12);
    let got = crc32(payload);
    if want != got {
        return Err(format!(
            "the press-refinement record's CRC-32 is {got:08x}, its header says {want:08x}"
        ));
    }
    Ok(payload)
}

/// Why a simulated firmware refuses a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nak(pub String);

/// The firmware's press-refinement storage in memory: it validates records as the firmware does,
/// writes them whole and answers the three commands. Simulated keyboards use it.
#[derive(Debug, Clone, Default)]
pub struct FirmwareStore {
    /// The stored bytes, valid or not.
    pub stored: Option<Vec<u8>>,
    /// The storage can't be used: every command reports [`State::StorageUnavailable`] or NAKs.
    pub storage_unavailable: bool,
    /// The firmware applies a valid record ([`State::Installed`]); otherwise a valid record is
    /// reported as [`State::InstalledInactive`], as a firmware build without press refinement does.
    pub applies: bool,
}

impl FirmwareStore {
    /// `PRESS_REFINEMENT_STATUS`.
    pub fn status(&self) -> Status {
        if self.storage_unavailable {
            return Status::empty(State::StorageUnavailable);
        }
        match &self.stored {
            None => Status::empty(State::Absent),
            Some(raw) => match parse_record(raw) {
                Ok(payload) => Status {
                    state: if self.applies {
                        State::Installed
                    } else {
                        State::InstalledInactive
                    },
                    payload_length: PAYLOAD_LEN as u32,
                    payload_crc32: crc32(payload),
                },
                Err(_) => Status::empty(State::Invalid),
            },
        }
    }

    /// `PUT_PRESS_REFINEMENT`: stores a valid record, or refuses and keeps what was stored.
    pub fn put(&mut self, record: &[u8]) -> Result<(), Nak> {
        if self.storage_unavailable {
            return Err(Nak("press-refinement storage unavailable".into()));
        }
        parse_record(record).map_err(Nak)?;
        self.stored = Some(record.to_vec());
        Ok(())
    }

    /// `GET_PRESS_REFINEMENT`: the stored record when it validates.
    pub fn get(&self) -> Result<Vec<u8>, Nak> {
        if self.storage_unavailable {
            return Err(Nak("press-refinement storage unavailable".into()));
        }
        match &self.stored {
            Some(raw) if parse_record(raw).is_ok() => Ok(raw.clone()),
            Some(_) => Err(Nak("the stored press-refinement record is invalid".into())),
            None => Err(Nak("no press-refinement record is stored".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> Vec<u8> {
        (0..PAYLOAD_LEN).map(|i| (i * 7 + 3) as u8).collect()
    }

    #[test]
    fn crc32_is_zlib_crc32() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn record_has_header_then_payload() {
        let p = payload();
        let r = build_record(&p).unwrap();
        assert_eq!(r.len(), 1472);
        assert_eq!(&r[0..4], b"OFRM");
        assert_eq!(r[4..8], 1u32.to_le_bytes());
        assert_eq!(r[8..12], 1456u32.to_le_bytes());
        assert_eq!(r[12..16], crc32(&p).to_le_bytes());
        assert_eq!(&r[16..], &p[..]);
        assert_eq!(parse_record(&r).unwrap(), &p[..]);
        assert!(build_record(&p[1..]).is_err());
    }

    #[test]
    fn damaged_records_are_rejected() {
        let good = build_record(&payload()).unwrap();
        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        assert!(parse_record(&bad_magic).unwrap_err().contains("OFRM"));
        let mut bad_format = good.clone();
        bad_format[4] = 2;
        assert!(parse_record(&bad_format).unwrap_err().contains("format"));
        let mut bad_len = good.clone();
        bad_len[8] = 0;
        assert!(parse_record(&bad_len)
            .unwrap_err()
            .contains("payload bytes"));
        let mut bad_crc = good.clone();
        bad_crc[100] ^= 1;
        assert!(parse_record(&bad_crc).unwrap_err().contains("CRC-32"));
        assert!(parse_record(&good[..1471]).is_err());
    }

    #[test]
    fn status_is_four_le_words_and_checked_for_consistency() {
        let s = Status {
            state: State::Installed,
            payload_length: 1456,
            payload_crc32: 0xDEAD_BEEF,
        };
        let raw = s.to_bytes();
        assert_eq!(raw[0..4], 1u32.to_le_bytes());
        assert_eq!(raw[4..8], 1u32.to_le_bytes());
        assert_eq!(raw[8..12], 1456u32.to_le_bytes());
        assert_eq!(raw[12..16], 0xDEAD_BEEFu32.to_le_bytes());
        assert_eq!(parse_status(&raw).unwrap(), s);
        let inactive = Status {
            state: State::InstalledInactive,
            ..s
        };
        assert_eq!(inactive.to_bytes()[4..8], 4u32.to_le_bytes());
        assert_eq!(parse_status(&inactive.to_bytes()).unwrap(), inactive);
        for state in [State::Absent, State::Invalid, State::StorageUnavailable] {
            assert_eq!(
                parse_status(&Status::empty(state).to_bytes())
                    .unwrap()
                    .state,
                state
            );
        }
        let mut unknown = raw;
        unknown[4] = 5;
        assert!(parse_status(&unknown)
            .unwrap_err()
            .contains("unknown state"));
        let mut format2 = raw;
        format2[0] = 2;
        assert!(parse_status(&format2).is_err());
        let absent_with_crc = Status {
            state: State::Absent,
            payload_length: 0,
            payload_crc32: 5,
        };
        assert!(parse_status(&absent_with_crc.to_bytes()).is_err());
        assert!(parse_status(&raw[..15]).is_err());
    }

    #[test]
    fn simulated_firmware_keeps_the_previous_record_when_a_put_is_refused() {
        let mut fw = FirmwareStore::default();
        assert_eq!(fw.status().state, State::Absent);
        assert!(fw.get().is_err());
        let good = build_record(&payload()).unwrap();
        fw.put(&good).unwrap();
        assert_eq!(fw.status().state, State::InstalledInactive);
        fw.applies = true;
        assert_eq!(fw.status().state, State::Installed);
        assert_eq!(fw.status().payload_crc32, crc32(&payload()));
        let mut bad = good.clone();
        bad[20] ^= 1;
        assert!(fw.put(&bad).is_err());
        assert_eq!(fw.get().unwrap(), good);
        fw.stored = Some(bad);
        assert_eq!(fw.status().state, State::Invalid);
        fw.storage_unavailable = true;
        assert_eq!(fw.status().state, State::StorageUnavailable);
        assert!(fw.put(&good).is_err());
    }
}
