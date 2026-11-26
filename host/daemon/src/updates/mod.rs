//! Update discovery, keyboard updates and the jobs that carry them out.
//!
//! The daemon fetches the signed release index on start, every day and on request, verifies it
//! and falls back to the index bundled with the installer. From the verified releases and what
//! the keyboard reports it works out, with the `openflux_update` engine, what can be installed.
//! Installing happens in jobs ([`jobs`]): one at a time, under the daemon's busy lease, with a
//! journal that survives a restart.
//!
//! While a job runs it has the keyboard to itself: the USB worker leaves the keyboard alone and
//! ordinary keyboard writes are refused.

pub mod install;
pub mod jobs;
pub mod local;
pub mod source;
pub mod status;

pub use status::Area;

use crate::api::state::DeviceSink;
use crate::lifecycle::{BusyLease, Lifecycle};
use jobs::{JobError, JobKind, JobRecord, JobState, Journal, PlannedStep, StepKind};
use openflux_update::compat::{Compat, HostProfile};
use openflux_update::{Component, Engine, PlanRequest, Preferences, TrustAnchors, TrustStore};
use serde::{Deserialize, Serialize};
use source::{FileTrustStore, SourceError, Sources};
use status::{AreaState, KeyboardView, ViewMode};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The interface registry this host was built from.
pub const COMPAT_TOML: &str = include_str!("../../../../compat.toml");

/// Where the published index lives.
pub const DEFAULT_INDEX_URL: &str = "https://keyglass-dev.github.io/keyglass/index.json";

/// How long installs may take before a job gives up waiting, and how often it looks.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// Time for a component to come back with its new version after an install.
    pub install_wait: Duration,
    /// Time for the keyboard to reconnect before a step starts.
    pub reconnect_wait: Duration,
    pub poll: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            install_wait: Duration::from_secs(240),
            reconnect_wait: Duration::from_secs(60),
            poll: Duration::from_secs(1),
        }
    }
}

pub struct UpdateConfig {
    /// The daemon's data directory; updates keep their state in `updates/` inside it.
    pub data_dir: PathBuf,
    pub index_url: String,
    /// The installer's bundled keyboard software (`index.json`, `index.json.sig` and artifacts).
    pub bundled_dir: Option<PathBuf>,
    pub anchors: TrustAnchors,
    /// Time between automatic checks.
    pub check_interval: Duration,
    pub timings: Timings,
    /// A mirror of the factory container, used instead of the URL the signed index records. The
    /// files are checked against the index either way.
    pub factory_container_url: Option<String>,
    /// A mirror of the press-refinement source's container, used instead of the URLs the signed
    /// index records. Every stage is checked against the index either way.
    pub press_refinement_container_url: Option<String>,
}

impl UpdateConfig {
    /// The configuration of an installed daemon: the published index (or
    /// `OPENFLUX_UPDATE_INDEX_URL`), the installer's bundled keyboard software (or
    /// `OPENFLUX_BUNDLED_KEYBOARD_DIR`), and the keys built into this host.
    pub fn installed(data_dir: PathBuf) -> Self {
        let index_url = std::env::var("OPENFLUX_UPDATE_INDEX_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_INDEX_URL.into());
        let bundled_dir = std::env::var_os("OPENFLUX_BUNDLED_KEYBOARD_DIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(bundled_keyboard_dir);
        Self {
            data_dir,
            index_url,
            bundled_dir,
            anchors: openflux_update::trust_anchors(),
            check_interval: Duration::from_secs(24 * 3600),
            timings: Timings::default(),
            factory_container_url: std::env::var("OPENFLUX_FACTORY_CONTAINER_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            press_refinement_container_url: std::env::var(
                "OPENFLUX_PRESS_REFINEMENT_CONTAINER_URL",
            )
            .ok()
            .filter(|s| !s.is_empty()),
        }
    }
}

/// The installer's `keyboard/` resource directory, relative to the daemon's executable: the app
/// bundle's `Resources` on macOS, the install directory on Windows, `lib/<app>` on Linux.
pub fn bundled_keyboard_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidates = [
        dir.join("../Resources/keyboard"),
        dir.join("keyboard"),
        dir.join("../lib/OpenFlux Studio/keyboard"),
        dir.join("../lib/openflux-studio/keyboard"),
    ];
    candidates
        .into_iter()
        .find(|p| p.join("index.json").is_file())
}

/// Update source chosen by the user: signed remote channels (`stable`/`beta`) or an unsigned
/// local developer build folder (`local`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateChannel {
    Stable,
    Beta,
    Local,
}

impl std::str::FromStr for UpdateChannel {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stable" => Ok(Self::Stable),
            "beta" => Ok(Self::Beta),
            "local" => Ok(Self::Local),
            _ => Err("channel must be stable, beta, or local"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub channel: UpdateChannel,
    pub auto_check: bool,
    #[serde(default)]
    pub local_dir: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            channel: UpdateChannel::Stable,
            auto_check: true,
            local_dir: None,
        }
    }
}

/// Why ordinary keyboard work was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteRefusal {
    /// `busy`, `conversionRequired`, `readOnly`, `unsupported` or `unknown`.
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for WriteRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Why a job wasn't started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// HTTP status to answer with.
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl Refused {
    fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: 409,
            code,
            message: message.into(),
        }
    }
}

struct Inner {
    settings: Settings,
    checked_at: Option<String>,
    checking: bool,
    error: Option<SourceError>,
    sources: Sources,
    connected: (bool, bool),
    versions: serde_json::Value,
    job: Option<JobRecord>,
    /// A short keyboard operation outside a job holds the keyboard; see
    /// [`UpdateService::hold_keyboard`].
    held: Option<&'static str>,
}

/// Keeps jobs from starting while a short keyboard operation runs; released when dropped.
pub struct KeyboardHold {
    svc: Arc<UpdateService>,
    _lease: BusyLease,
}

impl Drop for KeyboardHold {
    fn drop(&mut self) {
        self.svc.inner.lock().unwrap().held = None;
    }
}

pub struct UpdateService {
    config: UpdateConfig,
    pub(crate) device: Arc<dyn DeviceSink>,
    lifecycle: Arc<Lifecycle>,
    pub(crate) compat: Compat,
    pub(crate) profile: HostProfile,
    inner: Mutex<Inner>,
    reserved: AtomicBool,
    check_running: AtomicBool,
    started: AtomicBool,
    journal: Journal,
}

fn updates_dir(config: &UpdateConfig) -> PathBuf {
    config.data_dir.join("updates")
}

impl UpdateService {
    pub fn new(
        config: UpdateConfig,
        device: Arc<dyn DeviceSink>,
        lifecycle: Arc<Lifecycle>,
    ) -> Arc<Self> {
        let compat = Compat::parse(COMPAT_TOML).expect("the built-in compat.toml is valid");
        let profile = compat.host_profile();
        let dir = updates_dir(&config);
        let settings = std::fs::read(dir.join("settings.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        let journal = Journal::new(dir.join("jobs"));
        let has_interrupted = journal.interrupted().is_some();
        let job = journal.latest();
        let svc = Arc::new(Self {
            device,
            lifecycle,
            compat,
            profile,
            inner: Mutex::new(Inner {
                settings,
                checked_at: None,
                checking: false,
                error: None,
                sources: Sources::default(),
                connected: (false, false),
                versions: crate::api::state::empty_versions(),
                job,
                held: None,
            }),
            reserved: AtomicBool::new(has_interrupted),
            check_running: AtomicBool::new(false),
            started: AtomicBool::new(false),
            journal,
            config,
        });
        svc.load_bundled();
        svc
    }

    fn trust_store(&self) -> FileTrustStore {
        FileTrustStore::new(updates_dir(&self.config).join("trust.json"))
    }

    pub(crate) fn cache_dir(&self) -> PathBuf {
        updates_dir(&self.config).join("cache")
    }

    pub(crate) fn job_dir(&self, id: &str) -> PathBuf {
        self.journal.dir().join(id)
    }

    pub(crate) fn timings(&self) -> Timings {
        self.config.timings
    }

    fn load_bundled(&self) {
        let Some(dir) = self.config.bundled_dir.clone() else {
            return;
        };
        let state = self.trust_store().load().unwrap_or_default();
        match source::load_bundled_index(&dir, &self.config.anchors, &state) {
            Ok(Some(idx)) => {
                self.inner.lock().unwrap().sources.bundled = Some((dir, idx));
            }
            Ok(None) => {}
            Err(e) => {
                let mut g = self.inner.lock().unwrap();
                g.sources.bundled = None;
                g.error.get_or_insert(SourceError {
                    code: e.code,
                    message: format!("Bundled index: {}", e.message),
                });
            }
        }
    }

    /// Starts the background work: a check now (when automatic checks are on) and every
    /// `check_interval`, and resuming a job a restart interrupted.
    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            svc.resume_interrupted();
            if svc.settings().auto_check {
                svc.check();
            } else {
                svc.refresh_keyboard();
            }
            loop {
                std::thread::sleep(svc.config.check_interval);
                if svc.settings().auto_check {
                    svc.check();
                }
            }
        });
    }

    pub fn settings(&self) -> Settings {
        self.inner.lock().unwrap().settings.clone()
    }

    pub fn put_settings(
        &self,
        channel: Option<UpdateChannel>,
        auto_check: Option<bool>,
        local_dir: Option<Option<String>>,
    ) -> Result<(), String> {
        let s = {
            let mut g = self.inner.lock().unwrap();
            if let Some(c) = channel {
                g.settings.channel = c;
            }
            if let Some(a) = auto_check {
                g.settings.auto_check = a;
            }
            if let Some(dir) = local_dir {
                g.settings.local_dir = dir.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
            }
            g.settings.clone()
        };
        let raw = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?;
        source::write_durably(&updates_dir(&self.config).join("settings.json"), &raw)
    }

    fn prefs(&self) -> Preferences {
        Preferences {
            beta: self.settings().channel == UpdateChannel::Beta,
        }
    }

    /// True while a job has the keyboard to itself.
    pub fn keyboard_reserved(&self) -> bool {
        self.reserved.load(Ordering::SeqCst)
    }

    /// Records what the keyboard reports. Called by the USB worker when it reads the versions.
    pub fn observe_keyboard(&self, connected: (bool, bool), versions: &serde_json::Value) {
        let mut g = self.inner.lock().unwrap();
        g.connected = connected;
        g.versions = versions.clone();
    }

    /// Reads the keyboard's versions directly, unless a job has the keyboard.
    pub fn refresh_keyboard(&self) {
        if self.keyboard_reserved() {
            return;
        }
        let connected = self.device.is_connected();
        let versions = if connected.0 || connected.1 {
            self.device.versions()
        } else {
            crate::api::state::empty_versions()
        };
        self.observe_keyboard(connected, &versions);
    }

    /// Starts a check in the background unless one is running.
    pub fn request_check(self: &Arc<Self>) {
        if self.check_running.load(Ordering::SeqCst) {
            return;
        }
        self.inner.lock().unwrap().checking = true;
        let svc = self.clone();
        std::thread::spawn(move || svc.check());
    }

    /// Fetches and verifies the online index, reloads the bundled one and reads the keyboard.
    /// Errors are recorded for the status body.
    pub fn check(&self) {
        if self.check_running.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.lock().unwrap().checking = true;
        self.refresh_keyboard();
        self.load_bundled();
        let mut store = self.trust_store();
        let url = self.config.index_url.clone();
        let result = source::fetch_online_index(&url, &self.config.anchors, &mut store);
        {
            let mut g = self.inner.lock().unwrap();
            match result {
                Ok(idx) => {
                    g.sources.online = Some((url, idx));
                    g.error = None;
                }
                Err(e) => {
                    // A rejected index is never used; the bundled one stays available.
                    g.sources.online = None;
                    g.error = Some(e);
                }
            }
            g.checked_at = Some(jobs::now_rfc3339());
            g.checking = false;
        }
        self.check_running.store(false, Ordering::SeqCst);
    }

    pub(crate) fn sources(&self) -> Sources {
        self.inner.lock().unwrap().sources.clone()
    }

    /// Where press-refinement data is found, from the newest verified index that records it.
    pub(crate) fn press_refinement_source(&self) -> Option<openflux_update::PressRefinementSource> {
        self.sources().catalog().press_refinement_source().cloned()
    }

    /// The mirror that replaces the press-refinement container URLs, if one is configured.
    pub(crate) fn press_refinement_container_url(&self) -> Option<&str> {
        self.config.press_refinement_container_url.as_deref()
    }

    /// Where verified press-refinement data is kept, in the daemon's data directory.
    pub(crate) fn press_refinement_dir(&self) -> PathBuf {
        self.config.data_dir.join("press-refinement")
    }

    /// Takes the keyboard for a short operation outside a job, `activity`: refused while a job
    /// runs, another operation holds it or the daemon is about to exit, and jobs are refused
    /// until the hold is dropped.
    pub fn hold_keyboard(
        self: &Arc<Self>,
        activity: &'static str,
    ) -> Result<KeyboardHold, Refused> {
        let mut g = self.inner.lock().unwrap();
        if g.job.as_ref().is_some_and(|j| j.state == JobState::Running) || self.keyboard_reserved()
        {
            return Err(Refused::conflict(
                "busy",
                "The keyboard is being updated; try again when it finishes",
            ));
        }
        if let Some(other) = g.held {
            return Err(Refused::conflict(
                "busy",
                format!("OpenFlux is {other}; try again when it finishes"),
            ));
        }
        let lease = self
            .lifecycle
            .hold(activity)
            .map_err(|e| Refused::conflict("busy", e.to_string()))?;
        g.held = Some(activity);
        Ok(KeyboardHold {
            svc: self.clone(),
            _lease: lease,
        })
    }

    /// Where the factory container is fetched from instead of the URL the index records.
    pub(crate) fn factory_container_url(&self) -> Option<&str> {
        self.config.factory_container_url.as_deref()
    }

    pub(crate) fn view(&self) -> KeyboardView {
        let (connected, versions, catalog) = {
            let g = self.inner.lock().unwrap();
            (g.connected, g.versions.clone(), g.sources.catalog())
        };
        KeyboardView::identify(connected, &versions, &catalog, &self.compat)
    }

    /// `GET /v1/updates`.
    pub fn status_body(&self) -> serde_json::Value {
        let view = self.view();
        let (settings, checked_at, checking, error, catalog, source, job) = {
            let g = self.inner.lock().unwrap();
            (
                g.settings.clone(),
                g.checked_at.clone(),
                g.checking,
                g.error.clone(),
                g.sources.catalog(),
                g.sources.source_name(),
                g.job.as_ref().map(JobRecord::summary),
            )
        };
        let (host, keyboard) = status::status_blocks(&view, &catalog, &self.profile, self.prefs());
        let local_builds = settings
            .local_dir
            .as_deref()
            .map(|d| local::scan_dir(std::path::Path::new(d)));
        serde_json::json!({
            "checkedAt": checked_at,
            "checking": checking,
            "channel": settings.channel,
            "autoCheck": settings.auto_check,
            "localDir": settings.local_dir,
            "localBuilds": local_builds,
            "indexSource": source,
            "error": error.map(|e| serde_json::json!({"code": e.code, "message": e.message})),
            "host": host,
            "keyboard": keyboard,
            "job": job,
        })
    }

    /// Per-area compatibility, for `GET /v1/device/status`.
    pub fn compatibility(&self) -> serde_json::Value {
        status::compatibility_json(&self.view(), &self.profile)
    }

    /// Whether ordinary work may write `areas` now.
    pub fn check_write(&self, areas: &[Area]) -> Result<(), WriteRefusal> {
        if self.keyboard_reserved() {
            return Err(WriteRefusal {
                code: "busy",
                message: "The keyboard is being updated; try again when it finishes".into(),
            });
        }
        if let Some(kind) = self.lifecycle.exit_requested() {
            return Err(WriteRefusal {
                code: "busy",
                message: crate::lifecycle::ExitPending(kind).to_string(),
            });
        }
        let view = self.view();
        match view.mode() {
            ViewMode::NeedsConversion => {
                return Err(WriteRefusal {
                    code: "conversionRequired",
                    message: "The keyboard isn't fully running Keyglass; convert it to Keyglass before changing it".into(),
                })
            }
            ViewMode::Disconnected if !view.connected() => return Ok(()),
            _ => {}
        }
        for a in areas {
            match status::area_state(*a, &view, &self.profile) {
                AreaState::Ok => {}
                AreaState::ReadOnly => {
                    return Err(WriteRefusal {
                        code: "readOnly",
                        message: format!("The keyboard stores {} in a newer format than this Keyglass version understands; update Keyglass to change it", a.describe()),
                    })
                }
                AreaState::Unsupported => {
                    return Err(WriteRefusal {
                        code: "unsupported",
                        message: format!("This Keyglass version doesn't support the keyboard's version of {}; update Keyglass", a.describe()),
                    })
                }
                AreaState::Unknown => {
                    return Err(WriteRefusal {
                        code: "unknown",
                        message: format!("The keyboard's software isn't in the release index, so {} can't be changed safely; check for updates", a.describe()),
                    })
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------------------------
    // Jobs
    // -----------------------------------------------------------------------------------------

    /// `GET /v1/jobs/{id}`.
    pub fn job(&self, id: &str) -> Option<serde_json::Value> {
        if let Some(j) = self
            .inner
            .lock()
            .unwrap()
            .job
            .as_ref()
            .filter(|j| j.id == id)
        {
            return Some(j.summary());
        }
        self.journal.load(id).map(|j| j.summary())
    }

    pub fn current_job(&self) -> Option<JobRecord> {
        self.inner.lock().unwrap().job.clone()
    }

    /// Saves a job's progress to the journal and the status body.
    pub(crate) fn save_job(&self, job: &JobRecord) {
        let _ = self.journal.save(job);
        self.inner.lock().unwrap().job = Some(job.clone());
    }

    /// Starts a job: refused while another runs or the daemon is about to exit.
    fn admit(self: &Arc<Self>, kind: JobKind, steps: Vec<PlannedStep>) -> Result<String, Refused> {
        let mut g = self.inner.lock().unwrap();
        if g.job.as_ref().is_some_and(|j| j.state == JobState::Running) {
            return Err(Refused::conflict("busy", "Another keyboard job is running"));
        }
        if let Some(other) = g.held {
            return Err(Refused::conflict(
                "busy",
                format!("OpenFlux is {other}; try again when it finishes"),
            ));
        }
        let lease = self
            .lifecycle
            .hold(kind.activity())
            .map_err(|e| Refused::conflict("busy", e.to_string()))?;
        let job = JobRecord::new(kind, steps);
        self.journal.save(&job).map_err(|e| Refused {
            status: 500,
            code: "storage",
            message: format!("The job couldn't be recorded: {e}"),
        })?;
        let id = job.id.clone();
        g.job = Some(job.clone());
        self.reserved.store(true, Ordering::SeqCst);
        drop(g);
        let svc = self.clone();
        std::thread::spawn(move || svc.run(job, lease));
        Ok(id)
    }

    /// Picks up the job a restart interrupted.
    fn resume_interrupted(self: &Arc<Self>) {
        let Some(mut job) = self.journal.interrupted() else {
            self.reserved.store(false, Ordering::SeqCst);
            return;
        };
        job.resumed += 1;
        job.log.push(format!(
            "OpenFlux restarted during step {} ({}); resuming",
            job.step.index, job.step.label
        ));
        let lease = match self.lifecycle.hold(job.kind.activity()) {
            Ok(l) => l,
            Err(e) => {
                job.state = JobState::Failed;
                job.error = Some(JobError::new(
                    "interrupted",
                    e.to_string(),
                    "Start the job again once OpenFlux is running.",
                ));
                job.finished_at = Some(jobs::now_rfc3339());
                self.reserved.store(false, Ordering::SeqCst);
                self.save_job(&job);
                return;
            }
        };
        self.reserved.store(true, Ordering::SeqCst);
        self.save_job(&job);
        let svc = self.clone();
        std::thread::spawn(move || svc.run(job, lease));
    }

    fn run(self: Arc<Self>, mut job: JobRecord, lease: BusyLease) {
        let count = job.steps.len();
        let result = (|| -> Result<(), JobError> {
            while job.next_step < count {
                let i = job.next_step;
                job.step = jobs::StepInfo {
                    index: i + 1,
                    count,
                    label: job.steps[i].label.clone(),
                };
                job.progress = Some(i as f64 / count.max(1) as f64);
                job.log.push(job.steps[i].label.clone());
                self.save_job(&job);
                self.run_step(&mut job, i)?;
                job.next_step = i + 1;
                self.save_job(&job);
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                job.state = JobState::Succeeded;
                job.progress = Some(1.0);
                job.log.push("Done".into());
            }
            Err(e) => {
                job.log.push(format!("Failed: {}", e.message));
                job.state = JobState::Failed;
                job.error = Some(e);
            }
        }
        job.finished_at = Some(jobs::now_rfc3339());
        self.save_job(&job);
        self.reserved.store(false, Ordering::SeqCst);
        self.refresh_keyboard();
        drop(lease);
    }

    fn run_step(&self, job: &mut JobRecord, i: usize) -> Result<(), JobError> {
        match job.steps[i].kind {
            StepKind::Prepare => install::prepare(self, job),
            StepKind::InstallPackage => install::install_package(self, job, i),
            StepKind::FlashFirmware => install::flash_firmware(self, job, i),
            StepKind::InstallLocalPackage => install::install_local_package(self, job, i),
            StepKind::FlashLocalFirmware => install::flash_local_firmware(self, job, i),
            StepKind::Verify => install::verify(self, job, i),
            StepKind::ReadSettings => crate::conversion::jobs::read_settings(self, job, i),
            StepKind::StageRescue => crate::conversion::jobs::stage_rescue(self, job, i),
            StepKind::ImportSettings => crate::conversion::jobs::import_settings(self, job, i),
            StepKind::StageFactoryRestore => {
                crate::conversion::jobs::stage_factory_restore(self, job, i)
            }
            StepKind::AwaitFactory => crate::conversion::jobs::await_factory(self, job, i),
            StepKind::FactoryKeymap => crate::conversion::jobs::factory_keymap(self, job, i),
            StepKind::InstallPressRefinement => {
                crate::conversion::jobs::install_press_refinement(self, job, i)
            }
        }
    }

    fn parse_plan_request(components: Option<Vec<String>>) -> Result<PlanRequest, Refused> {
        match components {
            None => Ok(PlanRequest::keyboard()),
            Some(list) => {
                if list.is_empty() {
                    return Err(Refused {
                        status: 400,
                        code: "badRequest",
                        message: "components must name at least one component".into(),
                    });
                }
                let mut set = std::collections::BTreeSet::new();
                for c in list {
                    let comp: Component =
                        c.parse()
                            .map_err(|e: openflux_update::version::ParseError| Refused {
                                status: 400,
                                code: "badRequest",
                                message: e.0,
                            })?;
                    if comp == Component::Host {
                        return Err(Refused {
                            status: 400,
                            code: "badRequest",
                            message: "OpenFlux updates itself; apply covers keyboard components"
                                .into(),
                        });
                    }
                    set.insert(comp);
                }
                Ok(PlanRequest { components: set })
            }
        }
    }

    /// `POST /v1/updates/plan`: previews the update plan for `components` (or the whole keyboard).
    pub fn plan_body(&self, components: Option<Vec<String>>) -> Result<serde_json::Value, Refused> {
        let request = Self::parse_plan_request(components)?;
        let view = self.view();
        if view.mode() != ViewMode::OpenFlux || !view.undescribed().is_empty() {
            return Ok(serde_json::json!({ "plan": serde_json::Value::Null }));
        }
        let catalog = self.sources().catalog();
        let inventory = view.inventory();
        let engine = Engine::new(&self.profile, &inventory, &catalog, self.prefs());
        Ok(serde_json::json!({ "plan": status::plan_json(&engine, &request) }))
    }

    /// `POST /v1/updates/apply`: installs the keyboard updates the plan lists, or those needed
    /// for `components`. When `expected_steps` is provided, refuses if the computed plan differs.
    pub fn apply(
        self: &Arc<Self>,
        components: Option<Vec<String>>,
        expected_steps: Option<Vec<(String, String)>>,
    ) -> Result<String, Refused> {
        let request = Self::parse_plan_request(components)?;
        self.refresh_keyboard();
        let view = self.view();
        match view.mode() {
            ViewMode::Disconnected => {
                return Err(Refused::conflict(
                    "keyboardDisconnected",
                    "The keyboard isn't connected",
                ))
            }
            ViewMode::NeedsConversion => {
                return Err(Refused::conflict(
                    "conversionRequired",
                    "The keyboard isn't fully running Keyglass; convert it instead",
                ))
            }
            ViewMode::OpenFlux => {}
        }
        if !view.undescribed().is_empty() {
            return Err(Refused::conflict(
                "unknown",
                "The keyboard runs a version the release index doesn't list; check for updates",
            ));
        }
        let catalog = self.sources().catalog();
        let inventory = view.inventory();
        let engine = Engine::new(&self.profile, &inventory, &catalog, self.prefs());
        let plan = engine
            .plan(&request)
            .map_err(|r| Refused::conflict("noPlan", r.to_string()))?;
        if let Some(expected) = expected_steps {
            let actual: Vec<(String, String)> = plan
                .steps
                .iter()
                .map(|s| (s.component.as_str().to_string(), s.to.to_string()))
                .collect();
            if actual != expected {
                return Err(Refused::conflict(
                    "planChanged",
                    "The update plan changed before you confirmed. Review the updated plan and try again.",
                ));
            }
        }
        let mut steps = vec![PlannedStep {
            kind: StepKind::Prepare,
            label: "Downloading and verifying the update".into(),
            component: None,
            from: None,
            expect: None,
            artifact: None,
            state: Default::default(),
        }];
        let mut expected: serde_json::Map<String, serde_json::Value> = view
            .components
            .iter()
            .map(|c| {
                (
                    c.component.as_str().to_string(),
                    serde_json::json!(c.reported),
                )
            })
            .collect();
        for s in &plan.steps {
            let artifact = install::artifact_for(&s.release).ok_or_else(|| {
                Refused::conflict(
                    "malformed",
                    format!("{} {} has no installable file", s.component, s.to),
                )
            })?;
            let from = format!("openflux-{} {}", s.component, s.from);
            let to = format!("openflux-{} {}", s.component, s.to);
            expected.insert(s.component.as_str().into(), serde_json::json!(to));
            steps.push(PlannedStep {
                kind: if s.component == Component::Firmware {
                    StepKind::FlashFirmware
                } else {
                    StepKind::InstallPackage
                },
                label: format!("Installing {} {}", s.component, s.to),
                component: Some(s.component.as_str().into()),
                from: Some(from),
                expect: Some(to),
                artifact: Some(artifact),
                state: Default::default(),
            });
        }
        steps.push(PlannedStep {
            kind: StepKind::Verify,
            label: "Checking the installed versions".into(),
            component: None,
            from: None,
            expect: None,
            artifact: None,
            state: [("expect".to_string(), serde_json::Value::Object(expected))]
                .into_iter()
                .collect(),
        });
        self.admit(JobKind::Update, steps)
    }

    /// `POST /v1/updates/install-local`: validates and installs unsigned local developer builds
    /// from the configured `localDir`.
    pub fn apply_local(
        self: &Arc<Self>,
        components: Vec<String>,
        confirm: bool,
    ) -> Result<String, Refused> {
        if !confirm {
            return Err(Refused {
                status: 400,
                code: "confirmationRequired",
                message: "Installing unsigned local builds needs {\"confirm\": true}".into(),
            });
        }
        let req = Self::parse_plan_request(Some(components))?;
        let Some(local_dir) = self.settings().local_dir else {
            return Err(Refused {
                status: 400,
                code: "badRequest",
                message: "Choose a local build folder first".into(),
            });
        };
        let root = std::path::PathBuf::from(&local_dir);
        let scan = local::scan_dir(&root);
        if !scan.exists {
            return Err(Refused {
                status: 400,
                code: "badRequest",
                message: scan
                    .error
                    .unwrap_or_else(|| "Local build folder doesn't exist".into()),
            });
        }

        self.refresh_keyboard();
        let view = self.view();
        if !view.connected() {
            return Err(Refused::conflict(
                "keyboardDisconnected",
                "The keyboard isn't connected",
            ));
        }

        // Order: firmware -> bridge -> display (matches standard update ordering).
        let order = [Component::Firmware, Component::Bridge, Component::Display];
        let mut steps = Vec::new();
        for c in order {
            if !req.components.contains(&c) {
                continue;
            }
            let Some(art) = scan.artifact_for(c) else {
                return Err(Refused {
                    status: 400,
                    code: "badRequest",
                    message: format!("No local {} build found in {}", c.as_str(), scan.dir),
                });
            };
            local::validate_local_artifact(c, &art.path_buf, &root).map_err(|e| Refused {
                status: 400,
                code: "badRequest",
                message: e,
            })?;
            let expect = art
                .version
                .as_ref()
                .map(|v| format!("openflux-{} {v}", c.as_str()));
            let mut state = serde_json::Map::new();
            state.insert("localPath".into(), serde_json::json!(art.path));
            let (kind, label) = match c {
                Component::Firmware => (
                    StepKind::FlashLocalFirmware,
                    format!("Flash local firmware ({})", art.name),
                ),
                Component::Bridge => (
                    StepKind::InstallLocalPackage,
                    format!("Install local keyboard bridge ({})", art.name),
                ),
                Component::Display => (
                    StepKind::InstallLocalPackage,
                    format!("Install local display ({})", art.name),
                ),
                Component::Host => continue,
            };
            steps.push(PlannedStep {
                kind,
                label,
                component: Some(c.as_str().into()),
                from: None,
                expect,
                artifact: None,
                state,
            });
        }

        self.admit(JobKind::LocalInstall, steps)
    }

    /// `POST /v1/conversion/convert`.
    pub fn convert(self: &Arc<Self>, confirm: bool) -> Result<String, Refused> {
        if !confirm {
            return Err(Refused {
                status: 400,
                code: "confirmationRequired",
                message: "Converting needs {\"confirm\": true}".into(),
            });
        }
        self.refresh_keyboard();
        let view = self.view();
        let steps = crate::conversion::jobs::conversion_steps(self, &view)?;
        self.admit(JobKind::Convert, steps)
    }

    /// `POST /v1/conversion/restore`.
    pub fn restore(self: &Arc<Self>, confirm: bool) -> Result<String, Refused> {
        if !confirm {
            return Err(Refused {
                status: 400,
                code: "confirmationRequired",
                message: "Restoring needs {\"confirm\": true}".into(),
            });
        }
        self.refresh_keyboard();
        let view = self.view();
        let steps = crate::conversion::jobs::restore_steps(self, &view)?;
        self.admit(JobKind::Restore, steps)
    }

    /// `GET /v1/conversion`.
    pub fn conversion_body(&self) -> serde_json::Value {
        let view = self.view();
        let running = self.current_job().filter(|j| j.state == JobState::Running);
        let convert = if running.is_some() {
            Err("A keyboard job is running".to_string())
        } else {
            crate::conversion::jobs::conversion_steps(self, &view)
                .map(|_| ())
                .map_err(|r| r.message)
        };
        let restore = if running.is_some() {
            Err("A keyboard job is running".to_string())
        } else {
            crate::conversion::jobs::restore_steps(self, &view)
                .map(|_| ())
                .map_err(|r| r.message)
        };
        let factory = self.sources().catalog().factory().cloned();
        serde_json::json!({
            "canConvert": convert.is_ok(),
            "canRestore": restore.is_ok(),
            "reason": convert.as_ref().err().cloned(),
            "restoreReason": restore.as_ref().err().cloned(),
            "factory": factory.map(|f| serde_json::json!({"label": f.label})),
            "job": self.current_job().filter(|j| matches!(j.kind, JobKind::Convert | JobKind::Restore)).map(|j| j.summary()),
        })
    }
}
