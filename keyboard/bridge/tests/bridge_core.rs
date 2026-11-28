use keyglass_bridge::ffs_aio::FfsTxQueue;
use keyglass_bridge::protocol::{
    BRIDGE_PACKET_SIZE, CMD_DELETE_FILE, CMD_GET_LIST_OF_FILES, CMD_REQUEST_BRIDGE_VERSION,
    CMD_REQUEST_DISPLAY_VERSION, CMD_SEND_FILE_FROM_HOST, CMD_SEND_FILE_TO_HOST,
    CMD_SEND_LIVE_DATA, CMD_SET_SYSTEM_TIME, CMD_STAT_FILE, FW_MSG_QC_DATA_LEN, FW_MSG_UPDATE_LEN,
    RESP_ACK, RESP_NAK,
};
use keyglass_bridge::simple_hdlc::{simple_hdlc_encode, SimpleHdlcDecoder};
use keyglass_bridge::uart_bridge::UartBridge;
use keyglass_bridge::usb_commands::{
    bridge_handle_command, bridge_publish_trial_marker_with_version, bridge_resolve_path,
    CommandHooks,
};
use keyglass_bridge::usb_rx::{BridgeRxState, RxMode};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

static SYSROOT_LOCK: Mutex<()> = Mutex::new(());

struct SysrootGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<OsString>,
}

impl SysrootGuard {
    fn enter(path: &Path) -> Self {
        let lock = SYSROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("KEYGLASS_SYSROOT");
        std::env::set_var("KEYGLASS_SYSROOT", path);
        Self { _lock: lock, prev }
    }
}

impl Drop for SysrootGuard {
    fn drop(&mut self) {
        if let Some(ref p) = self.prev {
            std::env::set_var("KEYGLASS_SYSROOT", p);
        } else {
            std::env::remove_var("KEYGLASS_SYSROOT");
        }
    }
}

struct MockHooks {
    tx: Vec<u8>,
    zlp_count: usize,
    reserve_limit: Option<usize>,
    clock_rc: Result<(), String>,
    last_clock: Option<i64>,
    live_seen: Vec<Vec<u8>>,
}

impl MockHooks {
    fn new() -> Self {
        Self {
            tx: Vec::new(),
            zlp_count: 0,
            reserve_limit: None,
            clock_rc: Ok(()),
            last_clock: None,
            live_seen: Vec::new(),
        }
    }
    fn reset(&mut self) {
        self.tx.clear();
        self.zlp_count = 0;
    }
}

impl CommandHooks for MockHooks {
    fn send_packet(&mut self, pkt: Option<&[u8]>) -> bool {
        match pkt {
            None => self.zlp_count += 1,
            Some(b) => self.tx.extend_from_slice(b),
        }
        true
    }
    fn reserve_packets(&mut self, total_packets: usize) -> bool {
        if let Some(lim) = self.reserve_limit {
            total_packets <= lim
        } else {
            true
        }
    }
    fn on_live_data(&mut self, data: &[u8]) {
        self.live_seen.push(data.to_vec());
    }
    fn clock_set(&mut self, utc_sec: i64) -> Result<(), String> {
        self.last_clock = Some(utc_sec);
        self.clock_rc.clone()
    }
}

fn make_hdr(cmd: u8, packets: u32, bytes: u32) -> Vec<u8> {
    let mut h = vec![0u8; BRIDGE_PACKET_SIZE];
    h[0] = cmd;
    h[1..5].copy_from_slice(&packets.to_le_bytes());
    h[5..9].copy_from_slice(&bytes.to_le_bytes());
    h
}

#[test]
fn path_sandbox_commands_framing_uart_ipc_and_streaming_upload() {
    // 1. Path sandbox
    assert_eq!(bridge_resolve_path(".").unwrap(), "/home/root");
    assert_eq!(
        bridge_resolve_path("foo.ipk").unwrap(),
        "/home/root/foo.ipk"
    );
    assert_eq!(
        bridge_resolve_path("/home/root/keyglass/config/keymap.json").unwrap(),
        "/home/root/keyglass/config/keymap.json"
    );
    assert_eq!(
        bridge_resolve_path("/home/root/.keyglass/rollback/bridge/attempt").unwrap(),
        "/home/root/.keyglass/rollback/bridge/attempt"
    );
    assert_eq!(bridge_resolve_path("/home").unwrap(), "/home");
    assert!(bridge_resolve_path("/homes/root/x").is_err());
    assert!(bridge_resolve_path("/etc/passwd").is_err());
    assert!(bridge_resolve_path("/home/root/../etc/passwd").is_err());
    assert!(bridge_resolve_path("foo\\bar").is_err());

    // 2. Version + unknown command
    let mut hooks = MockHooks::new();
    bridge_handle_command(CMD_REQUEST_BRIDGE_VERSION, &[], &mut hooks);
    assert_eq!(hooks.zlp_count, 1);
    assert_eq!(hooks.tx.len(), BRIDGE_PACKET_SIZE * 2);
    assert_eq!(hooks.tx[0], RESP_ACK);
    assert!(hooks.tx[BRIDGE_PACKET_SIZE..].starts_with(b"keyglass-bridge "));

    let tmp_ver = std::env::temp_dir().join(format!("keyglass_ver_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_ver);
    std::fs::create_dir_all(tmp_ver.join("home/root/.keyglass/versions")).unwrap();
    {
        let _sysroot = SysrootGuard::enter(&tmp_ver);
        hooks.reset();
        bridge_handle_command(CMD_REQUEST_DISPLAY_VERSION, &[], &mut hooks);
        assert_eq!(hooks.tx[0], RESP_ACK);
        assert!(hooks.tx[BRIDGE_PACKET_SIZE..].starts_with(b"unknown\0"));
        std::fs::write(
            tmp_ver.join("home/root/.keyglass/versions/display"),
            "keyglass-display 1.0.0\n",
        )
        .unwrap();
        hooks.reset();
        bridge_handle_command(CMD_REQUEST_DISPLAY_VERSION, &[], &mut hooks);
        assert!(hooks.tx[BRIDGE_PACKET_SIZE..].starts_with(b"keyglass-display 1.0.0\0"));
    }
    let _ = std::fs::remove_dir_all(&tmp_ver);

    hooks.reset();
    bridge_handle_command(250, &[], &mut hooks);
    assert_eq!(hooks.tx.len(), BRIDGE_PACKET_SIZE);
    assert_eq!(hooks.tx[0], RESP_NAK);

    // 3. RX short-read + discarding recovery
    let mut rx = BridgeRxState::new();
    let mut short_buf = make_hdr(CMD_SEND_FILE_FROM_HOST, 2, 700);
    short_buf.extend(vec![b'A'; BRIDGE_PACKET_SIZE]);
    hooks.reset();
    rx.feed(&short_buf, true, &mut hooks);
    assert_eq!(rx.mode, RxMode::Idle);
    assert_eq!(hooks.tx[0], RESP_NAK);

    hooks.reset();
    rx.feed(
        &make_hdr(CMD_REQUEST_BRIDGE_VERSION, 0, 0),
        true,
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_ACK);

    // 4. Sysroot file put/get/list + clock set rollback
    let tmp = std::env::temp_dir().join(format!("keyglass_bridge_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("home/root/keyglass/config")).unwrap();
    std::fs::create_dir_all(tmp.join("usr/share/zoneinfo/America")).unwrap();
    std::fs::create_dir_all(tmp.join("usr/share/zoneinfo/Europe")).unwrap();
    std::fs::create_dir_all(tmp.join("etc")).unwrap();
    std::fs::write(tmp.join("usr/share/zoneinfo/America/New_York"), b"TZif2").unwrap();
    std::fs::write(tmp.join("usr/share/zoneinfo/Europe/London"), b"TZif2").unwrap();
    let sysroot_guard = SysrootGuard::enter(&tmp);

    let put = b"/home/root/keyglass/config/hello.txt\0world!".to_vec();
    hooks.reset();
    bridge_handle_command(CMD_SEND_FILE_FROM_HOST, &put, &mut hooks);
    assert_eq!(hooks.tx[0], RESP_ACK);
    hooks.reset();
    bridge_handle_command(
        CMD_SEND_FILE_TO_HOST,
        b"/home/root/keyglass/config/hello.txt",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_ACK);
    assert!(hooks.tx[BRIDGE_PACKET_SIZE..].starts_with(b"world!"));

    hooks.reset();
    bridge_handle_command(
        CMD_GET_LIST_OF_FILES,
        b"/home/root/keyglass/config",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_ACK);
    assert!(String::from_utf8_lossy(&hooks.tx[BRIDGE_PACKET_SIZE..]).contains("6:hello.txt"));

    // Streaming >16 MiB (18 MiB) file upload directly through BridgeRxState without full-file RAM allocation
    {
        let path_prefix = b"/home/root/keyglass/config/big_video.bin\0";
        let file_sz = 18 * 1024 * 1024usize;
        let total_bytes = path_prefix.len() + file_sz;
        let total_pkts = total_bytes.div_ceil(BRIDGE_PACKET_SIZE);
        let hdr = make_hdr(
            CMD_SEND_FILE_FROM_HOST,
            total_pkts as u32,
            total_bytes as u32,
        );
        hooks.reset();
        rx.feed(&hdr, false, &mut hooks);
        assert_eq!(rx.mode, RxMode::Receiving);

        // First chunk carries the NUL-terminated destination path plus initial data
        let mut chunk = vec![0u8; 64 * 1024];
        chunk[..path_prefix.len()].copy_from_slice(path_prefix);
        for (idx, b) in chunk[path_prefix.len()..].iter_mut().enumerate() {
            *b = ((idx + 7) % 251) as u8;
        }
        rx.feed(&chunk, false, &mut hooks);

        let mut sent = chunk.len();
        let total_wire = total_pkts * BRIDGE_PACKET_SIZE;
        while sent < total_wire {
            let take = (total_wire - sent).min(64 * 1024);
            let mut body = vec![0xA5u8; take];
            if sent + take == total_wire && total_wire > total_bytes {
                let pad_start = take - (total_wire - total_bytes);
                for b in &mut body[pad_start..] {
                    *b = 0;
                }
            }
            sent += take;
            rx.feed(&body, sent >= total_wire, &mut hooks);
        }
        assert_eq!(rx.mode, RxMode::Idle);
        assert_eq!(hooks.tx[0], RESP_ACK);
        let meta = std::fs::metadata(tmp.join("home/root/keyglass/config/big_video.bin")).unwrap();
        assert_eq!(meta.len(), file_sz as u64);
        let _ = std::fs::remove_file(tmp.join("home/root/keyglass/config/big_video.bin"));
    }

    // Reserve limit overflow sends header-only NAK
    hooks.reserve_limit = Some(1);
    hooks.reset();
    bridge_handle_command(
        CMD_SEND_FILE_TO_HOST,
        b"/home/root/keyglass/config/hello.txt",
        &mut hooks,
    );
    assert_eq!(hooks.tx.first().copied(), Some(RESP_NAK));
    hooks.reserve_limit = None;

    // Reserve within USB_TX_MAX_PACKETS works; exceeding USB_TX_MAX_PACKETS fails.
    let mut tx_big = FfsTxQueue::init(0, false).unwrap();
    assert!(tx_big.reserve(35_000));
    assert!(!tx_big.reserve(keyglass_bridge::ffs_aio::USB_TX_MAX_PACKETS + 1));

    // Clock rollback when /etc/localtime is initially absent
    hooks.clock_rc = Err("EPERM".into());
    hooks.last_clock = None;
    hooks.reset();
    bridge_handle_command(
        CMD_SET_SYSTEM_TIME,
        b"2026-11-18 14:30:00 America/New_York",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_NAK);
    assert!(hooks.last_clock.is_some());
    assert!(std::fs::read_link(tmp.join("etc/localtime")).is_err());

    // Set America/New_York, then verify a failed Europe/London update restores it.
    hooks.clock_rc = Ok(());
    hooks.reset();
    bridge_handle_command(
        CMD_SET_SYSTEM_TIME,
        b"2026-11-18 14:30:00 America/New_York",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_ACK);

    hooks.clock_rc = Err("EPERM".into());
    hooks.last_clock = None;
    hooks.reset();
    bridge_handle_command(
        CMD_SET_SYSTEM_TIME,
        b"2026-11-18 14:30:00 Europe/London",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_NAK);
    assert!(hooks.last_clock.is_some());
    hooks.reset();
    bridge_handle_command(
        CMD_SET_SYSTEM_TIME,
        "202é-1-01 00:00:00 America/New_York".as_bytes(),
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_NAK);
    hooks.reset();
    bridge_handle_command(
        CMD_SET_SYSTEM_TIME,
        b"2026-+1-18 14:30:00 America/New_York",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_NAK);
    assert_eq!(
        std::fs::read_link(tmp.join("etc/localtime"))
            .unwrap()
            .to_string_lossy(),
        "/usr/share/zoneinfo/America/New_York"
    );

    // A zone name that names a directory or a file that isn't time zone data is refused
    // without touching /etc/localtime.
    std::fs::write(tmp.join("usr/share/zoneinfo/Europe/Notes"), b"not tz data").unwrap();
    hooks.clock_rc = Ok(());
    for payload in [
        &b"2026-11-18 14:30:00 America"[..],
        &b"2026-11-18 14:30:00 Europe/Notes"[..],
    ] {
        hooks.last_clock = None;
        hooks.reset();
        bridge_handle_command(CMD_SET_SYSTEM_TIME, payload, &mut hooks);
        assert_eq!(hooks.tx[0], RESP_NAK);
        assert!(hooks.last_clock.is_none());
        assert_eq!(
            std::fs::read_link(tmp.join("etc/localtime")).unwrap(),
            Path::new("/usr/share/zoneinfo/America/New_York")
        );
    }

    // A regular /etc/localtime file survives a failed update unchanged, and a successful one
    // replaces it with a link.
    std::fs::remove_file(tmp.join("etc/localtime")).unwrap();
    std::fs::write(tmp.join("etc/localtime"), b"TZif2 original").unwrap();
    hooks.clock_rc = Err("EPERM".into());
    hooks.reset();
    bridge_handle_command(
        CMD_SET_SYSTEM_TIME,
        b"2026-11-18 14:30:00 Europe/London",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_NAK);
    let meta = std::fs::symlink_metadata(tmp.join("etc/localtime")).unwrap();
    assert!(meta.is_file());
    assert_eq!(
        std::fs::read(tmp.join("etc/localtime")).unwrap(),
        b"TZif2 original"
    );
    hooks.clock_rc = Ok(());
    hooks.reset();
    bridge_handle_command(
        CMD_SET_SYSTEM_TIME,
        b"2026-11-18 14:30:00 Europe/London",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_ACK);
    assert_eq!(
        std::fs::read_link(tmp.join("etc/localtime")).unwrap(),
        Path::new("/usr/share/zoneinfo/Europe/London")
    );
    let leftovers: Vec<_> = std::fs::read_dir(tmp.join("etc"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "localtime")
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // 5. SimpleHDLC + canonical 139 B UART updates -> DisplayIPC Unix socket
    let mut uart = UartBridge::new();
    let mut msg = vec![0u8; FW_MSG_UPDATE_LEN];
    msg[0] = 2u8;
    msg[1..5].copy_from_slice(&42u32.to_le_bytes());
    msg[5] = 0xAA;
    msg[5 + 0x24] = 1; // ISO
    msg[5 + 0x25] = 1; // attached

    // Without the ordered-event marker (0x2F == 0), the line omits of_events_seq and of_events_ring.
    msg[5 + 0x5C] = 0x07; // Num + Caps + Scroll Lock
    msg[5 + 0x65..5 + 0x69].copy_from_slice(&2u32.to_le_bytes()); // active_profile = 2
    msg[5 + 0x6A] = 2;
    msg[5 + 0x6B] = 1;
    msg[5 + 0x84..5 + 0x86].copy_from_slice(&0x0304u16.to_le_bytes());
    let enc = simple_hdlc_encode(&msg).unwrap();

    let sock_path = tmp.join("bridge.sock");
    let mut ipc = keyglass_bridge::display_ipc::DisplayIpc::new();
    ipc.init(Some(sock_path.to_str().unwrap())).unwrap();
    let mut cli = UnixStream::connect(&sock_path).unwrap();
    cli.set_nonblocking(true).unwrap();
    ipc.accept();

    uart.feed_bytes(&enc, |st, mt| ipc.publish_uart(st, mt));
    assert!(uart.state.valid);
    assert_eq!(uart.state.seq, 42);
    assert_eq!(uart.state.active_profile, 2);
    assert_eq!(uart.state.host_state_status, 0x07);
    assert_eq!(uart.state.module_buttons, 0x0304);

    let mut sbuf = [0u8; 1024];
    let sn = cli.read(&mut sbuf).unwrap();
    let line = std::str::from_utf8(&sbuf[..sn]).unwrap();
    assert!(line.contains("\"type\":\"key_state\""));
    assert!(line.contains("\"seq\":42"));
    assert!(line.contains("\"active_profile\":2"));
    assert!(line.contains("\"caps_lock\":true"));
    assert!(line.contains("\"num_lock\":true"));
    assert!(line.contains("\"scroll_lock\":true"));
    assert!(!line.contains("of_events_seq"));

    // With the ordered-event marker (0x2F == 1), ordered-event fields appear even when seq is 0.
    let mut msg_v1 = msg.clone();
    msg_v1[5 + 0x2F] = 1;
    msg_v1[5 + 0x30..5 + 0x32].copy_from_slice(&0u16.to_le_bytes());
    uart.feed_bytes(&simple_hdlc_encode(&msg_v1).unwrap(), |st, mt| {
        ipc.publish_uart(st, mt)
    });
    let sn_v1 = cli.read(&mut sbuf).unwrap();
    let line_v1 = std::str::from_utf8(&sbuf[..sn_v1]).unwrap();
    assert!(line_v1.contains("\"of_events_seq\":0"));
    assert!(line_v1.contains("\"of_events_ring\":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]"));

    let mut qc = vec![0u8; FW_MSG_QC_DATA_LEN];
    qc[0] = 3u8;
    qc[1..5].copy_from_slice(&99u32.to_le_bytes());
    uart.feed_bytes(&simple_hdlc_encode(&qc).unwrap(), |st, mt| {
        ipc.publish_uart(st, mt)
    });
    let sn_qc = cli.read(&mut sbuf).unwrap();
    let line_qc = std::str::from_utf8(&sbuf[..sn_qc]).unwrap();
    assert!(line_qc.contains("\"type\":\"key_qc\""));
    assert!(line_qc.contains("\"seq\":99"));

    // Live data command -> IPC broadcast + 1-byte status
    let live_json = b"\0{\"cpu\":\"44%\"}";
    let mut len_hdr = vec![0xFFu8];
    len_hdr.extend_from_slice(&(live_json.len() as u32).to_be_bytes());
    let mut live_wire = simple_hdlc_encode(&len_hdr).unwrap();
    live_wire.extend(simple_hdlc_encode(live_json).unwrap());

    hooks.reset();
    bridge_handle_command(CMD_SEND_LIVE_DATA, &live_wire, &mut hooks);
    assert_eq!(hooks.tx[0], RESP_ACK);
    let resp_len = u32::from_le_bytes(hooks.tx[5..9].try_into().unwrap()) as usize;
    assert_eq!(resp_len, 1);
    assert_eq!(hooks.tx[BRIDGE_PACKET_SIZE], 1);
    assert_eq!(hooks.live_seen.len(), 1);
    ipc.feed_live_data(&hooks.live_seen[0]);
    let sn2 = cli.read(&mut sbuf).unwrap();
    assert!(std::str::from_utf8(&sbuf[..sn2]).unwrap().contains("44%"));

    // SimpleHDLC decode check
    let mut dec = SimpleHdlcDecoder::new();
    let mut got = Vec::new();
    dec.feed(&enc, |f| got.push(f.to_vec()));
    assert_eq!(got.len(), 1);
    assert_eq!(got[0], msg);

    // 6. Two IPC clients: batching two get_state commands on client 1 when reply fails never leaks to client 2
    let mut cli_a = UnixStream::connect(&sock_path).unwrap();
    let mut cli_b = UnixStream::connect(&sock_path).unwrap();
    ipc.accept();
    cli_a
        .write_all(b"{\"cmd\":\"get_state\"}\n{\"cmd\":\"get_state\"}\n")
        .unwrap();
    cli_a.shutdown(std::net::Shutdown::Both).unwrap();
    let fds = ipc.client_fds();
    let a_fd = fds[fds.len() - 2];
    ipc.read_client_by_fd(a_fd, &mut uart);
    let mut b_buf = [0u8; 256];
    assert!(cli_b.set_nonblocking(true).is_ok());
    assert!(cli_b.read(&mut b_buf).is_err());

    // 7. CMD_DELETE_FILE is idempotent for NotFound + CMD_STAT_FILE distinguishes absent vs exists
    hooks.reset();
    bridge_handle_command(
        CMD_STAT_FILE,
        b"/home/root/keyglass/config/hello.txt",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_ACK);
    assert!(String::from_utf8_lossy(&hooks.tx[BRIDGE_PACKET_SIZE..]).starts_with("exists:"));
    hooks.reset();
    bridge_handle_command(
        CMD_STAT_FILE,
        b"/home/root/nonexistent_sentinel",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_ACK);
    assert!(String::from_utf8_lossy(&hooks.tx[BRIDGE_PACKET_SIZE..]).starts_with("absent"));

    hooks.reset();
    bridge_handle_command(
        CMD_DELETE_FILE,
        b"/home/root/nonexistent_sentinel",
        &mut hooks,
    );
    assert_eq!(hooks.tx[0], RESP_ACK);

    // 8. Trial marker publication gates confirmation on matching attempt token + version + armed state
    let rb_dir = tmp.join("home/root/.keyglass/rollback/bridge");
    std::fs::create_dir_all(&rb_dir).unwrap();
    std::fs::write(
        rb_dir.join("attempt"),
        "attempt_id=abc123\nversion=keyglass-bridge 1.2.3\n",
    )
    .unwrap();
    // Without `armed`, nothing is written even with matching token.
    bridge_publish_trial_marker_with_version("keyglass-bridge 1.2.3", Some("abc123"));
    assert!(!rb_dir.join("trial-abc123").exists());
    // With `armed`, a running bridge of another version or a missing/wrong token publishes no trial marker.
    std::fs::write(rb_dir.join("armed"), "pending-host-usb-confirm\n").unwrap();
    bridge_publish_trial_marker_with_version("keyglass-bridge 1.0.0", Some("abc123"));
    assert!(!rb_dir.join("trial-abc123").exists());
    // A bridge of the same version started without the postinst attempt token can't publish for this attempt.
    bridge_publish_trial_marker_with_version("keyglass-bridge 1.2.3", None);
    assert!(!rb_dir.join("trial-abc123").exists());
    bridge_publish_trial_marker_with_version("keyglass-bridge 1.2.3", Some("oldtoken"));
    assert!(!rb_dir.join("trial-abc123").exists());
    // Matching target version + postinst token writes trial-<attempt_id> cleanly.
    bridge_publish_trial_marker_with_version("keyglass-bridge 1.2.3", Some("abc123"));
    let trial_body = std::fs::read_to_string(rb_dir.join("trial-abc123")).unwrap();
    assert!(trial_body.contains("version=keyglass-bridge 1.2.3"));
    // Traversal attempt_id is rejected.
    std::fs::write(
        rb_dir.join("attempt"),
        "attempt_id=../escape\nversion=keyglass-bridge 1.2.3\n",
    )
    .unwrap();
    bridge_publish_trial_marker_with_version("keyglass-bridge 1.2.3", Some("../escape"));
    assert!(!rb_dir.join("../escape").exists());

    drop(sysroot_guard);
    let _ = std::fs::remove_dir_all(&tmp);
}

fn live_wire(message: &[u8]) -> Vec<u8> {
    let mut len_hdr = vec![0xFFu8];
    len_hdr.extend_from_slice(&(message.len() as u32).to_be_bytes());
    let mut wire = simple_hdlc_encode(&len_hdr).unwrap();
    wire.extend(simple_hdlc_encode(message).unwrap());
    wire
}

#[test]
fn live_data_reaches_display_clients_only_as_single_valid_json_lines() {
    let tmp = std::env::temp_dir().join(format!("keyglass_live_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let sock_path = tmp.join("bridge.sock");
    let mut ipc = keyglass_bridge::display_ipc::DisplayIpc::new();
    ipc.init(Some(sock_path.to_str().unwrap())).unwrap();
    let mut cli = UnixStream::connect(&sock_path).unwrap();
    cli.set_nonblocking(true).unwrap();
    ipc.accept();

    // Not JSON, a JSON value that isn't an object, and an object followed by an
    // injected second line: none of them reaches the display.
    ipc.feed_live_data(&live_wire(b"\0not json"));
    ipc.feed_live_data(&live_wire(b"\x0042"));
    ipc.feed_live_data(&live_wire(
        b"\0{\"cpu\":1}\n{\"type\":\"key_state\",\"seq\":1}",
    ));
    ipc.feed_live_data(&live_wire(b"\x07{}"));
    let mut buf = [0u8; 4096];
    assert!(
        cli.read(&mut buf).is_err(),
        "rejected live data must not be broadcast"
    );

    // A JSON object whose strings contain newlines arrives as one escaped line,
    // followed by a heartbeat.
    ipc.feed_live_data(&live_wire(b"\0{\"mediaFile\":{\"title\":\"a\\nb\"}}"));
    ipc.feed_live_data(&live_wire(b"\x04"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    let n = cli.read(&mut buf).unwrap();
    let text = std::str::from_utf8(&buf[..n]).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["type"], "live_data");
    assert_eq!(first["data_type"], 0);
    assert_eq!(first["payload"]["mediaFile"]["title"], "a\nb");
    assert_eq!(lines[1], "{\"type\":\"live_data\",\"data_type\":4}");

    drop(ipc);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn streaming_put_file_rejects_over_limit_cleans_partial_on_abort_and_naks_oversized_get() {
    let tmp = std::env::temp_dir().join(format!("keyglass_stream_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("home/root/keyglass/media")).unwrap();
    let sysroot_guard = SysrootGuard::enter(&tmp);

    let mut hooks = MockHooks::new();
    let mut rx = BridgeRxState::new();

    // 1. Over-limit file upload (>128 MiB) must be rejected immediately with NAK
    let over_bytes = (keyglass_bridge::protocol::BRIDGE_MAX_FILE_UPLOAD_BYTES + 1) as u32;
    let over_pkts = (over_bytes as usize).div_ceil(BRIDGE_PACKET_SIZE) as u32;
    rx.feed(
        &make_hdr(CMD_SEND_FILE_FROM_HOST, over_pkts, over_bytes),
        true,
        &mut hooks,
    );
    assert_eq!(rx.mode, RxMode::Idle);
    assert_eq!(hooks.tx[0], RESP_NAK);

    // 2. Chunked streaming PUT_FILE interrupted mid-transfer must NAK and leave zero .part files
    let path_hdr = b"/home/root/keyglass/media/aborted.mp4\0";
    let total_bytes = path_hdr.len() + 2048;
    let total_pkts = total_bytes.div_ceil(BRIDGE_PACKET_SIZE);
    let mut first = make_hdr(
        CMD_SEND_FILE_FROM_HOST,
        total_pkts as u32,
        total_bytes as u32,
    );
    first.extend_from_slice(path_hdr);
    first.extend_from_slice(&vec![0x5Au8; 600]);
    hooks.reset();
    rx.feed(&first, false, &mut hooks);
    assert_eq!(rx.mode, RxMode::Receiving);
    // Abort mid-transfer (e.g. USB disconnect / ZLP termination)
    rx.abort(&mut hooks);
    assert_eq!(rx.mode, RxMode::Idle);
    assert_eq!(hooks.tx[0], RESP_NAK);
    assert!(!tmp.join("home/root/keyglass/media/aborted.mp4").exists());
    for ent in std::fs::read_dir(tmp.join("home/root/keyglass/media")).unwrap() {
        let name = ent.unwrap().file_name().to_string_lossy().to_string();
        assert!(!name.contains(".part."), "leftover partial file: {name}");
    }

    // 3. Oversized GET_FILE exceeding TX packet reserve sends immediate header-only NAK
    let big_path = tmp.join("home/root/keyglass/media/toobig.bin");
    std::fs::write(&big_path, vec![0x11u8; 4096]).unwrap();
    hooks.reserve_limit = Some(2);
    hooks.reset();
    bridge_handle_command(
        CMD_SEND_FILE_TO_HOST,
        b"/home/root/keyglass/media/toobig.bin",
        &mut hooks,
    );
    assert_eq!(hooks.tx.len(), BRIDGE_PACKET_SIZE);
    assert_eq!(hooks.tx[0], RESP_NAK);
    assert_eq!(hooks.zlp_count, 1);

    // 4. >64 MiB (65 MiB) and exactly 128 MiB readbacks succeed with bounded queue memory; 128 MiB + 1 NAKs.
    // Also verify FfsTxQueue::enqueue_stream with a stalled pipe + 257-byte short reads + premature EOF abort.
    {
        use keyglass_bridge::ffs_aio::PendingTxStream;
        use std::os::unix::io::FromRawFd;

        // 4a. 257-byte short-read injection produces exact contiguous payload with zero mid-stream padding.
        struct ChunkyReader {
            data: Vec<u8>,
            pos: usize,
            max_step: usize,
        }
        impl std::io::Read for ChunkyReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.pos >= self.data.len() {
                    return Ok(0);
                }
                let take = buf.len().min(self.max_step).min(self.data.len() - self.pos);
                buf[..take].copy_from_slice(&self.data[self.pos..self.pos + take]);
                self.pos += take;
                Ok(take)
            }
        }
        let payload_src: Vec<u8> = (0..1300usize).map(|i| ((i * 37 + 3) % 251) as u8).collect();
        let mut ps = PendingTxStream::new(
            Box::new(ChunkyReader {
                data: payload_src.clone(),
                pos: 0,
                max_step: 257,
            }),
            payload_src.len(),
        );
        let mut reassembled = Vec::new();
        let mut zlp_seen = false;
        while let Some(pkt) = ps.next_packet().unwrap() {
            if pkt.len == 0 {
                zlp_seen = true;
            } else {
                assert_eq!(pkt.len, 512);
                reassembled.extend_from_slice(&pkt.data);
            }
        }
        assert!(zlp_seen);
        assert_eq!(&reassembled[..payload_src.len()], &payload_src[..]);
        assert!(reassembled[payload_src.len()..].iter().all(|&b| b == 0));

        // 4b. A truncated stream (EOF before the declared size) returns Err(()), never silent padding.
        let mut ps_trunc = PendingTxStream::new(
            Box::new(ChunkyReader {
                data: vec![0xAA; 600],
                pos: 0,
                max_step: 257,
            }),
            1200,
        );
        assert!(ps_trunc.next_packet().unwrap().is_some()); // first 512B ok
        assert!(
            ps_trunc.next_packet().is_err(),
            "truncated stream must abort with Err"
        );

        // 4c. Bounded-memory FfsTxQueue streaming of 96 MiB + a trailing reply over a stalled, nonblocking pipe.
        struct ZeroGen {
            rem: usize,
        }
        impl std::io::Read for ZeroGen {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = buf.len().min(self.rem);
                buf[..n].fill(0x5Cu8);
                self.rem -= n;
                Ok(n)
            }
        }
        let mut pipe_fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(pipe_fds.as_mut_ptr()) }, 0);
        unsafe {
            let fl = libc::fcntl(pipe_fds[1], libc::F_GETFL, 0);
            libc::fcntl(pipe_fds[1], libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
        let stream_sz = 96 * 1024 * 1024usize;
        let mut tx_q = FfsTxQueue::init(pipe_fds[1], false).unwrap();
        assert!(tx_q.enqueue_stream(Box::new(ZeroGen { rem: stream_sz }), stream_sz));
        assert!(tx_q.enqueue(Some(b"AFTER_STREAM")));
        // The pipe is full (stalled receiver) -> the queue holds O(1) items, not 196,608 packets.
        assert!(
            tx_q.queued_items() <= 4,
            "stalled 96 MiB stream buffered {} items instead of O(1)",
            tx_q.queued_items()
        );

        let mut reader_f = unsafe { std::fs::File::from_raw_fd(pipe_fds[0]) };
        let jh = std::thread::spawn(move || {
            let mut total = 0usize;
            let mut buf = [0u8; 65536];
            let mut tail = Vec::new();
            loop {
                match reader_f.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        total += n;
                        tail.extend_from_slice(&buf[..n]);
                        if tail.len() > 64 {
                            let drop_n = tail.len() - 64;
                            tail.drain(..drop_n);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            (total, tail)
        });

        while tx_q.has_pending() {
            assert!(tx_q.drive() >= 0);
            std::thread::yield_now();
        }
        drop(tx_q);
        unsafe { libc::close(pipe_fds[1]) };
        let (total_read, tail) = jh.join().unwrap();
        assert_eq!(total_read, stream_sz + b"AFTER_STREAM".len());
        assert!(
            tail.ends_with(b"AFTER_STREAM"),
            "trailing reply out of order or missing: {tail:?}"
        );
    }

    struct CountingTxHooks {
        hdr: Option<Vec<u8>>,
        data_pkts: usize,
        zlp_count: usize,
    }
    impl CommandHooks for CountingTxHooks {
        fn send_packet(&mut self, pkt: Option<&[u8]>) -> bool {
            match pkt {
                None => self.zlp_count += 1,
                Some(b) if self.hdr.is_none() => self.hdr = Some(b.to_vec()),
                Some(_) => self.data_pkts += 1,
            }
            true
        }
        fn reserve_file_packets(&mut self, _total_packets: usize) -> bool {
            true
        }
    }

    for (fname, size_bytes, expect_ok) in [
        ("65mib.bin", 65 * 1024 * 1024usize, true),
        (
            "128mib.bin",
            keyglass_bridge::protocol::BRIDGE_MAX_FILE_UPLOAD_BYTES,
            true,
        ),
        (
            "over128mib.bin",
            keyglass_bridge::protocol::BRIDGE_MAX_FILE_UPLOAD_BYTES + 1,
            false,
        ),
    ] {
        let p = tmp.join("home/root/keyglass/media").join(fname);
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(size_bytes as u64).unwrap();
        drop(f);

        let mut ch = CountingTxHooks {
            hdr: None,
            data_pkts: 0,
            zlp_count: 0,
        };
        let remote = format!("/home/root/keyglass/media/{fname}");
        bridge_handle_command(CMD_SEND_FILE_TO_HOST, remote.as_bytes(), &mut ch);
        let hdr = ch.hdr.as_ref().expect("missing response header");
        assert_eq!(ch.zlp_count, 1);
        if expect_ok {
            assert_eq!(hdr[0], RESP_ACK, "expected ACK reading back {fname}");
            let exp_pkts = size_bytes / BRIDGE_PACKET_SIZE;
            assert_eq!(
                u32::from_le_bytes(hdr[1..5].try_into().unwrap()) as usize,
                exp_pkts
            );
            assert_eq!(
                u32::from_le_bytes(hdr[5..9].try_into().unwrap()) as usize,
                size_bytes
            );
            assert_eq!(ch.data_pkts, exp_pkts);
        } else {
            assert_eq!(
                hdr[0], RESP_NAK,
                "expected NAK reading back >128 MiB {fname}"
            );
            assert_eq!(ch.data_pkts, 0);
        }
        let _ = std::fs::remove_file(&p);
    }

    drop(sysroot_guard);
    let _ = std::fs::remove_dir_all(&tmp);
}
