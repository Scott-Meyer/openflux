use openflux_bridge::device_paths::{
    BRIDGE_READY_STAMP_DEFAULT, BRIDGE_SOCK_DEFAULT, DISPLAY_ACTIVE_MARKER_DEFAULT,
};
use openflux_bridge::display_ipc::DisplayIpc;
use openflux_bridge::ffs_aio::{FfsAioReader, FfsTxQueue, SubmitOutcome};
use openflux_bridge::protocol::{BRIDGE_VERSION, FW_UART_DEFAULT_DEV};
use openflux_bridge::uart_bridge::UartBridge;
use openflux_bridge::usb_commands::CommandHooks;
use openflux_bridge::usb_gadget::{bridge_open_ffs, bridge_setup_gadget};
use openflux_bridge::usb_rx::BridgeRxState;
use openflux_bridge::v2_mux::V2BridgeMux;
use std::os::unix::io::RawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static RUNNING: AtomicBool = AtomicBool::new(true);
static SIG_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_sig(_sig: libc::c_int) {
    RUNNING.store(false, Ordering::SeqCst);
    let fd = SIG_WRITE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        let b: u8 = 1;
        unsafe { libc::write(fd, &b as *const u8 as *const libc::c_void, 1) };
    }
}

fn set_nonblock(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL, 0);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

fn parse_fd_pair_env(name: &str) -> Result<Option<(RawFd, RawFd)>, ()> {
    let Ok(val) = std::env::var(name) else {
        return Ok(None);
    };
    if val.is_empty() {
        return Ok(None);
    }
    let mut parts = val.split(',');
    let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(());
    };
    let (Ok(fa), Ok(fb)) = (a.trim().parse::<RawFd>(), b.trim().parse::<RawFd>()) else {
        return Err(());
    };
    if fa < 0 || fb < 0 {
        return Err(());
    }
    Ok(Some((fa, fb)))
}

fn ready_file_path() -> String {
    std::env::var("OPENFLUX_READY_FILE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| BRIDGE_READY_STAMP_DEFAULT.into())
}

fn write_ready_stamp(path: &str) {
    let tmp = format!("{path}.tmp.{}", std::process::id());
    if std::fs::write(&tmp, format!("{}\n", std::process::id())).is_ok()
        && std::fs::rename(&tmp, path).is_err()
    {
        let _ = std::fs::remove_file(&tmp);
    }
}

struct LoopHooks<'a> {
    tx_q: &'a mut FfsTxQueue,
    ipc: &'a mut DisplayIpc,
}

impl<'a> CommandHooks for LoopHooks<'a> {
    fn send_packet(&mut self, pkt: Option<&[u8]>) -> bool {
        if !RUNNING.load(Ordering::SeqCst) {
            return false;
        }
        self.tx_q.enqueue(pkt)
    }

    fn reserve_packets(&mut self, total_packets: usize) -> bool {
        self.tx_q.reserve(total_packets)
    }

    fn reserve_file_packets(&mut self, _total_packets: usize) -> bool {
        // A file is a header packet plus one streamed item, whatever its size.
        self.tx_q.reserve(2)
    }

    fn send_stream(&mut self, reader: Box<dyn std::io::Read + Send>, len: usize) -> bool {
        if !RUNNING.load(Ordering::SeqCst) {
            return false;
        }
        self.tx_q.enqueue_stream(reader, len)
    }

    fn on_live_data(&mut self, data: &[u8]) {
        self.ipc.feed_live_data(data);
    }

    fn drain_bus_frames(&mut self) -> Vec<u8> {
        self.ipc.drain_host_bus_frames()
    }
}

/// Queues the next OUT-endpoint read. A vanished endpoint drops the transfers in flight in both
/// directions. Returns false when the reader can no longer be used and the bridge must stop.
fn resubmit_read(aio: &mut FfsAioReader, tx_q: &mut FfsTxQueue, rx: &mut BridgeRxState) -> bool {
    match aio.submit() {
        Err(_) => false,
        Ok(SubmitOutcome::Disconnected) => {
            tx_q.abort_all();
            rx.reset();
            true
        }
        Ok(SubmitOutcome::Submitted | SubmitOutcome::Retry) => true,
    }
}

// FunctionFS event types (linux/usb/functionfs.h).
#[cfg(target_os = "linux")]
const FUNCTIONFS_UNBIND: u8 = 1;
#[cfg(target_os = "linux")]
const FUNCTIONFS_DISABLE: u8 = 3;
#[cfg(target_os = "linux")]
const FUNCTIONFS_SETUP: u8 = 4;

#[cfg(target_os = "linux")]
fn drain_ep0_events(ep0: RawFd, tx_q: &mut FfsTxQueue, rx: &mut BridgeRxState) -> bool {
    if ep0 < 0 {
        return false;
    }
    let mut disconnected = false;
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct UsbCtrlRequest {
        b_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        w_length: u16,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct UsbFfsEvent {
        u_setup: UsbCtrlRequest,
        ev_type: u8,
        _pad: [u8; 3],
    }
    let mut evs = [UsbFfsEvent {
        u_setup: UsbCtrlRequest {
            b_request_type: 0,
            b_request: 0,
            w_value: 0,
            w_index: 0,
            w_length: 0,
        },
        ev_type: 0,
        _pad: [0; 3],
    }; 8];
    loop {
        let n = unsafe {
            libc::read(
                ep0,
                evs.as_mut_ptr() as *mut libc::c_void,
                std::mem::size_of_val(&evs),
            )
        };
        if n <= 0 {
            break;
        }
        let count = (n as usize) / std::mem::size_of::<UsbFfsEvent>();
        for ev in &evs[..count] {
            if ev.ev_type == FUNCTIONFS_DISABLE || ev.ev_type == FUNCTIONFS_UNBIND {
                tx_q.abort_all();
                rx.reset();
                disconnected = true;
            } else if ev.ev_type == FUNCTIONFS_SETUP {
                let mut dummy: u8 = 0;
                if (ev.u_setup.b_request_type & 0x80) != 0 {
                    unsafe { libc::write(ep0, &dummy as *const u8 as *const libc::c_void, 0) };
                } else {
                    unsafe { libc::read(ep0, &mut dummy as *mut u8 as *mut libc::c_void, 0) };
                }
            }
        }
    }
    disconnected
}

#[cfg(not(target_os = "linux"))]
fn drain_ep0_events(_ep0: RawFd, _tx_q: &mut FfsTxQueue, _rx: &mut BridgeRxState) {}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--version") {
        println!("{BRIDGE_VERSION}");
        return;
    }

    let mut enable_uart_flag = std::env::var("OPENFLUX_ENABLE_UART").as_deref() == Ok("1");
    let mut mnt_arg: Option<String> = None;
    for a in args.iter().skip(1) {
        if a == "--enable-uart" {
            enable_uart_flag = true;
        } else if mnt_arg.is_none() {
            mnt_arg = Some(a.clone());
        }
    }

    let test_eps = match parse_fd_pair_env("OPENFLUX_TEST_EP_FDS") {
        Ok(v) => v,
        Err(()) => {
            eprintln!("Invalid OPENFLUX_TEST_EP_FDS");
            std::process::exit(1);
        }
    };

    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let mut sig_pipe = [-1 as RawFd; 2];
    if unsafe { libc::pipe(sig_pipe.as_mut_ptr()) } == 0 {
        set_nonblock(sig_pipe[0]);
        set_nonblock(sig_pipe[1]);
        SIG_WRITE_FD.store(sig_pipe[1], Ordering::SeqCst);
    }
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_sig as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
    }

    let (ep0, ep_in, ep_out, test_mode) = if let Some((ein, eout)) = test_eps {
        (-1, ein, eout, true)
    } else {
        let Some(ref mnt) = mnt_arg else {
            eprintln!("Usage: {} [--enable-uart] <mount_point>", args[0]);
            std::process::exit(1);
        };
        let _ = std::env::set_current_dir("/home/root");
        if let Err(e) = bridge_setup_gadget(mnt) {
            eprintln!("bridge_setup_gadget: {e}");
            std::process::exit(1);
        }
        match bridge_open_ffs(mnt) {
            Ok((p0, p1, p2)) => (p0, p1, p2, false),
            Err(e) => {
                eprintln!("bridge_open_ffs: {e}");
                std::process::exit(1);
            }
        }
    };

    if ep0 >= 0 {
        set_nonblock(ep0);
    }
    set_nonblock(ep_out);
    set_nonblock(ep_in);

    let mut uart = UartBridge::new();
    let uart_test = parse_fd_pair_env("OPENFLUX_TEST_UART_FDS").unwrap_or(None);
    let display_marker = std::env::var("OPENFLUX_DISPLAY_MARKER")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DISPLAY_ACTIVE_MARKER_DEFAULT.into());
    let udev = std::env::var("OPENFLUX_UART_DEV")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| FW_UART_DEFAULT_DEV.into());

    if let Some((urx, utx)) = uart_test {
        set_nonblock(urx);
        set_nonblock(utx);
        uart.attach_fds(urx, utx);
    } else if !test_mode && (enable_uart_flag || Path::new(&display_marker).exists()) {
        let _ = uart.open(&udev);
    }

    let mut ipc = DisplayIpc::new();
    let sock_env = std::env::var("OPENFLUX_BRIDGE_SOCK")
        .ok()
        .filter(|s| !s.is_empty());
    if let Some(ref sp) = sock_env {
        let _ = ipc.init(Some(sp));
    } else if !test_mode {
        let _ = ipc.init(Some(BRIDGE_SOCK_DEFAULT));
    }
    let mut v2_mux = V2BridgeMux::new();
    if !test_mode {
        let _ = v2_mux.init_socket();
    }

    let (mut aio, mut tx_q) = if !test_mode {
        let a = match FfsAioReader::init(ep_out, 16384) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("FfsAioReader::init: {e}");
                std::process::exit(1);
            }
        };
        let t = match FfsTxQueue::init(ep_in, true) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("FfsTxQueue::init: {e}");
                std::process::exit(1);
            }
        };
        (Some(a), t)
    } else {
        let t = FfsTxQueue::init(ep_in, false).expect("non-AIO TX queue init");
        (None, t)
    };

    let ready_path = ready_file_path();
    write_ready_stamp(&ready_path);
    openflux_bridge::usb_commands::bridge_publish_trial_marker();

    let mut rx = BridgeRxState::new();
    let watch_marker = uart_test.is_none() && !enable_uart_flag && !test_mode;

    while RUNNING.load(Ordering::SeqCst) {
        let mut pfds: Vec<libc::pollfd> = Vec::with_capacity(16);
        let ep0_idx = if ep0 >= 0 {
            let idx = pfds.len();
            pfds.push(libc::pollfd {
                fd: ep0,
                events: libc::POLLIN,
                revents: 0,
            });
            Some(idx)
        } else {
            None
        };

        let ep_idx = pfds.len();
        let ep_fd = aio.as_ref().map(|a| a.event_fd).unwrap_or(ep_out);
        pfds.push(libc::pollfd {
            fd: ep_fd,
            events: libc::POLLIN,
            revents: 0,
        });

        let ep_tx_idx = if aio.is_some() && tx_q.event_fd >= 0 {
            let idx = pfds.len();
            pfds.push(libc::pollfd {
                fd: tx_q.event_fd,
                events: libc::POLLIN,
                revents: 0,
            });
            Some(idx)
        } else if aio.is_none() && tx_q.has_pending() {
            let idx = pfds.len();
            pfds.push(libc::pollfd {
                fd: ep_in,
                events: libc::POLLOUT,
                revents: 0,
            });
            Some(idx)
        } else {
            None
        };

        let sig_idx = if sig_pipe[0] >= 0 {
            let idx = pfds.len();
            pfds.push(libc::pollfd {
                fd: sig_pipe[0],
                events: libc::POLLIN,
                revents: 0,
            });
            Some(idx)
        } else {
            None
        };

        let uart_rx_idx = if uart.rx_fd >= 0 {
            let mut ev = libc::POLLIN;
            if uart.tx_fd == uart.rx_fd && uart.has_pending_tx() {
                ev |= libc::POLLOUT;
            }
            let idx = pfds.len();
            pfds.push(libc::pollfd {
                fd: uart.rx_fd,
                events: ev,
                revents: 0,
            });
            Some(idx)
        } else {
            None
        };

        let uart_tx_idx = if uart.tx_fd >= 0 && uart.tx_fd != uart.rx_fd && uart.has_pending_tx() {
            let idx = pfds.len();
            pfds.push(libc::pollfd {
                fd: uart.tx_fd,
                events: libc::POLLOUT,
                revents: 0,
            });
            Some(idx)
        } else {
            None
        };

        let ipc_listen_fd = ipc.listen_fd();
        let ipc_idx = if ipc_listen_fd >= 0 {
            let idx = pfds.len();
            pfds.push(libc::pollfd {
                fd: ipc_listen_fd,
                events: libc::POLLIN,
                revents: 0,
            });
            Some(idx)
        } else {
            None
        };

        let client_fds = ipc.client_fds();
        let mut client_poll_indices: Vec<(usize, RawFd)> = Vec::with_capacity(client_fds.len());
        for cfd in client_fds {
            let idx = pfds.len();
            pfds.push(libc::pollfd {
                fd: cfd,
                events: libc::POLLIN,
                revents: 0,
            });
            client_poll_indices.push((idx, cfd));
        }

        let v2_listen_fd = v2_mux.listen_fd();
        let v2_listen_idx = if v2_listen_fd >= 0 {
            let idx = pfds.len();
            pfds.push(libc::pollfd {
                fd: v2_listen_fd,
                events: libc::POLLIN,
                revents: 0,
            });
            Some(idx)
        } else {
            None
        };
        let v2_client_fds = v2_mux.client_fds();
        let mut v2_client_poll_indices: Vec<(usize, RawFd)> = Vec::with_capacity(v2_client_fds.len());
        for cfd in v2_client_fds {
            let idx = pfds.len();
            pfds.push(libc::pollfd {
                fd: cfd,
                events: libc::POLLIN,
                revents: 0,
            });
            v2_client_poll_indices.push((idx, cfd));
        }

        let need_aio_retry = aio
            .as_ref()
            .is_some_and(|a| !a.pending || (tx_q.has_pending() && !tx_q.pending));
        let poll_timeout_ms = if need_aio_retry {
            50
        } else if watch_marker {
            250
        } else {
            -1
        };
        let prc = unsafe {
            libc::poll(
                pfds.as_mut_ptr(),
                pfds.len() as libc::nfds_t,
                poll_timeout_ms,
            )
        };
        if prc < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                if !RUNNING.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            }
            break;
        }
        if !RUNNING.load(Ordering::SeqCst)
            || sig_idx.is_some_and(|i| (pfds[i].revents & libc::POLLIN) != 0)
        {
            break;
        }

        if watch_marker {
            let marker_up = Path::new(&display_marker).exists();
            if marker_up && uart.rx_fd < 0 {
                let _ = uart.open(&udev);
            } else if !marker_up && uart.rx_fd >= 0 {
                uart.close();
            }
        }

        if let Some(ref mut a) = aio {
            if !a.pending && !resubmit_read(a, &mut tx_q, &mut rx) {
                break;
            }
        }
        if aio.is_some()
            && tx_q.has_pending()
            && !tx_q.pending
            && !tx_q.cancelling
            && tx_q.drive() < 0
        {
            tx_q.abort_all();
            rx.reset();
        }

        if let Some(i) = ep_tx_idx {
            if (pfds[i].revents & (libc::POLLIN | libc::POLLOUT)) != 0 {
                if aio.is_some() {
                    match tx_q.reap() {
                        Ok(Some(wres)) if wres < 0 => {
                            let werr = (-wres) as i32;
                            if werr != libc::EAGAIN
                                && werr != libc::EWOULDBLOCK
                                && werr != libc::EINTR
                            {
                                tx_q.abort_all();
                                rx.reset();
                            }
                        }
                        Err(_) => {
                            tx_q.abort_all();
                            rx.reset();
                        }
                        _ => {}
                    }
                }
                if tx_q.drive() < 0 {
                    tx_q.abort_all();
                    rx.reset();
                }
            }
        }

        if let Some(i) = uart_rx_idx {
            let rev = pfds[i].revents;
            if (rev & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0 {
                let rc = uart.poll_read(|st, mt| ipc.publish_uart(st, mt));
                if rc < 0 && (rev & (libc::POLLERR | libc::POLLHUP)) != 0 {
                    uart.close();
                }
            }
            if uart.tx_fd == uart.rx_fd && (rev & libc::POLLOUT) != 0 {
                let _ = uart.flush_tx();
            }
        }
        if let Some(i) = uart_tx_idx {
            if (pfds[i].revents & libc::POLLOUT) != 0 {
                let _ = uart.flush_tx();
            }
        }

        if let Some(i) = ep0_idx {
            if (pfds[i].revents & libc::POLLIN) != 0
                && drain_ep0_events(ep0, &mut tx_q, &mut rx)
            {
                v2_mux.on_usb_disconnect();
            }
        }
        if let Some(i) = ipc_idx {
            if (pfds[i].revents & libc::POLLIN) != 0 {
                ipc.accept();
            }
        }
        for (idx, cfd) in client_poll_indices {
            if (pfds[idx].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0 {
                ipc.read_client_by_fd(cfd, &mut uart);
            }
        }
        if let Some(i) = v2_listen_idx {
            if (pfds[i].revents & libc::POLLIN) != 0 {
                v2_mux.accept();
            }
        }
        for (idx, cfd) in v2_client_poll_indices {
            if (pfds[idx].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0 {
                v2_mux.read_display_client_by_fd(cfd);
            }
        }
        while let Some(tx_bytes) = v2_mux.pop_next_usb_tx() {
            if !tx_q.enqueue(Some(&tx_bytes)) {
                break;
            }
        }

        if (pfds[ep_idx].revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0 {
            let r_res: isize;
            let mut rx_chunk: Vec<u8> = Vec::new();
            let mut max_read = 16384usize;
            if let Some(ref mut a) = aio {
                max_read = a.buf_len();
                match a.reap() {
                    Ok(None) => continue,
                    Ok(Some(n)) => {
                        r_res = n;
                        if n > 0 {
                            rx_chunk.extend_from_slice(a.buf_slice(n as usize));
                        }
                    }
                    Err(e) => r_res = e as isize,
                }
            } else {
                let mut tmp = vec![0u8; 16384];
                let n =
                    unsafe { libc::read(ep_out, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len()) };
                if n >= 0 {
                    r_res = n;
                    rx_chunk.extend_from_slice(&tmp[..n as usize]);
                } else {
                    r_res = -(std::io::Error::last_os_error()
                        .raw_os_error()
                        .unwrap_or(libc::EIO) as isize);
                }
            }

            if r_res < 0 {
                let err = (-r_res) as i32;
                if err == libc::EAGAIN || err == libc::EWOULDBLOCK {
                    if let Some(ref mut a) = aio {
                        if !resubmit_read(a, &mut tx_q, &mut rx) {
                            break;
                        }
                        if !a.pending {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                    continue;
                }
                if err == libc::EINTR {
                    if !RUNNING.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Some(ref mut a) = aio {
                        if !resubmit_read(a, &mut tx_q, &mut rx) {
                            break;
                        }
                    }
                    continue;
                }
                if err == libc::ENODEV
                    || err == libc::ESHUTDOWN
                    || err == libc::EPIPE
                    || err == libc::ECONNRESET
                {
                    tx_q.abort_all();
                    rx.reset();
                    v2_mux.on_usb_disconnect();
                } else {
                    let mut hooks = LoopHooks {
                        tx_q: &mut tx_q,
                        ipc: &mut ipc,
                    };
                    rx.abort(&mut hooks);
                }
                if !RUNNING.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
                if let Some(ref mut a) = aio {
                    if !resubmit_read(a, &mut tx_q, &mut rx) {
                        break;
                    }
                }
                continue;
            }

            let terminated = rx_chunk.len() < max_read;
            if v2_mux.try_feed_usb(&rx_chunk) {
                while let Some(tx_bytes) = v2_mux.pop_next_usb_tx() {
                    if !tx_q.enqueue(Some(&tx_bytes)) {
                        break;
                    }
                }
            } else {
                let mut hooks = LoopHooks {
                    tx_q: &mut tx_q,
                    ipc: &mut ipc,
                };
                rx.feed(&rx_chunk, terminated, &mut hooks);
            }
            if RUNNING.load(Ordering::SeqCst) {
                if let Some(ref mut a) = aio {
                    if !resubmit_read(a, &mut tx_q, &mut rx) {
                        break;
                    }
                }
            }
        }
    }

    let _ = std::fs::remove_file(&ready_path);
    if let Some(ref mut a) = aio {
        a.destroy();
    }
    tx_q.destroy();
    rx.reset();
    v2_mux.close_socket();
    ipc.close();
    uart.close();
    unsafe {
        libc::close(ep_out);
        libc::close(ep_in);
        if ep0 >= 0 {
            libc::close(ep0);
        }
        if sig_pipe[0] >= 0 {
            libc::close(sig_pipe[0]);
        }
        if sig_pipe[1] >= 0 {
            libc::close(sig_pipe[1]);
        }
    }
}
