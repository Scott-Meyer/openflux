//! OpenFlux release manifests, update trust and compatibility planning.
//!
//! This crate is shared by the host daemon, which consumes releases, and the
//! release pipeline, which produces them. It performs no I/O of its own beyond
//! reading the data it is handed.
//!
//! - [`compat`]: the interface registry (`compat.toml`) and what a host built
//!   from it understands.
//! - [`manifest`]: `release.json` and `index.json`.
//! - [`trust`]: signatures, release and root keys, index acceptance and
//!   artifact verification.
//! - [`factory`]: the factory software reference and the press-refinement
//!   source carried in the index.
//! - [`engine`]: per-component status and validated update plans.
//! - [`keys`]: the public keys built into every host.
//! - [`signing`]: key generation and signing for the release pipeline.

pub mod compat;
pub mod engine;
pub mod factory;
pub mod keys;
pub mod manifest;
#[cfg(feature = "signing")]
pub mod signing;
pub mod trust;
pub mod version;

pub use compat::{Compat, HostProfile, InterfaceKind, InterfaceSet};
pub use engine::{
    Catalog, ComponentState, ComponentStatus, ConversionPlan, Engine, InstalledSoftware, Inventory,
    KeyboardMode, Plan, PlanRequest, Preferences, Refusal, Step,
};
pub use factory::{
    FactoryFile, FactoryReference, PressRefinementSource, SourceLocation, SourceRange,
};
pub use keys::trust_anchors;
pub use manifest::{Artifact, Channel, Index, Release};
pub use trust::{
    verify_artifact, verify_bundled_index, verify_index, KeyId, PublicKey, TrustAnchors,
    TrustState, TrustStore, VerifiedIndex,
};
pub use version::{Component, InterfaceVersion, Requirement};
