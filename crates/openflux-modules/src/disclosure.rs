//! Install and update disclosure inspection (Trust & Transparency Over Capability Prisons).
//!
//! Instead of locking modules inside a restrictive capability sandbox, OpenFlux inspects every
//! file in a module folder/bundle—combining direct static file inspection (executables, host
//! scripts, native binaries, Lua scripts, shaders) with manifest declarations—and produces a
//! clear disclosure report and update diff for both OpenFlux Studio popups and `fluxctl`.

use crate::manifest::{HostRequirement, ModuleManifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectedFileKind {
    Manifest,
    LuaScript,
    Shader,
    HostScript,
    NativeBinary,
    Asset,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectedFile {
    pub rel_path: String,
    pub kind: InspectedFileKind,
    pub size_bytes: u64,
    pub executable: bool,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisclosureReport {
    pub module_id: String,
    pub name: String,
    pub version: String,
    pub host_requirement: HostRequirement,
    pub runs_host_process: bool,
    pub host_entrypoint: Option<String>,
    pub has_native_binaries: bool,
    pub has_lua_script: bool,
    pub has_shaders: bool,
    pub subscribes: Vec<String>,
    pub publishes: Vec<String>,
    pub total_bytes: u64,
    pub files: Vec<InspectedFile>,
    pub warnings: Vec<String>,
    pub summary_lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateDisclosureDiff {
    pub module_id: String,
    pub old_version: String,
    pub new_version: String,
    pub added_files: Vec<InspectedFile>,
    pub removed_files: Vec<InspectedFile>,
    pub modified_files: Vec<InspectedFile>,
    pub executable_or_host_changed: bool,
    pub channels_added: Vec<String>,
    pub channels_removed: Vec<String>,
    pub summary_lines: Vec<String>,
}

fn is_native_binary_magic(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    // ELF, Mach-O (32/64/fat), PE/MZ
    bytes.starts_with(b"\x7fELF")
        || bytes.starts_with(&[0xCF, 0xFA, 0xED, 0xFE])
        || bytes.starts_with(&[0xCE, 0xFA, 0xED, 0xFE])
        || bytes.starts_with(&[0xFE, 0xED, 0xFA, 0xCF])
        || bytes.starts_with(&[0xCA, 0xFE, 0xBA, 0xBE])
        || bytes.starts_with(b"MZ")
}

fn is_executable_mode(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

fn classify_file(
    rel_path: &str,
    bytes: &[u8],
    executable: bool,
    host_entrypoint: Option<&str>,
) -> InspectedFileKind {
    if rel_path == "module.toml" {
        return InspectedFileKind::Manifest;
    }
    if is_native_binary_magic(bytes) {
        return InspectedFileKind::NativeBinary;
    }
    if host_entrypoint.is_some_and(|h| h == rel_path)
        || rel_path.starts_with("host/")
        || bytes.starts_with(b"#!")
    {
        return InspectedFileKind::HostScript;
    }
    let lower = rel_path.to_ascii_lowercase();
    if lower.ends_with(".lua") {
        return InspectedFileKind::LuaScript;
    }
    if lower.ends_with(".wgsl")
        || lower.ends_with(".glsl")
        || lower.ends_with(".frag")
        || lower.ends_with(".vert")
    {
        return InspectedFileKind::Shader;
    }
    if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".svg")
        || lower.ends_with(".webp")
        || lower.ends_with(".ttf")
        || lower.ends_with(".otf")
        || lower.ends_with(".mp4")
    {
        return InspectedFileKind::Asset;
    }
    if executable
        || lower.ends_with(".sh")
        || lower.ends_with(".py")
        || lower.ends_with(".js")
        || lower.ends_with(".mjs")
        || lower.ends_with(".ps1")
        || lower.ends_with(".bat")
    {
        return InspectedFileKind::HostScript;
    }
    InspectedFileKind::Other
}

/// Inspects a module directory directly from disk to produce a transparent disclosure report.
pub fn inspect_module_dir(dir: &Path) -> Result<DisclosureReport, String> {
    let manifest = ModuleManifest::load_from_dir(dir).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    let mut total_bytes = 0u64;

    for entry in WalkDir::new(dir).follow_links(false).sort_by_file_name() {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(dir)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with(".git/") || rel == ".DS_Store" {
            continue;
        }
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        let bytes = std::fs::read(entry.path()).map_err(|e| e.to_string())?;
        let executable = is_executable_mode(&meta);
        let kind = classify_file(
            &rel,
            &bytes,
            executable,
            manifest.entrypoints.host.as_deref(),
        );
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let sha256 = format!("{:x}", hasher.finalize());
        total_bytes += meta.len();
        files.push(InspectedFile {
            rel_path: rel,
            kind,
            size_bytes: meta.len(),
            executable,
            sha256,
        });
    }

    let has_native_binaries = files
        .iter()
        .any(|f| f.kind == InspectedFileKind::NativeBinary);
    let has_host_files = files
        .iter()
        .any(|f| f.kind == InspectedFileKind::HostScript || f.kind == InspectedFileKind::NativeBinary);
    let runs_host_process = manifest.entrypoints.host.is_some() || has_host_files;
    let has_lua_script = files.iter().any(|f| f.kind == InspectedFileKind::LuaScript);
    let has_shaders = files.iter().any(|f| f.kind == InspectedFileKind::Shader);

    let mut warnings = Vec::new();
    if manifest.entrypoints.host.is_some() {
        warnings.push(format!(
            "Runs unrestricted host process on your computer: {}",
            manifest.entrypoints.host.as_deref().unwrap_or("")
        ));
    }
    if has_native_binaries {
        warnings.push("Contains native executable binary files.".to_string());
    }
    let watches_raw_keys = manifest.channels.subscribes.iter().any(|s| {
        s.starts_with("input/keys")
            || openflux_bus::topic_matches(s, "input/keys/event")
            || openflux_bus::topic_matches(s, "input/keys/analog")
    });
    if watches_raw_keys {
        warnings.push(
            "Subscribes to keyboard input events (key positions/timing can reflect typing)."
                .to_string(),
        );
    }

    let mut summary_lines = Vec::new();
    summary_lines.push(format!(
        "{} ({}) v{} — {} files ({} bytes)",
        manifest.name,
        manifest.id,
        manifest.version,
        files.len(),
        total_bytes
    ));
    summary_lines.push(match manifest.host_requirement {
        HostRequirement::None => "Runtime: 100% standalone on keyboard (no host needed).".into(),
        HostRequirement::Optional => {
            "Runtime: hybrid (runs standalone on keyboard, enriches with host when connected)."
                .into()
        }
        HostRequirement::Required => {
            "Runtime: host-driven (requires connected computer running openfluxd).".into()
        }
    });
    if runs_host_process {
        summary_lines.push("Host execution: YES (unrestricted script/binary present).".into());
    } else {
        summary_lines.push("Host execution: none (display script/shaders/assets only).".into());
    }
    if !manifest.channels.subscribes.is_empty() {
        summary_lines.push(format!(
            "Subscribes to: {}",
            manifest.channels.subscribes.join(", ")
        ));
    }
    if !manifest.channels.publishes.is_empty() {
        summary_lines.push(format!(
            "Publishes to: {}",
            manifest.channels.publishes.join(", ")
        ));
    }

    Ok(DisclosureReport {
        module_id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        host_requirement: manifest.host_requirement,
        runs_host_process,
        host_entrypoint: manifest.entrypoints.host,
        has_native_binaries,
        has_lua_script,
        has_shaders,
        subscribes: manifest.channels.subscribes,
        publishes: manifest.channels.publishes,
        total_bytes,
        files,
        warnings,
        summary_lines,
    })
}

/// Computes a transparent diff between an installed module and a candidate update.
pub fn diff_disclosure(old: &DisclosureReport, new: &DisclosureReport) -> UpdateDisclosureDiff {
    let old_map: BTreeMap<&str, &InspectedFile> =
        old.files.iter().map(|f| (f.rel_path.as_str(), f)).collect();
    let new_map: BTreeMap<&str, &InspectedFile> =
        new.files.iter().map(|f| (f.rel_path.as_str(), f)).collect();

    let mut added_files = Vec::new();
    let mut removed_files = Vec::new();
    let mut modified_files = Vec::new();

    for (path, nf) in &new_map {
        match old_map.get(path) {
            None => added_files.push((*nf).clone()),
            Some(of) if of.sha256 != nf.sha256 || of.executable != nf.executable => {
                modified_files.push((*nf).clone())
            }
            _ => {}
        }
    }
    for (path, of) in &old_map {
        if !new_map.contains_key(path) {
            removed_files.push((*of).clone());
        }
    }

    let is_exec_or_host = |f: &InspectedFile| {
        f.executable
            || matches!(
                f.kind,
                InspectedFileKind::HostScript | InspectedFileKind::NativeBinary
            )
    };
    let executable_or_host_changed = added_files.iter().any(is_exec_or_host)
        || modified_files.iter().any(is_exec_or_host)
        || removed_files.iter().any(is_exec_or_host)
        || old.host_entrypoint != new.host_entrypoint;

    let channels_added: Vec<String> = new
        .subscribes
        .iter()
        .chain(new.publishes.iter())
        .filter(|c| !old.subscribes.contains(c) && !old.publishes.contains(c))
        .cloned()
        .collect();
    let channels_removed: Vec<String> = old
        .subscribes
        .iter()
        .chain(old.publishes.iter())
        .filter(|c| !new.subscribes.contains(c) && !new.publishes.contains(c))
        .cloned()
        .collect();

    let mut summary_lines = vec![format!(
        "Update {} from v{} -> v{}",
        new.module_id, old.version, new.version
    )];
    if executable_or_host_changed {
        summary_lines.push("ATTENTION: Executable or host-side scripts/binaries changed.".into());
    }
    if !added_files.is_empty() {
        summary_lines.push(format!(
            "Added ({}): {}",
            added_files.len(),
            added_files
                .iter()
                .map(|f| f.rel_path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !modified_files.is_empty() {
        summary_lines.push(format!(
            "Modified ({}): {}",
            modified_files.len(),
            modified_files
                .iter()
                .map(|f| f.rel_path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !removed_files.is_empty() {
        summary_lines.push(format!(
            "Removed ({}): {}",
            removed_files.len(),
            removed_files
                .iter()
                .map(|f| f.rel_path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    UpdateDisclosureDiff {
        module_id: new.module_id.clone(),
        old_version: old.version.clone(),
        new_version: new.version.clone(),
        added_files,
        removed_files,
        modified_files,
        executable_or_host_changed,
        channels_added,
        channels_removed,
        summary_lines,
    }
}
