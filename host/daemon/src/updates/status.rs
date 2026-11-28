//! What the keyboard runs, what can be updated, and which areas this Keyglass may write.

use crate::conversion::Identity;
use keyglass_update::compat::{Compat, HostProfile, InterfaceKind, InterfaceSet};
use keyglass_update::engine::{
    ComponentState, InstalledSoftware, Inventory, KeyboardMode, Refusal,
};
use keyglass_update::{Catalog, Component, Engine, PlanRequest, Preferences};
use serde::Serialize;

/// One keyboard component as the daemon identified it.
#[derive(Debug, Clone)]
pub struct Identified {
    pub component: Component,
    pub reported: Option<String>,
    pub identity: Identity,
    /// Interfaces of an Keyglass version, from the verified indexes or, for the version this
    /// host was built with (or a same-major custom/local Keyglass build), from its own registry.
    /// `None` when neither knows the version or the major version differs.
    pub interfaces: Option<InterfaceSet>,
    /// True when the component reports an Keyglass version not listed in the release catalog
    /// (e.g. a local developer build or community fork) and is using the host's compiled
    /// same-major interface compatibility table.
    pub custom_build: bool,
}

impl Identified {
    pub fn kind(&self) -> &'static str {
        match self.identity {
            Identity::Keyglass(_) => "keyglass",
            Identity::DevelopmentBuild(_) => "preRelease",
            Identity::Factory(_) => "factory",
            Identity::Unknown(_) => "unknown",
        }
    }

    fn installed_software(&self) -> InstalledSoftware {
        match (&self.identity, &self.interfaces) {
            (Identity::Keyglass(v), Some(i)) => InstalledSoftware::Keyglass {
                version: v.clone(),
                interfaces: i.clone(),
            },
            (Identity::Keyglass(v), None) => InstalledSoftware::Unknown {
                label: format!("keyglass-{} {v}", self.component),
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
    Keyglass,
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
                let (interfaces, custom_build) = match &identity {
                    Identity::Keyglass(v) => {
                        if let Some(r) = catalog.find(c, v) {
                            (Some(r.interfaces.clone()), false)
                        } else if let Some(d) = compat.components.get(&c) {
                            if &d.version == v {
                                (Some(compat.interface_set(c)), false)
                            } else if d.version.major == v.major {
                                (Some(compat.interface_set(c)), true)
                            } else {
                                (None, false)
                            }
                        } else {
                            (None, false)
                        }
                    }
                    _ => (None, false),
                };
                Identified {
                    component: c,
                    reported,
                    identity,
                    interfaces,
                    custom_build,
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
            .all(|c| matches!(c.identity, Identity::Keyglass(_)))
        {
            ViewMode::Keyglass
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

    /// Keyglass components whose version neither index lists.
    pub fn undescribed(&self) -> Vec<&Identified> {
        self.components
            .iter()
            .filter(|c| matches!(c.identity, Identity::Keyglass(_)) && c.interfaces.is_none())
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
    /// The keyboard stores a newer minor version of this data than this Keyglass understands.
    ReadOnly,
    /// The keyboard uses a major version this Keyglass doesn't support, or doesn't run Keyglass.
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
        (Identity::Keyglass(_), Some(set)) => {
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
        (Identity::Keyglass(_), None) => AreaState::Unknown,
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
            "The installed {c} doesn't fully work with the rest of Keyglass; install this update"
        )),
        ComponentState::NeedsNewerHost { to, host } => Some(format!(
            "{c} {to} needs Keyglass {host} or later; update Keyglass first"
        )),
        ComponentState::NewerThanHost { gaps } => Some(format!(
            "The keyboard's {} is newer than this Keyglass understands; update Keyglass",
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
            Some("The keyboard isn't fully running Keyglass; convert it to Keyglass".into())
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
) -> Option<&keyglass_update::Release> {
    catalog
        .releases()
        .iter()
        .filter(|r| {
            r.component == c
                && !r.withdrawn
                && (prefs.beta || r.channel == keyglass_update::Channel::Stable)
        })
        .max_by(|a, b| a.version.cmp(&b.version))
}

pub(crate) fn plan_json(engine: &Engine<'_>, request: &PlanRequest) -> serde_json::Value {
    match engine.plan(request) {
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
                Identity::Keyglass(v) => Some(v.to_string()),
                _ => None,
            };
            let (state, reason, required) = if mode == ViewMode::Disconnected {
                (
                    "unknown",
                    Some("The keyboard isn't connected".to_string()),
                    false,
                )
            } else if mode == ViewMode::Keyglass && c.interfaces.is_none() {
                (
                    "unknown",
                    Some(format!(
                        "{} {} isn't listed in the release index; check for updates",
                        c.component,
                        installed.clone().unwrap_or_default()
                    )),
                    false,
                )
            } else if mode == ViewMode::Keyglass && !undescribed.is_empty() {
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
                } else if c.custom_build && matches!(st.state, ComponentState::UpToDate) {
                    reason.get_or_insert_with(|| {
                        "Custom or local Keyglass build (not listed in the official release index)".into()
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
                "customBuild": c.custom_build,
                "latest": latest.map(|r| r.version.to_string()),
                "state": state,
                "required": required,
                "reason": reason,
                "notesUrl": latest.map(|r| r.notes_url.clone()),
            })
        })
        .collect();

    let plan = if mode == ViewMode::Keyglass && undescribed.is_empty() {
        plan_json(&engine, &PlanRequest::keyboard())
    } else {
        serde_json::Value::Null
    };

    let conversion = if mode == ViewMode::NeedsConversion && engine.mode() != KeyboardMode::Keyglass
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
            ViewMode::Keyglass => "keyglass",
            ViewMode::NeedsConversion => "needsConversion",
        },
        "components": components,
        "plan": plan,
        "conversion": conversion,
    });
    (host, keyboard)
}
