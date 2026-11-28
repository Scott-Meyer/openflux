//! Keyglass v2 Module System: manifests, 3-tier registry, disclosure inspector, Lua 5.4 runtime,
//! curated lists, presets, and unrestricted host process management.

pub mod disclosure;
pub mod host_process;
pub mod installer;
pub mod ir;
pub mod lua_runtime;
pub mod manifest;
pub mod net_bridge;
pub mod registry;
pub mod validator;

pub use disclosure::{
    diff_disclosure, inspect_module_dir, DisclosureReport, InspectedFile, InspectedFileKind,
    UpdateDisclosureDiff,
};
pub use host_process::{HostProcessManager, HostProcessStatus};
pub use installer::{
    check_module_dependencies, fetch_curated_list_document, stage_module_source, DependencyIssue,
    ModuleOrigin, StagedModuleSource,
};
pub use ir::{
    DrawCommand, FrameScene, KeyGlowCmd, KeyGlowShape, ParticleSpawnCmd, ShaderPassCmd,
    StreamOverlayCmd, TextAlign, WaveFieldParams, WaveImpulseCmd,
};
pub use lua_runtime::{
    parse_hex_color, ModuleLogEntry, ModulePerfStats, ModuleRuntime, RuntimeModuleStatus,
};
pub use manifest::{
    ChannelDeclarations, CompositorStage, Entrypoints, HostRequirement, ManifestError,
    ModuleActionField, ModuleCategory, ModuleDependency, ModuleManifest, ModuleMaturity,
    ModulePermissions, ModuleUiSpec, SettingField, SettingKind, TopBarSpec,
    declared_actions_topic,
};
pub use registry::{
    CuratedListDocument, CuratedListEntry, CuratedListSubscription, DiscoveredModule,
    InstallPreview, MissingPresetModule, ModuleInstanceState, ModuleRegistry, ModuleTier,
    ModuleUpdateInfo, ModulesStateFile, PresetImportReport, PresetModuleRef, RemovedModuleRecord,
    SetupPreset,
};
pub use validator::{validate_module_behavior, ModuleValidationReport};
