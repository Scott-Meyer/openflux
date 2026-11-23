//! Three-tier module registry (bundled defaults < installed < local overrides),
//! zero-upload local scaffolding, fork/duplicate protection, curated lists, and preset sharing.

use crate::disclosure::{diff_disclosure, inspect_module_dir, DisclosureReport, UpdateDisclosureDiff};
use crate::installer::{
    check_module_dependencies, origin_from_staged, stage_module_source, DependencyIssue,
    ModuleOrigin,
};
use crate::manifest::{ModuleCategory, ModuleManifest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleTier {
    Bundled,
    Installed,
    Local,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredModule {
    pub manifest: ModuleManifest,
    pub tier: ModuleTier,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModuleInstanceState {
    pub enabled: bool,
    /// Legacy integer top-bar slot assignment (`0..=3`). Kept for backward compatibility.
    #[serde(default)]
    pub slot: Option<u8>,
    /// Exact fractional half-slot start position (`0.0, 0.5, 1.0, ..., 3.5`). Overrides `slot` when set.
    #[serde(default)]
    pub slot_pos: Option<f32>,
    /// Optional slot width span in slot units (e.g. `0.5` = half-slot, `1.0` = 1 slot, `2.0`..`4.0` = multi-slot!).
    #[serde(default)]
    pub slot_span: Option<f32>,
    /// Optional user override for whether this module appears as a dedicated top tab in Studio.
    #[serde(default)]
    pub pin_tab: Option<bool>,
    #[serde(default)]
    pub config: BTreeMap<String, Value>,
}

impl ModuleInstanceState {
    /// Effective fractional start position in slot units (`0.0..=3.5`), if placed in the top bar.
    pub fn effective_slot_pos(&self) -> Option<f32> {
        self.slot_pos.or_else(|| self.slot.map(|s| s as f32))
    }
}

/// Persistent user configuration of enabled modules, slot placements, and settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModulesStateFile {
    #[serde(default)]
    pub modules: BTreeMap<String, ModuleInstanceState>,
    #[serde(default)]
    pub curated_lists: Vec<CuratedListSubscription>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuratedListSubscription {
    pub id: String,
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub official: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuratedListEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub category: ModuleCategory,
    pub description: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuratedListDocument {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub entries: Vec<CuratedListEntry>,
}

/// Source & version reference embedded in a shareable `SetupPreset` so another user's Studio or
/// CLI knows exactly where to fetch missing modules and which version was used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetModuleRef {
    pub version: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub tier: Option<ModuleTier>,
}

/// Shareable personal setup / preset with all secret fields stripped plus module source provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupPreset {
    pub format_version: u32,
    pub name: String,
    pub description: String,
    pub modules: BTreeMap<String, ModuleInstanceState>,
    #[serde(default)]
    pub module_sources: BTreeMap<String, PresetModuleRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingPresetModule {
    pub id: String,
    pub version: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetImportReport {
    pub state: ModulesStateFile,
    pub imported_count: usize,
    pub missing_modules: Vec<MissingPresetModule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPreview {
    pub disclosure: DisclosureReport,
    pub update_diff: Option<UpdateDisclosureDiff>,
}

pub struct ModuleRegistry {
    pub bundled_dir: PathBuf,
    pub installed_dir: PathBuf,
    pub local_dir: PathBuf,
    pub state_path: PathBuf,
}

/// Validates a module ID before using it as a filesystem path component.
/// Prevents path traversal (`..`, `/`, `\`, empty `""`) from ever escaping `local_dir`/`installed_dir`.
pub fn validate_module_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err(format!("invalid module id {id:?}: must be 1..=64 characters"));
    }
    let bytes = id.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err(format!(
            "invalid module id {id:?}: must start with a lowercase letter or digit"
        ));
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
    {
        return Err(format!(
            "invalid module id {id:?}: only lowercase letters, digits, '-' and '_' are allowed"
        ));
    }
    Ok(())
}

fn safe_child_dir(parent: &Path, id: &str) -> Result<PathBuf, String> {
    validate_module_id(id)?;
    let dest = parent.join(id);
    if dest.parent() != Some(parent) {
        return Err(format!("invalid module path for id {id:?}"));
    }
    Ok(dest)
}

impl ModuleRegistry {
    pub fn new(
        bundled_dir: PathBuf,
        installed_dir: PathBuf,
        local_dir: PathBuf,
        state_path: PathBuf,
    ) -> Self {
        Self {
            bundled_dir,
            installed_dir,
            local_dir,
            state_path,
        }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.installed_dir)?;
        std::fs::create_dir_all(&self.local_dir)?;
        if let Some(p) = self.state_path.parent() {
            std::fs::create_dir_all(p)?;
        }
        Ok(())
    }

    /// Discovers all modules across tiers. Higher tiers (`Local` > `Installed` > `Bundled`) win
    /// on ID collisions so upstream default updates can never overwrite a user's local edits.
    pub fn discover(&self) -> BTreeMap<String, DiscoveredModule> {
        let mut out = BTreeMap::new();
        for (dir, tier) in [
            (&self.bundled_dir, ModuleTier::Bundled),
            (&self.installed_dir, ModuleTier::Installed),
            (&self.local_dir, ModuleTier::Local),
        ] {
            let Ok(rd) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in rd.flatten() {
                let path = entry.path();
                if !path.is_dir() || !path.join("module.toml").is_file() {
                    continue;
                }
                if let Ok(manifest) = ModuleManifest::load_from_dir(&path) {
                    out.insert(
                        manifest.id.clone(),
                        DiscoveredModule {
                            manifest,
                            tier,
                            path,
                        },
                    );
                }
            }
        }
        out
    }

    pub fn load_state(&self) -> ModulesStateFile {
        std::fs::read_to_string(&self.state_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_state(&self, state: &ModulesStateFile) -> Result<(), String> {
        self.ensure_dirs().map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
        let tmp = self.state_path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.state_path).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Inspects a candidate module folder before installation, including an update diff if the
    /// module is already installed.
    pub fn preview_install(&self, source_dir: &Path) -> Result<InstallPreview, String> {
        let disclosure = inspect_module_dir(source_dir)?;
        let existing_path = self.installed_dir.join(&disclosure.module_id);
        let update_diff = if existing_path.join("module.toml").is_file() {
            let old = inspect_module_dir(&existing_path)?;
            Some(diff_disclosure(&old, &disclosure))
        } else {
            None
        };
        Ok(InstallPreview {
            disclosure,
            update_diff,
        })
    }

    /// Installs (or updates) a module from a local directory into `installed_dir`.
    /// Inspects ANY module source (local folder, Git/GitHub URL, HTTP/local archive, or curated
    /// list entry ID) BEFORE installation, including an update diff if already installed.
    pub fn preview_install_source(&self, source: &str) -> Result<InstallPreview, String> {
        let state = self.load_state();
        let staged = stage_module_source(source, &state.curated_lists, &self.bundled_dir)?;
        self.preview_install(&staged.module_dir)
    }

    /// Installs (or updates) a module directly from an already-staged, already-inspected
    /// [`StagedModuleSource`] without re-fetching from the network (prevents disclosure TOCTOU!).
    pub fn install_from_staged(
        &self,
        staged: &crate::installer::StagedModuleSource,
    ) -> Result<DiscoveredModule, String> {
        self.ensure_dirs().map_err(|e| e.to_string())?;
        let manifest =
            ModuleManifest::load_from_dir(&staged.module_dir).map_err(|e| e.to_string())?;
        let dest = safe_child_dir(&self.installed_dir, &manifest.id)?;
        copy_dir_atomic(&staged.module_dir, &dest)?;
        let origin = origin_from_staged(staged, &manifest);
        let _ = origin.save(&dest);
        Ok(DiscoveredModule {
            manifest,
            tier: ModuleTier::Installed,
            path: dest,
        })
    }

    /// Installs (or updates) a module from ANY source (local dir, Git/GitHub URL, archive, or
    /// curated list entry ID) into `installed_dir` and records `.openflux-source.json` provenance.
    pub fn install_from_source(&self, source: &str) -> Result<DiscoveredModule, String> {
        let state = self.load_state();
        let staged = stage_module_source(source, &state.curated_lists, &self.bundled_dir)?;
        self.install_from_staged(&staged)
    }

    /// Installs (or updates) a module from a local directory into `installed_dir`.
    pub fn install_from_dir(&self, source_dir: &Path) -> Result<DiscoveredModule, String> {
        self.ensure_dirs().map_err(|e| e.to_string())?;
        let manifest = ModuleManifest::load_from_dir(source_dir).map_err(|e| e.to_string())?;
        let dest = safe_child_dir(&self.installed_dir, &manifest.id)?;
        copy_dir_atomic(source_dir, &dest)?;
        let origin = ModuleOrigin {
            source: source_dir.to_string_lossy().into_owned(),
            kind: "local".into(),
            subdir: None,
            git_ref: None,
            list_id: None,
            installed_version: manifest.version.clone(),
            installed_at_ms: crate::installer::now_ms(),
        };
        let _ = origin.save(&dest);
        Ok(DiscoveredModule {
            manifest,
            tier: ModuleTier::Installed,
            path: dest,
        })
    }

    /// Uninstalls a user-installed or local module (bundled defaults are read-only and protected).
    pub fn uninstall_module(&self, id: &str) -> Result<ModuleTier, String> {
        let local_path = safe_child_dir(&self.local_dir, id)?;
        let installed_path = safe_child_dir(&self.installed_dir, id)?;
        let bundled_path = safe_child_dir(&self.bundled_dir, id)?;
        let removed_tier = if local_path.is_dir() {
            std::fs::remove_dir_all(&local_path).map_err(|e| e.to_string())?;
            ModuleTier::Local
        } else if installed_path.is_dir() {
            std::fs::remove_dir_all(&installed_path).map_err(|e| e.to_string())?;
            ModuleTier::Installed
        } else if bundled_path.is_dir() {
            return Err(format!(
                "module {id:?} is a bundled default and cannot be deleted (disable it instead)"
            ));
        } else {
            return Err(format!("module {id:?} is not installed"));
        };

        // Remove persisted state entry only if no lower-tier fallback remains.
        if !self.bundled_dir.join(id).is_dir() && !self.installed_dir.join(id).is_dir() {
            let mut state = self.load_state();
            state.modules.remove(id);
            let _ = self.save_state(&state);
        }
        Ok(removed_tier)
    }

    /// Previews an update for an installed module from its recorded `.openflux-source.json`.
    pub fn preview_update(&self, id: &str) -> Result<(ModuleOrigin, InstallPreview), String> {
        let installed_path = self.installed_dir.join(id);
        let origin = ModuleOrigin::load(&installed_path)
            .ok_or_else(|| format!("module {id:?} has no recorded upstream source"))?;
        let preview = self.preview_install_source(&origin.source)?;
        Ok((origin, preview))
    }

    /// Updates an installed module in place from its recorded `.openflux-source.json` source.
    /// Any local fork in `local_dir` remains untouched and continues to take priority.
    pub fn update_module_from_source(&self, id: &str) -> Result<DiscoveredModule, String> {
        let installed_path = self.installed_dir.join(id);
        let origin = ModuleOrigin::load(&installed_path)
            .ok_or_else(|| format!("module {id:?} has no recorded upstream source"))?;
        self.install_from_source(&origin.source)
    }

    /// Reads recorded upstream origin metadata for a module, if installed from a source.
    pub fn module_origin(&self, dm: &DiscoveredModule) -> Option<ModuleOrigin> {
        ModuleOrigin::load(&dm.path)
    }

    /// Validates dependencies for all currently enabled modules.
    pub fn check_all_dependencies(
        &self,
        discovered: &BTreeMap<String, DiscoveredModule>,
        state: &ModulesStateFile,
    ) -> BTreeMap<String, Vec<DependencyIssue>> {
        let enabled_ids: BTreeSet<String> = state
            .modules
            .iter()
            .filter(|(_, inst)| inst.enabled)
            .map(|(id, _)| id.clone())
            .collect();
        let mut out = BTreeMap::new();
        for (id, dm) in discovered {
            if !enabled_ids.contains(id) {
                continue;
            }
            let issues = check_module_dependencies(dm, discovered, &enabled_ids);
            if !issues.is_empty() {
                out.insert(id.clone(), issues);
            }
        }
        out
    }

    /// Duplicates any bundled, installed, or local module into `local_dir/<new_id>` so the user
    /// or AI agent can customize it freely without upstream updates ever touching it.
    pub fn fork_module(
        &self,
        source_id: &str,
        new_id: &str,
        new_name: Option<&str>,
    ) -> Result<DiscoveredModule, String> {
        validate_module_id(source_id)?;
        let dest = safe_child_dir(&self.local_dir, new_id)?;
        self.ensure_dirs().map_err(|e| e.to_string())?;
        let discovered = self.discover();
        let src = discovered
            .get(source_id)
            .ok_or_else(|| format!("module {source_id:?} not found"))?;
        if dest.exists() {
            return Err(format!(
                "local module {new_id:?} already exists at {}",
                dest.display()
            ));
        }
        copy_dir_atomic(&src.path, &dest)?;

        let mut manifest = src.manifest.clone();
        manifest.id = new_id.to_string();
        manifest.name = new_name
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} (Custom)", src.manifest.name));
        manifest.forked_from = Some(source_id.to_string());
        manifest.validate().map_err(|e| e.to_string())?;

        let toml_str = toml::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        std::fs::write(dest.join("module.toml"), toml_str).map_err(|e| e.to_string())?;

        Ok(DiscoveredModule {
            manifest,
            tier: ModuleTier::Local,
            path: dest,
        })
    }

    /// Scaffolds a brand-new working plain-folder module directly inside `local_dir`.
    pub fn scaffold_local(
        &self,
        id: &str,
        category: ModuleCategory,
        name: Option<&str>,
    ) -> Result<DiscoveredModule, String> {
        let dest = safe_child_dir(&self.local_dir, id)?;
        self.ensure_dirs().map_err(|e| e.to_string())?;
        if dest.exists() {
            return Err(format!("module {id:?} already exists at {}", dest.display()));
        }
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        let display_name = name.unwrap_or(id);

        let (manifest_toml, script_lua, extra_files) = scaffold_templates(id, display_name, category);
        std::fs::write(dest.join("module.toml"), manifest_toml).map_err(|e| e.to_string())?;
        if let Some(lua) = script_lua {
            std::fs::write(dest.join("script.lua"), lua).map_err(|e| e.to_string())?;
        }
        for (rel, content) in extra_files {
            let p = dest.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&p, content).map_err(|e| e.to_string())?;
        }

        let manifest = ModuleManifest::load_from_dir(&dest).map_err(|e| e.to_string())?;
        Ok(DiscoveredModule {
            manifest,
            tier: ModuleTier::Local,
            path: dest,
        })
    }

    /// Exports the current setup as a shareable preset, automatically stripping any config keys
    /// marked `secret = true` in each module's manifest and embedding source+version provenance.
    pub fn export_preset(&self, name: &str, description: &str) -> SetupPreset {
        let state = self.load_state();
        let discovered = self.discover();
        let mut sanitized = BTreeMap::new();
        let mut module_sources = BTreeMap::new();

        for (id, mut inst) in state.modules {
            if let Some(dm) = discovered.get(&id) {
                for secret_key in dm.manifest.secret_keys() {
                    inst.config.remove(secret_key);
                }
                let origin_src = ModuleOrigin::load(&dm.path).map(|o| o.source);
                module_sources.insert(
                    id.clone(),
                    PresetModuleRef {
                        version: dm.manifest.version.clone(),
                        source: origin_src,
                        tier: Some(dm.tier),
                    },
                );
            }
            sanitized.insert(id, inst);
        }

        SetupPreset {
            format_version: 3,
            name: name.to_string(),
            description: description.to_string(),
            modules: sanitized,
            module_sources,
        }
    }

    /// Imports a preset into the user's module state (preserving any existing local secret values)
    /// and returns a detailed report listing any missing modules with their declared sources/versions.
    pub fn import_preset_detailed(&self, preset: &SetupPreset) -> Result<PresetImportReport, String> {
        let mut current = self.load_state();
        let discovered = self.discover();
        let mut missing_modules = Vec::new();
        let mut imported_count = 0usize;

        for (id, incoming) in &preset.modules {
            if !discovered.contains_key(id) {
                let src_ref = preset.module_sources.get(id);
                missing_modules.push(MissingPresetModule {
                    id: id.clone(),
                    version: src_ref.map(|r| r.version.clone()),
                    source: src_ref.and_then(|r| r.source.clone()),
                });
            }
            let mut merged = incoming.clone();
            if let (Some(prev), Some(dm)) = (current.modules.get(id), discovered.get(id)) {
                for secret_key in dm.manifest.secret_keys() {
                    if !merged.config.contains_key(secret_key) {
                        if let Some(old_val) = prev.config.get(secret_key) {
                            merged.config.insert(secret_key.to_string(), old_val.clone());
                        }
                    }
                }
            }
            current.modules.insert(id.clone(), merged);
            imported_count += 1;
        }
        self.save_state(&current)?;
        Ok(PresetImportReport {
            state: current,
            imported_count,
            missing_modules,
        })
    }

    /// Imports a preset into the user's module state (preserving any existing local secret values).
    pub fn import_preset(&self, preset: &SetupPreset) -> Result<ModulesStateFile, String> {
        Ok(self.import_preset_detailed(preset)?.state)
    }
}

fn copy_dir_atomic(src: &Path, dest: &Path) -> Result<(), String> {
    let parent = dest
        .parent()
        .ok_or_else(|| format!("invalid destination {}", dest.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let staging = parent.join(format!(
        ".staging-{}-{}",
        dest.file_name().and_then(|n| n.to_str()).unwrap_or("mod"),
        std::process::id()
    ));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    for entry in WalkDir::new(src).follow_links(false) {
        let entry = entry.map_err(|e| e.to_string())?;
        let rel = entry.path().strip_prefix(src).map_err(|e| e.to_string())?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = staging.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        } else if entry.file_type().is_file() {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }

    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&staging, dest).map_err(|e| e.to_string())?;
    Ok(())
}

fn scaffold_templates(
    id: &str,
    name: &str,
    category: ModuleCategory,
) -> (String, Option<String>, Vec<(&'static str, String)>) {
    match category {
        ModuleCategory::Effect => (
            format!(
                r##"id = "{id}"
name = "{name}"
version = "0.1.0"
description = "Custom stackable key press effect"
author = "Local"
category = "effect"
stages = ["under_key"]
host_requirement = "none"
slot_capable = false
stackable = true

[entrypoints]
script = "script.lua"

[channels]
subscribes = ["input/keys/event", "fx/surface/impulse"]
publishes = []

[[settings]]
key = "color"
label = "Effect Color"
kind = "color"
default = "#4de1ff"
"##
            ),
            Some(
                r##"local pulses = {}

function on_event(topic, payload)
  if topic == "input/keys/event" and payload.kind == "press" then
    table.insert(pulses, { x = payload.x_px, y = payload.y_px, w = payload.w_px, h = payload.h_px, age = 0 })
  elseif topic == "fx/surface/impulse" then
    table.insert(pulses, { x = payload.x, y = payload.y, w = 80, h = 80, age = 0 })
  end
end

function on_update(dt)
  local kept = {}
  for _, p in ipairs(pulses) do
    p.age = p.age + dt
    if p.age < 0.65 then
      table.insert(kept, p)
    end
  end
  pulses = kept
end

function on_render(ctx, stage)
  if stage ~= "under_key" then return end
  local c = module.color("color", "#4de1ff")
  for _, p in ipairs(pulses) do
    local t = p.age / 0.65
    local alpha = (1.0 - t) * 0.9
    ctx:key_glow("ring", p.x, p.y, p.w, p.h, 16 + t * 90, 5.0, alpha, { c[1], c[2], c[3], alpha })
  end
end
"##
                .to_string(),
            ),
            vec![],
        ),
        ModuleCategory::Widget => (
            format!(
                r#"id = "{id}"
name = "{name}"
version = "0.1.0"
description = "Custom top-bar widget module"
author = "Local"
category = "widget"
stages = ["tile"]
host_requirement = "optional"
slot_capable = true
stackable = true

[entrypoints]
script = "script.lua"

[channels]
subscribes = ["modules/{id}/#"]
publishes = []

[[settings]]
key = "title"
label = "Title"
kind = "string"
default = "{name}"
"#
            ),
            Some(
                r#"local latest_text = "Ready"

function on_event(topic, payload)
  if type(payload) == "table" and payload.text then
    latest_text = tostring(payload.text)
  end
end

function on_render(ctx, stage)
  if stage ~= "tile" then return end
  local slot = module.slot_rect()
  if not slot then return end
  ctx:clip(slot.x, slot.y, slot.w, slot.h)
  ctx:round_rect(slot.x + 2, slot.y + 2, slot.w - 4, slot.h - 4, 16, { 0.06, 0.08, 0.13, 0.72 }, 1.5, { 0.35, 0.75, 1.0, 0.45 })
  ctx:text(slot.x + 20, slot.y + 24, tostring(module.config().title or "Widget"), 24, { 0.65, 0.85, 1.0, 0.95 }, "left")
  ctx:text(slot.x + 20, slot.y + 72, latest_text, 34, { 1.0, 1.0, 1.0, 1.0 }, "left")
  ctx:clear_clip()
end
"#
                .to_string(),
            ),
            vec![],
        ),
        ModuleCategory::Companion => (
            format!(
                r#"id = "{id}"
name = "{name}"
version = "0.1.0"
description = "Custom roaming companion module"
author = "Local"
category = "companion"
stages = ["tile", "world_overlay"]
host_requirement = "none"
slot_capable = true
stackable = true

[entrypoints]
script = "script.lua"

[channels]
subscribes = ["input/keys/event", "fx/surface/impulse"]
publishes = ["fx/surface/impulse"]
"#
            ),
            Some(
                r#"local t = 0

function on_update(dt)
  t = t + dt
end

function on_render(ctx, stage)
  if stage == "tile" then
    local slot = module.slot_rect()
    if slot then
      ctx:circle(slot.x + slot.w * 0.5, slot.y + slot.h * 0.5 + math.sin(t * 3) * 8, 28, { 1.0, 0.75, 0.35, 0.95 })
    end
  end
end
"#
                .to_string(),
            ),
            vec![],
        ),
        ModuleCategory::Background => (
            format!(
                r#"id = "{id}"
name = "{name}"
version = "0.1.0"
description = "Custom procedural shader background"
author = "Local"
category = "background"
stages = ["background"]
host_requirement = "none"
slot_capable = false
stackable = false

[entrypoints]
script = "script.lua"
shader = "shaders/bg.wgsl"

[channels]
subscribes = []
publishes = []
"#
            ),
            Some(
                r#"function on_render(ctx, stage)
  if stage ~= "background" then return end
  ctx:shader("shaders/bg.wgsl", { 1.0, 0.5, 0.2, 1.0 })
end
"#
                .to_string(),
            ),
            vec![(
                "shaders/bg.wgsl",
                r#"struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    _pad: f32,
    params: array<vec4<f32>, 4>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let wave = 0.5 + 0.5 * sin(uv.x * 6.0 + u.time * 0.8 + cos(uv.y * 4.0 - u.time * 0.5));
    let col = mix(vec3<f32>(0.04, 0.08, 0.18), vec3<f32>(0.12, 0.45, 0.62), wave);
    return vec4<f32>(col, 1.0);
}
"#
                .to_string(),
            )],
        ),
        ModuleCategory::HostService => (
            format!(
                r#"id = "{id}"
name = "{name}"
version = "0.1.0"
description = "Custom unrestricted host-side service publishing to the OpenFlux bus"
author = "Local"
category = "host_service"
stages = []
host_requirement = "required"
slot_capable = false
stackable = true

[entrypoints]
host = "host/service.py"

[channels]
subscribes = []
publishes = ["modules/{id}/status"]
"#
            ),
            None,
            vec![(
                "host/service.py",
                r#"#!/usr/bin/env python3
import os, time, json, urllib.request

bus_url = os.environ.get("OPENFLUX_BUS_URL", "http://127.0.0.1:52323")
mod_id = os.environ.get("OPENFLUX_MODULE_ID", "custom")

while True:
    print(f"[{mod_id}] heartbeat -> {bus_url}", flush=True)
    time.sleep(5)
"#
                .to_string(),
            )],
        ),
    }
}
