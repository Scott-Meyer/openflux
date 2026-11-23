//! Plain-folder `module.toml` manifest schema and validation.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("failed to read manifest {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse module.toml: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid manifest: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleCategory {
    Effect,
    Widget,
    Background,
    Companion,
    HostService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositorStage {
    Background,
    Distortion,
    UnderKey,
    Tile,
    WorldOverlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HostRequirement {
    /// Works 100% standalone on the keyboard with no computer connected.
    #[default]
    None,
    /// Runs standalone on the keyboard and enriches with host channels when connected.
    Optional,
    /// Requires a connected computer running `openfluxd` (e.g. live OS metrics or host service).
    Required,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entrypoints {
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub shader: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelDeclarations {
    #[serde(default)]
    pub subscribes: Vec<String>,
    #[serde(default)]
    pub publishes: Vec<String>,
    /// Optional semver of the message payload schemas this module publishes (e.g. `"1.0.0"`).
    #[serde(default)]
    pub schema_version: Option<String>,
}

/// Dependency declared by a module on another module ID or capability role (e.g. `keys_renderer`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModuleDependency {
    /// Short form: `"keys-renderer"` (required module ID) or `"role:keys_renderer"`.
    Simple(String),
    /// Structured form with optional role, minimum version, and optional flag.
    Detailed {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        min_version: Option<String>,
        #[serde(default)]
        optional: bool,
    },
}

impl ModuleDependency {
    pub fn target_id(&self) -> Option<&str> {
        match self {
            Self::Simple(s) if !s.starts_with("role:") => Some(s.as_str()),
            Self::Detailed { id: Some(i), .. } => Some(i.as_str()),
            _ => None,
        }
    }

    pub fn target_role(&self) -> Option<&str> {
        match self {
            Self::Simple(s) => s.strip_prefix("role:"),
            Self::Detailed { role: Some(r), .. } => Some(r.as_str()),
            _ => None,
        }
    }

    pub fn min_version(&self) -> Option<&str> {
        match self {
            Self::Detailed { min_version, .. } => min_version.as_deref(),
            Self::Simple(_) => None,
        }
    }

    pub fn is_optional(&self) -> bool {
        match self {
            Self::Detailed { optional, .. } => *optional,
            Self::Simple(_) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKind {
    Number,
    Boolean,
    String,
    Color,
    Select,
    File,
    Key,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleUiSpec {
    /// Whether this module offers its own dedicated top-level tab in OpenFlux Studio when enabled.
    #[serde(default)]
    pub top_tab: bool,
    /// Optional custom label for the top-level tab (defaults to `manifest.name`).
    #[serde(default)]
    pub top_tab_label: Option<String>,
    /// Sort order among module-contributed top tabs (lower appears first).
    #[serde(default = "default_tab_order")]
    pub top_tab_order: i32,
    /// Optional built-in Studio view key (e.g. `"shortcuts"` for the v1 shortcut-pack editor).
    #[serde(default)]
    pub builtin_view: Option<String>,
}

fn default_tab_order() -> i32 {
    50
}

fn default_action_mode() -> String {
    "tap".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleActionField {
    pub id: String,
    pub label: String,
    pub topic: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub retain: bool,
    /// Optional concise keycap legend (e.g. `"Prusa"` or `"Pomo"`).
    #[serde(default)]
    pub short_label: Option<String>,
    /// Optional built-in vector icon name (e.g. `"play"`) or relative image file inside the module folder (e.g. `"prusa-cam.png"`).
    #[serde(default)]
    pub icon: Option<String>,
    /// Optional accent hex color (e.g. `"#fa6831"`) rendered on bound keycaps.
    #[serde(default)]
    pub accent: Option<String>,
    /// Trigger mode: `"tap"` (fires on key press only) or `"hold"` (fires on both press and release).
    #[serde(default = "default_action_mode")]
    pub mode: String,
    /// Optional payload published on key release when `mode == "hold"` (defaults to `{"active": false}`).
    #[serde(default)]
    pub release_payload: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingField {
    pub key: String,
    pub label: String,
    pub kind: SettingKind,
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Secret fields (API keys, tokens) are automatically stripped when exporting a shared preset.
    #[serde(default)]
    pub secret: bool,
}

/// Explicit top-bar placement contract advertised by a slot-capable module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopBarSpec {
    /// Width spans (in slot units) this module explicitly supports and has layouts for (e.g. `[0.5, 1.0, 2.0]`).
    #[serde(default = "default_supported_spans")]
    pub supported_spans: Vec<f32>,
    /// Default span when newly added to the top bar.
    #[serde(default = "default_span_one")]
    pub default_span: f32,
    /// Whether this module is a singleton (at most 1 active instance on the top bar).
    #[serde(default = "default_true")]
    pub singleton: bool,
}

fn default_supported_spans() -> Vec<f32> {
    vec![1.0]
}

fn default_span_one() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub issues_url: Option<String>,
    pub category: ModuleCategory,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Capability roles this module fulfills (e.g. `"keys_renderer"`, `"top_bar_dock"`).
    /// Any fork or replacement declaring the same role seamlessly takes over without name checks.
    #[serde(default)]
    pub provides: Vec<String>,
    /// Explicit dependencies on other module IDs or capability roles.
    #[serde(default)]
    pub depends: Vec<ModuleDependency>,
    #[serde(default)]
    pub stages: Vec<CompositorStage>,
    #[serde(default)]
    pub host_requirement: HostRequirement,
    #[serde(default)]
    pub slot_capable: bool,
    /// Optional explicit top-bar span & singleton contract.
    #[serde(default)]
    pub top_bar: Option<TopBarSpec>,
    #[serde(default = "default_true")]
    pub stackable: bool,
    #[serde(default)]
    pub forked_from: Option<String>,
    #[serde(default)]
    pub ui: ModuleUiSpec,
    #[serde(default)]
    pub entrypoints: Entrypoints,
    #[serde(default)]
    pub channels: ChannelDeclarations,
    #[serde(default)]
    pub settings: Vec<SettingField>,
    #[serde(default)]
    pub actions: Vec<ModuleActionField>,
}

fn default_true() -> bool {
    true
}

impl ModuleManifest {
    pub fn load_from_dir(dir: &Path) -> Result<Self, ManifestError> {
        let path = dir.join("module.toml");
        let text = std::fs::read_to_string(&path).map_err(|source| ManifestError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let manifest: Self = toml::from_str(&text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(ManifestError::Invalid(format!(
                "module id {:?} must be non-empty lowercase alphanumeric, '-' or '_'",
                self.id
            )));
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError::Invalid("module name must not be empty".into()));
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError::Invalid("module version must not be empty".into()));
        }
        for pat in &self.channels.subscribes {
            openflux_bus::validate_pattern(pat)
                .map_err(|e| ManifestError::Invalid(format!("subscribe pattern {pat:?}: {e}")))?;
        }
        for topic in &self.channels.publishes {
            if topic.contains('#') || topic.contains('+') {
                openflux_bus::validate_pattern(topic).map_err(|e| {
                    ManifestError::Invalid(format!("publish pattern {topic:?}: {e}"))
                })?;
            } else {
                openflux_bus::validate_topic(topic).map_err(|e| {
                    ManifestError::Invalid(format!("publish topic {topic:?}: {e}"))
                })?;
            }
        }
        Ok(())
    }

    pub fn default_config(&self) -> BTreeMap<String, Value> {
        self.settings
            .iter()
            .map(|f| (f.key.clone(), f.default.clone()))
            .collect()
    }

    pub fn secret_keys(&self) -> Vec<&str> {
        self.settings
            .iter()
            .filter(|f| f.secret)
            .map(|f| f.key.as_str())
            .collect()
    }
}
