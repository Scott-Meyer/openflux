//! OpenFlux v2 Keyboard Display Compositor (pure v2 — zero legacy v1 rendering/config).

pub mod device_paths;
#[cfg(target_os = "linux")]
mod dl;
pub mod ipc;
pub mod layout;
#[cfg(target_os = "linux")]
pub mod rkmpp;
pub mod v2_mux_client;
pub mod v2_runtime;
#[cfg(target_os = "linux")]
pub mod wayland_egl;

pub use layout::{compute_key_parallax, ParallaxConfig, DISPLAY_HEIGHT, DISPLAY_WIDTH};
