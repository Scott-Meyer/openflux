//! The factory reference: where the keyboard's factory software is published
//! and the exact files that make it up (`factory-reference.toml`).
//!
//! OpenFlux never hosts the factory software. The reference names the
//! container it is published in and pins every file needed to return a
//! keyboard to it by size and SHA-256. The release workflow embeds the
//! reference in the signed index, so a host trusts a factory file only when
//! its hash matches what a trusted index records.
//!
//! The same file and index also carry the [`PressRefinementSource`]: where the
//! keyboard maker's press-refinement coefficients can be obtained and how they
//! are checked. OpenFlux never hosts them either.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::manifest::ManifestError;
use crate::version::Component;

/// One file of the factory software, a member of the container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactoryFile {
    /// Which keyboard component the file restores: firmware, bridge or display.
    pub component: Component,
    /// Path of the file inside the container.
    pub member: String,
    /// The factory version the file installs.
    pub version: String,
    pub size: u64,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: String,
}

/// The factory software reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactoryReference {
    /// Shown to the user, for example "Factory software 2.3".
    pub label: String,
    /// HTTPS URL of the container that holds the files.
    pub container_url: String,
    pub container_size: u64,
    /// Exactly one file each for firmware, bridge and display.
    pub files: Vec<FactoryFile>,
}

/// Where a [`PressRefinementSource`]'s firmware file can be obtained.
///
/// Sources are tried in order. A kind this version doesn't know is read as
/// [`SourceLocation::Unrecognized`] and skipped by hosts; the release tool refuses
/// to publish one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SourceLocation {
    /// A published ZIP container (such as an MSIX package) holding the
    /// source member, read with HTTP range requests or downloaded whole.
    Container {
        /// HTTPS URL of the container.
        url: String,
        size: u64,
        /// Lowercase hex SHA-256 of the whole container, checked when it is
        /// downloaded whole. Range reads are gated by the member's hashes.
        sha256: String,
    },
    /// A source kind this version doesn't know.
    #[serde(other)]
    Unrecognized,
}

/// A range of the source image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRange {
    /// Offset from the image's start.
    pub offset: u32,
    pub length: u32,
}

/// Where the keyboard maker's press-refinement payload is found, and the size
/// and SHA-256 of every stage: the firmware file (a UF2), the image its blocks
/// make at their target addresses, and the payload taken from the image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PressRefinementSource {
    /// Which published software this is, for logs.
    pub description: String,
    /// Where the firmware file can be obtained, in order of preference.
    pub sources: Vec<SourceLocation>,
    /// Path of the firmware file inside a container source.
    pub member: String,
    pub member_size: u64,
    pub member_sha256: String,
    /// Target address of the image's first byte.
    pub image_base: u32,
    pub image_size: u64,
    pub image_sha256: String,
    /// The image ranges that make up the payload, in order.
    pub payload_ranges: Vec<SourceRange>,
    pub payload_length: u64,
    pub payload_sha256: String,
}

impl PressRefinementSource {
    /// Checks the structure. Sources of unrecognized kinds are
    /// allowed here, so a host keeps reading an index that lists a kind added
    /// after it was built; [`PressRefinementSource::check_publishable`] refuses
    /// them.
    pub fn validate(&self) -> Result<(), ManifestError> {
        let mut problems = Vec::new();
        if self.description.trim().is_empty() {
            problems.push("description is empty".to_string());
        }
        if self.sources.is_empty() {
            problems.push("sources is empty".to_string());
        }
        for (i, src) in self.sources.iter().enumerate() {
            if let SourceLocation::Container { url, size, sha256 } = src {
                if !is_https_url(url) {
                    problems.push(format!("source {i}: url {url:?} is not an https URL"));
                }
                if *size == 0 {
                    problems.push(format!("source {i}: size must be greater than zero"));
                } else if self.member_size > *size {
                    problems.push(format!(
                        "source {i}: member_size is larger than the container"
                    ));
                }
                if !is_sha256(sha256) {
                    problems.push(format!(
                        "source {i}: sha256 must be 64 lowercase hex digits"
                    ));
                }
            }
        }
        if self.member.trim().is_empty() || self.member.ends_with('/') {
            problems.push("member is not a file path".to_string());
        }
        if self.member_size == 0 {
            problems.push("member_size must be greater than zero".to_string());
        }
        for (name, sha) in [
            ("member_sha256", &self.member_sha256),
            ("image_sha256", &self.image_sha256),
            ("payload_sha256", &self.payload_sha256),
        ] {
            if !is_sha256(sha) {
                problems.push(format!("{name} must be 64 lowercase hex digits"));
            }
        }
        let mut total = 0u64;
        for r in &self.payload_ranges {
            if r.length == 0 || u64::from(r.offset) + u64::from(r.length) > self.image_size {
                problems.push(format!(
                    "range 0x{:x}+{} is outside the image",
                    r.offset, r.length
                ));
            }
            total += u64::from(r.length);
        }
        if self.payload_ranges.is_empty() || total != self.payload_length {
            problems.push(format!(
                "the ranges make {total} bytes, payload_length is {}",
                self.payload_length
            ));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(ManifestError(format!(
                "press-refinement source: {}",
                problems.join("; ")
            )))
        }
    }

    /// [`PressRefinementSource::validate`], and every source is of a kind this
    /// version knows.
    pub fn check_publishable(&self) -> Result<(), ManifestError> {
        self.validate()?;
        if self.sources.contains(&SourceLocation::Unrecognized) {
            return Err(ManifestError(
                "press-refinement source: a source has a kind this tool doesn't know".to_string(),
            ));
        }
        Ok(())
    }
}

/// `factory-reference.toml`: the `[factory]` table and the optional
/// `[press_refinement_source]` table.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceFile {
    pub factory: FactoryReference,
    #[serde(default)]
    pub press_refinement_source: Option<PressRefinementSource>,
}

impl ReferenceFile {
    /// Parse and validate `factory-reference.toml` for publishing.
    pub fn parse_toml(text: &str) -> Result<Self, ManifestError> {
        let file: ReferenceFile =
            toml::from_str(text).map_err(|e| ManifestError(format!("factory reference: {e}")))?;
        file.factory.validate()?;
        if let Some(d) = &file.press_refinement_source {
            d.check_publishable()?;
        }
        Ok(file)
    }
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_https_url(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !s.chars().any(char::is_whitespace)
}

impl FactoryReference {
    /// Parse and validate `factory-reference.toml` and return its `[factory]`
    /// table (see [`ReferenceFile`]).
    pub fn parse_toml(text: &str) -> Result<Self, ManifestError> {
        ReferenceFile::parse_toml(text).map(|f| f.factory)
    }

    /// The file for one component.
    pub fn file(&self, component: Component) -> Option<&FactoryFile> {
        self.files.iter().find(|f| f.component == component)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        let mut problems = Vec::new();
        if self.label.trim().is_empty() {
            problems.push("label is empty".to_string());
        }
        if !is_https_url(&self.container_url) {
            problems.push(format!(
                "container_url {:?} is not an https URL",
                self.container_url
            ));
        }
        if self.container_size == 0 {
            problems.push("container_size must be greater than zero".to_string());
        }
        let mut seen = BTreeSet::new();
        for f in &self.files {
            let c = f.component;
            if c == Component::Host {
                problems.push("host is not a factory component".to_string());
            } else if !seen.insert(c) {
                problems.push(format!("{c} is listed more than once"));
            }
            if f.member.trim().is_empty() {
                problems.push(format!("{c}: member is empty"));
            }
            if f.version.trim().is_empty() {
                problems.push(format!("{c}: version is empty"));
            }
            if f.size == 0 {
                problems.push(format!("{c}: size must be greater than zero"));
            } else if f.size > self.container_size {
                problems.push(format!("{c}: size is larger than the container"));
            }
            if !is_sha256(&f.sha256) {
                problems.push(format!("{c}: sha256 must be 64 lowercase hex digits"));
            }
        }
        for c in Component::DEVICE {
            if !seen.contains(&c) {
                problems.push(format!("{c} is missing"));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(ManifestError(format!(
                "factory reference: {}",
                problems.join("; ")
            )))
        }
    }
}
