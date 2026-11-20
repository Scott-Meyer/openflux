//! `release.json` and `index.json`.
//!
//! A `release.json` describes one release of one component: its version,
//! channel, interface declarations and artifacts. The release pipeline merges
//! every release into `index.json`, which the release key signs. Hosts fetch
//! `index.json` and its detached signature `index.json.sig`.
//!
//! Both formats only grow: new fields are optional and readers ignore fields
//! they do not know, so older hosts keep reading newer indexes. `schema`
//! changes only for an incompatible format, which would be published under a
//! different file name so existing hosts keep their index.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::compat::InterfaceSet;
use crate::factory::{FactoryReference, PressRefinementSource};
use crate::trust::{KeyId, Signature};
use crate::version::{Component, ParseError};

/// Release channel. Beta releases are SemVer prereleases (`1.1.0-beta.2`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        }
    }

    /// The channel a version belongs to: prereleases are beta.
    pub fn of(version: &semver::Version) -> Channel {
        if version.pre.is_empty() {
            Channel::Stable
        } else {
            Channel::Beta
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Channel {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stable" => Ok(Channel::Stable),
            "beta" => Ok(Channel::Beta),
            _ => Err(ParseError(format!(
                "unknown channel {s:?} (expected stable or beta)"
            ))),
        }
    }
}

/// One downloadable file of a release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// File name, unique within the release.
    pub name: String,
    pub url: String,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: String,
    pub size: u64,
    /// Minisign signatures of the file (the contents of `<name>.sig`), at
    /// most one per release key. A file signed again after a key rotation
    /// carries both signatures; a host accepts any signature by a key it
    /// trusts. A published index needs at least one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signatures: Vec<String>,
}

impl Artifact {
    /// Add signatures, keeping one per key. Where two signatures by the same
    /// key exist, the one that sorts first is kept, so the result does not
    /// depend on the order signatures arrive in.
    pub fn add_signatures(&mut self, more: &[String]) -> Result<(), ManifestError> {
        let mut by_key: BTreeMap<KeyId, String> = BTreeMap::new();
        for text in self.signatures.iter().chain(more) {
            let sig = Signature::parse(text)
                .map_err(|e| ManifestError(format!("artifact {}: {e}", self.name)))?;
            let entry = by_key.entry(sig.key_id()).or_insert_with(|| text.clone());
            if text < entry {
                *entry = text.clone();
            }
        }
        self.signatures = by_key.into_values().collect();
        Ok(())
    }

    /// The key ids that signed this artifact, as recorded in the signatures.
    pub fn signed_by(&self) -> Vec<KeyId> {
        self.signatures
            .iter()
            .filter_map(|t| Signature::parse(t).ok().map(|s| s.key_id()))
            .collect()
    }
}

/// One release of one component (`release.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub schema: u32,
    pub component: Component,
    pub version: semver::Version,
    pub channel: Channel,
    #[serde(with = "time::serde::rfc3339")]
    pub released: OffsetDateTime,
    pub notes_url: String,
    #[serde(flatten)]
    pub interfaces: InterfaceSet,
    pub artifacts: Vec<Artifact>,
    /// A withdrawn release stays listed so hosts can recognise it, but it is
    /// never offered as an update or a downgrade target.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub withdrawn: bool,
}

/// A structural problem in a manifest.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ManifestError(pub String);

impl Release {
    pub const SCHEMA: u32 = 1;

    pub fn parse(json: &[u8]) -> Result<Self, ManifestError> {
        let r: Release = serde_json::from_slice(json)
            .map_err(|e| ManifestError(format!("release.json: {e}")))?;
        r.validate()?;
        Ok(r)
    }

    /// `component@version`, used in messages.
    pub fn label(&self) -> String {
        format!("{}@{}", self.component, self.version)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        let label = self.label();
        if self.schema != Self::SCHEMA {
            return Err(ManifestError(format!(
                "{label}: schema {} is not supported",
                self.schema
            )));
        }
        if !self.version.build.is_empty() {
            return Err(ManifestError(format!(
                "{label}: build metadata is not allowed in release versions"
            )));
        }
        let expected = Channel::of(&self.version);
        if self.channel != expected {
            return Err(ManifestError(format!(
                "{label}: channel is {} but a {} version belongs to {expected}",
                self.channel,
                if self.version.pre.is_empty() {
                    "release"
                } else {
                    "prerelease"
                }
            )));
        }
        if self.version.major < 1 {
            return Err(ManifestError(format!(
                "{label}: release versions start at 1.0.0"
            )));
        }
        if self.artifacts.is_empty() {
            return Err(ManifestError(format!(
                "{label}: a release needs at least one artifact"
            )));
        }
        let mut names = BTreeSet::new();
        for a in &self.artifacts {
            if a.name.is_empty() || a.name.contains('/') || a.name.contains('\\') {
                return Err(ManifestError(format!(
                    "{label}: invalid artifact name {:?}",
                    a.name
                )));
            }
            if !names.insert(&a.name) {
                return Err(ManifestError(format!(
                    "{label}: artifact {} is listed twice",
                    a.name
                )));
            }
            if a.sha256.len() != 64
                || !a
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(ManifestError(format!(
                    "{label}: artifact {} has an invalid sha256",
                    a.name
                )));
            }
            if a.url.is_empty() {
                return Err(ManifestError(format!(
                    "{label}: artifact {} has no url",
                    a.name
                )));
            }
            let mut keys = BTreeSet::new();
            for text in &a.signatures {
                let sig = Signature::parse(text)
                    .map_err(|e| ManifestError(format!("{label}: artifact {}: {e}", a.name)))?;
                if !keys.insert(sig.key_id()) {
                    return Err(ManifestError(format!(
                        "{label}: artifact {} has two signatures by key {}",
                        a.name,
                        sig.key_id()
                    )));
                }
            }
        }
        for (name, reqs) in &self.interfaces.requires {
            if reqs.is_empty() {
                return Err(ManifestError(format!(
                    "{label}: requirement on {name} is empty"
                )));
            }
        }
        for (name, versions) in &self.interfaces.provides {
            if versions.is_empty() {
                return Err(ManifestError(format!(
                    "{label}: provides {name} with no version"
                )));
            }
        }
        Ok(())
    }

    /// Same release, comparing everything except withdrawal and artifact
    /// signatures. Signatures are randomized, and the same bytes may be signed
    /// again (for example after a key rotation) without changing the release.
    pub fn same_content(&self, other: &Release) -> bool {
        let strip = |r: &Release| {
            let mut r = r.clone();
            r.withdrawn = false;
            for a in &mut r.artifacts {
                a.signatures.clear();
            }
            r
        };
        strip(self) == strip(other)
    }

    /// Combine two descriptions of the same published release.
    ///
    /// A published release is immutable except that it can be withdrawn, and a
    /// withdrawal is never undone: the result is withdrawn if either side is,
    /// whichever order the two arrive in. Artifact signatures are combined,
    /// one per key (see [`Artifact::add_signatures`]), so a release signed
    /// again by a rotated key keeps both signatures. The result is the same in
    /// either merge order. Any other difference is an error.
    pub fn merge(&mut self, other: &Release) -> Result<(), ManifestError> {
        if self.component != other.component || self.version != other.version {
            return Err(ManifestError(format!(
                "cannot merge {} with {}",
                self.label(),
                other.label()
            )));
        }
        if !self.same_content(other) {
            return Err(ManifestError(format!(
                "{} is published with different contents in two places; published releases are immutable",
                self.label()
            )));
        }
        self.withdrawn |= other.withdrawn;
        for (mine, theirs) in self.artifacts.iter_mut().zip(&other.artifacts) {
            mine.add_signatures(&theirs.signatures)?;
        }
        Ok(())
    }
}

/// A root-signed statement embedded in the index.
///
/// `statement` holds the exact bytes the root key signed (a JSON
/// [`KeyStatement`]); keeping it as text avoids any canonical-encoding
/// question when verifying.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedStatement {
    pub statement: String,
    /// Minisign signature of `statement` by a root key.
    pub signature: String,
}

/// A change to the set of trusted release keys, signed by a root key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum KeyStatement {
    /// Trust an additional release key.
    Trust {
        key_id: KeyId,
        /// Minisign public key, base64 line.
        public_key: String,
        #[serde(with = "time::serde::rfc3339")]
        issued: OffsetDateTime,
    },
    /// Never trust this release key again. Revocation is permanent and
    /// overrides any trust statement for the same key.
    Revoke {
        key_id: KeyId,
        #[serde(with = "time::serde::rfc3339")]
        issued: OffsetDateTime,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
}

impl KeyStatement {
    pub fn key_id(&self) -> KeyId {
        match self {
            KeyStatement::Trust { key_id, .. } | KeyStatement::Revoke { key_id, .. } => *key_id,
        }
    }
}

/// The signed aggregate of all releases (`index.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub schema: u32,
    /// Increases by one with every published index. Hosts refuse an index
    /// older than the newest one they have accepted.
    pub sequence: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub issued: OffsetDateTime,
    /// Hosts refuse the index after this time, so a stale index cannot be
    /// served indefinitely to hide newer releases.
    #[serde(with = "time::serde::rfc3339")]
    pub expires: OffsetDateTime,
    /// The release key that signs this index.
    pub key_id: KeyId,
    pub releases: BTreeMap<Channel, Vec<Release>>,
    #[serde(default)]
    pub key_statements: Vec<SignedStatement>,
    /// Where the factory software is published and the files that make it
    /// up, pinned by hash. Covered by the index signature like everything
    /// else, so a host trusts a factory file only through a trusted index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factory: Option<FactoryReference>,
    /// Where the keyboard maker's press-refinement data is found and how it
    /// is checked, from `factory-reference.toml`. Readers that do not recognize this
    /// field ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press_refinement_source: Option<PressRefinementSource>,
}

impl Index {
    pub const SCHEMA: u32 = 1;

    pub fn parse(json: &[u8]) -> Result<Self, ManifestError> {
        let index: Index =
            serde_json::from_slice(json).map_err(|e| ManifestError(format!("index.json: {e}")))?;
        index.validate()?;
        Ok(index)
    }

    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("index serializes");
        s.push('\n');
        s
    }

    /// Every release in the index, in channel order.
    pub fn all_releases(&self) -> impl Iterator<Item = &Release> {
        self.releases.values().flatten()
    }

    /// Validate a publishable index: structure plus a signature on every artifact.
    pub fn validate(&self) -> Result<(), ManifestError> {
        self.validate_with(true)
    }

    /// Validate structure, optionally allowing unsigned artifacts (test indexes).
    pub fn validate_with(&self, require_signatures: bool) -> Result<(), ManifestError> {
        if self.schema != Self::SCHEMA {
            return Err(ManifestError(format!(
                "index schema {} is not supported",
                self.schema
            )));
        }
        if self.expires <= self.issued {
            return Err(ManifestError(
                "index expires before it was issued".to_string(),
            ));
        }
        if let Some(f) = &self.factory {
            f.validate()?;
        }
        if let Some(d) = &self.press_refinement_source {
            d.validate()?;
        }
        let mut seen = BTreeSet::new();
        for (channel, releases) in &self.releases {
            for r in releases {
                r.validate()?;
                if r.channel != *channel {
                    return Err(ManifestError(format!(
                        "{} is listed under {channel}",
                        r.label()
                    )));
                }
                if !seen.insert((r.component, r.version.clone())) {
                    return Err(ManifestError(format!("{} is listed twice", r.label())));
                }
                for a in &r.artifacts {
                    if require_signatures && a.signatures.is_empty() {
                        return Err(ManifestError(format!(
                            "{}: artifact {} is not signed",
                            r.label(),
                            a.name
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}
