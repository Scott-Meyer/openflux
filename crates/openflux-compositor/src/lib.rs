//! OpenFlux v2 Unified Cross-Platform GPU Compositor.

pub mod canvas2d;
pub mod compositor;
pub mod particles;
pub mod shaders;

pub use canvas2d::{Canvas2D, DEFAULT_ANSI_LABELS};
pub use compositor::{Compositor, CompositorOptions, FrameTimings};
pub use particles::{KeySdfGrid, LiveParticle, ParticleSystem, SdfCell};
