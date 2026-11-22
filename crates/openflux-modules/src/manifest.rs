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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleActionField {
    pub id: String,
    pub label: String,
    pub topic: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub retain: bool,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    pub category: ModuleCategory,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub stages: Vec<CompositorStage>,
    #[serde(default)]
    pub host_requirement: HostRequirement,
    #[serde(default)]
    pub slot_capable: bool,
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
