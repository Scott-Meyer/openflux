#![allow(dead_code)]

use std::collections::BTreeMap;

use openflux_update::compat::{Compat, HostProfile, InterfaceSet};
use openflux_update::engine::{Catalog, InstalledSoftware, Inventory};
use openflux_update::manifest::{Artifact, Channel, Release};
use openflux_update::version::{Component, InterfaceVersion, Requirement};
use time::macros::datetime;

/// A registry shaped like the real one (`tests/fixtures/compat.toml`), frozen
/// so these tests describe the model rather than whatever versions the
/// repository is at.
pub const REGISTRY: &str = include_str!("../fixtures/compat.toml");

pub fn registry() -> Compat {
    Compat::parse(REGISTRY).expect("test registry is valid")
}

pub fn profile() -> HostProfile {
    registry().host_profile()
}

pub fn iv(s: &str) -> InterfaceVersion {
    s.parse().unwrap()
}

/// `"1.2"` is shorthand for `>=1.2, <2.0`.
pub fn req(s: &str) -> Requirement {
    let v = iv(s);
    Requirement::new(v.major, v.minor)
}

pub fn ver(s: &str) -> semver::Version {
    s.parse().unwrap()
}

/// Interface set from `(name, versions)` lists; requirement versions use the
/// `"1.2"` shorthand.
pub fn set(provides: &[(&str, &[&str])], requires: &[(&str, &[&str])]) -> InterfaceSet {
    InterfaceSet {
        provides: provides
            .iter()
            .map(|(n, v)| (n.to_string(), v.iter().map(|s| iv(s)).collect()))
            .collect(),
        requires: requires
            .iter()
            .map(|(n, v)| (n.to_string(), v.iter().map(|s| req(s)).collect()))
            .collect(),
        built_with: BTreeMap::new(),
    }
}

pub fn with_built(mut s: InterfaceSet, built: &[(&str, &str)]) -> InterfaceSet {
    s.built_with = built.iter().map(|(n, v)| (n.to_string(), iv(v))).collect();
    s
}

pub fn fw_set() -> InterfaceSet {
    set(
        &[
            ("firmware-usb", &["1.0"]),
            ("uart", &["1.0"]),
            ("keymap", &["1.0"]),
        ],
        &[],
    )
}

pub fn br_set() -> InterfaceSet {
    set(
        &[("bridge-usb", &["1.0"]), ("display-ipc", &["1.0"])],
        &[("uart", &["1.0"])],
    )
}

pub fn dp_set() -> InterfaceSet {
    set(&[("config", &["1.0"])], &[("display-ipc", &["1.0"])])
}

pub fn host_set() -> InterfaceSet {
    profile().interfaces
}

pub fn openflux(v: &str, interfaces: InterfaceSet) -> InstalledSoftware {
    InstalledSoftware::OpenFlux {
        version: ver(v),
        interfaces,
    }
}

/// Everything at 1.0.0 with the 1.0 interfaces.
pub fn baseline() -> Inventory {
    Inventory {
        firmware: openflux("1.0.0", fw_set()),
        bridge: openflux("1.0.0", br_set()),
        display: openflux("1.0.0", dp_set()),
    }
}

pub fn release(component: Component, version: &str, interfaces: InterfaceSet) -> Release {
    let version = ver(version);
    let name = format!("openflux-{component}-{version}.bin");
    Release {
        schema: 1,
        component,
        channel: Channel::of(&version),
        released: datetime!(2030-01-01 00:00 UTC),
        notes_url: format!("https://example.invalid/{component}/{version}"),
        interfaces,
        artifacts: vec![Artifact {
            url: format!("https://example.invalid/{name}"),
            name,
            sha256: "0".repeat(64),
            size: 1,
            signatures: Vec::new(),
        }],
        withdrawn: false,
        version,
    }
}

pub fn catalog(releases: Vec<Release>) -> Catalog {
    Catalog::from_trusted_releases(releases).expect("test releases merge")
}
