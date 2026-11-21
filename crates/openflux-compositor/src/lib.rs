//! OpenFlux v2 Unified Cross-Platform GPU Compositor, Key-SDF Particle Engine, Wave2D & Canvas2D.

pub mod canvas2d;
#[cfg(feature = "wgpu-backend")]
pub mod compositor;
pub mod particles;
#[cfg(feature = "wgpu-backend")]
pub mod shaders;
pub mod wave2d;

pub use canvas2d::{Canvas2D, DEFAULT_ANSI_LABELS};
#[cfg(feature = "wgpu-backend")]
pub use compositor::{Compositor, CompositorOptions, FrameTimings};
pub use particles::{KeySdfGrid, LiveParticle, ParticleSystem, SdfCell};
pub use wave2d::{WaveField2D, WAVE2D_H, WAVE2D_W};
