//! What the keyboard runs, what can be updated, and which areas this OpenFlux may write.

use crate::conversion::Identity;
use openflux_update::compat::{Compat, HostProfile, InterfaceKind, InterfaceSet};
use openflux_update::engine::{
    ComponentState, InstalledSoftware, Inventory, KeyboardMode, Refusal,
};
use openflux_update::{Catalog, Component, Engine, PlanRequest, Preferences};
use serde::Serialize;

/// One keyboard component as the daemon identified it.
#[derive(Debug, Clone)]
pub struct Identified {
    pub component: Component,
    pub reported: Option<String>,
    pub identity: Identity,
    /// Interfaces of an OpenFlux version, from the verified indexes or, for the version this
    /// host was built with, from its own registry. `None` when neither knows the version.
    pub interfaces: Option<InterfaceSet>,
}

impl Identified {
    pub fn kind(&self) -> &'static str {
        match self.identity {
            Identity::OpenFlux(_) => "openflux",
            Identity::DevelopmentBuild(_) => "preRelease",
            Identity::Factory(_) => "factory",
            Identity::Unknown(_) => "unknown",
        }
    }

    fn installed_software(&self) -> InstalledSoftware {
        match (&self.identity, &self.interfaces) {
            (Identity::OpenFlux(v), Some(i)) => InstalledSoftware::OpenFlux {
                version: v.clone(),
                interfaces: i.clone(),
            },
            (Identity::OpenFlux(v), None) => InstalledSoftware::Unknown {
                label: format!("openflux-{} {v}", self.component),
            },
            (Identity::DevelopmentBuild(l), _) => {
                InstalledSoftware::DevelopmentBuild { label: l.clone() }
            }
            (Identity::Factory(l), _) => InstalledSoftware::Factory { label: l.clone() },
            (Identity::Unknown(l), _) => InstalledSoftware::Unknown { label: l.clone() },
        }
    }
}

/// The keyboard as last observed.
#[derive(Debug, Clone)]
pub struct KeyboardView {
    pub bridge_connected: bool,
    pub firmware_connected: bool,
    pub components: Vec<Identified>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Disconnected,
    OpenFlux,
    NeedsConversion,
}

impl KeyboardView {
    pub fn identify(
        connected: (bool, bool),
        versions: &serde_json::Value,
        catalog: &Catalog,
        compat: &Compat,
    ) -> Self {
        let components = Component::DEVICE
            .into_iter()
            .map(|c| {
                let reported = versions
                    .get(c.as_str())
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let identity = crate::conversion::identify(c, reported.as_deref());
                let interfaces = match &identity {
                    Identity::OpenFlux(v) => catalog
                        .find(c, v)
                        .map(|r| r.interfaces.clone())
                        .or_else(|| {
                            compat
                                .components
                                .get(&c)
                                .filter(|d| &d.version == v)
                                .map(|_| compat.interface_set(c))
                        }),
                    _ => None,
                };
                Identified {
                    component: c,
                    reported,
                    identity,
                    interfaces,
                }
            })
            .collect();
        Self {
            bridge_connected: connected.0,
            firmware_connected: connected.1,
            components,
        }
    }

    pub fn connected(&self) -> bool {
        self.bridge_connected || self.firmware_connected
    }

    /// Disconnected unless both the bridge and the firmware answer.
    pub fn mode(&self) -> ViewMode {
        if !(self.bridge_connected && self.firmware_connected) {
            return ViewMode::Disconnected;
        }
        if self
            .components
            .iter()
            .all(|c| matches!(c.identity, Identity::OpenFlux(_)))
        {
            ViewMode::OpenFlux
        } else {
            ViewMode::NeedsConversion
        }
    }

    pub fn get(&self, c: Component) -> Option<&Identified> {
        self.components.iter().find(|x| x.component == c)
    }

    /// The engine's inventory; components that can't be described are `Unknown`.
    pub fn inventory(&self) -> Inventory {
        let sw = |c| {
            self.get(c)
                .map(Identified::installed_software)
                .unwrap_or(InstalledSoftware::Unknown {
                    label: String::new(),
                })
        };
        Inventory {
            firmware: sw(Component::Firmware),
            bridge: sw(Component::Bridge),
            display: sw(Component::Display),
        }
    }

    /// OpenFlux components whose version neither index lists.
    pub fn undescribed(&self) -> Vec<&Identified> {
        self.components
            .iter()
            .filter(|c| matches!(c.identity, Identity::OpenFlux(_)) && c.interfaces.is_none())
            .collect()
    }
}

/// A keyboard area with its own compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Area {
    Config,
    Keymap,
    ProfileSettings,
    Calibration,
    BridgeUsb,
    FirmwareUsb,
}

impl Area {
    pub const ALL: [Area; 6] = [
        Area::Config,
        Area::Keymap,
        Area::ProfileSettings,
        Area::Calibration,
        Area::BridgeUsb,
        Area::FirmwareUsb,
    ];

    pub fn interface(self) -> &'static str {
        match self {
            Area::Config => "config",
            Area::Keymap => "keymap",
            Area::ProfileSettings => "profile-settings",
            Area::Calibration => "calibration-lfs",
            Area::BridgeUsb => "bridge-usb",
            Area::FirmwareUsb => "firmware-usb",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Area::Config => "config",
            Area::Keymap => "keymap",
            Area::ProfileSettings => "profileSettings",
            Area::Calibration => "calibration",
            Area::BridgeUsb => "bridgeUsb",
            Area::FirmwareUsb => "firmwareUsb",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Area::Config => "the display configuration",
            Area::Keymap => "the keymap",
            Area::ProfileSettings => "the profile settings",
            Area::Calibration => "the calibration",
            Area::BridgeUsb => "the keyboard bridge connection",
            Area::FirmwareUsb => "the keyboard firmware connection",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AreaState {
    Ok,
    /// The keyboard stores a newer minor version of this data than this OpenFlux understands.
    ReadOnly,
    /// The keyboard uses a major version this OpenFlux doesn't support, or doesn't run OpenFlux.
    Unsupported,
    /// The keyboard's software isn't known well enough to tell.
    Unknown,
}

pub fn area_state(area: Area, view: &KeyboardView, profile: &HostProfile) -> AreaState {
    let iface = area.interface();
    let Some(provider) = profile.providers.get(iface).copied() else {
        return AreaState::Ok;
    };
    let Some(comp) = view.get(provider) else {
        return AreaState::Unknown;
    };
    match (&comp.identity, &comp.interfaces) {
        (Identity::OpenFlux(_), Some(set)) => {
            let reqs = profile.interfaces.required(iface);
            if reqs.is_empty() {
                return AreaState::Ok;
            }
            let Some(v) = set
                .provided(iface)
                .iter()
                .filter(|v| reqs.iter().any(|r| r.accepts(**v)))
                .max()
                .copied()
            else {
                return AreaState::Unsupported;
            };
            let data = profile.kinds.get(iface) == Some(&InterfaceKind::Data);
            let newer = profile
                .interfaces
                .understands(iface)
                .is_some_and(|u| u.major == v.major && v.minor > u.minor);
            if data && newer {
                AreaState::ReadOnly
            } else {
                AreaState::Ok
            }
        }
        (Identity::OpenFlux(_), None) => AreaState::Unknown,
        (Identity::Unknown(_), _) if comp.reported.is_none() => AreaState::Unknown,
        _ => AreaState::Unsupported,
    }
}

pub fn compatibility_json(view: &KeyboardView, profile: &HostProfile) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for a in Area::ALL {
        out.insert(
            a.key().into(),
            serde_json::to_value(area_state(a, view, profile)).unwrap_or_default(),
        );
    }
    serde_json::Value::Object(out)
}

fn state_name(s: &ComponentState) -> &'static str {
    match s {
        ComponentState::UpToDate => "upToDate",
        ComponentState::UpdateAvailable { .. } => "updateAvailable",
        ComponentState::NeedsNewerHost { .. } => "needsNewerHost",
        ComponentState::NewerThanHost { .. } => "newerThanHost",
        ComponentState::UpdateBlocked { .. } => "blocked",
        ComponentState::Incompatible { .. } => "incompatible",
        ComponentState::ConversionRequired { .. } => "conversionRequired",
    }
}

fn state_reason(c: Component, s: &ComponentState) -> Option<String> {
    match s {
        ComponentState::UpToDate
        | ComponentState::UpdateAvailable {
            required: false, ..
        } => None,
        ComponentState::UpdateAvailable { required: true, .. } => Some(format!(
            "The installed {c} doesn't fully work with the rest of OpenFlux; install this update"
        )),
        ComponentState::NeedsNewerHost { to, host } => Some(format!(
            "{c} {to} needs OpenFlux {host} or later; update OpenFlux first"
        )),
        ComponentState::NewerThanHost { gaps } => Some(format!(
            "The keyboard's {} is newer than this OpenFlux understands; update OpenFlux",
            gaps.iter()
                .map(|g| g.interface.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        ComponentState::UpdateBlocked { refusal, .. } => Some(capitalize(&refusal.to_string())),
        ComponentState::Incompatible { unmet } => Some(capitalize(
            &unmet
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join("; "),
        )),
        ComponentState::ConversionRequired { .. } => {
            Some("The keyboard isn't fully running OpenFlux; convert it to OpenFlux".into())
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn newest(
    catalog: &Catalog,
    c: Component,
    prefs: Preferences,
) -> Option<&openflux_update::Release> {
    catalog
        .releases()
        .iter()
        .filter(|r| {
            r.component == c
                && !r.withdrawn
                && (prefs.beta || r.channel == openflux_update::Channel::Stable)
        })
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// The `host` and `keyboard` blocks of the status body.
pub fn status_blocks(
    view: &KeyboardView,
    catalog: &Catalog,
    profile: &HostProfile,
    prefs: Preferences,
) -> (serde_json::Value, serde_json::Value) {
    let inventory = view.inventory();
    let engine = Engine::new(profile, &inventory, catalog, prefs);

    let host_status = engine.component_status(Component::Host);
    let host_latest = newest(catalog, Component::Host, prefs);
    let host = serde_json::json!({
        "installed": profile.version.to_string(),
        "latest": host_latest.map(|r| r.version.to_string()),
        "state": match host_status.state {
            ComponentState::UpToDate => "upToDate",
            ComponentState::UpdateAvailable { .. } => "updateAvailable",
            _ => "blocked",
        },
        "reason": match &host_status.state {
            ComponentState::UpToDate | ComponentState::UpdateAvailable { .. } => None,
            s => state_reason(Component::Host, s),
        },
        "notesUrl": host_latest.map(|r| r.notes_url.clone()),
    });

    let mode = view.mode();
    let undescribed = view.undescribed();
    let components: Vec<serde_json::Value> = view
        .components
        .iter()
        .map(|c| {
            let latest = newest(catalog, c.component, prefs);
            let installed = match &c.identity {
                Identity::OpenFlux(v) => Some(v.to_string()),
                _ => None,
            };
            let (state, reason, required) = if mode == ViewMode::Disconnected {
                (
                    "unknown",
                    Some("The keyboard isn't connected".to_string()),
                    false,
                )
            } else if mode == ViewMode::OpenFlux && c.interfaces.is_none() {
                (
                    "unknown",
                    Some(format!(
                        "{} {} isn't listed in the release index; check for updates",
                        c.component,
                        installed.clone().unwrap_or_default()
                    )),
                    false,
                )
            } else if mode == ViewMode::OpenFlux && !undescribed.is_empty() {
                (
                    "unknown",
                    Some(
                        "Another keyboard component isn't listed in the release index".to_string(),
                    ),
                    false,
                )
            } else {
                let st = engine.component_status(c.component);
                let mut reason = state_reason(c.component, &st.state);
                if st.installed_withdrawn {
                    reason.get_or_insert_with(|| {
                        "The installed version was withdrawn by its publisher".into()
                    });
                }
                let required = matches!(
                    st.state,
                    ComponentState::UpdateAvailable { required: true, .. }
                );
                (state_name(&st.state), reason, required)
            };
            serde_json::json!({
                "component": c.component.as_str(),
                "installed": installed,
                "reported": c.reported,
                "kind": c.kind(),
                "latest": latest.map(|r| r.version.to_string()),
                "state": state,
                "required": required,
                "reason": reason,
                "notesUrl": latest.map(|r| r.notes_url.clone()),
            })
        })
        .collect();

    let plan = if mode == ViewMode::OpenFlux && undescribed.is_empty() {
        match engine.plan(&PlanRequest::keyboard()) {
            Ok(plan) => serde_json::json!({
                "steps": plan.steps.iter().map(|s| serde_json::json!({
                    "component": s.component.as_str(),
                    "from": s.from.to_string(),
                    "to": s.to.to_string(),
                    "reason": s.reason,
                    "pauses": s.temporarily_unmet.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "reason": serde_json::Value::Null,
            }),
            Err(Refusal::NothingToUpdate) => serde_json::Value::Null,
            Err(r) => serde_json::json!({"steps": [], "reason": capitalize(&r.to_string())}),
        }
    } else {
        serde_json::Value::Null
    };

    let conversion = if mode == ViewMode::NeedsConversion && engine.mode() != KeyboardMode::OpenFlux
    {
        let found: Vec<serde_json::Value> = components.clone();
        match engine.propose_conversion() {
            Ok(p) => serde_json::json!({
                "found": found,
                "install": p.install.iter().map(|r| serde_json::json!({"component": r.component.as_str(), "version": r.version.to_string()})).collect::<Vec<_>>(),
                "reason": serde_json::Value::Null,
            }),
            Err(r) => {
                serde_json::json!({"found": found, "install": [], "reason": capitalize(&r.to_string())})
            }
        }
    } else {
        serde_json::Value::Null
    };

    let keyboard = serde_json::json!({
        "connected": view.connected(),
        "mode": match mode {
            ViewMode::Disconnected => "disconnected",
            ViewMode::OpenFlux => "openflux",
            ViewMode::NeedsConversion => "needsConversion",
        },
        "components": components,
        "plan": plan,
        "conversion": conversion,
    });
    (host, keyboard)
}
