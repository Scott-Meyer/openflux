//! USB command protocol for the keyboard's two USB functions: the bridge (the keyboard computer's
//! USB gadget) and the firmware (the RP2040 key scanner).
//!
//! A command is a header packet (`cmd`, packet count, byte count), the payload padded to whole
//! packets, and a zero-length packet. The response has the same shape, led by ACK or NAK.

use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const KEYBOARD_VID: u16 = 0x3761;
pub const BRIDGE_PID: u16 = 0x0104;
pub const FIRMWARE_PID: u16 = 0x4015;

pub const BRIDGE_CMD_BRIDGE_VERSION: u8 = 1;
pub const BRIDGE_CMD_DISPLAY_VERSION: u8 = 2;
pub const BRIDGE_CMD_OS_VERSION: u8 = 3;
pub const BRIDGE_CMD_SET_SYSTEM_TIME: u8 = 4;
pub const BRIDGE_CMD_GET_FILE_LIST: u8 = 100;
pub const BRIDGE_CMD_GET_FILE: u8 = 101;
pub const BRIDGE_CMD_PUT_FILE: u8 = 102;
pub const BRIDGE_CMD_DELETE_FILE: u8 = 103;
pub const BRIDGE_CMD_GET_FREE_SPACE: u8 = 104;
pub const BRIDGE_CMD_STAT_FILE: u8 = 105;
pub const BRIDGE_CMD_SEND_LIVE_DATA: u8 = 200;
pub const BRIDGE_CMD_GET_MEDIA_STATUS: u8 = 201;
pub const BRIDGE_CMD_GET_SHORTCUT_CONTEXT: u8 = 202;
pub const BRIDGE_CMD_POLL_BUS_FRAMES: u8 = 205;

pub const FW_CMD_VERSION: u8 = 1;
pub const FW_CMD_SEND_KEYMAP: u8 = 100;
pub const FW_CMD_ENTER_BOOTLOADER: u8 = 101;
pub const FW_CMD_ENTER_OS_BOOTLOADER: u8 = 102;
pub const FW_CMD_GET_KEYMAP: u8 = 103;
pub const FW_CMD_SET_LANGUAGE: u8 = 104;
pub const FW_CMD_SET_PLATFORM: u8 = 105;
pub const FW_CMD_SET_ACTIVE_PROFILE: u8 = 106;
pub const FW_CMD_FILE_FROM_HOST: u8 = 107;
pub const FW_CMD_FILE_TO_HOST: u8 = 108;
pub const FW_CMD_GET_RUNTIME_STATE: u8 = 109;
pub const FW_CMD_PRESS_REFINEMENT_STATUS: u8 = 0x70;
pub const FW_CMD_PUT_PRESS_REFINEMENT: u8 = 0x71;
pub const FW_CMD_GET_PRESS_REFINEMENT: u8 = 0x72;
pub const FW_CMD_PUT_VIAL_DEF: u8 = 0x73;
pub const FW_CMD_SET_GAMEPAD_CONFIG: u8 = 0x74;
pub const FW_CMD_SET_LAYER_OVERRIDE: u8 = 0x75;

pub const RESP_NAK: u8 = 0;
pub const RESP_ACK: u8 = 1;
/// Largest response accepted: the largest file the bridge stores.
pub const MAX_USB_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
/// Slowest bulk throughput a transfer may run at. A command's timeout covers its latency; each
/// byte sent or received adds time at this rate, so large files get proportionally longer.
pub const MIN_BULK_BYTES_PER_SEC: u64 = 2 * 1024 * 1024;
pub const MAX_DRAIN_BYTES: usize = 128 * 1024;
pub const MAX_DRAIN_READS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsbTarget {
    pub pid: u16,
    pub interface: u8,
    pub ep_out: u8,
    pub ep_in: u8,
    pub packet_size: usize,
}

pub const BRIDGE_TARGET: UsbTarget = UsbTarget {
    pid: BRIDGE_PID,
    interface: 0,
    ep_out: 0x01,
    ep_in: 0x81,
    packet_size: 512,
};

pub const FIRMWARE_TARGET: UsbTarget = UsbTarget {
    pid: FIRMWARE_PID,
    interface: 1,
    ep_out: 0x01,
    ep_in: 0x84,
    packet_size: 64,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsbError {
    NotFound(String),
    Nak {
        resp_type: u8,
        packets: u32,
        bytes: u32,
    },
    Protocol(String),
    Io(String),
}

impl fmt::Display for UsbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsbError::NotFound(s) => write!(f, "Device not found: {s}"),
            UsbError::Nak {
                resp_type,
                packets,
                bytes,
            } => write!(
                f,
                "Device returned NAK (type={resp_type}, packets={packets}, bytes={bytes})"
            ),
            UsbError::Protocol(s) => write!(f, "Protocol error: {s}"),
            UsbError::Io(s) => write!(f, "USB I/O error: {s}"),
        }
    }
}
impl std::error::Error for UsbError {}

pub trait UsbEndpointIo {
    fn bulk_write(&mut self, ep: u8, data: &[u8], timeout_ms: u64) -> Result<(), UsbError>;
    fn bulk_read(&mut self, ep: u8, len: usize, timeout_ms: u64) -> Result<Vec<u8>, UsbError>;
}

fn transfer_time(bytes: usize) -> Duration {
    Duration::from_millis((bytes as u64).saturating_mul(1000) / MIN_BULK_BYTES_PER_SEC)
}

fn remaining_ms(deadline: Instant) -> Result<u64, UsbError> {
    let now = Instant::now();
    if now >= deadline {
        return Err(UsbError::Io("USB transaction deadline exceeded".into()));
    }
    let rem = deadline.saturating_duration_since(now).as_millis();
    Ok(u64::try_from(rem).unwrap_or(u64::MAX).max(1))
}

fn drain_trailing_packets<I: UsbEndpointIo>(
    io: &mut I,
    ep_in: u8,
    pkt_sz: usize,
    resp_packets: u32,
    resp_bytes: u32,
    deadline: Instant,
) {
    let Ok(ms) = remaining_ms(deadline) else {
        return;
    };
    if resp_bytes == 0 {
        let _ = io.bulk_read(ep_in, pkt_sz, ms.min(500));
        return;
    }
    if resp_packets == 0 {
        return;
    }
    let max_adv = (resp_packets as usize).saturating_mul(pkt_sz);
    let mut rem = max_adv.min(MAX_DRAIN_BYTES);
    let mut reads = 0usize;
    while rem > 0 && reads < MAX_DRAIN_READS {
        let Ok(step_ms) = remaining_ms(deadline) else {
            return;
        };
        reads += 1;
        match io.bulk_read(ep_in, rem.min(16384), step_ms.min(500)) {
            Ok(chunk) if !chunk.is_empty() => rem = rem.saturating_sub(chunk.len()),
            _ => break,
        }
    }
    if reads < MAX_DRAIN_READS {
        if let Ok(step_ms) = remaining_ms(deadline) {
            let _ = io.bulk_read(ep_in, pkt_sz, step_ms.min(200));
        }
    }
}

pub fn transact_io<I: UsbEndpointIo>(
    io: &mut I,
    target: UsbTarget,
    cmd: u8,
    payload: &[u8],
    timeout_ms: u64,
) -> Result<Vec<u8>, UsbError> {
    let mut deadline =
        Instant::now() + Duration::from_millis(timeout_ms.max(1)) + transfer_time(payload.len());
    let pkt_sz = target.packet_size;
    let num_packets = if payload.is_empty() {
        0u32
    } else {
        payload.len().div_ceil(pkt_sz) as u32
    };

    let mut hdr = vec![0u8; pkt_sz];
    hdr[0] = cmd;
    hdr[1..5].copy_from_slice(&num_packets.to_le_bytes());
    hdr[5..9].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    io.bulk_write(target.ep_out, &hdr, remaining_ms(deadline)?)?;

    if !payload.is_empty() {
        for chunk in payload.chunks(pkt_sz) {
            let rem_ms = remaining_ms(deadline)?;
            if chunk.len() == pkt_sz {
                io.bulk_write(target.ep_out, chunk, rem_ms)?;
            } else {
                let mut padded = vec![0u8; pkt_sz];
                padded[..chunk.len()].copy_from_slice(chunk);
                io.bulk_write(target.ep_out, &padded, rem_ms)?;
            }
        }
    }
    // A zero-length packet ends the bulk OUT transfer.
    io.bulk_write(target.ep_out, &[], remaining_ms(deadline)?)?;

    let resp_hdr = io.bulk_read(target.ep_in, pkt_sz, remaining_ms(deadline)?)?;
    if resp_hdr.len() < 9 {
        return Err(UsbError::Protocol(format!(
            "Short response header: {} bytes",
            resp_hdr.len()
        )));
    }
    let resp_type = resp_hdr[0];
    let resp_packets = u32::from_le_bytes([resp_hdr[1], resp_hdr[2], resp_hdr[3], resp_hdr[4]]);
    let resp_bytes = u32::from_le_bytes([resp_hdr[5], resp_hdr[6], resp_hdr[7], resp_hdr[8]]);

    if resp_type != RESP_ACK {
        drain_trailing_packets(io, target.ep_in, pkt_sz, resp_packets, resp_bytes, deadline);
        return Err(UsbError::Nak {
            resp_type,
            packets: resp_packets,
            bytes: resp_bytes,
        });
    }

    if resp_bytes == 0 {
        if resp_packets != 0 && resp_packets != 1 {
            return Err(UsbError::Protocol(format!(
                "Invalid response header: bytes=0 with packets={resp_packets}"
            )));
        }
        if let Ok(step_ms) = remaining_ms(deadline) {
            if let Ok(d) = io.bulk_read(target.ep_in, pkt_sz, step_ms.min(1000)) {
                if !d.is_empty() && resp_packets == 1 {
                    if let Ok(step2) = remaining_ms(deadline) {
                        let _ = io.bulk_read(target.ep_in, pkt_sz, step2.min(1000));
                    }
                }
            }
        }
        return Ok(Vec::new());
    }

    let resp_bytes_usize = resp_bytes as usize;
    if resp_bytes_usize > MAX_USB_RESPONSE_BYTES {
        drain_trailing_packets(io, target.ep_in, pkt_sz, resp_packets, resp_bytes, deadline);
        return Err(UsbError::Protocol(format!(
            "Response size {resp_bytes} exceeds cap ({MAX_USB_RESPONSE_BYTES} B)"
        )));
    }

    deadline = deadline.max(Instant::now() + transfer_time(resp_bytes_usize));

    // The packet count is ceil(bytes / packet size); the bridge may count one packet more.
    let expected_pkts = resp_bytes_usize.div_ceil(pkt_sz);
    let resp_pkts_usize = resp_packets as usize;
    if resp_pkts_usize < expected_pkts || resp_pkts_usize > expected_pkts + 1 {
        drain_trailing_packets(io, target.ep_in, pkt_sz, resp_packets, resp_bytes, deadline);
        return Err(UsbError::Protocol(format!(
            "Response packets={resp_packets} out of expected range [{expected_pkts}..={}] for bytes={resp_bytes}",
            expected_pkts + 1
        )));
    }

    let total_read = resp_pkts_usize * pkt_sz;
    let mut raw = Vec::with_capacity(resp_bytes_usize);
    let mut remaining = total_read;
    let mut total_received = 0usize;
    let mut zlp_consumed = false;

    while remaining > 0 {
        let req_len = remaining.min(16384);
        let d = io.bulk_read(target.ep_in, req_len, remaining_ms(deadline)?)?;
        if d.is_empty() {
            if total_received >= resp_bytes_usize {
                zlp_consumed = true;
                break;
            }
            return Err(UsbError::Protocol(format!(
                "Premature ZLP: received {total_received} of {total_read} (advertised={resp_bytes})"
            )));
        }
        total_received += d.len();
        remaining = remaining.saturating_sub(d.len());
        raw.extend_from_slice(&d);
        if d.len() < req_len && total_received >= resp_bytes_usize {
            zlp_consumed = true;
            break;
        }
    }

    if raw.len() < resp_bytes_usize {
        return Err(UsbError::Protocol(format!(
            "Truncated response: {} < {}",
            raw.len(),
            resp_bytes
        )));
    }
    if total_read.is_multiple_of(pkt_sz) && !zlp_consumed {
        if let Ok(step_ms) = remaining_ms(deadline) {
            let _ = io.bulk_read(target.ep_in, pkt_sz, step_ms.min(1000));
        }
    }
    raw.truncate(resp_bytes_usize);
    Ok(raw)
}

pub fn build_put_file_payload(remote_path: &str, data: &[u8]) -> Result<Vec<u8>, UsbError> {
    if remote_path.is_empty() || remote_path.contains('\0') {
        return Err(UsbError::Protocol(
            "remote_path must be non-empty without NUL".into(),
        ));
    }
    let nb = remote_path.as_bytes();
    let mut out = Vec::with_capacity(nb.len() + 1 + data.len());
    out.extend_from_slice(nb);
    out.push(0);
    out.extend_from_slice(data);
    Ok(out)
}

pub fn build_named_payload(remote_path: &str) -> Result<Vec<u8>, UsbError> {
    if remote_path.is_empty() || remote_path.contains('\0') {
        return Err(UsbError::Protocol(
            "remote_path must be non-empty without NUL".into(),
        ));
    }
    let nb = remote_path.as_bytes();
    let mut out = Vec::with_capacity(nb.len() + 1);
    out.extend_from_slice(nb);
    out.push(0);
    Ok(out)
}

pub fn build_set_time_payload(
    utc_timestamp: &str,
    timezone_name: &str,
) -> Result<Vec<u8>, UsbError> {
    if utc_timestamp.is_empty()
        || utc_timestamp.contains('\0')
        || timezone_name.is_empty()
        || timezone_name.contains('\0')
    {
        return Err(UsbError::Protocol(
            "timestamp and timezone must be non-empty and must not contain NUL".into(),
        ));
    }
    let s = format!("{utc_timestamp} {timezone_name}\0");
    Ok(s.into_bytes())
}

pub fn format_utc_system_time(unix_secs: u64) -> String {
    // Convert Unix epoch seconds to a UTC civil date (the `civil_from_days` algorithm).
    let days = (unix_secs / 86400) as i64;
    let sod = (unix_secs % 86400) as u32;
    let h = sod / 3600;
    let m = (sod % 3600) / 60;
    let s = sod % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = y + if mo <= 2 { 1 } else { 0 };
    format!("{yr:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

pub fn current_utc_system_time() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc_system_time(secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct MockIo {
        writes: Vec<(u8, Vec<u8>)>,
        reads: VecDeque<Vec<u8>>,
    }

    impl MockIo {
        fn new(reads: Vec<Vec<u8>>) -> Self {
            Self {
                writes: Vec::new(),
                reads: reads.into_iter().collect(),
            }
        }
    }

    impl UsbEndpointIo for MockIo {
        fn bulk_write(&mut self, ep: u8, data: &[u8], _timeout_ms: u64) -> Result<(), UsbError> {
            self.writes.push((ep, data.to_vec()));
            Ok(())
        }
        fn bulk_read(
            &mut self,
            _ep: u8,
            _len: usize,
            _timeout_ms: u64,
        ) -> Result<Vec<u8>, UsbError> {
            self.reads
                .pop_front()
                .ok_or_else(|| UsbError::Io("mock queue empty".into()))
        }
    }

    fn make_hdr(pkt_sz: usize, typ: u8, packets: u32, bytes: u32) -> Vec<u8> {
        let mut h = vec![0u8; pkt_sz];
        h[0] = typ;
        h[1..5].copy_from_slice(&packets.to_le_bytes());
        h[5..9].copy_from_slice(&bytes.to_le_bytes());
        h
    }

    #[test]
    fn accepts_one_extra_packet_and_trailing_zero_length_packet() {
        let payload = vec![0xABu8; 16384];
        let hdr = make_hdr(512, RESP_ACK, (16384 / 512) + 1, 16384);
        let mut io = MockIo::new(vec![hdr, payload.clone(), vec![]]);
        let got = transact_io(&mut io, BRIDGE_TARGET, BRIDGE_CMD_GET_FILE, b"test", 2000).unwrap();
        assert_eq!(got, payload);
        // Header + 1 payload packet + ZLP
        assert_eq!(io.writes.len(), 3);
        assert!(io.writes[2].1.is_empty());
    }

    #[test]
    fn empty_ack_with_one_packet_succeeds_and_malformed_response_is_drained() {
        let hdr = make_hdr(512, RESP_ACK, 1, 0);
        let mut io = MockIo::new(vec![hdr, vec![]]);
        let got = transact_io(
            &mut io,
            BRIDGE_TARGET,
            BRIDGE_CMD_SEND_LIVE_DATA,
            b"{}",
            2000,
        )
        .unwrap();
        assert!(got.is_empty());

        // After a malformed header the queued packet is drained, so the next command succeeds.
        let bad_hdr = make_hdr(64, RESP_ACK, 1, 999);
        let good_hdr = make_hdr(64, RESP_ACK, 1, 4);
        let mut io2 = MockIo::new(vec![
            bad_hdr,
            vec![0xFFu8; 64],
            vec![],
            good_hdr,
            b"1.0\0"
                .iter()
                .copied()
                .chain(std::iter::repeat_n(0, 60))
                .collect(),
            vec![],
        ]);
        assert!(transact_io(&mut io2, FIRMWARE_TARGET, FW_CMD_VERSION, &[], 2000).is_err());
        let ver = transact_io(&mut io2, FIRMWARE_TARGET, FW_CMD_VERSION, &[], 2000).unwrap();
        assert_eq!(ver, b"1.0\0");
    }

    #[test]
    fn rejects_inflated_packets_and_enforces_monotonic_deadline_on_short_reads() {
        // 100 packets for 10 bytes is rejected.
        let bad_pkts = make_hdr(512, RESP_ACK, 100, 10);
        let mut io = MockIo::new(vec![bad_pkts]);
        let err =
            transact_io(&mut io, BRIDGE_TARGET, BRIDGE_CMD_GET_FILE, b"x\0", 500).unwrap_err();
        assert!(format!("{err}").contains("out of expected range"), "{err}");

        // One-byte reads that are too slow hit the deadline.
        struct SlowShortIo;
        impl UsbEndpointIo for SlowShortIo {
            fn bulk_write(
                &mut self,
                _ep: u8,
                _data: &[u8],
                _timeout_ms: u64,
            ) -> Result<(), UsbError> {
                Ok(())
            }
            fn bulk_read(
                &mut self,
                _ep: u8,
                len: usize,
                _timeout_ms: u64,
            ) -> Result<Vec<u8>, UsbError> {
                if len == 512 {
                    return Ok(make_hdr(512, RESP_ACK, 20, 10000));
                }
                std::thread::sleep(Duration::from_millis(8));
                Ok(vec![0x41])
            }
        }
        let mut sio = SlowShortIo;
        let err =
            transact_io(&mut sio, BRIDGE_TARGET, BRIDGE_CMD_GET_FILE, b"x\0", 30).unwrap_err();
        assert!(format!("{err}").contains("deadline exceeded"), "{err}");
    }

    #[test]
    fn rejects_u32_max_response_without_allocating_or_stalling() {
        let bad_hdr = make_hdr(512, RESP_ACK, u32::MAX, u32::MAX);
        let mut io = MockIo::new(vec![bad_hdr]);
        let err =
            transact_io(&mut io, BRIDGE_TARGET, BRIDGE_CMD_GET_FILE, b"x\0", 500).unwrap_err();
        assert!(format!("{err}").contains("exceeds cap"), "{err}");
    }

    #[test]
    fn bridge_payloads_are_nul_terminated() {
        assert_eq!(
            build_named_payload("/home/root/openflux/config").unwrap(),
            b"/home/root/openflux/config\0"
        );
        assert_eq!(
            build_put_file_payload("/home/root/openflux/config/a.json", b"{}").unwrap(),
            b"/home/root/openflux/config/a.json\0{}"
        );
        assert_eq!(
            build_set_time_payload("2026-11-18 01:02:03", "UTC").unwrap(),
            b"2026-11-18 01:02:03 UTC\0"
        );
        assert!(build_named_payload("bad\0path").is_err());
        assert!(build_put_file_payload("bad\0path", b"1").is_err());
        assert!(build_set_time_payload("2026-11-18 01:02:03", "UTC\0").is_err());
        assert!(build_set_time_payload("", "UTC").is_err());
        assert!(build_set_time_payload("2026-11-18 01:02:03", "").is_err());
    }

    #[test]
    fn press_refinement_record_travels_as_whole_firmware_packets() {
        let payload = vec![0x5Au8; crate::usb::press_refinement::PAYLOAD_LEN];
        let record = crate::usb::press_refinement::build_record(&payload).unwrap();
        let mut io = MockIo::new(vec![make_hdr(64, RESP_ACK, 0, 0), vec![]]);
        let got = transact_io(
            &mut io,
            FIRMWARE_TARGET,
            FW_CMD_PUT_PRESS_REFINEMENT,
            &record,
            2000,
        )
        .unwrap();
        assert!(got.is_empty());
        let hdr = &io.writes[0].1;
        assert_eq!(hdr[0], 0x71);
        assert_eq!(hdr[1..5], 23u32.to_le_bytes());
        assert_eq!(hdr[5..9], 1472u32.to_le_bytes());
        // Header, 23 payload packets, zero-length packet.
        assert_eq!(io.writes.len(), 25);
        let sent: Vec<u8> = io.writes[1..24]
            .iter()
            .flat_map(|(_, d)| d.clone())
            .collect();
        assert_eq!(sent, record);

        let status = crate::usb::press_refinement::Status::empty(
            crate::usb::press_refinement::State::Absent,
        )
        .to_bytes();
        let mut answer = status.to_vec();
        answer.resize(64, 0);
        let mut io = MockIo::new(vec![make_hdr(64, RESP_ACK, 1, 16), answer, vec![]]);
        let raw = transact_io(
            &mut io,
            FIRMWARE_TARGET,
            FW_CMD_PRESS_REFINEMENT_STATUS,
            &[],
            2000,
        )
        .unwrap();
        assert_eq!(
            crate::usb::press_refinement::parse_status(&raw).unwrap(),
            crate::usb::press_refinement::Status::empty(
                crate::usb::press_refinement::State::Absent
            )
        );
    }

    #[test]
    fn utc_time_formatting() {
        assert_eq!(format_utc_system_time(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc_system_time(1_763_414_400), "2025-11-17 21:20:00");
    }
}
