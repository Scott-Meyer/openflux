//! Supervised host-side module process manager.
//!
//! Modules declaring `entrypoints.host` run arbitrary host scripts, binaries, or local API
//! helpers on the user's computer (disclosed clearly on install/update).
//!
//! Guarantees:
//! 1. **No silent failures:** missing entrypoints, spawn failures, and unexpected child exits
//!    are tracked explicitly via [`HostProcessStatus`] so callers mark the module unhealthy with
//!    an actionable error message.
//! 2. **No orphan leaks:** every child runs in its own Unix process group and holds an open
//!    stdin pipe to this manager. Stopping a module or dropping the manager kills the entire
//!    process group (including grandchild `ffmpeg`/`yt-dlp` processes), and even a hard crash of
//!    the parent closes the stdin pipe immediately so children can exit cleanly.

use crate::lua_runtime::ModuleLogEntry;
use crate::manifest::HostRequirement;
use crate::registry::{DiscoveredModule, ModuleInstanceState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

const MAX_HOST_LOGS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HostProcessStatus {
    /// Module has no `entrypoints.host` script.
    NotRequired,
    /// Host helper process is currently alive.
    Running { pid: u32 },
    /// Host helper failed validation or could not be spawned (e.g. missing file).
    Failed { error: String },
    /// Host helper started but exited unexpectedly.
    Exited {
        code: Option<i32>,
        last_stderr: Option<String>,
    },
}

impl HostProcessStatus {
    pub fn is_healthy(&self, requirement: HostRequirement) -> bool {
        match self {
            Self::NotRequired | Self::Running { .. } => true,
            Self::Failed { .. } | Self::Exited { .. } => requirement != HostRequirement::Required,
        }
    }

    pub fn error_message(&self) -> Option<String> {
        match self {
            Self::NotRequired | Self::Running { .. } => None,
            Self::Failed { error } => Some(format!("host process error: {error}")),
            Self::Exited { code, last_stderr } => {
                let code_s = code
                    .map(|c| format!("exit code {c}"))
                    .unwrap_or_else(|| "terminated by signal".to_string());
                match last_stderr {
                    Some(line) if !line.is_empty() => {
                        Some(format!("host process exited ({code_s}): {line}"))
                    }
                    _ => Some(format!("host process exited ({code_s})")),
                }
            }
        }
    }
}

struct RunningHostModule {
    child: Child,
    /// Kept open until this module is stopped or the parent daemon dies; EOF signals clean exit.
    _stdin_guard: Option<ChildStdin>,
    config_json: String,
    entry_path: std::path::PathBuf,
    /// Module version + host entrypoint mtime; a change (install, update, local edit) restarts it.
    code_sig: String,
    logs: Arc<Mutex<VecDeque<ModuleLogEntry>>>,
    exited: Option<Option<i32>>,
}

pub struct HostProcessManager {
    bus_url: String,
    bus_token: Option<String>,
    running: BTreeMap<String, RunningHostModule>,
    failures: BTreeMap<String, String>,
}

impl HostProcessManager {
    pub fn new(bus_url: impl Into<String>) -> Self {
        Self {
            bus_url: bus_url.into(),
            bus_token: None,
            running: BTreeMap::new(),
            failures: BTreeMap::new(),
        }
    }

    pub fn with_bus_token(mut self, token: Option<String>) -> Self {
        self.bus_token = token.filter(|t| !t.trim().is_empty());
        self
    }

    pub fn start_module(
        &mut self,
        discovered: &DiscoveredModule,
        instance: &ModuleInstanceState,
    ) -> Result<(), String> {
        let id = &discovered.manifest.id;
        let Some(host_rel) = &discovered.manifest.entrypoints.host else {
            self.failures.remove(id);
            return Ok(());
        };
        let entry_path = discovered.path.join(host_rel);
        if !entry_path.is_file() {
            self.stop_module(id);
            let err = format!("host entrypoint not found: {}", entry_path.display());
            self.failures.insert(id.clone(), err.clone());
            return Err(err);
        }
        let cfg_json = serde_json::to_string(&instance.config).unwrap_or_else(|_| "{}".into());
        let code_sig = format!(
            "{}@{:?}",
            discovered.manifest.version,
            std::fs::metadata(&entry_path).and_then(|m| m.modified()).ok()
        );

        // Keep an already-running host process alive when its config and code are unchanged.
        if let Some(existing) = self.running.get_mut(id) {
            if existing.exited.is_none()
                && existing.config_json == cfg_json
                && existing.entry_path == entry_path
                && existing.code_sig == code_sig
                && matches!(existing.child.try_wait(), Ok(None))
            {
                self.failures.remove(id);
                return Ok(());
            }
        }
        self.stop_module(id);
        self.failures.remove(id);

        let mut cmd = build_host_command(&entry_path);
        cmd.current_dir(&discovered.path)
            .env("OPENFLUX_BUS_URL", &self.bus_url)
            .env("OPENFLUX_MODULE_ID", id)
            .env("OPENFLUX_MODULE_DIR", &discovered.path)
            .env("OPENFLUX_CONFIG_JSON", &cfg_json)
            .env("OPENFLUX_MODULE_CONFIG", &cfg_json)
            .env("OPENFLUX_PARENT_PID", std::process::id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(tok) = &self.bus_token {
            cmd.env("OPENFLUX_BUS_TOKEN", tok);
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Place each host helper and any subprocesses it spawns (e.g. ffmpeg/yt-dlp)
            // in its own process group so stopping the module reaps the entire tree cleanly.
            cmd.process_group(0);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let err = format!("failed to spawn {}: {e}", entry_path.display());
                self.failures.insert(id.clone(), err.clone());
                return Err(err);
            }
        };

        let stdin_guard = child.stdin.take();
        let logs = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_HOST_LOGS)));
        if let Some(out) = child.stdout.take() {
            spawn_log_reader(out, "info", logs.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_log_reader(err, "stderr", logs.clone());
        }

        self.running.insert(
            id.clone(),
            RunningHostModule {
                child,
                _stdin_guard: stdin_guard,
                config_json: cfg_json,
                code_sig,
                entry_path,
                logs,
                exited: None,
            },
        );
        Ok(())
    }

    pub fn running_ids(&self) -> Vec<String> {
        self.running.keys().cloned().collect()
    }

    pub fn stop_module(&mut self, id: &str) {
        self.failures.remove(id);
        if let Some(mut proc) = self.running.remove(id) {
            // Drop stdin first so cooperative helpers watching EOF can shut down gracefully.
            drop(proc._stdin_guard.take());
            kill_child_tree(&mut proc.child);
        }
    }

    pub fn stop_all(&mut self) {
        let ids: Vec<String> = self.running.keys().cloned().collect();
        for id in ids {
            self.stop_module(&id);
        }
        self.failures.clear();
    }

    /// Inspects live status of a module's host process (refreshing `try_wait` so crashes are
    /// surfaced immediately rather than silently hiding behind stale state).
    pub fn status(&mut self, id: &str, has_host_entrypoint: bool) -> HostProcessStatus {
        if !has_host_entrypoint {
            return HostProcessStatus::NotRequired;
        }
        if let Some(err) = self.failures.get(id) {
            return HostProcessStatus::Failed { error: err.clone() };
        }
        if let Some(proc) = self.running.get_mut(id) {
            if proc.exited.is_none() {
                if let Ok(Some(st)) = proc.child.try_wait() {
                    proc.exited = Some(st.code());
                }
            }
            if let Some(code) = proc.exited {
                let last_stderr = proc
                    .logs
                    .lock()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|l| l.level == "stderr")
                    .map(|l| l.message.clone());
                return HostProcessStatus::Exited { code, last_stderr };
            }
            return HostProcessStatus::Running {
                pid: proc.child.id(),
            };
        }
        HostProcessStatus::Failed {
            error: "host process is not running".to_string(),
        }
    }

    pub fn logs(&self, id: &str) -> Vec<ModuleLogEntry> {
        let mut out = self
            .running
            .get(id)
            .map(|r| r.logs.lock().unwrap().iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(err) = self.failures.get(id) {
            out.push(ModuleLogEntry {
                timestamp_ms: openflux_bus::now_unix_ms(),
                level: "error".to_string(),
                message: err.clone(),
            });
        }
        out
    }
}

impl Drop for HostProcessManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn kill_child_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        if pid > 0 {
            // Send SIGTERM to the entire process group first, then SIGKILL.
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
            for _ in 0..5 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn build_host_command(entry: &std::path::Path) -> Command {
    let ext = entry
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    #[allow(unused_mut)]
    let mut cmd = match ext.as_str() {
        "py" => {
            #[cfg(target_os = "windows")]
            let mut c = Command::new("python");
            #[cfg(not(target_os = "windows"))]
            let mut c = Command::new("python3");
            c.arg(entry);
            c
        }
        "js" | "mjs" => {
            let mut c = Command::new("node");
            c.arg(entry);
            c
        }
        "sh" => {
            let mut c = Command::new("sh");
            c.arg(entry);
            c
        }
        _ => Command::new(entry),
    };
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn spawn_log_reader<R: std::io::Read + Send + 'static>(
    reader: R,
    level: &'static str,
    logs: Arc<Mutex<VecDeque<ModuleLogEntry>>>,
) {
    std::thread::spawn(move || {
        let br = BufReader::new(reader);
        for line in br.lines().map_while(Result::ok) {
            let mut g = logs.lock().unwrap();
            if g.len() >= MAX_HOST_LOGS {
                g.pop_front();
            }
            g.push_back(ModuleLogEntry {
                timestamp_ms: openflux_bus::now_unix_ms(),
                level: level.to_string(),
                message: line,
            });
        }
    });
}
