//! The interface registry (`compat.toml`) and the interface sets derived from it.
//!
//! `compat.toml` is the single source of truth for interface versions. Each
//! interface has exactly one providing component. A component provides its
//! interfaces at the registry's version and states the ranges it requires
//! from other components. A release manifest copies these declarations for
//! the component being released, and the host derives its [`HostProfile`]
//! from the registry it was built with.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::version::{satisfies, Component, InterfaceVersion, Requirement};

/// The interfaces a component provides and the ranges it requires.
///
/// Keys are interface names. Names are kept as strings so a newer component
/// can describe interfaces this build has never heard of.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InterfaceSet {
    #[serde(default)]
    pub provides: BTreeMap<String, Vec<InterfaceVersion>>,
    #[serde(default)]
    pub requires: BTreeMap<String, Vec<Requirement>>,
    /// For each required interface, the registry version this component was
    /// built against. A consumer understands every addition up to that minor.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub built_with: BTreeMap<String, InterfaceVersion>,
}

impl InterfaceSet {
    pub fn provided(&self, interface: &str) -> &[InterfaceVersion] {
        self.provides
            .get(interface)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn required(&self, interface: &str) -> &[Requirement] {
        self.requires
            .get(interface)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The newest version of a required interface this component understands:
    /// the version it was built with, or the minimum it requires when that is
    /// not recorded.
    pub fn understands(&self, interface: &str) -> Option<InterfaceVersion> {
        self.built_with
            .get(interface)
            .copied()
            .or_else(|| self.required(interface).iter().map(|r| r.minimum()).max())
    }
}

/// How a newer minor of an interface affects an older consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceKind {
    /// A command protocol. An older consumer keeps working with a newer minor;
    /// it simply does not use the additions.
    Protocol,
    /// A stored data format. An older consumer can read a newer minor but
    /// would drop the additions if it wrote the data back, so it treats that
    /// area as read-only.
    Data,
}

/// One interface in the registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDef {
    pub version: InterfaceVersion,
    pub kind: InterfaceKind,
    pub summary: String,
}

/// A requirement written as one range or a list of ranges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Ranges {
    One(Requirement),
    Many(Vec<Requirement>),
}

impl<'de> Deserialize<'de> for Ranges {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Ranges;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a range such as \">=1.0, <2.0\" or a list of ranges")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Ranges, E> {
                v.parse().map(Ranges::One).map_err(E::custom)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Ranges, A::Error> {
                let mut out = Vec::new();
                while let Some(r) = seq.next_element::<Requirement>()? {
                    out.push(r);
                }
                Ok(Ranges::Many(out))
            }
        }
        d.deserialize_any(V)
    }
}

impl Ranges {
    pub fn to_vec(&self) -> Vec<Requirement> {
        match self {
            Ranges::One(r) => vec![*r],
            Ranges::Many(v) => v.clone(),
        }
    }
}

/// One component in the registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ComponentDef {
    /// The component's own version, kept in step with its releases.
    pub version: semver::Version,
    /// Interfaces this component provides, at the registry's version.
    #[serde(default)]
    pub provides: Vec<String>,
    /// Older majors this component keeps providing alongside the registry's
    /// version, so that consumers can move to a new major one at a time.
    #[serde(default)]
    pub also_provides: BTreeMap<String, Vec<InterfaceVersion>>,
    #[serde(default)]
    pub requires: BTreeMap<String, Ranges>,
    /// Interfaces the host uses to install and recover this component.
    /// Empty for the host, which updates itself.
    #[serde(default)]
    pub update_transport: Vec<String>,
}

/// The parsed and validated contents of `compat.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    pub schema: u32,
    pub interfaces: BTreeMap<String, InterfaceDef>,
    pub components: BTreeMap<Component, ComponentDef>,
}

/// Problems found while validating `compat.toml`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CompatError {
    #[error("compat.toml is not valid TOML for the registry: {0}")]
    Syntax(String),
    #[error("compat.toml is inconsistent:\n  {}", .0.join("\n  "))]
    Invalid(Vec<String>),
}

impl Compat {
    pub const SCHEMA: u32 = 1;

    /// Parse and validate the registry.
    pub fn parse(text: &str) -> Result<Self, CompatError> {
        let compat: Compat =
            toml::from_str(text).map_err(|e| CompatError::Syntax(e.to_string()))?;
        compat.validate()?;
        Ok(compat)
    }

    /// Check the registry for internal consistency.
    ///
    /// Every interface has one provider and at least one consumer, every
    /// requirement names a registered interface and is satisfied by the
    /// provider at the registry's version, and every keyboard component has an
    /// update transport the host can speak.
    pub fn validate(&self) -> Result<(), CompatError> {
        let mut problems = Vec::new();
        if self.schema != Self::SCHEMA {
            problems.push(format!(
                "schema is {}, this tool understands {}",
                self.schema,
                Self::SCHEMA
            ));
        }
        for c in Component::ALL {
            if !self.components.contains_key(&c) {
                problems.push(format!("component {c} is missing"));
            }
        }
        for name in self.interfaces.keys() {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                problems.push(format!(
                    "interface name {name:?} must be lowercase letters, digits and '-'"
                ));
            }
        }

        let mut providers: BTreeMap<&str, Vec<Component>> = BTreeMap::new();
        let mut consumers: BTreeMap<&str, Vec<Component>> = BTreeMap::new();
        for (c, def) in &self.components {
            let mut seen = BTreeSet::new();
            for name in &def.provides {
                if !seen.insert(name) {
                    problems.push(format!("{c} lists {name} twice in provides"));
                }
                if !self.interfaces.contains_key(name) {
                    problems.push(format!("{c} provides unregistered interface {name}"));
                }
                providers.entry(name).or_default().push(*c);
            }
            for (name, versions) in &def.also_provides {
                match self.interfaces.get(name) {
                    None => {
                        problems.push(format!("{c} also-provides unregistered interface {name}"))
                    }
                    Some(i) => {
                        if !def.provides.contains(name) {
                            problems
                                .push(format!("{c} also-provides {name} but does not provide it"));
                        }
                        for v in versions {
                            if v.major >= i.version.major {
                                problems.push(format!(
                                    "{c} also-provides {name} {v}; also-provides is for older majors than the registry's {}",
                                    i.version
                                ));
                            }
                        }
                    }
                }
            }
            for name in def.requires.keys() {
                if !self.interfaces.contains_key(name) {
                    problems.push(format!("{c} requires unregistered interface {name}"));
                }
                if def.provides.contains(name) {
                    problems.push(format!("{c} both provides and requires {name}"));
                }
                consumers.entry(name).or_default().push(*c);
            }
            if *c == Component::Host && !def.update_transport.is_empty() {
                problems.push("host updates itself and has no update-transport".to_string());
            }
            if *c != Component::Host && def.update_transport.is_empty() {
                problems.push(format!("{c} needs an update-transport"));
            }
        }

        for name in self.interfaces.keys() {
            match providers.get(name.as_str()).map(Vec::as_slice) {
                None | Some([]) => problems.push(format!("interface {name} has no provider")),
                Some([_]) => {}
                Some(many) => problems.push(format!(
                    "interface {name} has several providers: {}",
                    many.iter()
                        .map(|c| c.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
            if !consumers.contains_key(name.as_str()) {
                problems.push(format!("interface {name} has no consumer"));
            }
        }

        for (c, def) in &self.components {
            for (name, ranges) in &def.requires {
                let ranges = ranges.to_vec();
                if ranges.is_empty() {
                    problems.push(format!("{c} requires {name} with an empty list of ranges"));
                    continue;
                }
                let Some(provider) = providers.get(name.as_str()).and_then(|p| p.first()) else {
                    continue;
                };
                let provided = self.provided_versions(*provider, name);
                if !satisfies(&provided, &ranges) {
                    problems.push(format!(
                        "{c} requires {name} {} but {provider} provides {}",
                        join(&ranges),
                        join(&provided)
                    ));
                }
            }
        }

        if let Some(host) = self.components.get(&Component::Host) {
            for (c, def) in &self.components {
                for name in &def.update_transport {
                    let Some(provider) = providers.get(name.as_str()).and_then(|p| p.first())
                    else {
                        problems.push(format!("{c} update-transport {name} has no provider"));
                        continue;
                    };
                    if *provider == Component::Host {
                        problems.push(format!(
                            "{c} update-transport {name} must be provided by a keyboard component"
                        ));
                    }
                    if !host.requires.contains_key(name) {
                        problems.push(format!(
                            "{c} update-transport {name} is not required by the host"
                        ));
                    }
                }
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(CompatError::Invalid(problems))
        }
    }

    fn provided_versions(&self, c: Component, interface: &str) -> Vec<InterfaceVersion> {
        let Some(def) = self.components.get(&c) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if def.provides.iter().any(|n| n == interface) {
            out.extend(
                def.also_provides
                    .get(interface)
                    .into_iter()
                    .flatten()
                    .copied(),
            );
            if let Some(i) = self.interfaces.get(interface) {
                out.push(i.version);
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// The interface declarations a release of `component` carries.
    pub fn interface_set(&self, component: Component) -> InterfaceSet {
        let mut set = InterfaceSet::default();
        if let Some(def) = self.components.get(&component) {
            for name in &def.provides {
                set.provides
                    .insert(name.clone(), self.provided_versions(component, name));
            }
            for (name, ranges) in &def.requires {
                let mut v = ranges.to_vec();
                v.sort();
                v.dedup();
                set.requires.insert(name.clone(), v);
                if let Some(i) = self.interfaces.get(name) {
                    set.built_with.insert(name.clone(), i.version);
                }
            }
        }
        set
    }

    /// The component that provides `interface`, if registered.
    pub fn provider(&self, interface: &str) -> Option<Component> {
        self.components
            .iter()
            .find(|(_, d)| d.provides.iter().any(|n| n == interface))
            .map(|(c, _)| *c)
    }

    /// What a host built from this registry understands.
    pub fn host_profile(&self) -> HostProfile {
        let host = &self.components[&Component::Host];
        HostProfile {
            version: host.version.clone(),
            interfaces: self.interface_set(Component::Host),
            kinds: self
                .interfaces
                .iter()
                .map(|(n, d)| (n.clone(), d.kind))
                .collect(),
            providers: self
                .interfaces
                .keys()
                .filter_map(|n| self.provider(n).map(|c| (n.clone(), c)))
                .collect(),
            transports: self
                .components
                .iter()
                .map(|(c, d)| (*c, d.update_transport.clone()))
                .collect(),
        }
    }
}

fn join<T: std::fmt::Display>(items: &[T]) -> String {
    if items.is_empty() {
        return "nothing".to_string();
    }
    items
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Everything the running host knows about interfaces, taken from the
/// registry it was built with.
///
/// This is the host side of every compatibility decision. The daemon builds it
/// once with [`Compat::host_profile`] from the `compat.toml` compiled into it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostProfile {
    /// The running host's version.
    pub version: semver::Version,
    /// What the running host requires from the keyboard, and the interface
    /// versions it was built with.
    pub interfaces: InterfaceSet,
    pub kinds: BTreeMap<String, InterfaceKind>,
    /// The component expected to provide each interface.
    pub providers: BTreeMap<String, Component>,
    /// The interfaces used to install and recover each component.
    pub transports: BTreeMap<Component, Vec<String>>,
}

impl HostProfile {
    pub fn transport(&self, c: Component) -> &[String] {
        self.transports.get(&c).map(Vec::as_slice).unwrap_or(&[])
    }
}
