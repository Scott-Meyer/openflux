//! Converting a keyboard to OpenFlux, and resetting it to its factory software.
//!
//! This is the only place that recognizes keyboard software other than OpenFlux 1.0 or later.
//! Everything else in the daemon assumes the components report OpenFlux versions; a keyboard
//! classified as anything else is offered conversion instead of normal operation.
//!
//! - [`jobs`]: the conversion and reset job steps.
//! - [`factory`]: reading the pinned factory files out of their published container.
//! - [`ipk`]: reading files out of an opkg package.
//! - [`keymap`]: translating keymaps written by other keyboard software.
//! - [`press_refinement`]: obtaining the keyboard maker's press-refinement data from its published
//!   software and installing it on the keyboard.

use serde::Serialize;

pub mod factory;
pub mod ipk;
pub mod jobs;
pub mod keymap;
pub mod press_refinement;

/// What a component runs, in the terms of the update engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// An OpenFlux release, `openflux-<component> <semver>`.
    OpenFlux(semver::Version),
    /// An OpenFlux development build.
    DevelopmentBuild(String),
    /// The keyboard's factory software.
    Factory(String),
    /// Nothing reported, or something unreadable.
    Unknown(String),
}

/// Identifies a component from the version string it reports.
pub fn identify(component: openflux_update::Component, reported: Option<&str>) -> Identity {
    match classify_component(component.as_str(), reported) {
        ComponentClass::OpenFlux { version } => match version.parse::<semver::Version>() {
            Ok(v) => Identity::OpenFlux(v),
            Err(_) => Identity::Unknown(format!("openflux-{component} {version}")),
        },
        ComponentClass::PreReleaseOpenFlux { reported } => Identity::DevelopmentBuild(reported),
        ComponentClass::Other { reported } => Identity::Factory(reported),
        ComponentClass::Unknown => Identity::Unknown(String::new()),
    }
}

/// What a component's reported version string says about the software it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ComponentClass {
    /// `openflux-<component> <version>`.
    OpenFlux { version: String },
    /// An OpenFlux development build: a bare version ending in `-open`, such as `1.4.11-open`.
    PreReleaseOpenFlux { reported: String },
    /// Software that isn't OpenFlux: offer conversion.
    Other { reported: String },
    /// The component didn't report a version.
    Unknown,
}

impl ComponentClass {
    pub fn is_openflux(&self) -> bool {
        matches!(self, ComponentClass::OpenFlux { .. })
    }
}

/// Classifies the version string a component reports. `component` is the OpenFlux component name
/// (`firmware`, `bridge`, `display`).
pub fn classify_component(component: &str, reported: Option<&str>) -> ComponentClass {
    let Some(raw) = reported.map(str::trim).filter(|s| !s.is_empty()) else {
        return ComponentClass::Unknown;
    };
    let prefix = format!("openflux-{component} ");
    if let Some(version) = raw.strip_prefix(&prefix) {
        let version = version.trim();
        if !version.is_empty() {
            return ComponentClass::OpenFlux {
                version: version.to_string(),
            };
        }
    }
    if raw.ends_with("-open") {
        return ComponentClass::PreReleaseOpenFlux {
            reported: raw.to_string(),
        };
    }
    ComponentClass::Other {
        reported: raw.to_string(),
    }
}

/// Classification of every component in a `versions` object (`firmware`, `bridge`, `display`).
pub fn classify_versions(versions: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    let mut all_openflux = true;
    for component in ["firmware", "bridge", "display"] {
        let class = classify_component(component, versions.get(component).and_then(|v| v.as_str()));
        all_openflux &= class.is_openflux();
        out.insert(
            component.into(),
            serde_json::to_value(class).unwrap_or(serde_json::Value::Null),
        );
    }
    out.insert("conversionNeeded".into(), serde_json::json!(!all_openflux));
    serde_json::Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_openflux_prerelease_other_and_missing_versions() {
        assert_eq!(
            classify_component("bridge", Some("openflux-bridge 1.0.0")),
            ComponentClass::OpenFlux {
                version: "1.0.0".into()
            }
        );
        assert_eq!(
            classify_component("display", Some("1.4.12.D-open")),
            ComponentClass::PreReleaseOpenFlux {
                reported: "1.4.12.D-open".into()
            }
        );
        assert_eq!(
            classify_component("bridge", Some("openflux-display 1.0.0")),
            ComponentClass::Other {
                reported: "openflux-display 1.0.0".into()
            },
            "a version naming another component is not this component"
        );
        assert_eq!(
            classify_component("firmware", Some("1.4.12")),
            ComponentClass::Other {
                reported: "1.4.12".into()
            }
        );
        assert_eq!(
            classify_component("firmware", None),
            ComponentClass::Unknown
        );
        assert_eq!(
            classify_component("firmware", Some("  ")),
            ComponentClass::Unknown
        );
    }

    #[test]
    fn conversion_is_needed_unless_every_component_is_openflux() {
        let all = serde_json::json!({
            "firmware": "openflux-firmware 1.0.0",
            "bridge": "openflux-bridge 1.0.0",
            "display": "openflux-display 1.0.0",
            "os": "2.1",
        });
        assert_eq!(classify_versions(&all)["conversionNeeded"], false);
        let mixed = serde_json::json!({"firmware": "openflux-firmware 1.0.0", "bridge": "1.4.11-open", "display": null});
        let c = classify_versions(&mixed);
        assert_eq!(c["conversionNeeded"], true);
        assert_eq!(c["bridge"]["kind"], "preReleaseOpenFlux");
        assert_eq!(c["display"]["kind"], "unknown");
    }
}
