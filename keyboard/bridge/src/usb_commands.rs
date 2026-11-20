use crate::device_paths::{
    BRIDGE_ROLLBACK_DIR, DISPLAY_VERSION_FILE, MEDIA_STATUS_FILE, OS_VERSION_FILE,
    SHORTCUT_CONTEXT_FILE,
};
use crate::ffs_aio::PendingTxStream;
use crate::protocol::{
    BRIDGE_DISK_RESERVE_BYTES, BRIDGE_MAX_FILE_UPLOAD_BYTES, BRIDGE_PACKET_SIZE, BRIDGE_VERSION,
    CMD_DELETE_FILE, CMD_GET_FREE_DISK_SPACE, CMD_GET_LIST_OF_FILES, CMD_GET_MEDIA_STATUS,
    CMD_GET_SHORTCUT_CONTEXT, CMD_REQUEST_BRIDGE_VERSION, CMD_REQUEST_DISPLAY_VERSION,
    CMD_REQUEST_OS_VERSION, CMD_SEND_FILE_FROM_HOST, CMD_SEND_FILE_TO_HOST, CMD_SEND_LIVE_DATA,
    CMD_SET_SYSTEM_TIME, CMD_STAT_FILE, DISPLAY_VERSION_FALLBACK, OS_VERSION_FALLBACK, RESP_ACK,
    RESP_NAK,
};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const BASE_DIR: &str = "/home/root";
/// Every absolute path a command names must be inside this directory.
const ALLOWED_ROOT: &str = "/home";

pub trait CommandHooks {
    fn send_packet(&mut self, pkt: Option<&[u8]>) -> bool;
    fn reserve_packets(&mut self, _total_packets: usize) -> bool {
        true
    }
    fn reserve_file_packets(&mut self, total_packets: usize) -> bool {
        self.reserve_packets(total_packets)
    }
    fn send_stream(&mut self, reader: Box<dyn Read + Send>, len: usize) -> bool {
        let mut s = PendingTxStream::new(reader, len);
        loop {
            match s.next_packet() {
                Ok(Some(pkt)) => {
                    let arg = if pkt.len == 0 {
                        None
                    } else {
                        Some(&pkt.data[..pkt.len as usize])
                    };
                    if !self.send_packet(arg) {
                        return false;
                    }
                }
                Ok(None) => return true,
                Err(()) => return false,
            }
        }
    }
    fn on_live_data(&mut self, _data: &[u8]) {}
    fn clock_set(&mut self, utc_sec: i64) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            let ts = libc::timespec {
                tv_sec: utc_sec as _,
                tv_nsec: 0,
            };
            let rc = unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) };
            if rc == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error().to_string())
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = utc_sec;
            Err("clock_settime only supported on Linux".into())
        }
    }
}

fn sysroot_prefix() -> String {
    std::env::var("OPENFLUX_SYSROOT").unwrap_or_default()
}

#[allow(clippy::result_unit_err)]
pub fn bridge_resolve_path(input: &str) -> Result<String, ()> {
    if input.is_empty() || input.contains("..") || input.contains('\\') {
        return Err(());
    }
    if input == "." {
        return Ok(BASE_DIR.to_string());
    }
    if input.starts_with('/') {
        let inside = input
            .strip_prefix(ALLOWED_ROOT)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
        if !inside {
            return Err(());
        }
        return Ok(input.to_string());
    }
    Ok(format!("{BASE_DIR}/{input}"))
}

fn resolve_host_fs_path(input: &str) -> Result<PathBuf, ()> {
    let sandboxed = bridge_resolve_path(input)?;
    let root = sysroot_prefix();
    Ok(PathBuf::from(format!("{root}{sandboxed}")))
}

pub struct PutFileStream {
    target: PathBuf,
    part: PathBuf,
    writer: Option<BufWriter<std::fs::File>>,
    finished: bool,
}

impl PutFileStream {
    #[allow(clippy::result_unit_err)]
    pub fn open(raw_path: &str, expected_file_bytes: u64) -> Result<Self, ()> {
        let resolved = resolve_host_fs_path(raw_path)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent).map_err(|_| ())?;
        }
        check_disk_headroom(
            resolved.parent().unwrap_or(Path::new("/")),
            expected_file_bytes,
        )?;
        let part = PathBuf::from(format!(
            "{}.part.{}",
            resolved.to_string_lossy(),
            std::process::id()
        ));
        let _ = std::fs::remove_file(&part);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(|_| ())?;
        Ok(Self {
            target: resolved,
            part,
            writer: Some(BufWriter::with_capacity(64 * 1024, file)),
            finished: false,
        })
    }

    #[allow(clippy::result_unit_err)]
    pub fn write_chunk(&mut self, data: &[u8]) -> Result<(), ()> {
        let Some(ref mut w) = self.writer else {
            return Err(());
        };
        w.write_all(data).map_err(|_| ())
    }

    #[allow(clippy::result_unit_err)]
    pub fn finish(mut self) -> Result<(), ()> {
        let Some(mut w) = self.writer.take() else {
            return Err(());
        };
        if w.flush().is_err() {
            let _ = std::fs::remove_file(&self.part);
            return Err(());
        }
        if let Ok(f) = w.into_inner() {
            if f.sync_all().is_err() {
                let _ = std::fs::remove_file(&self.part);
                return Err(());
            }
        } else {
            let _ = std::fs::remove_file(&self.part);
            return Err(());
        }
        if std::fs::rename(&self.part, &self.target).is_err() {
            let _ = std::fs::remove_file(&self.part);
            return Err(());
        }
        if let Some(parent) = self.target.parent() {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for PutFileStream {
    fn drop(&mut self) {
        if !self.finished {
            drop(self.writer.take());
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

fn check_disk_headroom(dir: &Path, needed_bytes: u64) -> Result<(), ()> {
    let Ok(cdir) = std::ffi::CString::new(dir.to_string_lossy().as_bytes()) else {
        return Err(());
    };
    unsafe {
        let mut vfs: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(cdir.as_ptr(), &mut vfs) != 0 {
            return Err(());
        }
        #[allow(clippy::useless_conversion)]
        let avail = u64::from(vfs.f_bavail).saturating_mul(u64::from(vfs.f_frsize));
        #[allow(clippy::useless_conversion)]
        let total = u64::from(vfs.f_blocks).saturating_mul(u64::from(vfs.f_frsize));
        let reserve = if total > 512 * 1024 * 1024 {
            BRIDGE_DISK_RESERVE_BYTES
        } else {
            0
        };
        if avail < needed_bytes.saturating_add(reserve) {
            return Err(());
        }
    }
    Ok(())
}

pub fn bridge_send_response(hooks: &mut dyn CommandHooks, resp_type: u8, data: &[u8]) -> bool {
    let packets = if data.is_empty() {
        0u32
    } else {
        data.len().div_ceil(BRIDGE_PACKET_SIZE) as u32
    };
    if !hooks.reserve_packets(packets as usize + 2) {
        let mut nak = [0u8; BRIDGE_PACKET_SIZE];
        nak[0] = RESP_NAK;
        let _ = hooks.send_packet(Some(&nak));
        let _ = hooks.send_packet(None);
        return false;
    }
    let mut hdr = [0u8; BRIDGE_PACKET_SIZE];
    hdr[0] = resp_type;
    hdr[1..5].copy_from_slice(&packets.to_le_bytes());
    hdr[5..9].copy_from_slice(&(data.len() as u32).to_le_bytes());
    if !hooks.send_packet(Some(&hdr)) {
        return false;
    }
    for chunk in data.chunks(BRIDGE_PACKET_SIZE) {
        let mut pkt = [0u8; BRIDGE_PACKET_SIZE];
        pkt[..chunk.len()].copy_from_slice(chunk);
        if !hooks.send_packet(Some(&pkt)) {
            return false;
        }
    }
    hooks.send_packet(None)
}

fn read_text_file_trimmed(path: &str) -> Option<Vec<u8>> {
    let root = sysroot_prefix();
    let full = if !root.is_empty() && path.starts_with('/') {
        format!("{root}{path}")
    } else {
        path.to_string()
    };
    let mut b = std::fs::read(&full).ok()?;
    while matches!(b.last(), Some(b'\n' | b'\r')) {
        b.pop();
    }
    if b.len() > 4095 {
        b.truncate(4095);
    }
    Some(b)
}

fn send_text_file_or_default(hooks: &mut dyn CommandHooks, paths: &[&str], fallback: &str) {
    let mut bytes = paths
        .iter()
        .find_map(|p| read_text_file_trimmed(p))
        .unwrap_or_else(|| fallback.as_bytes().to_vec());
    bytes.push(0);
    bridge_send_response(hooks, RESP_ACK, &bytes);
}

fn handle_list_files(payload: &[u8], hooks: &mut dyn CommandHooks) {
    let raw = if payload.is_empty() {
        ".".to_string()
    } else {
        let nul = payload
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(payload.len());
        String::from_utf8_lossy(&payload[..nul]).to_string()
    };
    let Ok(resolved) = resolve_host_fs_path(&raw) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    let rd = match std::fs::read_dir(&resolved) {
        Ok(r) => r,
        Err(_) => {
            bridge_send_response(hooks, RESP_NAK, &[]);
            return;
        }
    };
    let mut out = Vec::new();
    for ent_res in rd {
        let Ok(ent) = ent_res else {
            bridge_send_response(hooks, RESP_NAK, &[]);
            return;
        };
        let name = ent.file_name();
        let name_s = name.to_string_lossy();
        if name_s.starts_with('.') {
            continue;
        }
        let Ok(meta) = std::fs::metadata(ent.path()) else {
            bridge_send_response(hooks, RESP_NAK, &[]);
            return;
        };
        if meta.is_file() {
            let entry = format!("{}:{}\0", meta.len(), name_s);
            out.extend_from_slice(entry.as_bytes());
        }
    }
    bridge_send_response(hooks, RESP_ACK, &out);
}

fn bridge_send_file_stream(path: &Path, hooks: &mut dyn CommandHooks) {
    // Open the file before inspecting metadata so the announced size belongs to this open handle.
    let Ok(file) = std::fs::File::open(path) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    let Ok(meta) = file.metadata() else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    if !meta.is_file() || meta.len() > BRIDGE_MAX_FILE_UPLOAD_BYTES as u64 {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    let len = meta.len() as usize;
    if len == 0 {
        bridge_send_response(hooks, RESP_ACK, &[]);
        return;
    }
    let packets = len.div_ceil(BRIDGE_PACKET_SIZE);
    if !hooks.reserve_file_packets(packets + 2) {
        let mut nak = [0u8; BRIDGE_PACKET_SIZE];
        nak[0] = RESP_NAK;
        let _ = hooks.send_packet(Some(&nak));
        let _ = hooks.send_packet(None);
        return;
    }

    let mut hdr = [0u8; BRIDGE_PACKET_SIZE];
    hdr[0] = RESP_ACK;
    hdr[1..5].copy_from_slice(&(packets as u32).to_le_bytes());
    hdr[5..9].copy_from_slice(&(len as u32).to_le_bytes());
    if !hooks.send_packet(Some(&hdr)) {
        return;
    }
    let _ = hooks.send_stream(Box::new(file), len);
}

fn handle_send_file_to_host(payload: &[u8], hooks: &mut dyn CommandHooks) {
    if payload.is_empty() {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    let nul = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    let raw = String::from_utf8_lossy(&payload[..nul]);
    let Ok(resolved) = resolve_host_fs_path(&raw) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    bridge_send_file_stream(&resolved, hooks);
}

fn handle_put_file_from_host(payload: &[u8], hooks: &mut dyn CommandHooks) {
    let Some(nul) = payload.iter().take(255).position(|&b| b == 0) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    let raw = String::from_utf8_lossy(&payload[..nul]);
    let file_bytes = &payload[nul + 1..];
    let Ok(mut stream) = PutFileStream::open(&raw, file_bytes.len() as u64) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    if stream.write_chunk(file_bytes).is_err() || stream.finish().is_err() {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    bridge_send_response(hooks, RESP_ACK, &[]);
}

fn handle_delete_file(payload: &[u8], hooks: &mut dyn CommandHooks) {
    if payload.is_empty() {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    let nul = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    let raw = String::from_utf8_lossy(&payload[..nul]);
    let Ok(resolved) = resolve_host_fs_path(&raw) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    match std::fs::remove_file(&resolved) {
        Ok(()) => {
            bridge_send_response(hooks, RESP_ACK, &[]);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bridge_send_response(hooks, RESP_ACK, &[]);
        }
        Err(_) => {
            bridge_send_response(hooks, RESP_NAK, &[]);
        }
    }
}

fn handle_get_free_space(hooks: &mut dyn CommandHooks) {
    let base = format!("{}{BASE_DIR}", sysroot_prefix());
    let Ok(cbase) = std::ffi::CString::new(base) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    unsafe {
        let mut vfs: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(cbase.as_ptr(), &mut vfs) != 0 {
            bridge_send_response(hooks, RESP_NAK, &[]);
            return;
        }
        #[allow(clippy::useless_conversion)]
        let raw_free = u64::from(vfs.f_bavail) * u64::from(vfs.f_frsize);
        #[allow(clippy::useless_conversion)]
        let total = u64::from(vfs.f_blocks) * u64::from(vfs.f_frsize);
        let usable = raw_free.saturating_sub(BRIDGE_DISK_RESERVE_BYTES);
        let mut msg = format!("free:{usable},total:{total}").into_bytes();
        msg.push(0);
        bridge_send_response(hooks, RESP_ACK, &msg);
    }
}

fn handle_send_live_data(payload: &[u8], hooks: &mut dyn CommandHooks) {
    if payload.is_empty() {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    hooks.on_live_data(payload);
    bridge_send_response(hooks, RESP_ACK, &[1u8]);
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(yr: i32, mon: u32) -> u32 {
    let leap = (yr % 4 == 0) && (yr % 100 != 0 || yr % 400 == 0);
    match mon {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_valid_tz_name(tz: &str) -> bool {
    !tz.is_empty()
        && !tz.starts_with('/')
        && !tz.contains("..")
        && tz
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'))
}

fn atomic_symlink_replace(target: &str, link_path: &Path) -> Result<(), ()> {
    #[cfg(unix)]
    {
        let tmp = PathBuf::from(format!(
            "{}.tmp.{}",
            link_path.to_string_lossy(),
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(target, &tmp).map_err(|_| ())?;
        if std::fs::rename(&tmp, link_path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return Err(());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link_path);
        Err(())
    }
}

fn parse_ascii_digits(b: &[u8]) -> Option<u32> {
    if b.is_empty() || !b.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    std::str::from_utf8(b).ok()?.parse().ok()
}

fn handle_set_system_time(payload: &[u8], hooks: &mut dyn CommandHooks) {
    if payload.is_empty() || payload.len() > 254 {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    let nul = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    let Ok(raw) = std::str::from_utf8(&payload[..nul]) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() != 3 {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    let (ds, ts, zs) = (parts[0].as_bytes(), parts[1].as_bytes(), parts[2]);
    if ds.len() != 10
        || ds[4] != b'-'
        || ds[7] != b'-'
        || ts.len() != 8
        || ts[2] != b':'
        || ts[5] != b':'
        || !is_valid_tz_name(zs)
    {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    let (Some(yr), Some(mon), Some(day)) = (
        parse_ascii_digits(&ds[0..4]).map(|v| v as i32),
        parse_ascii_digits(&ds[5..7]),
        parse_ascii_digits(&ds[8..10]),
    ) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    let (Some(hr), Some(min), Some(sec)) = (
        parse_ascii_digits(&ts[0..2]),
        parse_ascii_digits(&ts[3..5]),
        parse_ascii_digits(&ts[6..8]),
    ) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    if yr < 1970
        || !(1..=12).contains(&mon)
        || day < 1
        || day > days_in_month(yr, mon)
        || hr > 23
        || min > 59
        || sec > 59
    {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    let utc_sec = days_from_civil(yr as i64, mon as i64, day as i64) * 86_400
        + (hr as i64) * 3600
        + (min as i64) * 60
        + (sec as i64);

    let root = sysroot_prefix();
    let zoneinfo = PathBuf::from(format!("{root}/usr/share/zoneinfo/{zs}"));
    let localtime = PathBuf::from(format!("{root}/etc/localtime"));
    if !is_tzif_file(&zoneinfo) {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    let Ok(prev) = PreviousLocaltime::save(&localtime) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    let zone_target = format!("/usr/share/zoneinfo/{zs}");
    if atomic_symlink_replace(&zone_target, &localtime).is_err() {
        prev.restore(&localtime);
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    if hooks.clock_set(utc_sec).is_err() {
        prev.restore(&localtime);
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    prev.discard();
    bridge_send_response(hooks, RESP_ACK, &[]);
}

/// Whether `path` is a readable file in the compiled time zone format (it starts with `TZif`).
fn is_tzif_file(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    if !f.metadata().map(|m| m.is_file()).unwrap_or(false) {
        return false;
    }
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).is_ok() && &magic == b"TZif"
}

/// What `/etc/localtime` was before a time zone change, so a failed change can put it back.
enum PreviousLocaltime {
    Absent,
    Symlink(PathBuf),
    /// A regular file, copied aside to this path.
    File(PathBuf),
}

impl PreviousLocaltime {
    /// Records the current state. Anything other than a symlink, a regular file or nothing is
    /// refused, so the change never replaces something it couldn't restore.
    fn save(localtime: &Path) -> Result<Self, ()> {
        let meta = match std::fs::symlink_metadata(localtime) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::Absent),
            Err(_) => return Err(()),
        };
        if meta.file_type().is_symlink() {
            return std::fs::read_link(localtime)
                .map(Self::Symlink)
                .map_err(|_| ());
        }
        if meta.is_file() {
            let backup = PathBuf::from(format!(
                "{}.prev.{}",
                localtime.to_string_lossy(),
                std::process::id()
            ));
            if std::fs::copy(localtime, &backup).is_err() {
                let _ = std::fs::remove_file(&backup);
                return Err(());
            }
            return Ok(Self::File(backup));
        }
        Err(())
    }

    fn restore(self, localtime: &Path) {
        match self {
            Self::Absent => {
                let _ = std::fs::remove_file(localtime);
            }
            Self::Symlink(target) => {
                let _ = atomic_symlink_replace(&target.to_string_lossy(), localtime);
            }
            Self::File(backup) => {
                if std::fs::rename(&backup, localtime).is_err() {
                    let _ = std::fs::remove_file(&backup);
                }
            }
        }
    }

    fn discard(self) {
        if let Self::File(backup) = self {
            let _ = std::fs::remove_file(backup);
        }
    }
}

fn handle_stat_file(payload: &[u8], hooks: &mut dyn CommandHooks) {
    if payload.is_empty() {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    }
    let nul = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    let raw = String::from_utf8_lossy(&payload[..nul]);
    let Ok(resolved) = resolve_host_fs_path(&raw) else {
        bridge_send_response(hooks, RESP_NAK, &[]);
        return;
    };
    match std::fs::symlink_metadata(&resolved) {
        Ok(m) => {
            let msg = format!("exists:{}\0", m.len());
            bridge_send_response(hooks, RESP_ACK, msg.as_bytes());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bridge_send_response(hooks, RESP_ACK, b"absent\0");
        }
        Err(_) => {
            bridge_send_response(hooks, RESP_NAK, &[]);
        }
    }
}

pub fn bridge_handle_command(cmd: u8, payload: &[u8], hooks: &mut dyn CommandHooks) {
    match cmd {
        CMD_REQUEST_BRIDGE_VERSION => {
            let mut b = BRIDGE_VERSION.as_bytes().to_vec();
            b.push(0);
            bridge_send_response(hooks, RESP_ACK, &b);
        }
        CMD_REQUEST_DISPLAY_VERSION => {
            send_text_file_or_default(hooks, &[DISPLAY_VERSION_FILE], DISPLAY_VERSION_FALLBACK);
        }
        CMD_REQUEST_OS_VERSION => {
            send_text_file_or_default(hooks, &[OS_VERSION_FILE], OS_VERSION_FALLBACK)
        }
        CMD_SET_SYSTEM_TIME => handle_set_system_time(payload, hooks),
        CMD_GET_LIST_OF_FILES => handle_list_files(payload, hooks),
        CMD_SEND_FILE_TO_HOST => handle_send_file_to_host(payload, hooks),
        CMD_SEND_FILE_FROM_HOST => handle_put_file_from_host(payload, hooks),
        CMD_DELETE_FILE => handle_delete_file(payload, hooks),
        CMD_GET_FREE_DISK_SPACE => handle_get_free_space(hooks),
        CMD_STAT_FILE => handle_stat_file(payload, hooks),
        CMD_SEND_LIVE_DATA => handle_send_live_data(payload, hooks),
        CMD_GET_MEDIA_STATUS => send_text_file_or_default(hooks, &[MEDIA_STATUS_FILE], ""),
        CMD_GET_SHORTCUT_CONTEXT => {
            send_text_file_or_default(hooks, &[SHORTCUT_CONTEXT_FILE], "Unknown")
        }
        _ => {
            bridge_send_response(hooks, RESP_NAK, &[]);
        }
    }
}

pub fn bridge_publish_trial_marker() {
    let Ok(token) = std::env::var("OPENFLUX_TRIAL_ATTEMPT_ID") else {
        return;
    };
    let trimmed = token.trim();
    if !trimmed.is_empty() {
        bridge_publish_trial_marker_with_version(BRIDGE_VERSION, Some(trimmed));
    }
}

pub fn bridge_publish_trial_marker_with_version(own_version: &str, token_attempt_id: Option<&str>) {
    let Some(token) = token_attempt_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let Ok(rb_dir) = resolve_host_fs_path(BRIDGE_ROLLBACK_DIR) else {
        return;
    };
    let armed = rb_dir.join("armed");
    if !armed.is_file() {
        return;
    }
    let attempt_path = rb_dir.join("attempt");
    let Ok(raw) = std::fs::read_to_string(&attempt_path) else {
        return;
    };
    let mut attempt_id = None;
    let mut target_ver = None;
    for line in raw.lines() {
        if let Some(v) = line.strip_prefix("attempt_id=") {
            attempt_id = Some(v.trim());
        } else if let Some(v) = line.strip_prefix("version=") {
            target_ver = Some(v.trim());
        }
    }
    let (Some(aid), Some(ver)) = (attempt_id, target_ver) else {
        return;
    };
    if aid.is_empty()
        || aid.len() > 64
        || !aid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || aid != token
        || ver != own_version
    {
        return;
    }

    let trial_path = rb_dir.join(format!("trial-{aid}"));
    let tmp_path = rb_dir.join(format!("trial-{aid}.tmp.{}", std::process::id()));
    let body = format!("version={own_version}\npid={}\n", std::process::id());
    let Ok(mut f) = std::fs::File::create(&tmp_path) else {
        return;
    };
    if f.write_all(body.as_bytes()).is_err() || f.sync_all().is_err() {
        drop(f);
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    drop(f);
    if std::fs::rename(&tmp_path, &trial_path).is_err() {
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    if let Ok(dir) = std::fs::File::open(&rb_dir) {
        let _ = dir.sync_all();
    }
}
