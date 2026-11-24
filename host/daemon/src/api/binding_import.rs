//! Studio and agent access to a module's disclosed, on-demand binding import hook.
//!
//! Modules opt in with `ui.builtin_view = "binding-import"` and a host requirement.
//! `host/binding_import.py` receives UTF-8 on stdin and atomically updates
//! `data/layout.json`. No request can choose a command, argument, or host path.

use crate::api::state::ApiState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use openflux_modules::{DiscoveredModule, HostRequirement};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path as FsPath, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_BYTES: usize = 512 * 1024;
const HOOK: &str = "host/binding_import.py";
static IMPORT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

type Reply = (StatusCode, Json<Value>);

fn error(status: StatusCode, error: impl Into<String>) -> Reply {
    (status, Json(json!({"error": error.into()})))
}

#[derive(Serialize, Deserialize)]
struct BindingFace {
    key: String,
    mods: u8,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct BindingLayout {
    schema: u32,
    faces: Vec<BindingFace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flavor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped: Option<usize>,
}

fn module_for(state: &ApiState, id: &str) -> Result<DiscoveredModule, Reply> {
    let mut modules = state.v2.registry.discover();
    let dm = modules
        .remove(id)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "Module not installed"))?;
    let view = dm.manifest.ui.builtin_view.as_deref();
    if view != Some("binding-import") || dm.manifest.host_requirement == HostRequirement::None {
        return Err(error(
            StatusCode::NOT_FOUND,
            "Module does not offer binding import",
        ));
    }
    Ok(dm)
}

fn owned_path(root: &FsPath, relative: &str) -> std::io::Result<PathBuf> {
    let resolved = root.join(relative).canonicalize()?;
    if !resolved.starts_with(root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Path escapes module folder",
        ));
    }
    Ok(resolved)
}

fn read_layout(root: &FsPath) -> Result<Option<BindingLayout>, String> {
    use std::io::Read;
    let path = match owned_path(root, "data/layout.json") {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Cannot open module layout: {e}")),
    };
    let mut body = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_BYTES as u64 + 1).read_to_end(&mut body))
        .map_err(|e| format!("Cannot read module layout: {e}"))?;
    if body.len() > MAX_BYTES {
        return Err("Module layout exceeds 512 KiB".into());
    }
    let layout: BindingLayout =
        serde_json::from_slice(&body).map_err(|e| format!("Invalid module layout: {e}"))?;
    if layout.schema != 1 || layout.faces.iter().any(|face| face.mods > 15) {
        return Err("Unsupported binding layout".into());
    }
    Ok(Some(layout))
}

fn root_for(dm: &DiscoveredModule) -> Result<PathBuf, Reply> {
    dm.path.canonicalize().map_err(|e| {
        error(
            StatusCode::BAD_REQUEST,
            format!("Cannot open module folder: {e}"),
        )
    })
}

pub async fn get(State(state): State<Arc<ApiState>>, Path(id): Path<String>) -> Reply {
    let root = match module_for(&state, &id).and_then(|dm| root_for(&dm)) {
        Ok(root) => root,
        Err(reply) => return reply,
    };
    match read_layout(&root) {
        Ok(layout) => (StatusCode::OK, Json(json!({"layout": layout}))),
        Err(e) => error(StatusCode::UNPROCESSABLE_ENTITY, e),
    }
}

#[derive(Deserialize)]
pub struct ImportRequest {
    contents: String,
}

pub async fn import(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(body): Json<ImportRequest>,
) -> Reply {
    if body.contents.len() > MAX_BYTES {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Binding export exceeds 512 KiB",
        );
    }
    if body.contents.trim().is_empty() {
        return error(StatusCode::BAD_REQUEST, "The binding export is empty");
    }
    let root = match module_for(&state, &id).and_then(|dm| root_for(&dm)) {
        Ok(root) => root,
        Err(reply) => return reply,
    };
    let Ok(_guard) = IMPORT_LOCK.try_lock() else {
        return error(StatusCode::CONFLICT, "A binding import is already running");
    };
    let hook = match owned_path(&root, HOOK) {
        Ok(path) if path.is_file() => path,
        _ => {
            return error(
                StatusCode::NOT_FOUND,
                "The module's host binding import hook is missing",
            )
        }
    };
    match tokio::time::timeout(
        Duration::from_secs(110),
        run_hook(&root, &hook, body.contents),
    )
    .await
    {
        Ok(Ok(())) => match read_layout(&root) {
            Ok(Some(layout)) => {
                state.v2.sync_all_with_reason("binding_import", Some(&id));
                (StatusCode::OK, Json(json!({"layout": layout})))
            }
            Ok(None) => error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "The importer did not produce a layout",
            ),
            Err(e) => error(StatusCode::UNPROCESSABLE_ENTITY, e),
        },
        Ok(Err(e)) => error(StatusCode::UNPROCESSABLE_ENTITY, e),
        Err(_) => error(
            StatusCode::GATEWAY_TIMEOUT,
            "Binding import timed out. Check network access and try again.",
        ),
    }
}

async fn run_hook(root: &FsPath, hook: &FsPath, contents: String) -> Result<(), String> {
    #[cfg(windows)]
    let python = "python";
    #[cfg(not(windows))]
    let python = "python3";
    let mut child = tokio::process::Command::new(python)
        .arg(hook)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Could not start the module importer with {python}: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("Could not open import input")?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or("Could not open import diagnostics")?
        .take(8192);
    let (status, write, result) = tokio::join!(
        child.wait(),
        async move { stdin.write_all(contents.as_bytes()).await },
        async move {
            let mut diagnostics = Vec::new();
            stderr
                .read_to_end(&mut diagnostics)
                .await
                .map(|_| diagnostics)
        }
    );
    if !status.map_err(|e| e.to_string())?.success() {
        let diagnostics = result.map_err(|e| e.to_string())?;
        let message = String::from_utf8_lossy(&diagnostics).trim().to_string();
        return Err(if message.is_empty() {
            "The module importer failed".into()
        } else {
            message
        });
    }
    write.map_err(|e| format!("Could not send the binding export to the module: {e}"))?;
    Ok(())
}
