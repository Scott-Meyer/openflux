//! Universal module source fetcher & provenance tracker.
//!
//! Resolves a module source from:
//! - a local folder path
//! - a subscribed Curated List entry ID (local file or HTTP/HTTPS JSON list)
//! - a Git repository URL (`https://...git`, `git@...`, `github:owner/repo[/subdir][@ref]`,
//!   or `https://github.com/owner/repo[#subdir][@ref]`)
//! - an HTTP(S) or local `.tar.gz` / `.tgz` / `.zip` archive
//!
//! Fetches into an isolated staging directory so `DisclosureReport` & `UpdateDisclosureDiff`
//! ALWAYS run BEFORE installing or updating a single byte in `installed_dir`.

use crate::manifest::ModuleManifest;
use crate::registry::{CuratedListDocument, CuratedListSubscription, DiscoveredModule, ModuleTier};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

pub const ORIGIN_FILENAME: &str = ".openflux-source.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleOrigin {
    pub source: String,
    pub kind: String,
    #[serde(default)]
    pub subdir: Option<String>,
    #[serde(default)]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub list_id: Option<String>,
    pub installed_version: String,
    pub installed_at_ms: u64,
}

impl ModuleOrigin {
    pub fn load(module_dir: &Path) -> Option<Self> {
        let p = module_dir.join(ORIGIN_FILENAME);
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    pub fn save(&self, module_dir: &Path) -> Result<(), String> {
        let p = module_dir.join(ORIGIN_FILENAME);
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(p, text).map_err(|e| e.to_string())
    }
}

/// Temporarily staged module folder (automatically removes temp directory on drop when fetched remotely).
pub struct StagedModuleSource {
    pub module_dir: PathBuf,
    pub origin_source: String,
    pub origin_kind: String,
    pub subdir: Option<String>,
    pub git_ref: Option<String>,
    pub list_id: Option<String>,
    cleanup_dir: Option<PathBuf>,
}

impl Drop for StagedModuleSource {
    fn drop(&mut self) {
        if let Some(dir) = &self.cleanup_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Fetches a curated list JSON document from either a local path or HTTP(S) URL.
pub fn fetch_curated_list_document(source: &str) -> Result<CuratedListDocument, String> {
    let text = if source.starts_with("http://") || source.starts_with("https://") {
        let out = Command::new("curl")
            .args(["-fsSL", "--max-time", "20", source])
            .output()
            .map_err(|e| format!("failed to run curl: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "curl failed for {source}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        String::from_utf8(out.stdout).map_err(|e| format!("invalid utf-8 in list: {e}"))?
    } else {
        std::fs::read_to_string(source).map_err(|e| format!("read curated list {source}: {e}"))?
    };
    serde_json::from_str(&text).map_err(|e| format!("parse curated list {source}: {e}"))
}

/// Resolves and stages ANY module source (local folder, local/remote archive, Git/GitHub URL,
/// or curated list entry ID) into a ready-to-inspect directory containing `module.toml`.
pub fn stage_module_source(
    source: &str,
    curated_lists: &[CuratedListSubscription],
    bundled_dir: &Path,
) -> Result<StagedModuleSource, String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("module source cannot be empty".into());
    }

    // 1. Direct local folder containing `module.toml`
    let as_path = PathBuf::from(trimmed);
    if as_path.is_dir() && as_path.join("module.toml").is_file() {
        let abs = std::fs::canonicalize(&as_path).unwrap_or(as_path);
        return Ok(StagedModuleSource {
            module_dir: abs.clone(),
            origin_source: abs.to_string_lossy().into_owned(),
            origin_kind: "local".into(),
            subdir: None,
            git_ref: None,
            list_id: None,
            cleanup_dir: None,
        });
    }

    // 2. Relative path against bundled_dir's parent (e.g. "modules/bg-aurora-shader")
    if let Some(repo_root) = bundled_dir.parent() {
        let rel = repo_root.join(trimmed);
        if rel.is_dir() && rel.join("module.toml").is_file() {
            let abs = std::fs::canonicalize(&rel).unwrap_or(rel);
            return Ok(StagedModuleSource {
                module_dir: abs.clone(),
                origin_source: trimmed.to_string(),
                origin_kind: "local".into(),
                subdir: None,
                git_ref: None,
                list_id: None,
                cleanup_dir: None,
            });
        }
    }

    // 3. Local archive file (`.tar.gz`, `.tgz`, `.zip`)
    if as_path.is_file() && is_archive_name(trimmed) {
        return stage_archive(&as_path, trimmed, "archive", None);
    }

    // 4. Remote HTTP(S) archive (`.tar.gz`, `.tgz`, `.zip`)
    if (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        && is_archive_name(trimmed.split('?').next().unwrap_or(trimmed))
    {
        return stage_remote_archive(trimmed, None);
    }

    // 5. GitHub shorthand `github:owner/repo[/subdir][@ref]` or Git URL (`https://...`, `git@...`)
    if trimmed.starts_with("github:")
        || trimmed.starts_with("git@")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("http://")
        || trimmed.ends_with(".git")
    {
        return stage_git_source(trimmed, None);
    }

    // 6. Curated List entry ID lookup (e.g. `fluxctl modules install widget-clock`)
    for list in curated_lists {
        if let Ok(doc) = fetch_curated_list_document(&list.source) {
            if let Some(entry) = doc.entries.into_iter().find(|e| e.id == trimmed) {
                let mut staged = stage_module_source(&entry.source, &[], bundled_dir)?;
                staged.list_id = Some(list.id.clone());
                staged.origin_source = entry.source;
                if staged.origin_kind == "local" {
                    staged.origin_kind = "curated_list".into();
                }
                return Ok(staged);
            }
        }
    }

    Err(format!(
        "could not resolve module source {trimmed:?} (expected local folder, archive, git/github URL, or curated list entry ID)"
    ))
}

fn is_archive_name(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.ends_with(".tar.gz") || lower.ends_with(".tgz") || lower.ends_with(".zip")
}

fn make_temp_stage_dir(prefix: &str) -> Result<PathBuf, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("openflux-stage-{prefix}-{}-{now}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn find_module_dir_in_tree(root: &Path, subdir: Option<&str>) -> Result<PathBuf, String> {
    if let Some(sub) = subdir {
        let candidate = root.join(sub);
        if candidate.join("module.toml").is_file() {
            return Ok(candidate);
        }
        return Err(format!("subdir {sub:?} does not contain module.toml"));
    }
    if root.join("module.toml").is_file() {
        return Ok(root.to_path_buf());
    }
    // Common single top-level directory inside tarballs/zips (e.g. `my-mod-1.0.0/module.toml`)
    for entry in WalkDir::new(root).max_depth(3).into_iter().flatten() {
        if entry.file_type().is_file() && entry.file_name() == "module.toml" {
            if let Some(parent) = entry.path().parent() {
                return Ok(parent.to_path_buf());
            }
        }
    }
    Err("staged source does not contain a module.toml manifest".into())
}

fn stage_remote_archive(url: &str, list_id: Option<String>) -> Result<StagedModuleSource, String> {
    let tmp = make_temp_stage_dir("dl")?;
    let ext = if url.to_ascii_lowercase().ends_with(".zip") {
        "module.zip"
    } else {
        "module.tar.gz"
    };
    let archive_path = tmp.join(ext);
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "45", "-o"])
        .arg(&archive_path)
        .arg(url)
        .output()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!(
            "download failed for {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut staged = stage_archive(&archive_path, url, "archive", list_id)?;
    let _ = std::fs::remove_dir_all(&tmp);
    staged.origin_source = url.to_string();
    Ok(staged)
}

fn stage_archive(
    archive_path: &Path,
    origin_source: &str,
    origin_kind: &str,
    list_id: Option<String>,
) -> Result<StagedModuleSource, String> {
    let tmp = make_temp_stage_dir("arc")?;
    let lower = archive_path.to_string_lossy().to_ascii_lowercase();
    let status = if lower.ends_with(".zip") {
        Command::new("unzip")
            .args(["-qq", "-o"])
            .arg(archive_path)
            .arg("-d")
            .arg(&tmp)
            .output()
            .map_err(|e| format!("failed to run unzip: {e}"))?
    } else {
        Command::new("tar")
            .args(["-xzf"])
            .arg(archive_path)
            .arg("-C")
            .arg(&tmp)
            .output()
            .map_err(|e| format!("failed to run tar: {e}"))?
    };
    if !status.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!(
            "archive extract failed: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    let module_dir = match find_module_dir_in_tree(&tmp, None) {
        Ok(d) => d,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
    };
    Ok(StagedModuleSource {
        module_dir,
        origin_source: origin_source.to_string(),
        origin_kind: origin_kind.to_string(),
        subdir: None,
        git_ref: None,
        list_id,
        cleanup_dir: Some(tmp),
    })
}

fn stage_git_source(spec: &str, list_id: Option<String>) -> Result<StagedModuleSource, String> {
    // Parse optional `@ref` and `#subdir` (or `github:owner/repo/subdir@ref`)
    let (without_ref, git_ref) = if let Some((left, r)) = spec.rsplit_once('@') {
        if !r.contains('/') && !r.contains(':') && !left.ends_with("git") {
            (left, Some(r.to_string()))
        } else {
            (spec, None)
        }
    } else {
        (spec, None)
    };

    let (repo_url, subdir) = if let Some(gh) = without_ref.strip_prefix("github:") {
        let parts: Vec<&str> = gh.split('/').collect();
        if parts.len() < 2 {
            return Err(format!("invalid github shorthand {spec:?}; expected github:owner/repo[/subdir][@ref]"));
        }
        let url = format!("https://github.com/{}/{}.git", parts[0], parts[1]);
        let sub = if parts.len() > 2 {
            Some(parts[2..].join("/"))
        } else {
            None
        };
        (url, sub)
    } else if let Some((url, sub)) = without_ref.split_once('#') {
        (url.to_string(), if sub.is_empty() { None } else { Some(sub.to_string()) })
    } else {
        (without_ref.to_string(), None)
    };

    let tmp = make_temp_stage_dir("git")?;
    let mut cmd = Command::new("git");
    cmd.args(["clone", "--depth", "1"]);
    if let Some(ref br) = git_ref {
        cmd.args(["--branch", br]);
    }
    cmd.arg(&repo_url).arg(&tmp);
    let out = cmd.output().map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!(
            "git clone failed for {repo_url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let module_dir = match find_module_dir_in_tree(&tmp, subdir.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
    };
    Ok(StagedModuleSource {
        module_dir,
        origin_source: spec.to_string(),
        origin_kind: "git".into(),
        subdir,
        git_ref,
        list_id,
        cleanup_dir: Some(tmp),
    })
}

/// Checks whether `have_ver` satisfies a semver `min_ver` constraint (`major.minor.patch`).
pub fn version_satisfies(have_ver: &str, min_ver: &str) -> bool {
    let parse = |s: &str| -> (u64, u64, u64) {
        let clean = s.trim().trim_start_matches(['v', '>', '=']).split('-').next().unwrap_or("");
        let mut it = clean.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
        (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
        )
    };
    parse(have_ver) >= parse(min_ver)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyIssue {
    pub module_id: String,
    pub required_id: Option<String>,
    pub required_role: Option<String>,
    pub min_version: Option<String>,
    pub optional: bool,
    pub reason: String,
}

/// Validates all declared `depends` across enabled (or specified) modules against discovered modules.
pub fn check_module_dependencies(
    target: &DiscoveredModule,
    discovered: &std::collections::BTreeMap<String, DiscoveredModule>,
    enabled_ids: &std::collections::BTreeSet<String>,
) -> Vec<DependencyIssue> {
    let mut issues = Vec::new();
    for dep in &target.manifest.depends {
        let opt = dep.is_optional();
        let min_v = dep.min_version();

        if let Some(role) = dep.target_role() {
            let providers: Vec<&DiscoveredModule> = discovered
                .values()
                .filter(|m| m.manifest.provides.iter().any(|r| r == role))
                .collect();
            let active_providers: Vec<&DiscoveredModule> = providers
                .iter()
                .copied()
                .filter(|m| enabled_ids.contains(&m.manifest.id))
                .collect();

            if active_providers.is_empty() {
                let reason = if providers.is_empty() {
                    format!("No installed module provides role `{role}`")
                } else {
                    format!(
                        "Role `{role}` is provided by `{}` but it is disabled",
                        providers[0].manifest.id
                    )
                };
                issues.push(DependencyIssue {
                    module_id: target.manifest.id.clone(),
                    required_id: None,
                    required_role: Some(role.to_string()),
                    min_version: min_v.map(str::to_string),
                    optional: opt,
                    reason,
                });
            } else if let Some(mv) = min_v {
                if !active_providers
                    .iter()
                    .any(|p| version_satisfies(&p.manifest.version, mv))
                {
                    issues.push(DependencyIssue {
                        module_id: target.manifest.id.clone(),
                        required_id: None,
                        required_role: Some(role.to_string()),
                        min_version: Some(mv.to_string()),
                        optional: opt,
                        reason: format!("Role `{role}` requires version >= {mv}"),
                    });
                }
            }
        } else if let Some(req_id) = dep.target_id() {
            match discovered.get(req_id) {
                None => issues.push(DependencyIssue {
                    module_id: target.manifest.id.clone(),
                    required_id: Some(req_id.to_string()),
                    required_role: None,
                    min_version: min_v.map(str::to_string),
                    optional: opt,
                    reason: format!("Required module `{req_id}` is not installed"),
                }),
                Some(dm) => {
                    if !enabled_ids.contains(req_id) {
                        issues.push(DependencyIssue {
                            module_id: target.manifest.id.clone(),
                            required_id: Some(req_id.to_string()),
                            required_role: None,
                            min_version: min_v.map(str::to_string),
                            optional: opt,
                            reason: format!("Required module `{req_id}` is disabled"),
                        });
                    } else if let Some(mv) = min_v {
                        if !version_satisfies(&dm.manifest.version, mv) {
                            issues.push(DependencyIssue {
                                module_id: target.manifest.id.clone(),
                                required_id: Some(req_id.to_string()),
                                required_role: None,
                                min_version: Some(mv.to_string()),
                                optional: opt,
                                reason: format!(
                                    "Module `{req_id}` is v{}, requires >= {mv}",
                                    dm.manifest.version
                                ),
                            });
                        }
                    }
                }
            }
        }
    }
    issues
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn origin_from_staged(staged: &StagedModuleSource, manifest: &ModuleManifest) -> ModuleOrigin {
    ModuleOrigin {
        source: staged.origin_source.clone(),
        kind: staged.origin_kind.clone(),
        subdir: staged.subdir.clone(),
        git_ref: staged.git_ref.clone(),
        list_id: staged.list_id.clone(),
        installed_version: manifest.version.clone(),
        installed_at_ms: now_ms(),
    }
}

pub fn can_uninstall(dm: &DiscoveredModule) -> bool {
    dm.tier != ModuleTier::Bundled
}
