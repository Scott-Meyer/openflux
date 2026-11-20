use std::fs::File;
use std::os::unix::io::{AsFd, AsRawFd};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

struct ShmSlot {
    buffer: wl_buffer::WlBuffer,
    _file: File,
    ptr: *mut u8,
    len: usize,
    busy: bool,
}

impl Drop for ShmSlot {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                libc::munmap(self.ptr as *mut libc::c_void, self.len);
            }
            self.ptr = std::ptr::null_mut();
        }
    }
}

struct WaylandState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    surface: Option<wl_surface::WlSurface>,
    xdg_surface: Option<xdg_surface::XdgSurface>,
    xdg_toplevel: Option<xdg_toplevel::XdgToplevel>,
    configured: bool,
    closed: bool,
    slots: Vec<ShmSlot>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "wl_compositor" {
                state.compositor = Some(registry.bind::<wl_compositor::WlCompositor, _, _>(
                    name,
                    version.min(4),
                    qh,
                    (),
                ));
            } else if interface == "wl_shm" {
                state.shm =
                    Some(registry.bind::<wl_shm::WlShm, _, _>(name, version.min(1), qh, ()));
            } else if interface == "xdg_wm_base" {
                state.wm_base = Some(registry.bind::<xdg_wm_base::XdgWmBase, _, _>(
                    name,
                    version.min(1),
                    qh,
                    (),
                ));
            }
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wl_shm::WlShm, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wl_shm_pool::WlShmPool, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wl_surface::WlSurface, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for WaylandState {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for WaylandState {
    fn event(
        state: &mut Self,
        xs: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xs.ack_configure(serial);
            state.configured = true;
        }
    }
}
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Close = event {
            state.closed = true;
        }
    }
}
impl Dispatch<wl_buffer::WlBuffer, usize> for WaylandState {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            if let Some(s) = state.slots.get_mut(*idx) {
                s.busy = false;
            }
        }
    }
}

pub struct WaylandPresenter {
    conn: Connection,
    queue: EventQueue<WaylandState>,
    state: WaylandState,
    width: i32,
    height: i32,
    cur: usize,
}

impl WaylandPresenter {
    pub fn open(width: i32, height: i32, title: &str) -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("Wayland connect: {e}"))?;
        let display = conn.display();
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let mut state = WaylandState {
            compositor: None,
            shm: None,
            wm_base: None,
            surface: None,
            xdg_surface: None,
            xdg_toplevel: None,
            configured: false,
            closed: false,
            slots: Vec::new(),
        };
        display.get_registry(&qh, ());
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland roundtrip: {e}"))?;

        let comp = state.compositor.as_ref().ok_or("No wl_compositor")?;
        let shm = state.shm.as_ref().ok_or("No wl_shm")?;
        let wm_base = state.wm_base.as_ref().ok_or("No xdg_wm_base")?;

        let surface = comp.create_surface(&qh, ());
        let xdg_surf = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surf.get_toplevel(&qh, ());
        toplevel.set_title(title.to_string());
        toplevel.set_fullscreen(None);
        surface.commit();

        state.surface = Some(surface);
        state.xdg_surface = Some(xdg_surf);
        state.xdg_toplevel = Some(toplevel);

        let stride = width * 4;
        let len = (stride as usize) * (height as usize);
        for idx in 0..2usize {
            let tmp_path = std::env::temp_dir().join(format!(
                "openflux_wl_shm_{}_{}.bin",
                std::process::id(),
                idx
            ));
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)
                .map_err(|e| format!("shm file: {e}"))?;
            let _ = std::fs::remove_file(&tmp_path);
            file.set_len(len as u64)
                .map_err(|e| format!("shm set_len: {e}"))?;
            let mapped = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    file.as_raw_fd(),
                    0,
                )
            };
            if mapped == libc::MAP_FAILED || mapped.is_null() {
                return Err(format!("shm mmap: {}", std::io::Error::last_os_error()));
            }
            let pool = shm.create_pool(file.as_fd(), len as i32, &qh, ());
            let buffer =
                pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &qh, idx);
            pool.destroy();
            state.slots.push(ShmSlot {
                buffer,
                _file: file,
                ptr: mapped as *mut u8,
                len,
                busy: false,
            });
        }

        // Wait for initial xdg_surface.configure before attaching buffers
        for _ in 0..20 {
            if state.configured {
                break;
            }
            queue
                .roundtrip(&mut state)
                .map_err(|e| format!("Wayland xdg configure: {e}"))?;
        }

        Ok(Self {
            conn,
            queue,
            state,
            width,
            height,
            cur: 0,
        })
    }

    pub fn raw_fd(&self) -> i32 {
        self.conn.as_fd().as_raw_fd()
    }

    pub fn dispatch(&mut self) -> Result<bool, String> {
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| e.to_string())?;
        if let Some(guard) = self.conn.prepare_read() {
            if let Err(e) = guard.read() {
                if !matches!(e, wayland_client::backend::WaylandError::Io(ref io) if io.kind() == std::io::ErrorKind::WouldBlock)
                {
                    return Err(format!("wayland read failed: {e}"));
                }
            }
            self.queue
                .dispatch_pending(&mut self.state)
                .map_err(|e| e.to_string())?;
        }
        if let Err(e) = self.conn.flush() {
            if !matches!(e, wayland_client::backend::WaylandError::Io(ref io) if io.kind() == std::io::ErrorKind::WouldBlock)
            {
                return Err(format!("wayland flush failed: {e}"));
            }
        }
        Ok(!self.state.closed)
    }

    pub fn present(&mut self, argb: &[u32]) -> Result<bool, String> {
        let _ = self.queue.dispatch_pending(&mut self.state);
        if !self.state.configured {
            return Ok(false);
        }
        let idx = if !self.state.slots[self.cur].busy {
            self.cur
        } else if !self.state.slots[1 - self.cur].busy {
            1 - self.cur
        } else {
            // Both buffers are compositor-owned; never overwrite a busy buffer.
            return Ok(false);
        };
        let slot = &mut self.state.slots[idx];
        let copy_len = (argb.len() * 4).min(slot.len);
        unsafe {
            std::ptr::copy_nonoverlapping(argb.as_ptr() as *const u8, slot.ptr, copy_len);
        }
        slot.busy = true;
        if let Some(ref s) = self.state.surface {
            s.attach(Some(&slot.buffer), 0, 0);
            s.damage_buffer(0, 0, self.width, self.height);
            s.commit();
        }
        if let Err(e) = self.conn.flush() {
            if !matches!(e, wayland_client::backend::WaylandError::Io(ref io) if io.kind() == std::io::ErrorKind::WouldBlock)
            {
                return Err(format!("wayland flush failed: {e}"));
            }
        }
        self.cur = 1 - idx;
        Ok(true)
    }
}
