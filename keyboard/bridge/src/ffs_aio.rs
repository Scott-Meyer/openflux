use std::collections::VecDeque;
use std::io::Read;
use std::os::unix::io::RawFd;

pub const USB_TX_MAX_PACKETS: usize = 131_072;
const TX_PACKET_SIZE: usize = 512;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IoCb {
    pub aio_data: u64,
    pub aio_key: u32,
    pub aio_rw_flags: i32,
    pub aio_lio_opcode: u16,
    pub aio_reqprio: i16,
    pub aio_fildes: u32,
    pub aio_buf: u64,
    pub aio_nbytes: u64,
    pub aio_offset: i64,
    pub aio_reserved2: u64,
    pub aio_flags: u32,
    pub aio_resfd: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IoEvent {
    pub data: u64,
    pub obj: u64,
    pub res: i64,
    pub res2: i64,
}

#[cfg(target_os = "linux")]
mod sys {
    use super::{IoCb, IoEvent};
    pub const IOCB_CMD_PREAD: u16 = 0;
    pub const IOCB_CMD_PWRITE: u16 = 1;
    pub const IOCB_FLAG_RESFD: u32 = 1 << 0;

    pub unsafe fn io_setup(nr: libc::c_uint, ctx: *mut libc::c_ulong) -> libc::c_long {
        libc::syscall(libc::SYS_io_setup, nr, ctx)
    }
    pub unsafe fn io_destroy(ctx: libc::c_ulong) -> libc::c_long {
        libc::syscall(libc::SYS_io_destroy, ctx)
    }
    pub unsafe fn io_submit(
        ctx: libc::c_ulong,
        nr: libc::c_long,
        iocbpp: *mut *mut IoCb,
    ) -> libc::c_long {
        libc::syscall(libc::SYS_io_submit, ctx, nr, iocbpp)
    }
    pub unsafe fn io_cancel(
        ctx: libc::c_ulong,
        iocb: *mut IoCb,
        res: *mut IoEvent,
    ) -> libc::c_long {
        libc::syscall(libc::SYS_io_cancel, ctx, iocb, res)
    }
    pub unsafe fn io_getevents(
        ctx: libc::c_ulong,
        min_nr: libc::c_long,
        nr: libc::c_long,
        events: *mut IoEvent,
        timeout: *mut libc::timespec,
    ) -> libc::c_long {
        libc::syscall(libc::SYS_io_getevents, ctx, min_nr, nr, events, timeout)
    }
}

/// What happened when [`FfsAioReader::submit`] tried to queue a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// A read is queued.
    Submitted,
    /// The kernel had no room for the read; submit again later.
    Retry,
    /// The endpoint is gone: the host disconnected or the function was disabled.
    Disconnected,
}

pub struct FfsAioReader {
    pub ep_fd: RawFd,
    pub event_fd: RawFd,
    pub ctx: libc::c_ulong,
    pub enabled: bool,
    pub pending: bool,
    buf: Box<[u8]>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    cb: Box<IoCb>,
}

impl Drop for FfsAioReader {
    fn drop(&mut self) {
        self.destroy();
    }
}

impl FfsAioReader {
    pub fn init(ep_fd: RawFd, buf_len: usize) -> Result<Self, String> {
        if ep_fd < 0 || buf_len == 0 {
            return Err("invalid AIO reader args".into());
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::var("KEYGLASS_FORCE_AIO_FAIL").as_deref() == Ok("1") {
                return Err("KEYGLASS_FORCE_AIO_FAIL".into());
            }
            let efd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
            if efd < 0 {
                return Err(format!("eventfd: {}", std::io::Error::last_os_error()));
            }
            let mut ctx: libc::c_ulong = 0;
            if unsafe { sys::io_setup(2, &mut ctx) } != 0 {
                let e = std::io::Error::last_os_error();
                unsafe { libc::close(efd) };
                return Err(format!("io_setup: {e}"));
            }
            let mut reader = Self {
                ep_fd,
                event_fd: efd,
                ctx,
                enabled: true,
                pending: false,
                buf: vec![0u8; buf_len].into_boxed_slice(),
                cb: Box::new(IoCb::default()),
            };
            if reader.submit().is_err() {
                reader.destroy();
                return Err("initial ffs_aio_submit failed".into());
            }
            Ok(reader)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (ep_fd, buf_len);
            Err("Linux AIO unsupported on host".into())
        }
    }

    pub fn buf_slice(&self, n: usize) -> &[u8] {
        &self.buf[..n.min(self.buf.len())]
    }

    pub fn buf_len(&self) -> usize {
        self.buf.len()
    }

    /// Queues a read on the OUT endpoint unless one is already queued. An error means the
    /// reader can no longer be used.
    pub fn submit(&mut self) -> std::io::Result<SubmitOutcome> {
        #[cfg(target_os = "linux")]
        {
            if !self.enabled {
                return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
            }
            if self.pending {
                return Ok(SubmitOutcome::Submitted);
            }
            *self.cb = IoCb {
                aio_fildes: self.ep_fd as u32,
                aio_lio_opcode: sys::IOCB_CMD_PREAD,
                aio_buf: self.buf.as_mut_ptr() as u64,
                aio_nbytes: self.buf.len() as u64,
                aio_flags: sys::IOCB_FLAG_RESFD,
                aio_resfd: self.event_fd as u32,
                ..Default::default()
            };
            let mut ptr: *mut IoCb = &mut *self.cb;
            let rc = unsafe { sys::io_submit(self.ctx, 1, &mut ptr) };
            if rc != 1 {
                let e = std::io::Error::last_os_error();
                let err = e.raw_os_error().unwrap_or(0);
                if err == libc::EAGAIN || err == libc::EBUSY {
                    self.pending = false;
                    return Ok(SubmitOutcome::Retry);
                }
                if err == libc::ENODEV || err == libc::ESHUTDOWN {
                    self.pending = false;
                    return Ok(SubmitOutcome::Disconnected);
                }
                return Err(e);
            }
            self.pending = true;
            Ok(SubmitOutcome::Submitted)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
        }
    }

    pub fn reap(&mut self) -> Result<Option<isize>, i32> {
        #[cfg(target_os = "linux")]
        {
            if !self.enabled {
                return Err(-libc::EINVAL);
            }
            let mut cnt: u64 = 0;
            unsafe { libc::read(self.event_fd, &mut cnt as *mut u64 as *mut libc::c_void, 8) };
            let mut ev = IoEvent::default();
            let mut zero = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            let rc = unsafe { sys::io_getevents(self.ctx, 0, 1, &mut ev, &mut zero) };
            if rc < 0 {
                return Err(-(std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO)));
            }
            if rc == 0 {
                return Ok(None);
            }
            self.pending = false;
            Ok(Some(ev.res as isize))
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(-libc::ENOSYS)
        }
    }

    pub fn destroy(&mut self) {
        #[cfg(target_os = "linux")]
        {
            if self.enabled {
                if self.pending {
                    let mut ev = IoEvent::default();
                    unsafe { sys::io_cancel(self.ctx, &mut *self.cb, &mut ev) };
                    self.pending = false;
                }
                unsafe { sys::io_destroy(self.ctx) };
                self.enabled = false;
            }
            if self.event_fd >= 0 {
                unsafe { libc::close(self.event_fd) };
                self.event_fd = -1;
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct UsbTxPkt {
    pub data: [u8; TX_PACKET_SIZE],
    pub len: u16,
}

pub struct PendingTxStream {
    reader: Box<dyn Read + Send>,
    remaining: usize,
    zlp_pending: bool,
}

impl PendingTxStream {
    pub fn new(reader: Box<dyn Read + Send>, len: usize) -> Self {
        Self {
            reader,
            remaining: len,
            zlp_pending: true,
        }
    }

    /// Returns the next 512-byte packet of the stream (the last one zero-padded to 512 bytes; the
    /// response header carries the real length), then a zero-length packet, then `None`.
    #[allow(clippy::result_unit_err)]
    pub fn next_packet(&mut self) -> Result<Option<UsbTxPkt>, ()> {
        if self.remaining > 0 {
            let want = self.remaining.min(TX_PACKET_SIZE);
            let mut data = [0u8; TX_PACKET_SIZE];
            let mut filled = 0usize;
            while filled < want {
                match self.reader.read(&mut data[filled..want]) {
                    Ok(0) => return Err(()),
                    Ok(n) => filled += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err(()),
                }
            }
            self.remaining -= want;
            return Ok(Some(UsbTxPkt {
                data,
                len: TX_PACKET_SIZE as u16,
            }));
        }
        if self.zlp_pending {
            self.zlp_pending = false;
            return Ok(Some(UsbTxPkt {
                data: [0u8; TX_PACKET_SIZE],
                len: 0,
            }));
        }
        Ok(None)
    }
}

#[allow(clippy::large_enum_variant)]
enum TxItem {
    Packet(UsbTxPkt),
    Stream(PendingTxStream),
}

pub struct FfsTxQueue {
    pub ep_fd: RawFd,
    pub event_fd: RawFd,
    pub aio_enabled: bool,
    pub pending: bool,
    pub cancelling: bool,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    ctx: libc::c_ulong,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    cb: Box<IoCb>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    active_pkt: Box<UsbTxPkt>,
    ring: VecDeque<TxItem>,
    cur_off: usize,
}

impl Drop for FfsTxQueue {
    fn drop(&mut self) {
        self.destroy();
    }
}

impl FfsTxQueue {
    pub fn init(ep_fd: RawFd, use_aio: bool) -> Result<Self, String> {
        if ep_fd < 0 {
            return Err("invalid ep_fd".into());
        }
        if !use_aio {
            return Ok(Self {
                ep_fd,
                event_fd: -1,
                aio_enabled: false,
                pending: false,
                cancelling: false,
                ctx: 0,
                cb: Box::new(IoCb::default()),
                active_pkt: Box::new(UsbTxPkt {
                    data: [0; TX_PACKET_SIZE],
                    len: 0,
                }),
                ring: VecDeque::with_capacity(128),
                cur_off: 0,
            });
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::var("KEYGLASS_FORCE_AIO_FAIL").as_deref() == Ok("1") {
                return Err("KEYGLASS_FORCE_AIO_FAIL".into());
            }
            let efd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
            if efd < 0 {
                return Err(format!("tx eventfd: {}", std::io::Error::last_os_error()));
            }
            let mut ctx: libc::c_ulong = 0;
            if unsafe { sys::io_setup(2, &mut ctx) } != 0 {
                let e = std::io::Error::last_os_error();
                unsafe { libc::close(efd) };
                return Err(format!("tx io_setup: {e}"));
            }
            Ok(Self {
                ep_fd,
                event_fd: efd,
                aio_enabled: true,
                pending: false,
                cancelling: false,
                ctx,
                cb: Box::new(IoCb::default()),
                active_pkt: Box::new(UsbTxPkt {
                    data: [0; TX_PACKET_SIZE],
                    len: 0,
                }),
                ring: VecDeque::with_capacity(128),
                cur_off: 0,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("Linux TX AIO unsupported on host".into())
        }
    }

    pub fn reserve(&mut self, packets: usize) -> bool {
        if self.ring.len() + packets > USB_TX_MAX_PACKETS {
            return false;
        }
        self.ring.try_reserve(packets).is_ok()
    }

    pub fn has_pending(&self) -> bool {
        !self.ring.is_empty() || self.pending
    }

    pub fn queued_items(&self) -> usize {
        self.ring.len()
    }

    pub fn enqueue(&mut self, data: Option<&[u8]>) -> bool {
        match data {
            None | Some([]) => {
                if self.ring.len() + 1 > USB_TX_MAX_PACKETS {
                    return false;
                }
                self.ring.push_back(TxItem::Packet(UsbTxPkt {
                    data: [0; TX_PACKET_SIZE],
                    len: 0,
                }));
                self.drive() >= 0
            }
            Some(bytes) => {
                let needed = bytes.len().div_ceil(TX_PACKET_SIZE);
                if self.ring.len() + needed > USB_TX_MAX_PACKETS {
                    return false;
                }
                for chunk in bytes.chunks(TX_PACKET_SIZE) {
                    let mut pkt = UsbTxPkt {
                        data: [0; TX_PACKET_SIZE],
                        len: chunk.len() as u16,
                    };
                    pkt.data[..chunk.len()].copy_from_slice(chunk);
                    self.ring.push_back(TxItem::Packet(pkt));
                }
                self.drive() >= 0
            }
        }
    }

    /// Queues a reader-backed response that pumps one 512-byte packet at a time in O(1) memory,
    /// preserving FIFO ordering against any subsequent packets or streams.
    pub fn enqueue_stream(&mut self, reader: Box<dyn Read + Send>, len: usize) -> bool {
        if self.ring.len() + 1 > USB_TX_MAX_PACKETS {
            return false;
        }
        self.ring
            .push_back(TxItem::Stream(PendingTxStream::new(reader, len)));
        self.drive() >= 0
    }

    fn ensure_front_packet(&mut self) -> Result<bool, ()> {
        loop {
            match self.ring.front_mut() {
                None => return Ok(false),
                Some(TxItem::Packet(_)) => return Ok(true),
                Some(TxItem::Stream(stream)) => match stream.next_packet() {
                    Ok(Some(pkt)) => {
                        self.ring.push_front(TxItem::Packet(pkt));
                        return Ok(true);
                    }
                    Ok(None) => {
                        self.ring.pop_front();
                    }
                    Err(()) => {
                        self.ring.pop_front();
                        return Err(());
                    }
                },
            }
        }
    }

    pub fn drive(&mut self) -> i32 {
        if self.ep_fd < 0 {
            return -1;
        }
        if self.aio_enabled {
            #[cfg(target_os = "linux")]
            {
                if self.pending || self.cancelling {
                    return 0;
                }
                let Ok(has_pkt) = self.ensure_front_packet() else {
                    return -1;
                };
                if !has_pkt {
                    return 0;
                }
                let Some(TxItem::Packet(head)) = self.ring.front() else {
                    return 0;
                };
                self.active_pkt.len = head.len;
                self.active_pkt.data.copy_from_slice(&head.data);
                *self.cb = IoCb {
                    aio_fildes: self.ep_fd as u32,
                    aio_lio_opcode: sys::IOCB_CMD_PWRITE,
                    aio_buf: self.active_pkt.data.as_ptr() as u64,
                    aio_nbytes: self.active_pkt.len as u64,
                    aio_flags: sys::IOCB_FLAG_RESFD,
                    aio_resfd: self.event_fd as u32,
                    ..Default::default()
                };
                let mut ptr: *mut IoCb = &mut *self.cb;
                let rc = unsafe { sys::io_submit(self.ctx, 1, &mut ptr) };
                if rc != 1 {
                    let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                    if err == libc::EAGAIN || err == libc::EBUSY {
                        return 0;
                    }
                    return -1;
                }
                self.pending = true;
                return 0;
            }
            #[cfg(not(target_os = "linux"))]
            {
                return -1;
            }
        }

        loop {
            let Ok(has_pkt) = self.ensure_front_packet() else {
                return -1;
            };
            if !has_pkt {
                break;
            }
            let Some(TxItem::Packet(pkt)) = self.ring.front() else {
                break;
            };
            if pkt.len == 0 {
                let w = unsafe { libc::write(self.ep_fd, b"".as_ptr() as *const libc::c_void, 0) };
                if w < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        return 0;
                    }
                    return -1;
                }
                self.ring.pop_front();
                self.cur_off = 0;
                continue;
            }
            let len = pkt.len as usize;
            let w = unsafe {
                libc::write(
                    self.ep_fd,
                    pkt.data[self.cur_off..len].as_ptr() as *const libc::c_void,
                    len - self.cur_off,
                )
            };
            if w > 0 {
                self.cur_off += w as usize;
                if self.cur_off >= len {
                    self.ring.pop_front();
                    self.cur_off = 0;
                }
                continue;
            }
            if w == 0 {
                return 0;
            }
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return 0;
            }
            return -1;
        }
        0
    }

    pub fn reap(&mut self) -> Result<Option<isize>, i32> {
        #[cfg(target_os = "linux")]
        {
            if !self.aio_enabled {
                return Err(-libc::EINVAL);
            }
            let mut cnt: u64 = 0;
            unsafe { libc::read(self.event_fd, &mut cnt as *mut u64 as *mut libc::c_void, 8) };
            let mut ev = IoEvent::default();
            let mut zero = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            let rc = unsafe { sys::io_getevents(self.ctx, 0, 1, &mut ev, &mut zero) };
            if rc < 0 {
                return Err(-(std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO)));
            }
            if rc == 0 {
                return Ok(None);
            }
            self.pending = false;
            if self.cancelling {
                self.cancelling = false;
                return Ok(Some(0));
            }
            if ev.res < 0 {
                return Ok(Some(ev.res as isize));
            }
            if ev.res as usize != self.active_pkt.len as usize {
                return Ok(Some(-(libc::EIO as isize)));
            }
            if matches!(self.ring.front(), Some(TxItem::Packet(_))) {
                self.ring.pop_front();
                self.cur_off = 0;
            }
            Ok(Some(ev.res as isize))
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(-libc::ENOSYS)
        }
    }

    pub fn abort_all(&mut self) {
        #[cfg(target_os = "linux")]
        if self.aio_enabled && self.pending {
            let mut ev = IoEvent::default();
            if unsafe { sys::io_cancel(self.ctx, &mut *self.cb, &mut ev) } == 0 {
                self.pending = false;
                self.cancelling = false;
            } else {
                self.cancelling = true;
            }
        }
        self.ring.clear();
        self.cur_off = 0;
    }

    pub fn destroy(&mut self) {
        self.abort_all();
        #[cfg(target_os = "linux")]
        {
            if self.aio_enabled {
                unsafe { sys::io_destroy(self.ctx) };
                self.aio_enabled = false;
                self.pending = false;
                self.cancelling = false;
            }
            if self.event_fd >= 0 {
                unsafe { libc::close(self.event_fd) };
                self.event_fd = -1;
            }
        }
    }
}
