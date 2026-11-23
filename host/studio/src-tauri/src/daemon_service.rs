//! Starting the daemon, `openfluxd`, when Studio finds none running.
//!
//! Each platform registers the daemon with its own service manager:
//! - macOS: the LaunchAgent inside the app bundle, registered with `SMAppService`. The user can
//!   turn it off in System Settings > General > Login Items, in which case it needs approval
//!   there before it can run.
//! - Windows: the `OpenFlux` value under the per-user Run key, written by the installer. Studio
//!   writes it again if it is missing and starts the bundled daemon.
//! - Linux: the systemd user unit, when the package installed one; otherwise Studio starts the
//!   bundled daemon itself. Studio starts the unit but never enables it, so a user who disabled
//!   it at login keeps that choice.
//!
//! [`plan`] decides what to do from an [`Inspection`] of the platform; [`execute`] runs the plan
//! through a [`Platform`], so the decisions are tested without touching the system.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// LaunchAgent property list inside `Contents/Library/LaunchAgents`.
pub const LAUNCH_AGENT_PLIST: &str = "org.openflux.openfluxd.plist";
/// launchd label of the LaunchAgent.
pub const LAUNCH_AGENT_LABEL: &str = "org.openflux.openfluxd";
/// systemd user unit.
pub const SYSTEMD_UNIT: &str = "openfluxd.service";
/// Run key and value the Windows installer writes.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "OpenFlux";

/// What starts the daemon at login on this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Manager {
    LaunchAgent,
    RunKey,
    SystemdUser,
    /// No service manager is available; Studio starts the daemon itself.
    None,
}

/// Whether the daemon is registered to start at login.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Registration {
    /// Registered and allowed to run.
    Enabled,
    /// Registered, but the user must allow it in System Settings > General > Login Items.
    RequiresApproval,
    /// Not registered, or registered with a path that doesn't exist.
    NotRegistered,
    /// This installation has no service definition (for example a development build).
    NotFound,
    Unknown,
}

/// The platform's view of the daemon service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub platform: &'static str,
    pub manager: Manager,
    pub registration: Registration,
    /// The daemon executable bundled with this Studio, if present.
    pub daemon_binary: Option<PathBuf>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    /// `SMAppService.register()`; launchd starts the agent right away.
    RegisterLaunchAgent,
    /// `launchctl kickstart` of the registered agent, which stays stopped after a clean exit.
    KickstartLaunchAgent,
    /// Write the per-user Run value pointing at the bundled daemon.
    WriteRunEntry,
    /// `systemctl --user start`. Whether the unit starts at login stays the user's choice.
    StartSystemdUnit,
    /// Start the bundled daemon as a process of this session.
    StartBundledDaemon,
}

/// How far Studio may go to get a daemon running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StartMode {
    /// Use the service manager, and start the bundled daemon directly if that is not possible.
    Auto,
    /// Use the service manager only. Used after a host update: this process's own bundled
    /// daemon may be the version that was just replaced.
    ServiceOnly,
    /// Start the bundled daemon directly, for this session only.
    Direct,
}

/// Steps to run in order. If one fails, `fallback` runs instead of the remaining steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub fallback: Option<Step>,
}

pub fn plan(insp: &Inspection, mode: StartMode) -> Plan {
    use Registration as R;
    use Step::*;
    let direct = |s: Vec<Step>| Plan {
        steps: s,
        fallback: None,
    };
    let auto_fallback = if mode == StartMode::Auto {
        Some(StartBundledDaemon)
    } else {
        None
    };
    if mode == StartMode::Direct {
        return direct(vec![StartBundledDaemon]);
    }
    match (insp.manager, insp.registration) {
        (Manager::LaunchAgent, R::NotRegistered) => Plan {
            steps: vec![RegisterLaunchAgent],
            fallback: auto_fallback,
        },
        (Manager::LaunchAgent, R::Enabled) => Plan {
            steps: vec![KickstartLaunchAgent],
            fallback: auto_fallback,
        },
        // Only the user can allow it; Studio explains where.
        (Manager::LaunchAgent, R::RequiresApproval) => direct(vec![]),
        // The Run value starts nothing by itself until the next sign-in, so on Windows starting
        // the bundled daemon is the service manager's job, in every mode. The installer replaces
        // that file in place, so after a host update it is the new version.
        (Manager::RunKey, R::Enabled) => direct(vec![StartBundledDaemon]),
        (Manager::RunKey, _) => Plan {
            steps: vec![WriteRunEntry, StartBundledDaemon],
            fallback: Some(StartBundledDaemon),
        },
        (Manager::SystemdUser, R::Enabled | R::NotRegistered) => Plan {
            steps: vec![StartSystemdUnit],
            fallback: auto_fallback,
        },
        _ if mode == StartMode::Auto => direct(vec![StartBundledDaemon]),
        _ => direct(vec![]),
    }
}

/// Inspects and changes the system. Implemented per platform, and by fakes in tests.
pub trait Platform {
    fn inspect(&self) -> Inspection;
    fn run(&self, step: Step) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepResult {
    pub step: Step,
    pub ok: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartReport {
    pub before: Inspection,
    pub steps: Vec<StepResult>,
    pub after: Inspection,
}

pub fn execute(p: &dyn Platform, mode: StartMode) -> StartReport {
    let before = p.inspect();
    let plan = plan(&before, mode);
    let mut steps = Vec::new();
    let run = |step: Step, steps: &mut Vec<StepResult>| {
        let r = p.run(step);
        let ok = r.is_ok();
        steps.push(StepResult {
            step,
            ok,
            detail: r.err(),
        });
        ok
    };
    for step in &plan.steps {
        if !run(*step, &mut steps) {
            if let Some(fb) = plan.fallback {
                if !steps.iter().any(|s| s.step == fb) {
                    run(fb, &mut steps);
                }
            }
            break;
        }
    }
    StartReport {
        before,
        steps,
        after: p.inspect(),
    }
}

/// The daemon executable next to Studio's own. `OPENFLUX_DAEMON_BIN` overrides it.
pub fn bundled_daemon() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("OPENFLUX_DAEMON_BIN").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    let exe = std::env::current_exe().ok()?;
    let p = exe
        .parent()?
        .join(format!("openfluxd{}", std::env::consts::EXE_SUFFIX));
    p.is_file().then_some(p)
}

/// The Run value for `binary`: the quoted path.
pub fn run_entry_value(binary: &Path) -> String {
    format!("\"{}\"", binary.display())
}

/// The executable a Run value starts: the quoted path, or the first word when unquoted.
pub fn run_entry_target(value: &str) -> PathBuf {
    let v = value.trim();
    if let Some(rest) = v.strip_prefix('"') {
        PathBuf::from(rest.split('"').next().unwrap_or(""))
    } else {
        PathBuf::from(v.split_whitespace().next().unwrap_or(""))
    }
}

/// Registration of the systemd unit from `systemctl --user show -p LoadState -p UnitFileState`.
pub fn parse_systemd_show(out: &str) -> Registration {
    let field = |k: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
            .map(str::trim)
            .unwrap_or("")
    };
    match (field("LoadState"), field("UnitFileState")) {
        ("loaded", "enabled" | "enabled-runtime" | "static" | "linked" | "alias") => {
            Registration::Enabled
        }
        ("loaded", "masked") | ("masked", _) => Registration::Unknown,
        ("loaded", _) => Registration::NotRegistered,
        ("not-found", _) => Registration::NotFound,
        _ => Registration::Unknown,
    }
}

/// Starts `binary` detached from Studio, so it keeps running when Studio quits.
fn spawn_detached(binary: &Path) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(binary);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Couldn't start {}: {e}", binary.display()))?;
    // Reap the process when it exits so it doesn't linger while Studio runs.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn start_bundled() -> Result<(), String> {
    let bin = bundled_daemon()
        .ok_or_else(|| "This installation of Keyglass Studio has no openfluxd".to_string())?;
    spawn_detached(&bin)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use objc2_foundation::NSString;
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    pub struct System;

    fn agent() -> objc2::rc::Retained<SMAppService> {
        let name = NSString::from_str(LAUNCH_AGENT_PLIST);
        unsafe { SMAppService::agentServiceWithPlistName(&name) }
    }

    impl Platform for System {
        fn inspect(&self) -> Inspection {
            let status = unsafe { agent().status() };
            let registration = match status {
                SMAppServiceStatus::Enabled => Registration::Enabled,
                SMAppServiceStatus::RequiresApproval => Registration::RequiresApproval,
                SMAppServiceStatus::NotRegistered => Registration::NotRegistered,
                SMAppServiceStatus::NotFound => Registration::NotFound,
                _ => Registration::Unknown,
            };
            Inspection {
                platform: "macos",
                manager: Manager::LaunchAgent,
                registration,
                daemon_binary: bundled_daemon(),
                detail: None,
            }
        }

        fn run(&self, step: Step) -> Result<(), String> {
            match step {
                Step::RegisterLaunchAgent => unsafe { agent().registerAndReturnError() }
                    .map_err(|e| e.localizedDescription().to_string()),
                Step::KickstartLaunchAgent => {
                    let target = format!("gui/{}/{LAUNCH_AGENT_LABEL}", unsafe { libc::getuid() });
                    let out = std::process::Command::new("/bin/launchctl")
                        .args(["kickstart", &target])
                        .output()
                        .map_err(|e| format!("launchctl: {e}"))?;
                    if out.status.success() {
                        Ok(())
                    } else {
                        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
                    }
                }
                Step::StartBundledDaemon => start_bundled(),
                other => Err(format!("{other:?} is not available on macOS")),
            }
        }
    }

    pub fn open_login_items() -> Result<(), String> {
        unsafe { SMAppService::openSystemSettingsLoginItems() };
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE};
    use winreg::RegKey;

    pub struct System;

    fn read(root: winreg::HKEY) -> Option<String> {
        RegKey::predef(root)
            .open_subkey(RUN_KEY)
            .ok()?
            .get_value::<String, _>(RUN_VALUE)
            .ok()
    }

    impl Platform for System {
        fn inspect(&self) -> Inspection {
            // The setup program writes the per-user entry; a per-machine entry starts the daemon too.
            let entry = read(HKEY_CURRENT_USER).or_else(|| read(HKEY_LOCAL_MACHINE));
            let (registration, detail) = match entry {
                Some(v) if run_entry_target(&v).is_file() => (Registration::Enabled, None),
                Some(v) => (
                    Registration::NotRegistered,
                    Some(format!("The sign-in entry points at a missing file: {v}")),
                ),
                None => (Registration::NotRegistered, None),
            };
            Inspection {
                platform: "windows",
                manager: Manager::RunKey,
                registration,
                daemon_binary: bundled_daemon(),
                detail,
            }
        }

        fn run(&self, step: Step) -> Result<(), String> {
            match step {
                Step::WriteRunEntry => {
                    let bin = bundled_daemon()
                        .ok_or_else(|| "This installation has no openfluxd.exe".to_string())?;
                    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
                        .create_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
                        .map_err(|e| format!("Couldn't open the Run key: {e}"))?;
                    key.set_value(RUN_VALUE, &run_entry_value(&bin))
                        .map_err(|e| format!("Couldn't write the Run entry: {e}"))
                }
                Step::StartBundledDaemon => start_bundled(),
                other => Err(format!("{other:?} is not available on Windows")),
            }
        }
    }

    pub fn open_login_items() -> Result<(), String> {
        Err("Login items are a macOS setting".into())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::*;
    use std::process::Command;

    pub struct System;

    impl Platform for System {
        fn inspect(&self) -> Inspection {
            let out = Command::new("systemctl")
                .args([
                    "--user",
                    "show",
                    SYSTEMD_UNIT,
                    "-p",
                    "LoadState",
                    "-p",
                    "UnitFileState",
                ])
                .output();
            let (manager, registration, detail) = match out {
                Ok(o) if o.status.success() => {
                    let r = parse_systemd_show(&String::from_utf8_lossy(&o.stdout));
                    let m = if r == Registration::NotFound {
                        Manager::None
                    } else {
                        Manager::SystemdUser
                    };
                    (m, r, None)
                }
                Ok(o) => (
                    Manager::None,
                    Registration::NotFound,
                    Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                ),
                Err(_) => (Manager::None, Registration::NotFound, None),
            };
            Inspection {
                platform: if cfg!(target_os = "linux") {
                    "linux"
                } else {
                    "other"
                },
                manager,
                registration,
                daemon_binary: bundled_daemon(),
                detail,
            }
        }

        fn run(&self, step: Step) -> Result<(), String> {
            match step {
                Step::StartSystemdUnit => {
                    let out = Command::new("systemctl")
                        .args(["--user", "start", SYSTEMD_UNIT])
                        .output()
                        .map_err(|e| format!("systemctl: {e}"))?;
                    if out.status.success() {
                        Ok(())
                    } else {
                        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
                    }
                }
                Step::StartBundledDaemon => start_bundled(),
                other => Err(format!("{other:?} is not available on this platform")),
            }
        }
    }

    pub fn open_login_items() -> Result<(), String> {
        Err("Login items are a macOS setting".into())
    }
}

pub use platform::open_login_items;

/// What the interface shows about the daemon: whether it answers, and how it is registered.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub reachable: bool,
    /// Why the daemon couldn't be reached, when it couldn't.
    pub unreachable_reason: Option<String>,
    /// True when no daemon is listening at all (as opposed to one that fails to answer).
    pub not_running: bool,
    pub service: Inspection,
}

#[tauri::command]
pub async fn daemon_service_status(
    st: tauri::State<'_, crate::DesktopState>,
) -> Result<ServiceStatus, crate::daemon_api::CommandError> {
    let service = tokio::task::spawn_blocking(|| platform::System.inspect())
        .await
        .map_err(|e| crate::daemon_api::CommandError::local(e.to_string()))?;
    let (reachable, reason, not_running) = match st.ensure_conn_checked().await {
        Ok(_) => (true, None, false),
        Err(crate::ConnectError::NotRunning(s)) => (false, Some(s), true),
        Err(crate::ConnectError::Failed(s)) => (false, Some(s), false),
    };
    Ok(ServiceStatus {
        reachable,
        unreachable_reason: reason,
        not_running,
        service,
    })
}

#[tauri::command]
pub async fn daemon_service_start(
    st: tauri::State<'_, crate::DesktopState>,
    mode: Option<StartMode>,
) -> Result<StartReport, crate::daemon_api::CommandError> {
    // A daemon that starts writes its own discovery file and tokens.
    st.drop_conn().await;
    let mode = mode.unwrap_or(StartMode::Auto);
    tokio::task::spawn_blocking(move || execute(&platform::System, mode))
        .await
        .map_err(|e| crate::daemon_api::CommandError::local(e.to_string()))
}

#[tauri::command]
pub async fn open_login_items_settings() -> Result<(), crate::daemon_api::CommandError> {
    open_login_items().map_err(crate::daemon_api::CommandError::local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn insp(manager: Manager, registration: Registration) -> Inspection {
        Inspection {
            platform: "test",
            manager,
            registration,
            daemon_binary: Some(PathBuf::from("/app/openfluxd")),
            detail: None,
        }
    }

    #[test]
    fn plans_follow_the_service_manager_of_each_platform() {
        use Manager as M;
        use Registration as R;
        use StartMode::*;
        use Step::*;
        type Case = (M, R, StartMode, &'static [Step], Option<Step>);
        let cases: &[Case] = &[
            (
                M::LaunchAgent,
                R::NotRegistered,
                Auto,
                &[RegisterLaunchAgent],
                Some(StartBundledDaemon),
            ),
            (
                M::LaunchAgent,
                R::Enabled,
                Auto,
                &[KickstartLaunchAgent],
                Some(StartBundledDaemon),
            ),
            (
                M::LaunchAgent,
                R::Enabled,
                ServiceOnly,
                &[KickstartLaunchAgent],
                None,
            ),
            (M::LaunchAgent, R::RequiresApproval, Auto, &[], None),
            (
                M::LaunchAgent,
                R::NotFound,
                Auto,
                &[StartBundledDaemon],
                None,
            ),
            (M::LaunchAgent, R::NotFound, ServiceOnly, &[], None),
            (
                M::LaunchAgent,
                R::RequiresApproval,
                Direct,
                &[StartBundledDaemon],
                None,
            ),
            (M::RunKey, R::Enabled, Auto, &[StartBundledDaemon], None),
            (
                M::RunKey,
                R::NotRegistered,
                Auto,
                &[WriteRunEntry, StartBundledDaemon],
                Some(StartBundledDaemon),
            ),
            (
                M::RunKey,
                R::Enabled,
                ServiceOnly,
                &[StartBundledDaemon],
                None,
            ),
            (
                M::SystemdUser,
                R::NotRegistered,
                Auto,
                &[StartSystemdUnit],
                Some(StartBundledDaemon),
            ),
            (
                M::SystemdUser,
                R::Enabled,
                ServiceOnly,
                &[StartSystemdUnit],
                None,
            ),
            (
                M::SystemdUser,
                R::Unknown,
                Auto,
                &[StartBundledDaemon],
                None,
            ),
            (M::None, R::NotFound, Auto, &[StartBundledDaemon], None),
            (M::None, R::NotFound, ServiceOnly, &[], None),
        ];
        for (m, r, mode, steps, fallback) in cases {
            let p = plan(&insp(*m, *r), *mode);
            assert_eq!(p.steps, steps.to_vec(), "{m:?} {r:?} {mode:?}");
            assert_eq!(p.fallback, *fallback, "{m:?} {r:?} {mode:?}");
        }
    }

    struct Fake {
        before: Inspection,
        after: Inspection,
        failing: Vec<Step>,
        ran: RefCell<Vec<Step>>,
        inspected: RefCell<u32>,
    }

    impl Platform for Fake {
        fn inspect(&self) -> Inspection {
            let mut n = self.inspected.borrow_mut();
            *n += 1;
            if *n == 1 {
                self.before.clone()
            } else {
                self.after.clone()
            }
        }
        fn run(&self, step: Step) -> Result<(), String> {
            self.ran.borrow_mut().push(step);
            if self.failing.contains(&step) {
                Err(format!("{step:?} failed"))
            } else {
                Ok(())
            }
        }
    }

    fn fake(before: Inspection, failing: Vec<Step>) -> Fake {
        Fake {
            after: Inspection {
                registration: Registration::Enabled,
                ..before.clone()
            },
            before,
            failing,
            ran: RefCell::new(vec![]),
            inspected: RefCell::new(0),
        }
    }

    #[test]
    fn registering_the_launch_agent_reports_the_state_after_it() {
        let f = fake(
            insp(Manager::LaunchAgent, Registration::NotRegistered),
            vec![],
        );
        let r = execute(&f, StartMode::Auto);
        assert_eq!(*f.ran.borrow(), vec![Step::RegisterLaunchAgent]);
        assert!(r.steps.iter().all(|s| s.ok));
        assert_eq!(r.before.registration, Registration::NotRegistered);
        assert_eq!(r.after.registration, Registration::Enabled);
    }

    #[test]
    fn a_failed_service_step_falls_back_to_the_bundled_daemon_once() {
        let f = fake(
            insp(Manager::SystemdUser, Registration::NotRegistered),
            vec![Step::StartSystemdUnit],
        );
        let r = execute(&f, StartMode::Auto);
        assert_eq!(
            *f.ran.borrow(),
            vec![Step::StartSystemdUnit, Step::StartBundledDaemon]
        );
        assert_eq!(
            r.steps[0].detail.as_deref(),
            Some("StartSystemdUnit failed")
        );
        assert!(r.steps[1].ok);

        // A Run entry that can't be written still starts the daemon, but only once.
        let f = fake(
            insp(Manager::RunKey, Registration::NotRegistered),
            vec![Step::WriteRunEntry],
        );
        execute(&f, StartMode::Auto);
        assert_eq!(
            *f.ran.borrow(),
            vec![Step::WriteRunEntry, Step::StartBundledDaemon]
        );
    }

    #[test]
    fn after_an_update_only_the_service_manager_starts_the_daemon() {
        let f = fake(
            insp(Manager::LaunchAgent, Registration::Enabled),
            vec![Step::KickstartLaunchAgent],
        );
        let r = execute(&f, StartMode::ServiceOnly);
        assert_eq!(*f.ran.borrow(), vec![Step::KickstartLaunchAgent]);
        assert!(!r.steps[0].ok);
    }

    #[test]
    fn waiting_for_login_item_approval_changes_nothing() {
        let f = fake(
            insp(Manager::LaunchAgent, Registration::RequiresApproval),
            vec![],
        );
        let r = execute(&f, StartMode::Auto);
        assert!(f.ran.borrow().is_empty());
        assert!(r.steps.is_empty());
    }

    #[test]
    fn run_entries_quote_the_path_and_parse_back() {
        let p = PathBuf::from(r"C:\Users\a b\AppData\Local\OpenFlux Studio\openfluxd.exe");
        let v = run_entry_value(&p);
        assert_eq!(
            v,
            r#""C:\Users\a b\AppData\Local\OpenFlux Studio\openfluxd.exe""#
        );
        assert_eq!(run_entry_target(&v), p);
        assert_eq!(
            run_entry_target(r"C:\x\openfluxd.exe --flag"),
            PathBuf::from(r"C:\x\openfluxd.exe")
        );
    }

    #[test]
    fn systemd_unit_states_map_to_registration() {
        assert_eq!(
            parse_systemd_show("LoadState=loaded\nUnitFileState=enabled\n"),
            Registration::Enabled
        );
        assert_eq!(
            parse_systemd_show("LoadState=loaded\nUnitFileState=disabled\n"),
            Registration::NotRegistered
        );
        assert_eq!(
            parse_systemd_show("LoadState=not-found\nUnitFileState=\n"),
            Registration::NotFound
        );
        assert_eq!(
            parse_systemd_show("LoadState=masked\nUnitFileState=masked\n"),
            Registration::Unknown
        );
        assert_eq!(parse_systemd_show(""), Registration::Unknown);
    }
}
