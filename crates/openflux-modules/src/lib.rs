//! OpenFlux v2 Module System: manifests, 3-tier registry, disclosure inspector, Lua 5.4 runtime,
//! curated lists, presets, and unrestricted host process management.

pub mod disclosure;
pub mod host_process;
pub mod ir;
pub mod lua_runtime;
pub mod manifest;
pub mod registry;

pub use disclosure::{
    diff_disclosure, inspect_module_dir, DisclosureReport, InspectedFile, InspectedFileKind,
    UpdateDisclosureDiff,
};
pub use host_process::HostProcessManager;
pub use ir::{
    DrawCommand, FrameScene, KeyGlowCmd, KeyGlowShape, ParticleSpawnCmd, ShaderPassCmd, TextAlign,
    WaveFieldParams, WaveImpulseCmd,
};
pub use lua_runtime::{parse_hex_color, ModuleLogEntry, ModuleRuntime, RuntimeModuleStatus};
pub use manifest::{
    ChannelDeclarations, CompositorStage, Entrypoints, HostRequirement, ManifestError,
    ModuleCategory, ModuleManifest, SettingField, SettingKind,
};
pub use registry::{
    CuratedListDocument, CuratedListEntry, CuratedListSubscription, DiscoveredModule,
    InstallPreview, ModuleInstanceState, ModuleRegistry, ModuleTier, ModulesStateFile, SetupPreset,
};
