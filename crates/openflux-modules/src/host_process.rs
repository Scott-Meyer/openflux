//! Unrestricted host-side module process manager.
//!
//! Modules declaring `entrypoints.host` can run arbitrary host scripts, binaries, or local API
//! servers on the user's computer (disclosed clearly on install/update).

use crate::lua_runtime::ModuleLogEntry;
use crate::registry::{DiscoveredModule, ModuleInstanceState};
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

const MAX_HOST_LOGS: usize = 200;

struct RunningHostModule {
    child: Child,
    logs: Arc<Mutex<VecDeque<ModuleLogEntry>>>,
}

pub struct HostProcessManager {
    bus_url: String,
    running: BTreeMap<String, RunningHostModule>,
}

impl HostProcessManager {
    pub fn new(bus_url: impl Into<String>) -> Self {
        Self {
            bus_url: bus_url.into(),
            running: BTreeMap::new(),
        }
    }

    pub fn start_module(
        &mut self,
        discovered: &DiscoveredModule,
        instance: &ModuleInstanceState,
    ) -> Result<(), String> {
        let Some(host_rel) = &discovered.manifest.entrypoints.host else {
            return Ok(());
        };
        self.stop_module(&discovered.manifest.id);

        let entry_path = discovered.path.join(host_rel);
        if !entry_path.is_file() {
            return Err(format!(
                "host entrypoint not found: {}",
                entry_path.display()
            ));
        }

        let mut cmd = build_host_command(&entry_path);
        cmd.current_dir(&discovered.path)
            .env("OPENFLUX_BUS_URL", &self.bus_url)
            .env("OPENFLUX_MODULE_ID", &discovered.manifest.id)
            .env("OPENFLUX_MODULE_DIR", &discovered.path)
            .env(
                "OPENFLUX_CONFIG_JSON",
                serde_json::to_string(&instance.config).unwrap_or_else(|_| "{}".into()),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn {}: {e}", entry_path.display()))?;

        let logs = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_HOST_LOGS)));
        if let Some(out) = child.stdout.take() {
            spawn_log_reader(out, "info", logs.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_log_reader(err, "stderr", logs.clone());
        }

        self.running.insert(
            discovered.manifest.id.clone(),
            RunningHostModule { child, logs },
        );
        Ok(())
    }

    pub fn stop_module(&mut self, id: &str) {
        if let Some(mut proc) = self.running.remove(id) {
            let _ = proc.child.kill();
            let _ = proc.child.wait();
        }
    }

    pub fn stop_all(&mut self) {
        let ids: Vec<String> = self.running.keys().cloned().collect();
        for id in ids {
            self.stop_module(&id);
        }
    }

    pub fn logs(&self, id: &str) -> Vec<ModuleLogEntry> {
        self.running
            .get(id)
            .map(|r| r.logs.lock().unwrap().iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl Drop for HostProcessManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn build_host_command(entry: &std::path::Path) -> Command {
    let ext = entry
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "py" => {
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
    }
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
