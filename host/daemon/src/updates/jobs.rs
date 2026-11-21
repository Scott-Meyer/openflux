//! Long keyboard operations as jobs with a durable journal.
//!
//! One job runs at a time. Its record is written to the data directory at every change, so a
//! daemon that restarts mid-job finds it, reports the step it was on and resumes it.

use crate::updates::source::write_durably;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobKind {
    Update,
    Convert,
    Restore,
    LocalInstall,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Update => "update",
            JobKind::Convert => "convert",
            JobKind::Restore => "restore",
            JobKind::LocalInstall => "localInstall",
        }
    }

    /// What the busy lease says while the job runs.
    pub fn activity(self) -> &'static str {
        match self {
            JobKind::Update => "updating the keyboard",
            JobKind::Convert => "converting the keyboard to OpenFlux",
            JobKind::Restore => "restoring the keyboard's factory software",
            JobKind::LocalInstall => "installing local keyboard builds",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepInfo {
    /// 1-based index of the current step; 0 before the first step starts.
    pub index: usize,
    pub count: usize,
    pub label: String,
}

/// Why a job failed, and what the user can do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobError {
    pub code: String,
    pub message: String,
    pub recovery: String,
}

impl JobError {
    pub fn new(code: &str, message: impl Into<String>, recovery: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            recovery: recovery.into(),
        }
    }
}

/// One planned step of a job. `kind` says what it does; `component`, `from` and `expect` describe
/// what it installs, if anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedStep {
    pub kind: StepKind,
    pub label: String,
    /// The component installed by this step, if it installs one.
    #[serde(default)]
    pub component: Option<String>,
    /// Version string the component reports before the step, if known.
    #[serde(default)]
    pub from: Option<String>,
    /// Version string the component reports once the step is done.
    #[serde(default)]
    pub expect: Option<String>,
    /// The artifact installed by this step, as the verified index lists it.
    #[serde(default)]
    pub artifact: Option<openflux_update::manifest::Artifact>,
    /// Step-specific progress that must survive a restart, such as the bridge install attempt.
    #[serde(default)]
    pub state: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    /// Download and verify every artifact of the job.
    Prepare,
    /// Install a bridge or display package through the package drop folder.
    InstallPackage,
    /// Flash firmware through the boot loader.
    FlashFirmware,
    /// Install an unsigned local bridge or display package from a developer folder.
    InstallLocalPackage,
    /// Flash an unsigned local firmware `.uf2` from a developer folder.
    FlashLocalFirmware,
    /// Read the keyboard's existing settings for import.
    ReadSettings,
    /// Stage the OpenFlux bridge and its launcher as the keyboard's rescue pair.
    StageRescue,
    /// Write the imported settings into the OpenFlux configuration.
    ImportSettings,
    /// Stage the factory packages and start the keyboard's restore.
    StageFactoryRestore,
    /// Wait for the factory bridge and display to run.
    AwaitFactory,
    /// Write the firmware keymap in the factory firmware's encoding.
    FactoryKeymap,
    /// Check every component's version.
    Verify,
    /// Install press-refinement data on the keyboard. A failure is reported in the log and
    /// doesn't fail the job.
    InstallPressRefinement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub id: String,
    pub kind: JobKind,
    pub state: JobState,
    pub step: StepInfo,
    pub progress: Option<f64>,
    pub log: Vec<String>,
    pub error: Option<JobError>,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// The steps, in order. Not part of the API summary.
    pub steps: Vec<PlannedStep>,
    /// Index of the first step not yet completed.
    pub next_step: usize,
    /// How many times the daemon picked the job up after a restart.
    #[serde(default)]
    pub resumed: u32,
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// A random 16-digit hex id. Without a random source it is made from the clock and a counter, so
/// ids still differ.
pub fn new_job_id() -> String {
    let mut b = [0u8; 8];
    if getrandom::getrandom(&mut b).is_err() {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        b = (nanos ^ seq.rotate_right(16)).to_be_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// When a job started, for ordering; RFC 3339 strings don't sort by time when their fractions
/// differ in length.
fn started_at_time(job: &JobRecord) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(
        &job.started_at,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()
}

impl JobRecord {
    pub fn new(kind: JobKind, steps: Vec<PlannedStep>) -> Self {
        let count = steps.len();
        Self {
            id: new_job_id(),
            kind,
            state: JobState::Running,
            step: StepInfo {
                index: 0,
                count,
                label: "Starting".into(),
            },
            progress: Some(0.0),
            log: Vec::new(),
            error: None,
            started_at: now_rfc3339(),
            finished_at: None,
            steps,
            next_step: 0,
            resumed: 0,
        }
    }

    /// The job as `GET /v1/jobs/{id}` reports it. A conversion also reports the outcome of its
    /// press-refinement step as `pressRefinement` once that step has run.
    pub fn summary(&self) -> serde_json::Value {
        let mut out = serde_json::json!({
            "id": self.id,
            "kind": self.kind.as_str(),
            "state": self.state,
            "step": self.step,
            "progress": self.progress,
            "log": self.log,
            "error": self.error,
            "startedAt": self.started_at,
            "finishedAt": self.finished_at,
        });
        if let Some(pr) = self
            .steps
            .iter()
            .find_map(|s| s.state.get("pressRefinement"))
        {
            out["pressRefinement"] = pr.clone();
        }
        out
    }
}

/// The journal: one file per job, plus `current.json` naming the running job.
#[derive(Debug, Clone)]
pub struct Journal {
    dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Current {
    id: String,
}

impl Journal {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn valid_id(id: &str) -> bool {
        !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric())
    }

    /// Writes the record durably. A running job is also recorded as current.
    pub fn save(&self, job: &JobRecord) -> Result<(), String> {
        let raw = serde_json::to_vec_pretty(job).map_err(|e| e.to_string())?;
        write_durably(&self.path(&job.id), &raw)?;
        let current = self.dir.join("current.json");
        if job.state == JobState::Running {
            let raw =
                serde_json::to_vec(&Current { id: job.id.clone() }).map_err(|e| e.to_string())?;
            write_durably(&current, &raw)?;
        } else if self.current_id().as_deref() == Some(job.id.as_str()) {
            let _ = std::fs::remove_file(&current);
        }
        Ok(())
    }

    pub fn load(&self, id: &str) -> Option<JobRecord> {
        if !Self::valid_id(id) {
            return None;
        }
        let raw = std::fs::read(self.path(id)).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    fn current_id(&self) -> Option<String> {
        let raw = std::fs::read(self.dir.join("current.json")).ok()?;
        serde_json::from_slice::<Current>(&raw).ok().map(|c| c.id)
    }

    /// The job that was running when the daemon last stopped, if any.
    pub fn interrupted(&self) -> Option<JobRecord> {
        let job = self.load(&self.current_id()?)?;
        (job.state == JobState::Running).then_some(job)
    }

    /// The most recently started job, for the status body.
    pub fn latest(&self) -> Option<JobRecord> {
        let rd = std::fs::read_dir(&self.dir).ok()?;
        rd.filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "json")
                    && p.file_stem().is_some_and(|s| s != "current")
            })
            .filter_map(|p| std::fs::read(&p).ok())
            .filter_map(|raw| serde_json::from_slice::<JobRecord>(&raw).ok())
            .max_by_key(started_at_time)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_job_is_found_after_a_restart_and_a_finished_one_is_not() {
        let dir = std::env::temp_dir().join(format!("openfluxd_journal_{}", std::process::id()));
        let j = Journal::new(dir.clone());
        let mut job = JobRecord::new(JobKind::Update, Vec::new());
        j.save(&job).unwrap();
        assert_eq!(
            Journal::new(dir.clone()).interrupted().map(|x| x.id),
            Some(job.id.clone())
        );
        job.state = JobState::Succeeded;
        j.save(&job).unwrap();
        assert!(Journal::new(dir.clone()).interrupted().is_none());
        assert_eq!(j.load(&job.id).unwrap().state, JobState::Succeeded);
        assert!(j.load("../etc").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_latest_job_is_the_one_started_last_whatever_its_timestamp_precision() {
        let dir = std::env::temp_dir().join(format!("openfluxd_latest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let j = Journal::new(dir.clone());
        let mut earlier = JobRecord::new(JobKind::Update, Vec::new());
        earlier.started_at = "2030-01-01T00:00:05Z".into();
        earlier.state = JobState::Succeeded;
        let mut later = JobRecord::new(JobKind::Update, Vec::new());
        later.started_at = "2030-01-01T00:00:05.5Z".into();
        later.state = JobState::Succeeded;
        j.save(&later).unwrap();
        j.save(&earlier).unwrap();
        assert_eq!(j.latest().map(|x| x.id), Some(later.id));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
