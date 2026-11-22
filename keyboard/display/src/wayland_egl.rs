//! Hardware-accelerated Wayland EGL + OpenGL ES 2.0 GPU compositor for Mali-G52 (`RK3566`).
//! Loads `libwayland-egl.so.1`, `libEGL.so.1`, and `libGLESv2.so.2` at runtime via `dlopen`.
//! [`WaylandGpuPresenter::open`] returns an error when GPU EGL is unavailable (for example on a
//! headless machine), so the caller can fall back to `WaylandPresenter` (SHM).

use std::ffi::{c_char, c_int, c_void, CString};
use std::os::unix::io::{AsFd, AsRawFd};
use wayland_client::protocol::{wl_callback, wl_compositor, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const EGL_NO_CONTEXT: *mut c_void = std::ptr::null_mut();
const EGL_NO_DISPLAY: *mut c_void = std::ptr::null_mut();
const EGL_NO_SURFACE: *mut c_void = std::ptr::null_mut();
const EGL_RED_SIZE: i32 = 0x3024;
const EGL_GREEN_SIZE: i32 = 0x3023;
const EGL_BLUE_SIZE: i32 = 0x3022;
const EGL_ALPHA_SIZE: i32 = 0x3021;
const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_WINDOW_BIT: i32 = 0x0004;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_OPENGL_ES2_BIT: i32 = 0x0004;
const EGL_CONTEXT_CLIENT_VERSION: i32 = 0x3098;
const EGL_OPENGL_ES_API: u32 = 0x30A0;
const EGL_NONE: i32 = 0x3038;

const GL_COLOR_BUFFER_BIT: u32 = 0x0000_4000;
const GL_BLEND: u32 = 0x0BE2;
const GL_SRC_ALPHA: u32 = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
const GL_ONE: u32 = 0x0001;
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_TEXTURE0: u32 = 0x84C0;
const GL_TEXTURE1: u32 = 0x84C1;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_TEXTURE_WRAP_S: u32 = 0x2802;
const GL_TEXTURE_WRAP_T: u32 = 0x2803;
const GL_LINEAR: i32 = 0x2601;
const GL_CLAMP_TO_EDGE: i32 = 0x812F;
const GL_LUMINANCE: u32 = 0x1909;
const GL_LUMINANCE_ALPHA: u32 = 0x190A;
const GL_RGBA: u32 = 0x1908;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_UNPACK_ALIGNMENT: u32 = 0x0CF5;
const GL_FRAGMENT_SHADER: u32 = 0x8B30;
const GL_VERTEX_SHADER: u32 = 0x8B31;
const GL_COMPILE_STATUS: u32 = 0x8B81;
const GL_LINK_STATUS: u32 = 0x8B82;
const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_TRIANGLE_STRIP: u32 = 0x0005;
const GL_FLOAT: u32 = 0x1406;

#[derive(Clone, Copy, Debug)]
pub struct GpuKeyEffect {
    pub kcx: f32,
    pub kcy: f32,
    pub kw: f32,
    pub kh: f32,
    pub kind: i32, // 1=GlowingRings, 2=SolidHighlight, 3=Ripple, 4=AutofocusLock
    pub age_ms: f32,
    pub rel_age_ms: f32,
    pub glow: f32,
    pub ring_argb: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct GpuRoundRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub radius: f32,
    pub argb: u32,
}

struct DlHandle(*mut c_void);

impl Drop for DlHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                libc::dlclose(self.0);
            }
        }
    }
}

struct EglGlesLib {
    _wegl: DlHandle,
    _egl: DlHandle,
    _gles: DlHandle,
    wl_egl_window_create: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> *mut c_void,
    wl_egl_window_destroy: unsafe extern "C" fn(*mut c_void),
    egl_get_display: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    egl_initialize: unsafe extern "C" fn(*mut c_void, *mut i32, *mut i32) -> u32,
    egl_bind_api: unsafe extern "C" fn(u32) -> u32,
    egl_choose_config:
        unsafe extern "C" fn(*mut c_void, *const i32, *mut *mut c_void, i32, *mut i32) -> u32,
    egl_create_context:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const i32) -> *mut c_void,
    egl_create_window_surface:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const i32) -> *mut c_void,
    egl_make_current:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> u32,
    egl_swap_interval: Option<unsafe extern "C" fn(*mut c_void, i32) -> u32>,
    egl_swap_buffers: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32,
    egl_destroy_surface: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32,
    egl_destroy_context: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32,
    egl_terminate: unsafe extern "C" fn(*mut c_void) -> u32,

    gl_viewport: unsafe extern "C" fn(i32, i32, i32, i32),
    gl_clear_color: unsafe extern "C" fn(f32, f32, f32, f32),
    gl_clear: unsafe extern "C" fn(u32),
    gl_enable: unsafe extern "C" fn(u32),
    gl_disable: unsafe extern "C" fn(u32),
    gl_blend_func: unsafe extern "C" fn(u32, u32),
    gl_blend_func_separate: unsafe extern "C" fn(u32, u32, u32, u32),
    gl_gen_textures: unsafe extern "C" fn(i32, *mut u32),
    gl_delete_textures: unsafe extern "C" fn(i32, *const u32),
    gl_bind_texture: unsafe extern "C" fn(u32, u32),
    gl_active_texture: unsafe extern "C" fn(u32),
    gl_tex_parameteri: unsafe extern "C" fn(u32, u32, i32),
    gl_pixel_storei: unsafe extern "C" fn(u32, i32),
    gl_tex_image_2d: unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void),
    gl_tex_sub_image_2d:
        unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void),
    gl_gen_framebuffers: unsafe extern "C" fn(i32, *mut u32),
    gl_delete_framebuffers: unsafe extern "C" fn(i32, *const u32),
    gl_bind_framebuffer: unsafe extern "C" fn(u32, u32),
    gl_framebuffer_texture_2d: unsafe extern "C" fn(u32, u32, u32, u32, i32),
    gl_create_shader: unsafe extern "C" fn(u32) -> u32,
    gl_shader_source: unsafe extern "C" fn(u32, i32, *const *const c_char, *const i32),
    gl_compile_shader: unsafe extern "C" fn(u32),
    gl_get_shaderiv: unsafe extern "C" fn(u32, u32, *mut i32),
    gl_get_shader_info_log: unsafe extern "C" fn(u32, i32, *mut i32, *mut c_char),
    gl_delete_shader: unsafe extern "C" fn(u32),
    gl_create_program: unsafe extern "C" fn() -> u32,
    gl_attach_shader: unsafe extern "C" fn(u32, u32),
    gl_link_program: unsafe extern "C" fn(u32),
    gl_get_programiv: unsafe extern "C" fn(u32, u32, *mut i32),
    gl_get_program_info_log: unsafe extern "C" fn(u32, i32, *mut i32, *mut c_char),
    gl_use_program: unsafe extern "C" fn(u32),
    gl_delete_program: unsafe extern "C" fn(u32),
    gl_get_attrib_location: unsafe extern "C" fn(u32, *const c_char) -> i32,
    gl_get_uniform_location: unsafe extern "C" fn(u32, *const c_char) -> i32,
    gl_enable_vertex_attrib_array: unsafe extern "C" fn(u32),
    gl_vertex_attrib_pointer: unsafe extern "C" fn(u32, i32, u32, u8, i32, *const c_void),
    gl_uniform1i: unsafe extern "C" fn(i32, i32),
    gl_uniform1f: unsafe extern "C" fn(i32, f32),
    gl_uniform2f: unsafe extern "C" fn(i32, f32, f32),
    gl_uniform4f: unsafe extern "C" fn(i32, f32, f32, f32, f32),
    gl_draw_arrays: unsafe extern "C" fn(u32, i32, i32),
}

impl EglGlesLib {
    fn load() -> Result<Self, String> {
        unsafe {
            let wegl = DlHandle(libc::dlopen(
                c"libwayland-egl.so.1".as_ptr(),
                libc::RTLD_NOW,
            ));
            let egl = DlHandle(libc::dlopen(c"libEGL.so.1".as_ptr(), libc::RTLD_NOW));
            let gles = DlHandle(libc::dlopen(c"libGLESv2.so.2".as_ptr(), libc::RTLD_NOW));
            if wegl.0.is_null() || egl.0.is_null() || gles.0.is_null() {
                return Err("Wayland EGL/GLESv2 shared libraries unavailable".into());
            }
            macro_rules! sym {
                ($lib:expr, $name:literal) => {{
                    let p = libc::dlsym($lib.0, concat!($name, "\0").as_ptr() as *const c_char);
                    if p.is_null() {
                        return Err(format!("missing symbol {}", $name));
                    }
                    crate::dl::fn_ptr(p)
                }};
            }
            let swap_iv = {
                let p = libc::dlsym(egl.0, c"eglSwapInterval".as_ptr());
                if p.is_null() {
                    None
                } else {
                    Some(crate::dl::fn_ptr(p))
                }
            };
            Ok(Self {
                wl_egl_window_create: sym!(wegl, "wl_egl_window_create"),
                wl_egl_window_destroy: sym!(wegl, "wl_egl_window_destroy"),
                egl_get_display: sym!(egl, "eglGetDisplay"),
                egl_initialize: sym!(egl, "eglInitialize"),
                egl_bind_api: sym!(egl, "eglBindAPI"),
                egl_choose_config: sym!(egl, "eglChooseConfig"),
                egl_create_context: sym!(egl, "eglCreateContext"),
                egl_create_window_surface: sym!(egl, "eglCreateWindowSurface"),
                egl_make_current: sym!(egl, "eglMakeCurrent"),
                egl_swap_interval: swap_iv,
                egl_swap_buffers: sym!(egl, "eglSwapBuffers"),
                egl_destroy_surface: sym!(egl, "eglDestroySurface"),
                egl_destroy_context: sym!(egl, "eglDestroyContext"),
                egl_terminate: sym!(egl, "eglTerminate"),
                gl_viewport: sym!(gles, "glViewport"),
                gl_clear_color: sym!(gles, "glClearColor"),
                gl_clear: sym!(gles, "glClear"),
                gl_enable: sym!(gles, "glEnable"),
                gl_disable: sym!(gles, "glDisable"),
                gl_blend_func: sym!(gles, "glBlendFunc"),
                gl_blend_func_separate: sym!(gles, "glBlendFuncSeparate"),
                gl_gen_textures: sym!(gles, "glGenTextures"),
                gl_delete_textures: sym!(gles, "glDeleteTextures"),
                gl_bind_texture: sym!(gles, "glBindTexture"),
                gl_active_texture: sym!(gles, "glActiveTexture"),
                gl_tex_parameteri: sym!(gles, "glTexParameteri"),
                gl_pixel_storei: sym!(gles, "glPixelStorei"),
                gl_tex_image_2d: sym!(gles, "glTexImage2D"),
                gl_tex_sub_image_2d: sym!(gles, "glTexSubImage2D"),
                gl_gen_framebuffers: sym!(gles, "glGenFramebuffers"),
                gl_delete_framebuffers: sym!(gles, "glDeleteFramebuffers"),
                gl_bind_framebuffer: sym!(gles, "glBindFramebuffer"),
                gl_framebuffer_texture_2d: sym!(gles, "glFramebufferTexture2D"),
                gl_create_shader: sym!(gles, "glCreateShader"),
                gl_shader_source: sym!(gles, "glShaderSource"),
                gl_compile_shader: sym!(gles, "glCompileShader"),
                gl_get_shaderiv: sym!(gles, "glGetShaderiv"),
                gl_get_shader_info_log: sym!(gles, "glGetShaderInfoLog"),
                gl_delete_shader: sym!(gles, "glDeleteShader"),
                gl_create_program: sym!(gles, "glCreateProgram"),
                gl_attach_shader: sym!(gles, "glAttachShader"),
                gl_link_program: sym!(gles, "glLinkProgram"),
                gl_get_programiv: sym!(gles, "glGetProgramiv"),
                gl_get_program_info_log: sym!(gles, "glGetProgramInfoLog"),
                gl_use_program: sym!(gles, "glUseProgram"),
                gl_delete_program: sym!(gles, "glDeleteProgram"),
                gl_get_attrib_location: sym!(gles, "glGetAttribLocation"),
                gl_get_uniform_location: sym!(gles, "glGetUniformLocation"),
                gl_enable_vertex_attrib_array: sym!(gles, "glEnableVertexAttribArray"),
                gl_vertex_attrib_pointer: sym!(gles, "glVertexAttribPointer"),
                gl_uniform1i: sym!(gles, "glUniform1i"),
                gl_uniform1f: sym!(gles, "glUniform1f"),
                gl_uniform2f: sym!(gles, "glUniform2f"),
                gl_uniform4f: sym!(gles, "glUniform4f"),
                gl_draw_arrays: sym!(gles, "glDrawArrays"),
                _wegl: wegl,
                _egl: egl,
                _gles: gles,
            })
        }
    }
}

struct WlEglState {
    compositor: Option<wl_compositor::WlCompositor>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    surface: Option<wl_surface::WlSurface>,
    xdg_surface: Option<xdg_surface::XdgSurface>,
    xdg_toplevel: Option<xdg_toplevel::XdgToplevel>,
    configured: bool,
    closed: bool,
    frame_pending: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for WlEglState {
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

impl Dispatch<wl_compositor::WlCompositor, ()> for WlEglState {
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
impl Dispatch<wl_surface::WlSurface, ()> for WlEglState {
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
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for WlEglState {
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
impl Dispatch<xdg_surface::XdgSurface, ()> for WlEglState {
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
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for WlEglState {
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
impl Dispatch<wl_callback::WlCallback, ()> for WlEglState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.frame_pending = false;
    }
}

const VS_QUAD: &str = r#"
attribute vec2 a_pos;
varying vec2 v_uv;
void main() {
    v_uv = vec2((a_pos.x + 1.0) * 0.5, 1.0 - (a_pos.y + 1.0) * 0.5);
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
"#;

const FS_BG_NV12: &str = r#"
precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_tex_y;
uniform sampler2D u_tex_uv;
uniform sampler2D u_tex_rgba;
uniform int u_is_nv12;
uniform float u_dim;
void main() {
    vec3 rgb;
    if (u_is_nv12 == 1) {
        float y = texture2D(u_tex_y, v_uv).r;
        vec2 uv = texture2D(u_tex_uv, v_uv).ra - vec2(0.5, 0.5);
        // BT.709 video range conversion
        float y_s = (y - 0.062745) * 1.16438;
        rgb = vec3(
            y_s + 1.79274 * uv.y,
            y_s - 0.21325 * uv.x - 0.53291 * uv.y,
            y_s + 2.11240 * uv.x
        );
    } else {
        // u32 ARGB on little-endian is BGRA in memory -> swizzle .bgr
        rgb = texture2D(u_tex_rgba, v_uv).bgr;
    }
    gl_FragColor = vec4(clamp(rgb, 0.0, 1.0) * u_dim, 1.0);
}
"#;

const FS_BLUR: &str = r#"
precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_tex;
uniform vec2 u_step;
void main() {
    vec4 acc = texture2D(u_tex, v_uv) * 0.227027;
    acc += texture2D(u_tex, v_uv + u_step * 1.3846) * 0.316216;
    acc += texture2D(u_tex, v_uv - u_step * 1.3846) * 0.316216;
    acc += texture2D(u_tex, v_uv + u_step * 3.2307) * 0.070270;
    acc += texture2D(u_tex, v_uv - u_step * 3.2307) * 0.070270;
    gl_FragColor = acc;
}
"#;

const FS_SHADER_BG: &str = r#"
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
varying vec2 v_uv;
uniform int u_kind;
uniform float u_time;
uniform float u_hue;
uniform float u_dim;
uniform vec2 u_screen;

vec3 rotateHue(vec3 color, vec2 trig) {
    float c = trig.x;
    float s = trig.y;
    return max(vec3(0.0), vec3(
        dot(color, vec3(
            (0.213 + c * 0.787) - s * 0.213,
            (0.715 - c * 0.715) - s * 0.715,
            (0.072 - c * 0.072) + s * 0.928
        )),
        dot(color, vec3(
            (0.213 - c * 0.213) + s * 0.143,
            (0.715 + c * 0.285) + s * 0.140,
            (0.072 - c * 0.072) - s * 0.283
        )),
        dot(color, vec3(
            (0.213 - c * 0.213) - s * 0.787,
            (0.715 - c * 0.715) + s * 0.715,
            (0.072 + c * 0.928) + s * 0.072
        ))
    ));
}

float dither(vec2 pixel) {
    return fract(dot(pixel, vec2(0.7548776865, 0.5698403120)));
}

void main() {
    vec2 fragCoord = v_uv * u_screen;
    float invHeight = 1.0 / max(u_screen.y, 1.0);
    vec2 p = (fragCoord - u_screen * 0.5) * invHeight;
    vec2 hueTrig = vec2(cos(u_hue), sin(u_hue));
    vec3 color = vec3(0.0);

    if (u_kind == 0) {
        float t = u_time * 0.72;
        float r0 = 0.16 * sin(p.x * 2.1 + t) + 0.05 * cos(p.x * 4.7 - t * 0.6);
        float r1 = -0.14 + 0.18 * sin(p.x * 1.6 - t * 0.75 + 1.4) + 0.04 * sin(p.x * 3.9 + t * 0.9);
        float r2 = 0.10 + 0.15 * cos(p.x * 2.5 + t * 0.55 + 2.8) + 0.03 * sin(p.x * 5.1 - t * 0.8);
        float r3 = -0.03 + 0.12 * sin(p.x * 3.0 - t * 0.4 + 4.3);
        float b0 = 1.0 - smoothstep(0.02, 0.44, abs(p.y - r0));
        float b1 = 1.0 - smoothstep(0.01, 0.38, abs(p.y - r1));
        float b2 = 1.0 - smoothstep(0.01, 0.34, abs(p.y - r2));
        float b3 = 1.0 - smoothstep(0.00, 0.28, abs(p.y - r3));
        color = mix(vec3(0.014, 0.020, 0.072), vec3(0.092, 0.028, 0.155), clamp(p.y + 0.5, 0.0, 1.0));
        color = mix(color, vec3(0.040, 0.520, 0.680), b0 * 0.65);
        color = mix(color, vec3(0.380, 0.110, 0.720), b1 * 0.58);
        color = mix(color, vec3(0.880, 0.240, 0.480), b2 * 0.46);
        color = mix(color, vec3(0.160, 0.780, 0.620), b3 * 0.36);
    } else if (u_kind == 1) {
        float t = u_time * 0.68;
        float w0 = 0.15 * sin(p.x * 2.0 + t) + 0.04 * sin(p.x * 4.4 - t * 0.5);
        float w1 = -0.10 + 0.17 * sin(p.x * 1.5 - t * 0.65 + 1.9);
        float w2 = 0.13 + 0.13 * cos(p.x * 2.4 + t * 0.5 + 3.5);
        float w3 = -0.04 + 0.10 * sin(p.x * 3.1 - t * 0.4 + 5.0);
        float b0 = 1.0 - smoothstep(0.02, 0.46, abs(p.y - w0));
        float b1 = 1.0 - smoothstep(0.01, 0.40, abs(p.y - w1));
        float b2 = 1.0 - smoothstep(0.01, 0.34, abs(p.y - w2));
        float b3 = 1.0 - smoothstep(0.00, 0.30, abs(p.y - w3));
        color = mix(vec3(0.008, 0.016, 0.048), vec3(0.020, 0.050, 0.115), clamp(p.y + 0.5, 0.0, 1.0));
        color = mix(color, vec3(0.220, 0.680, 0.920), b0 * 0.66);
        color = mix(color, vec3(0.050, 0.440, 0.760), b1 * 0.60);
        color = mix(color, vec3(0.030, 0.240, 0.520), b2 * 0.50);
        color = mix(color, vec3(0.320, 0.820, 0.950), b3 * 0.34);
    } else if (u_kind == 2) {
        float t = u_time * 0.52;
        float w0 = -0.14 + 0.12 * sin(p.x * 1.4 + t) + 0.03 * cos(p.x * 3.2 - t * 0.6);
        float w1 = 0.15 + 0.10 * sin(p.x * 1.7 - t * 0.7 + 2.2);
        float w2 = 0.01 + 0.08 * sin(p.x * 2.2 + t * 0.5 + 4.1);
        float b0 = 1.0 - smoothstep(0.03, 0.55, abs(p.y - w0));
        float b1 = 1.0 - smoothstep(0.02, 0.48, abs(p.y - w1));
        float b2 = 1.0 - smoothstep(0.01, 0.36, abs(p.y - w2));
        color = mix(vec3(0.010, 0.026, 0.062), vec3(0.060, 0.026, 0.058), clamp(p.y + 0.5, 0.0, 1.0));
        color = mix(color, vec3(0.020, 0.490, 0.740), b0 * 0.68);
        color = mix(color, vec3(0.950, 0.380, 0.150), b1 * 0.54);
        color = mix(color, vec3(0.960, 0.660, 0.140), b2 * 0.32);
    } else if (u_kind == 3) {
        float t = u_time * 0.28;
        vec2 q = p + 0.10 * vec2(sin(p.y * 3.6 + t), cos(p.x * 4.1 - t * 0.75));
        float f1 = sin(q.x * 5.8 + q.y * 4.2 + t * 1.1);
        float f2 = sin(q.x * 4.0 - q.y * 6.5 - t * 0.85 + 1.2);
        float f3 = sin((q.x + q.y) * 5.0 + t * 0.55 + 2.7);
        float sum = f1 + f2 + f3;
        vec3 caust = pow(vec3(1.0) - clamp(abs(vec3(sum) + vec3(0.06, 0.0, -0.06)) * 0.35, 0.0, 1.0), vec3(3.0));
        float depth = clamp(0.64 + 0.24 * sin(q.x * 2.8 - q.y * 3.4 + t * 0.4), 0.0, 1.0);
        float ty = clamp(p.y + 0.5, 0.0, 1.0);
        color = vec3(0.012, 0.068, 0.082) + vec3(0.010, 0.028, 0.036) * ty + caust * depth * vec3(0.48, 0.72, 0.82);
    } else {
        float t = u_time * 0.45;
        vec2 halfSize = vec2(0.5 * u_screen.x * invHeight, 0.5);
        vec2 toEdge = halfSize - abs(p);
        float edgeNear = min(toEdge.x, toEdge.y);
        float edgeBoost = 0.0;
        if (edgeNear < 0.14) {
            float u = (0.14 - edgeNear) / 0.14;
            edgeBoost = u * u * 1.15;
        }
        float spin = t * 0.24 + 0.22 * sin(t * 0.12);
        color = vec3(0.020, 0.016, 0.024);
        for (int i = 0; i < 4; i++) {
            float a = spin + float(i) * 1.5707963;
            vec2 lp = vec2((halfSize.x + 0.22) * cos(a), (halfSize.y + 0.22) * sin(a));
            vec2 d = p - lp;
            float wLobe = exp(-dot(d, d) * 1.85) * (0.68 + edgeBoost);
            vec3 tint = (i == 0) ? vec3(1.00, 0.72, 0.34)
                : (i == 1) ? vec3(0.96, 0.48, 0.26)
                : (i == 2) ? vec3(0.82, 0.34, 0.46)
                : vec3(1.00, 0.86, 0.64);
            color += wLobe * tint;
        }
    }

    if (abs(u_hue) > 0.001) {
        color = rotateHue(color, hueTrig);
    }
    color += vec3((dither(fragCoord) - 0.5) / 255.0);
    gl_FragColor = vec4(clamp(color * u_dim, 0.0, 1.0), 1.0);
}
"#;

const FS_TEX_RGBA: &str = r#"
precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_tex;
void main() {
    // u32 ARGB in memory is BGRA; shader samples premultiplied RGBA
    gl_FragColor = texture2D(u_tex, v_uv).bgra;
}
"#;

const FS_ROUND_RECT: &str = r#"
precision mediump float;
varying vec2 v_uv;
uniform vec2 u_screen;
uniform vec4 u_rect;
uniform float u_radius;
uniform vec4 u_color;
void main() {
    vec2 p = v_uv * u_screen;
    vec2 half_sz = u_rect.zw * 0.5;
    vec2 center = u_rect.xy + half_sz;
    vec2 d = abs(p - center) - (half_sz - vec2(u_radius));
    float dist = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - u_radius;
    float a = clamp(0.5 - dist, 0.0, 1.0) * u_color.a;
    if (a <= 0.002) discard;
    gl_FragColor = vec4(u_color.rgb, a);
}
"#;

const FS_KEY_FX: &str = r#"
precision mediump float;
varying vec2 v_uv;
uniform vec2 u_screen;
uniform vec4 u_key;
uniform vec4 u_params; // kind, age_ms, rel_age_ms, glow
uniform vec4 u_ring_col;

float sd_box(vec2 p, vec2 b) {
    vec2 d = abs(p) - b;
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0);
}

void main() {
    vec2 p = v_uv * u_screen;
    vec2 c = u_key.xy;
    vec2 sz = u_key.zw;
    float kind = u_params.x;
    float age_ms = u_params.y;
    float rel_ms = u_params.z;
    float glow = u_params.w;
    vec4 col = vec4(0.0);

    if (kind < 1.5) {
        // GlowingRings
        float base_r = min(sz.x, sz.y) * 0.34;
        float dist = abs(length(p - c) - base_r);
        float a_core = clamp(1.0 - dist / 2.6, 0.0, 1.0) * glow * 0.92;
        float dist_h = abs(length(p - c) - (base_r + 5.0));
        float a_halo = clamp(1.0 - dist_h / 5.5, 0.0, 1.0) * glow * 0.42;
        float a = max(a_core, a_halo);
        if (age_ms > 0.0 && age_ms < 420.0) {
            float pt = age_ms / 420.0;
            float pr = base_r + pt * 18.0;
            float pd = abs(length(p - c) - pr);
            a = max(a, clamp(1.0 - pd / 2.2, 0.0, 1.0) * (1.0 - pt) * 0.65);
        }
        col = vec4(u_ring_col.rgb, a);
    } else if (kind < 2.5) {
        // SolidHighlight
        vec2 d = abs(p - c) - (sz * 0.5 - vec2(9.0));
        float dist = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - 8.0;
        float a = clamp(0.5 - dist, 0.0, 1.0) * glow * 0.72;
        col = vec4(u_ring_col.rgb, a);
    } else if (kind < 3.5) {
        // Ripple Pulse: single clean ring per press, gentle 2100ms wave, zero solid key fill
        float base_r = min(sz.x, sz.y) * 0.34;
        float a = 0.0;
        if (age_ms >= 0.0 && age_ms < 2100.0) {
            float unit = min(sz.x, sz.y);
            float pt = clamp(age_ms / 2100.0, 0.0, 1.0);
            float ease = 1.0 - (1.0 - pt) * (1.0 - pt);
            float pr = base_r + ease * (unit * 2.1);
            float pd = abs(length(p - c) - pr);
            a = clamp(1.0 - pd / 3.8, 0.0, 1.0) * (1.0 - pt) * 0.92;
        }
        col = vec4(u_ring_col.rgb, a);
    } else {
        // AutofocusLock (tight reticle framing key gaps without overlapping neighbor legends)
        float px = max(sz.x * 0.78, 56.0);
        float py = max(sz.y * 0.78, 56.0);
        float scale = 1.0;
        if (age_ms < 110.0) {
            float u = clamp(age_ms / 110.0, 0.0, 1.0);
            scale = 1.36 - 0.44 * (1.0 - (1.0 - u) * (1.0 - u));
        } else if (age_ms < 230.0) {
            float u = clamp((age_ms - 110.0) / 120.0, 0.0, 1.0);
            scale = 0.92 + 0.08 * (1.0 - (1.0 - u) * (1.0 - u));
        } else {
            scale = 1.0 + clamp(rel_ms / 480.0, 0.0, 1.0) * 0.14;
        }
        float flash = 0.0;
        if (age_ms >= 205.0 && age_ms <= 320.0) {
            flash = (1.0 - clamp(abs(age_ms - 245.0) / 55.0, 0.0, 1.0)) * 0.82;
        }
        vec3 lock_rgb = mix(u_ring_col.rgb, vec3(1.0), flash);
        float hw = px * scale;
        float hh = py * scale;
        float arm = clamp(sz.x * 0.28, 14.0, 26.0);
        vec2 dp = abs(p - c);
        float box_d = abs(sd_box(p - c, vec2(hw, hh)));
        float in_corner = step(hw - arm, dp.x) * step(hh - arm, dp.y);
        float bracket_a = clamp(1.0 - box_d / 2.4, 0.0, 1.0) * in_corner * glow * 0.95;
        float halo_a = clamp(1.0 - box_d / 5.5, 0.0, 1.0) * in_corner * glow * 0.30;
        // Keycap subtle outline + glow
        float kbox = abs(sd_box(p - c, sz * 0.47));
        float key_a = clamp(1.0 - kbox / 1.8, 0.0, 1.0) * glow * 0.80;
        // Side horizontal target ticks
        float tick_y = step(dp.y, 1.6);
        float tick_x = step(hw - 10.0, dp.x) * step(dp.x, hw + 2.0);
        float tick_a = tick_x * tick_y * glow * 0.90;
        // Early horizontal scan bar
        float scan_t = clamp(age_ms / 180.0, 0.0, 1.0);
        float scan_span = px * (0.45 + 0.72 * (1.0 - (1.0 - scan_t) * (1.0 - scan_t)));
        float scan_a = tick_y * step(sz.x * 0.42, dp.x) * step(dp.x, scan_span) * (1.0 - scan_t * 0.72) * 0.75 * glow;
        float a = max(max(bracket_a, halo_a), max(key_a, max(tick_a, scan_a)));
        col = vec4(lock_rgb, a);
    }

    if (col.a <= 0.002) discard;
    gl_FragColor = col;
}
"#;

unsafe fn compile_shader(gl: &EglGlesLib, name: &str, ty: u32, src: &str) -> Result<u32, String> {
    let sh = (gl.gl_create_shader)(ty);
    if sh == 0 {
        return Err(format!("glCreateShader failed for {name}"));
    }
    let csrc = CString::new(src).unwrap();
    let ptr = csrc.as_ptr();
    (gl.gl_shader_source)(sh, 1, &ptr, std::ptr::null());
    (gl.gl_compile_shader)(sh);
    let mut ok = 0i32;
    (gl.gl_get_shaderiv)(sh, GL_COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = [0u8; 512];
        let mut len = 0i32;
        (gl.gl_get_shader_info_log)(
            sh,
            buf.len() as i32,
            &mut len,
            buf.as_mut_ptr() as *mut c_char,
        );
        let msg = String::from_utf8_lossy(&buf[..(len.max(0) as usize).min(buf.len())])
            .trim()
            .to_string();
        (gl.gl_delete_shader)(sh);
        let stage = if ty == GL_VERTEX_SHADER {
            "vertex"
        } else {
            "fragment"
        };
        return Err(format!("GLSL {name} {stage} shader compile failed: {msg}"));
    }
    Ok(sh)
}

unsafe fn build_program(
    gl: &EglGlesLib,
    name: &str,
    vs_src: &str,
    fs_src: &str,
) -> Result<u32, String> {
    let vs = compile_shader(gl, name, GL_VERTEX_SHADER, vs_src)?;
    let fs = match compile_shader(gl, name, GL_FRAGMENT_SHADER, fs_src) {
        Ok(fs) => fs,
        Err(e) => {
            (gl.gl_delete_shader)(vs);
            return Err(e);
        }
    };
    let prog = (gl.gl_create_program)();
    if prog == 0 {
        (gl.gl_delete_shader)(vs);
        (gl.gl_delete_shader)(fs);
        return Err(format!("glCreateProgram failed for {name}"));
    }
    (gl.gl_attach_shader)(prog, vs);
    (gl.gl_attach_shader)(prog, fs);
    (gl.gl_link_program)(prog);
    (gl.gl_delete_shader)(vs);
    (gl.gl_delete_shader)(fs);
    let mut ok = 0i32;
    (gl.gl_get_programiv)(prog, GL_LINK_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = [0u8; 512];
        let mut len = 0i32;
        (gl.gl_get_program_info_log)(
            prog,
            buf.len() as i32,
            &mut len,
            buf.as_mut_ptr() as *mut c_char,
        );
        let msg = String::from_utf8_lossy(&buf[..(len.max(0) as usize).min(buf.len())])
            .trim()
            .to_string();
        (gl.gl_delete_program)(prog);
        return Err(format!("GLSL {name} program link failed: {msg}"));
    }
    Ok(prog)
}

struct InitGuard<'a> {
    gl: &'a EglGlesLib,
    wl_egl_win: *mut c_void,
    egl_disp: *mut c_void,
    egl_ctx: *mut c_void,
    egl_surf: *mut c_void,
    progs: Vec<u32>,
    active: bool,
}

impl Drop for InitGuard<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        unsafe {
            for &p in &self.progs {
                if p != 0 {
                    (self.gl.gl_delete_program)(p);
                }
            }
            if !self.egl_disp.is_null() {
                let _ = (self.gl.egl_make_current)(
                    self.egl_disp,
                    EGL_NO_SURFACE,
                    EGL_NO_SURFACE,
                    EGL_NO_CONTEXT,
                );
                if !self.egl_surf.is_null() {
                    (self.gl.egl_destroy_surface)(self.egl_disp, self.egl_surf);
                }
                if !self.egl_ctx.is_null() {
                    (self.gl.egl_destroy_context)(self.egl_disp, self.egl_ctx);
                }
                (self.gl.egl_terminate)(self.egl_disp);
            }
            if !self.wl_egl_win.is_null() {
                (self.gl.wl_egl_window_destroy)(self.wl_egl_win);
            }
        }
    }
}

const FS_V2_REFRACT: &str = r#"
precision highp float;
varying vec2 v_uv;
uniform sampler2D u_tex_y;
uniform sampler2D u_tex_uv;
uniform sampler2D u_tex_rgba;
uniform sampler2D u_tex_wave;
uniform int u_bg_mode; // 0 = aurora shader, 1 = rkmpp NV12 video, 2 = RGBA image
uniform float u_time;
uniform float u_refract_px;
uniform float u_specular;
uniform float u_wave_enabled;
uniform float u_bg_dim;
uniform vec2 u_screen;
uniform vec4 u_tint;

vec3 sample_bg(vec2 uv) {
    vec2 suv = clamp(uv, vec2(0.001), vec2(0.999));
    if (u_bg_mode == 1) {
        float y = texture2D(u_tex_y, suv).r;
        vec2 cbcr = texture2D(u_tex_uv, suv).ra - vec2(0.5, 0.5);
        float ys = (y - 0.062745) * 1.164383;
        float r = ys + 1.792741 * cbcr.y;
        float g = ys - 0.213249 * cbcr.x - 0.532909 * cbcr.y;
        float b = ys + 2.112402 * cbcr.x;
        return clamp(vec3(r, g, b), 0.0, 1.0);
    } else if (u_bg_mode == 2) {
        return texture2D(u_tex_rgba, suv).bgr;
    } else {
        float t = u_time * 0.45;
        vec2 p = vec2(suv.x * 1.777, suv.y);
        float r1 = sin(p.x * 3.8 + t + sin(p.y * 4.2 - t * 0.6) * 0.9);
        float r2 = sin(p.x * 6.5 - p.y * 3.2 - t * 1.15);
        float r3 = cos((p.x + p.y) * 9.0 + t * 0.8);
        float rib1 = smoothstep(-0.35, 0.95, r1 * 0.65 + r2 * 0.35);
        float rib2 = smoothstep(0.1, 0.98, r2 * 0.55 + r3 * 0.45);
        vec3 deep = vec3(0.025, 0.04, 0.09);
        vec3 cyan = vec3(0.06, 0.42, 0.58);
        vec3 emerald = vec3(0.07, 0.52, 0.44);
        vec3 violet = vec3(0.36, 0.14, 0.56);
        vec3 col = mix(deep, mix(cyan, emerald, suv.y), rib1 * 0.72);
        col = mix(col, violet, rib2 * 0.42 * (0.35 + 0.65 * suv.x));
        float gx = smoothstep(0.96, 1.0, abs(sin(suv.x * 34.0 + r1 * 0.35)));
        float gy = smoothstep(0.96, 1.0, abs(sin(suv.y * 20.0 + r2 * 0.35)));
        return clamp(col + vec3(0.16, 0.34, 0.48) * max(gx, gy) * 0.22, 0.0, 1.0);
    }
}

void main() {
    float top_t = clamp(v_uv.y / 0.25, 0.0, 1.0);
    float top_ease = (1.0 - top_t) * (1.0 - top_t) * (1.0 - top_t);
    float top_fade = clamp(u_tint.a, 0.0, 0.85) * top_ease;
    float dim_scale = (1.0 - clamp(u_bg_dim, 0.0, 0.9)) * (1.0 - top_fade);
    if (u_wave_enabled < 0.5) {
        gl_FragColor = vec4(sample_bg(v_uv) * dim_scale, 1.0);
        return;
    }
    vec4 w = texture2D(u_tex_wave, v_uv);
    vec2 grad = (w.rg - vec2(0.5, 0.5)) * 2.0;
    float hc = (w.b - 0.5) / 0.35;
    vec2 offset_uv = (grad * u_refract_px) / u_screen;

    float r = sample_bg(v_uv + offset_uv * 1.12).r;
    float g = sample_bg(v_uv + offset_uv).g;
    float b = sample_bg(v_uv + offset_uv * 0.88).b;
    vec3 col = vec3(r, g, b) * dim_scale;

    vec3 n = normalize(vec3(-grad.x * 4.2, -grad.y * 4.2, 1.0));
    vec3 l1 = normalize(vec3(-0.32, -0.52, 0.79));
    vec3 view = vec3(0.0, 0.0, 1.0);
    float spec = pow(max(dot(n, normalize(l1 + view)), 0.0), 26.0) * smoothstep(0.05, 0.45, max(hc, 0.0));
    float crest = pow(clamp(hc, 0.0, 1.0), 2.2) * 0.38;
    col = clamp(col + u_tint.rgb * (spec * u_specular + crest * u_specular), 0.0, 1.0);
    gl_FragColor = vec4(col, 1.0);
}
"#;

const FS_V2_GLOW: &str = r#"
precision highp float;
varying vec2 v_uv;
uniform vec2 u_screen;
uniform vec4 u_rect;   // cx, cy, w, h
uniform vec4 u_params; // shape, radius_px, thickness_px, intensity
uniform vec4 u_color;

float sd_round_box(vec2 p, vec2 b, float r) {
    vec2 q = abs(p) - b + vec2(r);
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

void main() {
    vec2 px = v_uv * u_screen;
    vec2 local = px - u_rect.xy;
    vec2 half_sz = u_rect.zw * 0.5;
    int shape = int(u_params.x + 0.5);
    float radius = max(u_params.y, 1.0);
    float thick = max(u_params.z, 1.0);
    float intensity = u_params.w;

    float alpha = 0.0;
    float boost = 1.0;

    if (shape == 0) {
        float d = sd_round_box(local, half_sz, 10.0);
        float inside = smoothstep(4.0, -6.0, d);
        float dout = max(d, 0.0);
        float win = smoothstep(radius * 2.3, radius * 0.4, dout);
        float halo = exp(-dout / max(radius * 0.48, 4.0)) * win;
        alpha = clamp(inside * 0.45 + halo * 0.78, 0.0, 1.0) * intensity;
        boost = 1.0 + inside * 0.35;
    } else if (shape == 1) {
        float d = sd_round_box(local, half_sz + vec2(radius), 12.0);
        alpha = smoothstep(thick, 0.0, abs(d)) * intensity;
        boost = 1.2;
    } else if (shape == 2) {
        float dist = length(local);
        alpha = smoothstep(thick, 0.0, abs(dist - radius)) * intensity;
        boost = 1.15;
    } else if (shape == 3) {
        float d = sd_round_box(local, half_sz, 10.0);
        alpha = smoothstep(1.5, -1.5, d) * intensity;
    } else {
        float dist = length(local);
        float core = smoothstep(radius, radius * 0.25, dist);
        float aura = exp(-dist / max(radius * 1.35, 2.0)) * 0.55;
        alpha = clamp(core + aura, 0.0, 1.0) * intensity;
        boost = 1.0 + core * 0.6;
    }

    float fa = clamp(alpha * u_color.a, 0.0, 1.0);
    if (fa < 0.002) discard;
    gl_FragColor = vec4(clamp(u_color.rgb * boost, 0.0, 1.0), fa);
}
"#;

const FS_V2_OVERLAY: &str = r#"
precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_tex;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv);
}
"#;

pub struct WaylandGpuPresenter {
    conn: Connection,
    queue: EventQueue<WlEglState>,
    state: WlEglState,
    gl: EglGlesLib,
    egl_disp: *mut c_void,
    egl_ctx: *mut c_void,
    egl_surf: *mut c_void,
    wl_egl_win: *mut c_void,
    width: i32,
    height: i32,

    prog_bg: u32,
    prog_shader: u32,
    prog_blur: u32,
    prog_rgba: u32,
    prog_rect: u32,
    prog_keyfx: u32,
    prog_v2_refract: u32,
    prog_v2_glow: u32,
    prog_v2_overlay: u32,
    shader_bg: Option<(i32, f32, f32)>,

    tex_y: [u32; 2],
    tex_uv: [u32; 2],
    tex_bg_rgba: u32,
    tex_ui_rgba: [u32; 2],
    tex_v2_wave: u32,
    tex_v2_overlay: u32,
    v2_wave_uploaded: bool,
    v2_overlay_uploaded: bool,
    fbo_a: u32,
    tex_fbo_a: u32,
    fbo_b: u32,
    tex_fbo_b: u32,
    blur_w: i32,
    blur_h: i32,

    bg_tex_dims: (i32, i32),
    nv12_dims: [(i32, i32); 2],
    nv12_idx: usize,
    ui_uploaded: [bool; 2],
    ui_idx: usize,
    bg_is_nv12: bool,
}

impl WaylandGpuPresenter {
    pub fn open(width: i32, height: i32, title: &str) -> Result<Self, String> {
        let gl = EglGlesLib::load()?;
        let conn = Connection::connect_to_env().map_err(|e| format!("Wayland connect: {e}"))?;
        let display = conn.display();
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let mut state = WlEglState {
            compositor: None,
            wm_base: None,
            surface: None,
            xdg_surface: None,
            xdg_toplevel: None,
            configured: false,
            closed: false,
            frame_pending: false,
        };
        display.get_registry(&qh, ());
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland roundtrip: {e}"))?;
        let comp = state.compositor.as_ref().ok_or("No wl_compositor")?;
        let wm_base = state.wm_base.as_ref().ok_or("No xdg_wm_base")?;

        let surface = comp.create_surface(&qh, ());
        let xdg_surf = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surf.get_toplevel(&qh, ());
        toplevel.set_title(title.to_string());
        toplevel.set_fullscreen(None);
        surface.commit();

        for _ in 0..20 {
            if state.configured {
                break;
            }
            queue
                .roundtrip(&mut state)
                .map_err(|e| format!("Wayland xdg configure: {e}"))?;
        }

        let wl_disp_ptr = conn.backend().display_ptr() as *mut c_void;
        let wl_surf_ptr = surface.id().as_ptr() as *mut c_void;
        if wl_disp_ptr.is_null() || wl_surf_ptr.is_null() {
            return Err(
                "Wayland backend does not expose native wl_display/wl_surface pointers".into(),
            );
        }

        state.surface = Some(surface);
        state.xdg_surface = Some(xdg_surf);
        state.xdg_toplevel = Some(toplevel);

        unsafe {
            let wl_egl_win = (gl.wl_egl_window_create)(wl_surf_ptr, width, height);
            if wl_egl_win.is_null() {
                return Err("wl_egl_window_create failed".into());
            }
            let mut guard = InitGuard {
                gl: &gl,
                wl_egl_win,
                egl_disp: EGL_NO_DISPLAY,
                egl_ctx: EGL_NO_CONTEXT,
                egl_surf: EGL_NO_SURFACE,
                progs: Vec::new(),
                active: true,
            };
            let egl_disp = (gl.egl_get_display)(wl_disp_ptr);
            if egl_disp == EGL_NO_DISPLAY {
                return Err("eglGetDisplay failed".into());
            }
            let mut maj = 0;
            let mut min = 0;
            if (gl.egl_initialize)(egl_disp, &mut maj, &mut min) == 0 {
                return Err("eglInitialize failed".into());
            }
            guard.egl_disp = egl_disp;
            (gl.egl_bind_api)(EGL_OPENGL_ES_API);
            let cfg_attrs = [
                EGL_RENDERABLE_TYPE,
                EGL_OPENGL_ES2_BIT,
                EGL_SURFACE_TYPE,
                EGL_WINDOW_BIT,
                EGL_RED_SIZE,
                8,
                EGL_GREEN_SIZE,
                8,
                EGL_BLUE_SIZE,
                8,
                EGL_ALPHA_SIZE,
                0,
                EGL_NONE,
            ];
            let mut egl_cfg = std::ptr::null_mut();
            let mut ncfg = 0;
            if (gl.egl_choose_config)(egl_disp, cfg_attrs.as_ptr(), &mut egl_cfg, 1, &mut ncfg) == 0
                || ncfg == 0
            {
                return Err("eglChooseConfig failed".into());
            }
            let ctx_attrs = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
            let egl_ctx =
                (gl.egl_create_context)(egl_disp, egl_cfg, EGL_NO_CONTEXT, ctx_attrs.as_ptr());
            let egl_surf =
                (gl.egl_create_window_surface)(egl_disp, egl_cfg, wl_egl_win, std::ptr::null());
            guard.egl_ctx = egl_ctx;
            guard.egl_surf = egl_surf;
            if egl_ctx.is_null() || egl_surf == EGL_NO_SURFACE {
                return Err("eglCreateContext/WindowSurface failed".into());
            }
            if (gl.egl_make_current)(egl_disp, egl_surf, egl_surf, egl_ctx) == 0 {
                return Err("eglMakeCurrent failed".into());
            }
            if let Some(si) = gl.egl_swap_interval {
                let _ = si(egl_disp, 1);
            }

            let prog_bg = build_program(&gl, "bg_nv12", VS_QUAD, FS_BG_NV12)?;
            guard.progs.push(prog_bg);
            let prog_shader = build_program(&gl, "shader_bg", VS_QUAD, FS_SHADER_BG)?;
            guard.progs.push(prog_shader);
            let prog_blur = build_program(&gl, "blur", VS_QUAD, FS_BLUR)?;
            guard.progs.push(prog_blur);
            let prog_rgba = build_program(&gl, "tex_rgba", VS_QUAD, FS_TEX_RGBA)?;
            guard.progs.push(prog_rgba);
            let prog_rect = build_program(&gl, "round_rect", VS_QUAD, FS_ROUND_RECT)?;
            guard.progs.push(prog_rect);
            let prog_keyfx = build_program(&gl, "key_fx", VS_QUAD, FS_KEY_FX)?;
            guard.progs.push(prog_keyfx);
            let prog_v2_refract = build_program(&gl, "v2_refract", VS_QUAD, FS_V2_REFRACT)?;
            guard.progs.push(prog_v2_refract);
            let prog_v2_glow = build_program(&gl, "v2_glow", VS_QUAD, FS_V2_GLOW)?;
            guard.progs.push(prog_v2_glow);
            let prog_v2_overlay = build_program(&gl, "v2_overlay", VS_QUAD, FS_V2_OVERLAY)?;
            guard.progs.push(prog_v2_overlay);
            guard.active = false;
            drop(guard);

            let mut texs = [0u32; 11];
            (gl.gl_gen_textures)(11, texs.as_mut_ptr());
            for &t in &texs {
                (gl.gl_bind_texture)(GL_TEXTURE_2D, t);
                (gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
                (gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
                (gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
                (gl.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            }
            let tex_y = [texs[0], texs[1]];
            let tex_uv = [texs[2], texs[3]];
            let tex_ui_rgba = [texs[5], texs[6]];
            let (tex_bg_rgba, tex_fbo_a, tex_fbo_b, tex_v2_wave, tex_v2_overlay) =
                (texs[4], texs[7], texs[8], texs[9], texs[10]);

            let blur_w = (width / 4).max(16);
            let blur_h = (height / 4).max(16);
            (gl.gl_bind_texture)(GL_TEXTURE_2D, tex_fbo_a);
            (gl.gl_tex_image_2d)(
                GL_TEXTURE_2D,
                0,
                GL_RGBA as i32,
                blur_w,
                blur_h,
                0,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                std::ptr::null(),
            );
            (gl.gl_bind_texture)(GL_TEXTURE_2D, tex_fbo_b);
            (gl.gl_tex_image_2d)(
                GL_TEXTURE_2D,
                0,
                GL_RGBA as i32,
                blur_w,
                blur_h,
                0,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                std::ptr::null(),
            );

            let mut fbos = [0u32; 2];
            (gl.gl_gen_framebuffers)(2, fbos.as_mut_ptr());
            (gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, fbos[0]);
            (gl.gl_framebuffer_texture_2d)(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                tex_fbo_a,
                0,
            );
            (gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, fbos[1]);
            (gl.gl_framebuffer_texture_2d)(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                tex_fbo_b,
                0,
            );
            (gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, 0);
            (gl.gl_pixel_storei)(GL_UNPACK_ALIGNMENT, 1);

            Ok(Self {
                conn,
                queue,
                state,
                gl,
                egl_disp,
                egl_ctx,
                egl_surf,
                wl_egl_win,
                width,
                height,
                prog_bg,
                prog_shader,
                prog_blur,
                prog_rgba,
                prog_rect,
                prog_keyfx,
                prog_v2_refract,
                prog_v2_glow,
                prog_v2_overlay,
                shader_bg: None,
                tex_y,
                tex_uv,
                tex_bg_rgba,
                tex_ui_rgba,
                tex_v2_wave,
                tex_v2_overlay,
                v2_wave_uploaded: false,
                v2_overlay_uploaded: false,
                fbo_a: fbos[0],
                tex_fbo_a,
                fbo_b: fbos[1],
                tex_fbo_b,
                blur_w,
                blur_h,
                bg_tex_dims: (0, 0),
                nv12_dims: [(0, 0), (0, 0)],
                nv12_idx: 0,
                ui_uploaded: [false, false],
                ui_idx: 0,
                bg_is_nv12: false,
            })
        }
    }

    pub fn raw_fd(&self) -> i32 {
        self.conn.as_fd().as_raw_fd()
    }

    pub fn is_frame_pending(&self) -> bool {
        self.state.frame_pending
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

    pub fn upload_bg_nv12(&mut self, w: i32, h: i32, nv12: &[u8]) {
        if w <= 0 || h <= 0 || nv12.len() < (w as usize) * (h as usize) * 3 / 2 {
            return;
        }
        let y_len = (w as usize) * (h as usize);
        let idx = self.nv12_idx ^ 1;
        self.nv12_idx = idx;
        unsafe {
            (self.gl.gl_active_texture)(GL_TEXTURE0);
            (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_y[idx]);
            if self.nv12_dims[idx] != (w, h) {
                (self.gl.gl_tex_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    GL_LUMINANCE as i32,
                    w,
                    h,
                    0,
                    GL_LUMINANCE,
                    GL_UNSIGNED_BYTE,
                    nv12.as_ptr() as *const c_void,
                );
            } else {
                (self.gl.gl_tex_sub_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    0,
                    0,
                    w,
                    h,
                    GL_LUMINANCE,
                    GL_UNSIGNED_BYTE,
                    nv12.as_ptr() as *const c_void,
                );
            }
            (self.gl.gl_active_texture)(GL_TEXTURE1);
            (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_uv[idx]);
            if self.nv12_dims[idx] != (w, h) {
                (self.gl.gl_tex_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    GL_LUMINANCE_ALPHA as i32,
                    w / 2,
                    h / 2,
                    0,
                    GL_LUMINANCE_ALPHA,
                    GL_UNSIGNED_BYTE,
                    nv12[y_len..].as_ptr() as *const c_void,
                );
                self.nv12_dims[idx] = (w, h);
            } else {
                (self.gl.gl_tex_sub_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    0,
                    0,
                    w / 2,
                    h / 2,
                    GL_LUMINANCE_ALPHA,
                    GL_UNSIGNED_BYTE,
                    nv12[y_len..].as_ptr() as *const c_void,
                );
            }
            self.bg_is_nv12 = true;
        }
    }

    pub fn upload_bg_argb(&mut self, w: i32, h: i32, argb: &[u32]) {
        if w <= 0 || h <= 0 || argb.len() < (w as usize) * (h as usize) {
            return;
        }
        unsafe {
            (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_bg_rgba);
            if self.bg_tex_dims != (w, h) {
                (self.gl.gl_tex_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    GL_RGBA as i32,
                    w,
                    h,
                    0,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    argb.as_ptr() as *const c_void,
                );
                self.bg_tex_dims = (w, h);
            } else {
                (self.gl.gl_tex_sub_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    0,
                    0,
                    w,
                    h,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    argb.as_ptr() as *const c_void,
                );
            }
            self.bg_is_nv12 = false;
        }
    }

    pub fn set_shader_bg(&mut self, shader: Option<(i32, f32, f32)>) {
        self.shader_bg = shader;
    }

    pub fn upload_ui_overlay_argb(&mut self, argb: &[u32]) {
        if argb.len() < (self.width as usize) * (self.height as usize) {
            return;
        }
        let idx = self.ui_idx ^ 1;
        self.ui_idx = idx;
        unsafe {
            (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_ui_rgba[idx]);
            if !self.ui_uploaded[idx] {
                (self.gl.gl_tex_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    GL_RGBA as i32,
                    self.width,
                    self.height,
                    0,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    argb.as_ptr() as *const c_void,
                );
                self.ui_uploaded[idx] = true;
            } else {
                (self.gl.gl_tex_sub_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    0,
                    0,
                    self.width,
                    self.height,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    argb.as_ptr() as *const c_void,
                );
            }
        }
    }

    unsafe fn draw_fullscreen_quad(&self, prog: u32) {
        static VERTS: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
        let loc = (self.gl.gl_get_attrib_location)(prog, c"a_pos".as_ptr());
        if loc >= 0 {
            (self.gl.gl_enable_vertex_attrib_array)(loc as u32);
            (self.gl.gl_vertex_attrib_pointer)(
                loc as u32,
                2,
                GL_FLOAT,
                0,
                0,
                VERTS.as_ptr() as *const c_void,
            );
            (self.gl.gl_draw_arrays)(GL_TRIANGLE_STRIP, 0, 4);
        }
    }

    unsafe fn draw_bounds_quad(&self, prog: u32, x0: f32, y0: f32, x1: f32, y1: f32) {
        let nx0 = (x0 / self.width as f32).clamp(0.0, 1.0) * 2.0 - 1.0;
        let nx1 = (x1 / self.width as f32).clamp(0.0, 1.0) * 2.0 - 1.0;
        // GL NDC Y is +1 top, -1 bottom
        let ny0 = 1.0 - (y1 / self.height as f32).clamp(0.0, 1.0) * 2.0;
        let ny1 = 1.0 - (y0 / self.height as f32).clamp(0.0, 1.0) * 2.0;
        let verts: [f32; 8] = [nx0, ny0, nx1, ny0, nx0, ny1, nx1, ny1];
        let loc = (self.gl.gl_get_attrib_location)(prog, c"a_pos".as_ptr());
        if loc >= 0 {
            (self.gl.gl_enable_vertex_attrib_array)(loc as u32);
            (self.gl.gl_vertex_attrib_pointer)(
                loc as u32,
                2,
                GL_FLOAT,
                0,
                0,
                verts.as_ptr() as *const c_void,
            );
            (self.gl.gl_draw_arrays)(GL_TRIANGLE_STRIP, 0, 4);
        }
    }

    pub fn render_and_swap(
        &mut self,
        dim_factor: f32,
        blur_amount: f32,
        suspended: bool,
        glass_rects: &[GpuRoundRect],
        key_effects: &[GpuKeyEffect],
    ) -> Result<(), String> {
        let _ = self.queue.dispatch_pending(&mut self.state);
        unsafe {
            if suspended {
                (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, 0);
                (self.gl.gl_viewport)(0, 0, self.width, self.height);
                (self.gl.gl_clear_color)(0.0, 0.0, 0.0, 1.0);
                (self.gl.gl_clear)(GL_COLOR_BUFFER_BIT);
            } else {
                (self.gl.gl_disable)(GL_BLEND);
                if blur_amount > 0.5 {
                    // Pass 1a: BG/video into 1/4-res FBO A
                    (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, self.fbo_a);
                    (self.gl.gl_viewport)(0, 0, self.blur_w, self.blur_h);
                    self.draw_bg_pass(dim_factor);

                    // Pass 1b: Horizontal blur A -> B
                    let passes = ((blur_amount / 12.0).ceil() as i32).clamp(1, 3);
                    let u_tex = CString::new("u_tex").unwrap();
                    let u_step = CString::new("u_step").unwrap();
                    (self.gl.gl_use_program)(self.prog_blur);
                    (self.gl.gl_uniform1i)(
                        (self.gl.gl_get_uniform_location)(self.prog_blur, u_tex.as_ptr()),
                        0,
                    );
                    (self.gl.gl_active_texture)(GL_TEXTURE0);

                    for p in 0..passes {
                        let scale = 1.0 + p as f32 * 0.8;
                        (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, self.fbo_b);
                        (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_fbo_a);
                        (self.gl.gl_uniform2f)(
                            (self.gl.gl_get_uniform_location)(self.prog_blur, u_step.as_ptr()),
                            scale / self.blur_w as f32,
                            0.0,
                        );
                        self.draw_fullscreen_quad(self.prog_blur);

                        if p + 1 < passes {
                            (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, self.fbo_a);
                            (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_fbo_b);
                            (self.gl.gl_uniform2f)(
                                (self.gl.gl_get_uniform_location)(self.prog_blur, u_step.as_ptr()),
                                0.0,
                                scale / self.blur_h as f32,
                            );
                            self.draw_fullscreen_quad(self.prog_blur);
                        } else {
                            // Draw the final vertical blur pass directly to the screen.
                            (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, 0);
                            (self.gl.gl_viewport)(0, 0, self.width, self.height);
                            (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_fbo_b);
                            (self.gl.gl_uniform2f)(
                                (self.gl.gl_get_uniform_location)(self.prog_blur, u_step.as_ptr()),
                                0.0,
                                scale / self.blur_h as f32,
                            );
                            self.draw_fullscreen_quad(self.prog_blur);
                        }
                    }
                } else {
                    (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, 0);
                    (self.gl.gl_viewport)(0, 0, self.width, self.height);
                    self.draw_bg_pass(dim_factor);
                }

                // Pass 2: Mosaic glass & slot rounded rects on GPU (keep framebuffer alpha == 1.0)
                (self.gl.gl_enable)(GL_BLEND);
                (self.gl.gl_blend_func_separate)(
                    GL_SRC_ALPHA,
                    GL_ONE_MINUS_SRC_ALPHA,
                    GL_ONE,
                    GL_ONE_MINUS_SRC_ALPHA,
                );
                if !glass_rects.is_empty() {
                    (self.gl.gl_use_program)(self.prog_rect);
                    let u_scr = CString::new("u_screen").unwrap();
                    let u_rect = CString::new("u_rect").unwrap();
                    let u_rad = CString::new("u_radius").unwrap();
                    let u_col = CString::new("u_color").unwrap();
                    (self.gl.gl_uniform2f)(
                        (self.gl.gl_get_uniform_location)(self.prog_rect, u_scr.as_ptr()),
                        self.width as f32,
                        self.height as f32,
                    );
                    for rr in glass_rects {
                        (self.gl.gl_uniform4f)(
                            (self.gl.gl_get_uniform_location)(self.prog_rect, u_rect.as_ptr()),
                            rr.x,
                            rr.y,
                            rr.w,
                            rr.h,
                        );
                        (self.gl.gl_uniform1f)(
                            (self.gl.gl_get_uniform_location)(self.prog_rect, u_rad.as_ptr()),
                            rr.radius,
                        );
                        let a = ((rr.argb >> 24) & 0xFF) as f32 / 255.0;
                        let r = ((rr.argb >> 16) & 0xFF) as f32 / 255.0;
                        let g = ((rr.argb >> 8) & 0xFF) as f32 / 255.0;
                        let b = (rr.argb & 0xFF) as f32 / 255.0;
                        (self.gl.gl_uniform4f)(
                            (self.gl.gl_get_uniform_location)(self.prog_rect, u_col.as_ptr()),
                            r,
                            g,
                            b,
                            a,
                        );
                        self.draw_bounds_quad(
                            self.prog_rect,
                            rr.x - 4.0,
                            rr.y - 4.0,
                            rr.x + rr.w + 4.0,
                            rr.y + rr.h + 4.0,
                        );
                    }
                }

                // Pass 3: GPU Key Press Shaders (bounded quads per active key — zero fullscreen overdraw)
                if !key_effects.is_empty() {
                    (self.gl.gl_use_program)(self.prog_keyfx);
                    let u_scr = CString::new("u_screen").unwrap();
                    let u_key = CString::new("u_key").unwrap();
                    let u_par = CString::new("u_params").unwrap();
                    let u_col = CString::new("u_ring_col").unwrap();
                    (self.gl.gl_uniform2f)(
                        (self.gl.gl_get_uniform_location)(self.prog_keyfx, u_scr.as_ptr()),
                        self.width as f32,
                        self.height as f32,
                    );
                    for fx in key_effects {
                        (self.gl.gl_uniform4f)(
                            (self.gl.gl_get_uniform_location)(self.prog_keyfx, u_key.as_ptr()),
                            fx.kcx,
                            fx.kcy,
                            fx.kw,
                            fx.kh,
                        );
                        (self.gl.gl_uniform4f)(
                            (self.gl.gl_get_uniform_location)(self.prog_keyfx, u_par.as_ptr()),
                            fx.kind as f32,
                            fx.age_ms,
                            fx.rel_age_ms,
                            fx.glow,
                        );
                        let r = ((fx.ring_argb >> 16) & 0xFF) as f32 / 255.0;
                        let g = ((fx.ring_argb >> 8) & 0xFF) as f32 / 255.0;
                        let b = (fx.ring_argb & 0xFF) as f32 / 255.0;
                        (self.gl.gl_uniform4f)(
                            (self.gl.gl_get_uniform_location)(self.prog_keyfx, u_col.as_ptr()),
                            r,
                            g,
                            b,
                            1.0,
                        );
                        let ripple_pad = fx.kw.min(fx.kh) * 2.5 + 24.0;
                        let pad_x = if fx.kind == 4 {
                            (fx.kw * 0.78).max(56.0) * 1.45 + 16.0
                        } else if fx.kind == 3 {
                            (fx.kw * 0.5).max(ripple_pad)
                        } else {
                            fx.kw * 0.8 + 36.0
                        };
                        let pad_y = if fx.kind == 4 {
                            (fx.kh * 0.78).max(56.0) * 1.45 + 16.0
                        } else if fx.kind == 3 {
                            (fx.kh * 0.5).max(ripple_pad)
                        } else {
                            fx.kh * 0.8 + 36.0
                        };
                        self.draw_bounds_quad(
                            self.prog_keyfx,
                            fx.kcx - pad_x,
                            fx.kcy - pad_y,
                            fx.kcx + pad_x,
                            fx.kcy + pad_y,
                        );
                    }
                }

                // Pass 4: Cached UI Overlay (key legends, icons, Mosaic text & charts)
                if self.ui_uploaded[self.ui_idx] {
                    (self.gl.gl_blend_func)(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
                    (self.gl.gl_use_program)(self.prog_rgba);
                    (self.gl.gl_active_texture)(GL_TEXTURE0);
                    (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_ui_rgba[self.ui_idx]);
                    let u_tex = CString::new("u_tex").unwrap();
                    (self.gl.gl_uniform1i)(
                        (self.gl.gl_get_uniform_location)(self.prog_rgba, u_tex.as_ptr()),
                        0,
                    );
                    self.draw_fullscreen_quad(self.prog_rgba);
                }
            }

            if let Some(ref s) = self.state.surface {
                let qh = self.queue.handle();
                s.frame(&qh, ());
                self.state.frame_pending = true;
            }
            if (self.gl.egl_swap_buffers)(self.egl_disp, self.egl_surf) == 0 {
                return Err("eglSwapBuffers failed".into());
            }
            let _ = self.conn.flush();
        }
        Ok(())
    }

    /// Renders the complete OpenFlux v2 frame natively on Mali-G52 GLES 2.0:
    /// 1. Single-pass 3D chromatic water refraction + Fresnel specular over live `rkmpp` NV12 video
    ///    (or RGBA image, or animated Aurora Velvet shader)
    /// 2. Bounded-quad additive under-key glows (Analog Depth/Velocity + Classics) & SDF particles
    /// 3. Straight-alpha 2D overlay (cached keycap legends + top-bar Lua widgets + roaming Idle Pet)
    #[allow(clippy::too_many_arguments)]
    pub fn render_v2_and_swap(
        &mut self,
        suspended: bool,
        has_bg_media: bool,
        bg_dimming: f32,
        time_sec: f32,
        wave_enabled: bool,
        wave_w: i32,
        wave_h: i32,
        wave_rgba: &[u8],
        refract_px: f32,
        specular: f32,
        tint: [f32; 4],
        glows: &[crate::v2_runtime::V2GlowDraw],
        overlay_rgba: &[u8],
        overlay_dirty: bool,
    ) -> Result<(), String> {
        let _ = self.queue.dispatch_pending(&mut self.state);
        unsafe {
            (self.gl.gl_bind_framebuffer)(GL_FRAMEBUFFER, 0);
            (self.gl.gl_viewport)(0, 0, self.width, self.height);

            if suspended {
                (self.gl.gl_clear_color)(0.0, 0.0, 0.0, 1.0);
                (self.gl.gl_clear)(GL_COLOR_BUFFER_BIT);
            } else {
                // Upload tiny 192x108 RGBA8 wave normal/height map (~82 KB)
                if wave_enabled && wave_rgba.len() >= (wave_w as usize) * (wave_h as usize) * 4 {
                    (self.gl.gl_active_texture)(GL_TEXTURE0 + 3);
                    (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_v2_wave);
                    if !self.v2_wave_uploaded {
                        (self.gl.gl_tex_image_2d)(
                            GL_TEXTURE_2D,
                            0,
                            GL_RGBA as i32,
                            wave_w,
                            wave_h,
                            0,
                            GL_RGBA,
                            GL_UNSIGNED_BYTE,
                            wave_rgba.as_ptr() as *const c_void,
                        );
                        self.v2_wave_uploaded = true;
                    } else {
                        (self.gl.gl_tex_sub_image_2d)(
                            GL_TEXTURE_2D,
                            0,
                            0,
                            0,
                            wave_w,
                            wave_h,
                            GL_RGBA,
                            GL_UNSIGNED_BYTE,
                            wave_rgba.as_ptr() as *const c_void,
                        );
                    }
                }

                // Pass 1: Background + 3D Water Refraction & Specular
                (self.gl.gl_disable)(GL_BLEND);
                (self.gl.gl_use_program)(self.prog_v2_refract);

                (self.gl.gl_active_texture)(GL_TEXTURE0);
                (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_y[self.nv12_idx]);
                (self.gl.gl_uniform1i)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_tex_y".as_ptr()),
                    0,
                );
                (self.gl.gl_active_texture)(GL_TEXTURE1);
                (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_uv[self.nv12_idx]);
                (self.gl.gl_uniform1i)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_tex_uv".as_ptr()),
                    1,
                );
                (self.gl.gl_active_texture)(GL_TEXTURE0 + 2);
                (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_bg_rgba);
                (self.gl.gl_uniform1i)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_tex_rgba".as_ptr()),
                    2,
                );
                (self.gl.gl_active_texture)(GL_TEXTURE0 + 3);
                (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_v2_wave);
                (self.gl.gl_uniform1i)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_tex_wave".as_ptr()),
                    3,
                );

                let bg_mode = if has_bg_media && self.bg_is_nv12 {
                    1
                } else if has_bg_media && self.bg_tex_dims.0 > 0 {
                    2
                } else {
                    0
                };
                (self.gl.gl_uniform1i)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_bg_mode".as_ptr()),
                    bg_mode,
                );
                (self.gl.gl_uniform1f)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_time".as_ptr()),
                    time_sec,
                );
                (self.gl.gl_uniform1f)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_refract_px".as_ptr()),
                    refract_px,
                );
                (self.gl.gl_uniform1f)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_specular".as_ptr()),
                    specular,
                );
                (self.gl.gl_uniform1f)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_wave_enabled".as_ptr()),
                    if wave_enabled { 1.0 } else { 0.0 },
                );
                (self.gl.gl_uniform1f)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_bg_dim".as_ptr()),
                    bg_dimming.clamp(0.0, 0.9),
                );
                (self.gl.gl_uniform2f)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_screen".as_ptr()),
                    self.width as f32,
                    self.height as f32,
                );
                (self.gl.gl_uniform4f)(
                    (self.gl.gl_get_uniform_location)(self.prog_v2_refract, c"u_tint".as_ptr()),
                    tint[0],
                    tint[1],
                    tint[2],
                    tint[3],
                );
                self.draw_fullscreen_quad(self.prog_v2_refract);

                // Pass 2: Additive Under-Key Glows + SDF Bouncing/Refracting Particles
                if !glows.is_empty() {
                    (self.gl.gl_enable)(GL_BLEND);
                    (self.gl.gl_blend_func_separate)(
                        GL_SRC_ALPHA,
                        GL_ONE,
                        GL_ONE,
                        GL_ONE_MINUS_SRC_ALPHA,
                    );
                    (self.gl.gl_use_program)(self.prog_v2_glow);
                    let loc_scr = (self.gl.gl_get_uniform_location)(self.prog_v2_glow, c"u_screen".as_ptr());
                    let loc_rect = (self.gl.gl_get_uniform_location)(self.prog_v2_glow, c"u_rect".as_ptr());
                    let loc_par = (self.gl.gl_get_uniform_location)(self.prog_v2_glow, c"u_params".as_ptr());
                    let loc_col = (self.gl.gl_get_uniform_location)(self.prog_v2_glow, c"u_color".as_ptr());
                    (self.gl.gl_uniform2f)(loc_scr, self.width as f32, self.height as f32);

                    for g in glows {
                        (self.gl.gl_uniform4f)(loc_rect, g.cx, g.cy, g.w, g.h);
                        (self.gl.gl_uniform4f)(loc_par, g.shape, g.radius_px, g.thickness_px, g.intensity);
                        (self.gl.gl_uniform4f)(loc_col, g.color[0], g.color[1], g.color[2], g.color[3]);
                        let expand = g.radius_px * 2.5 + g.thickness_px + 24.0;
                        let hx = g.w * 0.5 + expand;
                        let hy = g.h * 0.5 + expand;
                        self.draw_bounds_quad(self.prog_v2_glow, g.cx - hx, g.cy - hy, g.cx + hx, g.cy + hy);
                    }
                }

                // Pass 3: 2D Straight-Alpha Overlay (Legends + Top-Bar Widgets + Idle Pet)
                if overlay_rgba.len() >= (self.width as usize) * (self.height as usize) * 4 {
                    (self.gl.gl_active_texture)(GL_TEXTURE0);
                    (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_v2_overlay);
                    if !self.v2_overlay_uploaded {
                        (self.gl.gl_tex_image_2d)(
                            GL_TEXTURE_2D,
                            0,
                            GL_RGBA as i32,
                            self.width,
                            self.height,
                            0,
                            GL_RGBA,
                            GL_UNSIGNED_BYTE,
                            overlay_rgba.as_ptr() as *const c_void,
                        );
                        self.v2_overlay_uploaded = true;
                    } else if overlay_dirty {
                        (self.gl.gl_tex_sub_image_2d)(
                            GL_TEXTURE_2D,
                            0,
                            0,
                            0,
                            self.width,
                            self.height,
                            GL_RGBA,
                            GL_UNSIGNED_BYTE,
                            overlay_rgba.as_ptr() as *const c_void,
                        );
                    }
                    (self.gl.gl_enable)(GL_BLEND);
                    (self.gl.gl_blend_func_separate)(
                        GL_SRC_ALPHA,
                        GL_ONE_MINUS_SRC_ALPHA,
                        GL_ONE,
                        GL_ONE_MINUS_SRC_ALPHA,
                    );
                    (self.gl.gl_use_program)(self.prog_v2_overlay);
                    (self.gl.gl_uniform1i)(
                        (self.gl.gl_get_uniform_location)(self.prog_v2_overlay, c"u_tex".as_ptr()),
                        0,
                    );
                    self.draw_fullscreen_quad(self.prog_v2_overlay);
                }
            }

            if let Some(ref s) = self.state.surface {
                let qh = self.queue.handle();
                s.frame(&qh, ());
                self.state.frame_pending = true;
            }
            if (self.gl.egl_swap_buffers)(self.egl_disp, self.egl_surf) == 0 {
                return Err("eglSwapBuffers failed".into());
            }
            let _ = self.conn.flush();
        }
        Ok(())
    }

    unsafe fn draw_bg_pass(&self, dim_factor: f32) {
        if let Some((kind, time_s, hue_rad)) = self.shader_bg {
            (self.gl.gl_use_program)(self.prog_shader);
            (self.gl.gl_uniform1i)(
                (self.gl.gl_get_uniform_location)(self.prog_shader, c"u_kind".as_ptr()),
                kind,
            );
            (self.gl.gl_uniform1f)(
                (self.gl.gl_get_uniform_location)(self.prog_shader, c"u_time".as_ptr()),
                time_s,
            );
            (self.gl.gl_uniform1f)(
                (self.gl.gl_get_uniform_location)(self.prog_shader, c"u_hue".as_ptr()),
                hue_rad,
            );
            (self.gl.gl_uniform1f)(
                (self.gl.gl_get_uniform_location)(self.prog_shader, c"u_dim".as_ptr()),
                dim_factor,
            );
            (self.gl.gl_uniform2f)(
                (self.gl.gl_get_uniform_location)(self.prog_shader, c"u_screen".as_ptr()),
                self.width as f32,
                self.height as f32,
            );
            self.draw_fullscreen_quad(self.prog_shader);
            return;
        }
        (self.gl.gl_use_program)(self.prog_bg);
        let u_y = CString::new("u_tex_y").unwrap();
        let u_uv = CString::new("u_tex_uv").unwrap();
        let u_rgba = CString::new("u_tex_rgba").unwrap();
        let u_nv12 = CString::new("u_is_nv12").unwrap();
        let u_dim = CString::new("u_dim").unwrap();

        (self.gl.gl_active_texture)(GL_TEXTURE0);
        (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_y[self.nv12_idx]);
        (self.gl.gl_uniform1i)(
            (self.gl.gl_get_uniform_location)(self.prog_bg, u_y.as_ptr()),
            0,
        );
        (self.gl.gl_active_texture)(GL_TEXTURE1);
        (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_uv[self.nv12_idx]);
        (self.gl.gl_uniform1i)(
            (self.gl.gl_get_uniform_location)(self.prog_bg, u_uv.as_ptr()),
            1,
        );
        (self.gl.gl_active_texture)(GL_TEXTURE0 + 2);
        (self.gl.gl_bind_texture)(GL_TEXTURE_2D, self.tex_bg_rgba);
        (self.gl.gl_uniform1i)(
            (self.gl.gl_get_uniform_location)(self.prog_bg, u_rgba.as_ptr()),
            2,
        );
        (self.gl.gl_uniform1i)(
            (self.gl.gl_get_uniform_location)(self.prog_bg, u_nv12.as_ptr()),
            i32::from(self.bg_is_nv12),
        );
        (self.gl.gl_uniform1f)(
            (self.gl.gl_get_uniform_location)(self.prog_bg, u_dim.as_ptr()),
            dim_factor,
        );
        self.draw_fullscreen_quad(self.prog_bg);
    }
}

impl Drop for WaylandGpuPresenter {
    fn drop(&mut self) {
        unsafe {
            let _ = (self.gl.egl_make_current)(
                self.egl_disp,
                self.egl_surf,
                self.egl_surf,
                self.egl_ctx,
            );
            (self.gl.gl_delete_program)(self.prog_bg);
            (self.gl.gl_delete_program)(self.prog_shader);
            (self.gl.gl_delete_program)(self.prog_blur);
            (self.gl.gl_delete_program)(self.prog_rgba);
            (self.gl.gl_delete_program)(self.prog_rect);
            (self.gl.gl_delete_program)(self.prog_keyfx);
            (self.gl.gl_delete_program)(self.prog_v2_refract);
            (self.gl.gl_delete_program)(self.prog_v2_glow);
            (self.gl.gl_delete_program)(self.prog_v2_overlay);
            let fbos = [self.fbo_a, self.fbo_b];
            (self.gl.gl_delete_framebuffers)(2, fbos.as_ptr());
            let texs = [
                self.tex_y[0],
                self.tex_y[1],
                self.tex_uv[0],
                self.tex_uv[1],
                self.tex_bg_rgba,
                self.tex_ui_rgba[0],
                self.tex_ui_rgba[1],
                self.tex_fbo_a,
                self.tex_fbo_b,
                self.tex_v2_wave,
                self.tex_v2_overlay,
            ];
            (self.gl.gl_delete_textures)(11, texs.as_ptr());
            let _ = (self.gl.egl_make_current)(
                self.egl_disp,
                EGL_NO_SURFACE,
                EGL_NO_SURFACE,
                EGL_NO_CONTEXT,
            );
            (self.gl.egl_destroy_surface)(self.egl_disp, self.egl_surf);
            (self.gl.egl_destroy_context)(self.egl_disp, self.egl_ctx);
            (self.gl.egl_terminate)(self.egl_disp);
            (self.gl.wl_egl_window_destroy)(self.wl_egl_win);
        }
    }
}
