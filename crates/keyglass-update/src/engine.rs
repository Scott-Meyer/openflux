//! Compatibility and update planning.
//!
//! The engine looks at the running host, the software installed on the
//! keyboard and the available releases, and answers two questions:
//!
//! - [`Engine::status`]: what should the user be told about each component?
//! - [`Engine::plan`]: how can the requested components be updated safely?
//!
//! A plan is an ordered list of single-component steps. The order is not
//! fixed; it is searched for so that:
//!
//! - every update transport that works before the plan keeps working after
//!   every step, and the host can reach each component it touches before and
//!   after touching it, so a failed step can always be retried or rolled back;
//! - the final state breaks nothing that worked before and leaves no area
//!   read-only that was editable before;
//! - intermediate states that temporarily break a feature are avoided when
//!   possible and reported on the step when not.
//!
//! When no such order exists the engine refuses and says what is in the way.
//! Updates only move components forward; [`Engine::plan_downgrade`] is the
//! separate, explicit way back.
//!
//! All of this applies only to a keyboard on which every component runs
//! Keyglass 1.0 or later ([`KeyboardMode::Keyglass`]). A keyboard with any
//! other software (factory software, an Keyglass development build, or
//! anything unidentified) is not updated piece by piece. The only keyboard
//! action offered for it is converting the whole keyboard to Keyglass, which
//! the host's conversion module carries out from a [`ConversionPlan`]. Host
//! updates are offered in both modes.
//!
//! The engine is pure and deterministic. Execute a plan's steps in order,
//! including companion steps, even after the requested components have
//! reached their targets.

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::compat::{HostProfile, InterfaceKind, InterfaceSet};
use crate::factory::{FactoryReference, PressRefinementSource};
use crate::manifest::{Channel, ManifestError, Release};
use crate::trust::VerifiedIndex;
use crate::version::{satisfies, Component, InterfaceVersion, Requirement};

/// What is installed in one keyboard component, as identified by the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum InstalledSoftware {
    /// An Keyglass release that answered IDENTIFY.
    #[serde(rename = "keyglass")]
    Keyglass {
        version: Version,
        interfaces: InterfaceSet,
    },
    /// An Keyglass development build, which does not identify itself as a release.
    DevelopmentBuild { label: String },
    /// The keyboard's factory software.
    Factory { label: String },
    /// Software that could not be identified.
    Unknown { label: String },
}

/// The kind of software a component runs, without its details.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SoftwareKind {
    #[serde(rename = "keyglass")]
    Keyglass,
    DevelopmentBuild,
    Factory,
    Unknown,
}

impl InstalledSoftware {
    pub fn kind(&self) -> SoftwareKind {
        match self {
            InstalledSoftware::Keyglass { .. } => SoftwareKind::Keyglass,
            InstalledSoftware::DevelopmentBuild { .. } => SoftwareKind::DevelopmentBuild,
            InstalledSoftware::Factory { .. } => SoftwareKind::Factory,
            InstalledSoftware::Unknown { .. } => SoftwareKind::Unknown,
        }
    }
}

/// How the engine treats a keyboard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum KeyboardMode {
    /// Every keyboard component runs Keyglass 1.0 or later. Components are
    /// updated individually along validated plans.
    Keyglass,
    /// At least one component runs something else. The keyboard is converted
    /// to Keyglass as a whole; nothing else is offered for it.
    NeedsConversion {
        found: BTreeMap<Component, SoftwareKind>,
    },
}

/// What converting a keyboard to Keyglass installs.
///
/// The engine proposes the releases with [`Engine::propose_conversion`]; the
/// host's conversion module owns everything else: backing up the existing
/// software, the install order, and talking to software that is not Keyglass.
/// The normal updater never produces one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionPlan {
    /// What each keyboard component runs before the conversion.
    pub found: BTreeMap<Component, InstalledSoftware>,
    /// One release each of firmware, bridge and display, chosen to work
    /// together and with the running host.
    pub install: Vec<Release>,
}

/// The software found in each keyboard component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    pub firmware: InstalledSoftware,
    pub bridge: InstalledSoftware,
    pub display: InstalledSoftware,
}

impl Inventory {
    /// The engine's mode for this keyboard.
    pub fn mode(&self) -> KeyboardMode {
        let found: BTreeMap<Component, SoftwareKind> = Component::DEVICE
            .into_iter()
            .filter_map(|c| self.get(c).map(|s| (c, s.kind())))
            .collect();
        if found.values().all(|k| *k == SoftwareKind::Keyglass) {
            KeyboardMode::Keyglass
        } else {
            KeyboardMode::NeedsConversion { found }
        }
    }

    pub fn get(&self, c: Component) -> Option<&InstalledSoftware> {
        match c {
            Component::Host => None,
            Component::Firmware => Some(&self.firmware),
            Component::Bridge => Some(&self.bridge),
            Component::Display => Some(&self.display),
        }
    }
}

/// User choices that affect what is offered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    /// Offer beta releases in addition to stable ones.
    pub beta: bool,
}

impl Preferences {
    fn offers(&self, r: &Release) -> bool {
        !r.withdrawn && (self.beta || r.channel == Channel::Stable)
    }
}

/// Keeps `incoming` in `held` when it comes from a newer index.
fn newest<T: Clone + PartialEq>(
    held: &mut Option<(u64, T)>,
    incoming: Option<&T>,
    sequence: u64,
    what: &str,
) -> Result<(), ManifestError> {
    let Some(incoming) = incoming else {
        return Ok(());
    };
    match held {
        Some((seq, current)) if *seq == sequence && current != incoming => Err(ManifestError(
            format!("two indexes with sequence {sequence} record different {what}"),
        )),
        Some((seq, _)) if *seq >= sequence => Ok(()),
        _ => {
            *held = Some((sequence, incoming.clone()));
            Ok(())
        }
    }
}

/// The releases the engine may choose from.
///
/// Built from a [`VerifiedIndex`], or from releases whose authenticity is
/// established another way, such as those bundled in a signed host installer.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    releases: Vec<Release>,
    /// The factory reference and the sequence of the index it came from.
    factory: Option<(u64, FactoryReference)>,
    /// The press-refinement source and the sequence of the index it came from.
    press_refinement_source: Option<(u64, PressRefinementSource)>,
}

impl Catalog {
    pub fn from_index(index: &VerifiedIndex) -> Self {
        let mut c = Self::default();
        c.add_index(index)
            .expect("an index lists each release once");
        c
    }

    /// Releases the caller already trusts. See [`Catalog::add_trusted`].
    pub fn from_trusted_releases(
        releases: impl IntoIterator<Item = Release>,
    ) -> Result<Self, ManifestError> {
        let mut c = Self::default();
        c.add_trusted(releases)?;
        Ok(c)
    }

    /// Add every release of a verified index, such as a newer online index
    /// on top of the one bundled with the host.
    ///
    /// The factory reference and the press-refinement source each come from
    /// the index with the highest sequence that records one, whichever order
    /// the indexes are added in. Two indexes with the same sequence that
    /// record different ones are an error.
    pub fn add_index(&mut self, index: &VerifiedIndex) -> Result<(), ManifestError> {
        let sequence = index.index().sequence;
        newest(
            &mut self.factory,
            index.factory(),
            sequence,
            "factory references",
        )?;
        newest(
            &mut self.press_refinement_source,
            index.press_refinement_source(),
            sequence,
            "press-refinement sources",
        )?;
        self.add_trusted(index.index().all_releases().cloned())
    }

    /// The press-refinement source from the newest index that records one.
    pub fn press_refinement_source(&self) -> Option<&PressRefinementSource> {
        self.press_refinement_source.as_ref().map(|(_, d)| d)
    }

    /// The factory reference from the newest index that records one.
    pub fn factory(&self) -> Option<&FactoryReference> {
        self.factory.as_ref().map(|(_, f)| f)
    }

    /// Add releases the caller already trusts.
    ///
    /// A release already in the catalog is merged with [`Release::merge`]:
    /// it ends up withdrawn if either source withdrew it, and any other
    /// difference between the two is an error.
    pub fn add_trusted(
        &mut self,
        releases: impl IntoIterator<Item = Release>,
    ) -> Result<(), ManifestError> {
        for r in releases {
            match self
                .releases
                .iter_mut()
                .find(|e| e.component == r.component && e.version == r.version)
            {
                Some(existing) => existing.merge(&r)?,
                None => self.releases.push(r),
            }
        }
        self.releases.sort_by(|a, b| {
            a.component
                .cmp(&b.component)
                .then(a.version.cmp(&b.version))
        });
        Ok(())
    }

    pub fn releases(&self) -> &[Release] {
        &self.releases
    }

    pub fn find(&self, c: Component, v: &Version) -> Option<&Release> {
        self.releases
            .iter()
            .find(|r| r.component == c && &r.version == v)
    }
}

/// A requirement that is not met.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Unmet {
    pub consumer: Component,
    pub interface: String,
    pub requires: Vec<Requirement>,
    /// The component expected to provide the interface.
    pub provider: Option<Component>,
    /// What that component provides (empty when it does not provide it).
    pub provided: Vec<InterfaceVersion>,
}

impl Unmet {
    fn key(&self) -> (Component, String) {
        (self.consumer, self.interface.clone())
    }
}

/// How a keyboard component's interface relates to what the host understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GapEffect {
    /// The keyboard speaks a major this host does not: the area is unavailable.
    Unavailable,
    /// The keyboard stores a newer minor of a data format: the host can read
    /// it but does not write it, to avoid dropping what it does not understand.
    ReadOnly,
}

/// A keyboard interface newer than the host understands.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Gap {
    pub component: Component,
    pub interface: String,
    pub device: InterfaceVersion,
    pub host_understands: Option<InterfaceVersion>,
    pub effect: GapEffect,
}

impl Gap {
    fn key(&self) -> (Component, String) {
        (self.component, self.interface.clone())
    }
}

/// Why a candidate version cannot be installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "problem", rename_all = "kebab-case")]
pub enum Problem {
    /// The host would lose the interface it uses to update or recover `component`.
    LosesTransport {
        component: Component,
        interfaces: Vec<String>,
    },
    /// A requirement met before the change would be unmet after it.
    Breaks { unmet: Unmet },
    /// An area the host can use before the change would become unavailable or
    /// read-only.
    NewerThanHost { gap: Gap },
}

fn list(items: &[String]) -> String {
    items.join(", ")
}

impl std::fmt::Display for Unmet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let wanted: Vec<String> = self.requires.iter().map(|r| r.to_string()).collect();
        let provided: Vec<String> = self.provided.iter().map(|v| v.to_string()).collect();
        let Some(provider) = self.provider else {
            return write!(
                f,
                "{} needs {} {} but no component provides it",
                self.consumer,
                self.interface,
                wanted.join(" or ")
            );
        };
        if provided.is_empty() {
            write!(
                f,
                "{} needs {} {} but {provider} does not provide it",
                self.consumer,
                self.interface,
                wanted.join(" or ")
            )
        } else {
            write!(
                f,
                "{} needs {} {} but {provider} provides {}",
                self.consumer,
                self.interface,
                wanted.join(" or "),
                provided.join(", ")
            )
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Problem::LosesTransport {
                component,
                interfaces,
            } => {
                write!(
                    f,
                    "the host would lose {} and could not update {component} afterwards",
                    list(interfaces)
                )
            }
            Problem::Breaks { unmet } => write!(f, "{unmet}"),
            Problem::NewerThanHost { gap } => match gap.effect {
                GapEffect::Unavailable => write!(
                    f,
                    "{} {} {} is newer than this host understands; update the host first",
                    gap.component, gap.interface, gap.device
                ),
                GapEffect::ReadOnly => write!(
                    f,
                    "{} {} {} would be read-only on this host; update the host first",
                    gap.component, gap.interface, gap.device
                ),
            },
        }
    }
}

fn describe_found(found: &BTreeMap<Component, SoftwareKind>) -> String {
    let others: Vec<String> = found
        .iter()
        .filter(|(_, k)| **k != SoftwareKind::Keyglass)
        .map(|(c, k)| {
            let what = match k {
                SoftwareKind::Keyglass => "Keyglass",
                SoftwareKind::DevelopmentBuild => "a development build",
                SoftwareKind::Factory => "factory software",
                SoftwareKind::Unknown => "unidentified software",
            };
            format!("{c} runs {what}")
        })
        .collect();
    others.join(", ")
}

fn first_problem(blockers: &[Blocker]) -> String {
    blockers
        .iter()
        .find_map(|b| {
            b.problems
                .first()
                .map(|p| format!(": {} {}: {p}", b.component, b.version))
        })
        .unwrap_or_default()
}

/// A candidate that could not be reached, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocker {
    pub component: Component,
    pub version: Version,
    /// What goes wrong if this version is installed on its own. Empty when the
    /// version is fine alone but not together with the rest of the plan.
    pub problems: Vec<Problem>,
}

/// Why the engine declined to plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "refusal", rename_all = "kebab-case")]
pub enum Refusal {
    #[error("{}; convert the keyboard to Keyglass first", describe_found(found))]
    ConversionRequired {
        found: BTreeMap<Component, SoftwareKind>,
    },
    #[error("every keyboard component already runs Keyglass")]
    AlreadyKeyglass,
    #[error("the host cannot reach {component} over {}", interfaces.join(", "))]
    NoTransport {
        component: Component,
        interfaces: Vec<String>,
    },
    #[error("nothing to update")]
    NothingToUpdate,
    #[error("no safe update path exists{}", first_problem(blockers))]
    NoSafePath { blockers: Vec<Blocker> },
    #[error("no offered release of {component} is available")]
    NoRelease { component: Component },
    #[error("{component} {version} is not an available release")]
    NoSuchRelease {
        component: Component,
        version: Version,
    },
    #[error("{component} is not a keyboard component; only keyboard components are downgraded")]
    NotAKeyboardComponent { component: Component },
    #[error("{component} {version} is not older than the installed version")]
    NotADowngrade {
        component: Component,
        version: Version,
    },
    #[error("downgrading {component} to {version} is not safe")]
    UnsafeDowngrade {
        component: Component,
        version: Version,
        problems: Vec<Problem>,
    },
}

/// Why a step is in a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepReason {
    /// The user asked for this component.
    Requested,
    /// Needed so the requested components keep working together.
    Companion,
    /// An intermediate version that keeps the host connected while
    /// interfaces move to a new major; the component moves again later.
    Transitional,
    /// An explicit downgrade.
    Downgrade,
}

/// One step of a plan: install one release of one component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub component: Component,
    pub from: Version,
    pub to: Version,
    pub reason: StepReason,
    /// Interfaces the host uses to install this step. Empty for the host,
    /// which updates itself.
    pub transport: Vec<String>,
    /// Requirements that are unmet after this step until a later step
    /// completes. The UI can warn that these features pause briefly.
    pub temporarily_unmet: Vec<Unmet>,
    pub release: Release,
}

/// A validated update plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Requirements that were unmet before the plan and remain unmet.
    pub remaining_unmet: Vec<Unmet>,
    /// Newer versions of requested components that the plan does not reach.
    pub held_back: Vec<Blocker>,
}

impl Plan {
    /// The version each changed component ends at.
    pub fn targets(&self) -> BTreeMap<Component, Version> {
        self.steps
            .iter()
            .map(|s| (s.component, s.to.clone()))
            .collect()
    }
}

/// The components a plan should bring up to date.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRequest {
    pub components: BTreeSet<Component>,
}

impl PlanRequest {
    pub fn only(c: Component) -> Self {
        Self {
            components: [c].into(),
        }
    }
    /// Every keyboard component, with the host as it is.
    pub fn keyboard() -> Self {
        Self {
            components: Component::DEVICE.into(),
        }
    }
    /// Everything, the host included.
    pub fn everything() -> Self {
        Self {
            components: Component::ALL.into(),
        }
    }
}

/// A host update that unlocks a newer keyboard component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostGate {
    pub to: Version,
    pub host: Version,
}

/// What the user is told about one component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum ComponentState {
    UpToDate,
    /// `required` is true when the component does not fully work with the
    /// rest of the system as installed.
    UpdateAvailable {
        to: Version,
        required: bool,
    },
    /// A newer version exists but needs a newer host first.
    NeedsNewerHost {
        to: Version,
        host: Version,
    },
    /// The keyboard speaks interfaces newer than this host understands.
    /// Update the host.
    NewerThanHost {
        gaps: Vec<Gap>,
    },
    /// A newer version exists but cannot be installed safely.
    UpdateBlocked {
        to: Version,
        refusal: Refusal,
    },
    /// The installed software does not work with the rest of the system and
    /// no release fixes it.
    Incompatible {
        unmet: Vec<Unmet>,
    },
    /// The keyboard is not fully Keyglass; the only action is converting it.
    /// `found` is what this component runs.
    ConversionRequired {
        found: SoftwareKind,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentStatus {
    pub component: Component,
    /// Installed version; `None` unless the component runs an Keyglass release.
    pub installed: Option<Version>,
    pub state: ComponentState,
    /// A further update reachable after updating the host.
    pub after_host_update: Option<HostGate>,
    /// The installed release has been withdrawn by its publisher.
    pub installed_withdrawn: bool,
    /// Current requirements involving this component that are not met.
    pub unmet: Vec<Unmet>,
}

#[derive(Clone, Debug)]
struct Node {
    version: Option<Version>,
    interfaces: InterfaceSet,
    release: Option<usize>,
}

/// Index of each component's candidate within `Planner::cands`.
type State = [usize; 4];

/// Path cost: degraded intermediate states, steps, components in step order.
type Cost = (usize, usize, Vec<usize>);

struct Eval {
    unmet: Vec<Unmet>,
    gaps: Vec<Gap>,
}

impl Eval {
    fn unmet_keys(&self) -> BTreeSet<(Component, String)> {
        self.unmet.iter().map(Unmet::key).collect()
    }
    fn gap_keys(&self) -> BTreeSet<(Component, String)> {
        self.gaps.iter().map(Gap::key).collect()
    }
}

/// Compatibility and update planning over one snapshot of host, keyboard and
/// releases.
pub struct Engine<'a> {
    profile: &'a HostProfile,
    inventory: &'a Inventory,
    catalog: &'a Catalog,
    prefs: Preferences,
}

struct Planner<'e> {
    engine: &'e Engine<'e>,
    /// Candidates per component; index 0 is what is installed. Empty for
    /// factory or unidentified software.
    cands: [Vec<Node>; 4],
}

const PREFERENCE_ORDER: [Component; 4] = [
    Component::Host,
    Component::Bridge,
    Component::Display,
    Component::Firmware,
];

fn cmp_opt_version(a: &Option<Version>, b: &Option<Version>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => a.cmp(b),
    }
}

impl<'a> Engine<'a> {
    pub fn new(
        profile: &'a HostProfile,
        inventory: &'a Inventory,
        catalog: &'a Catalog,
        prefs: Preferences,
    ) -> Self {
        Self {
            profile,
            inventory,
            catalog,
            prefs,
        }
    }

    /// The engine's mode for this keyboard; see [`KeyboardMode`].
    pub fn mode(&self) -> KeyboardMode {
        self.inventory.mode()
    }

    fn require_keyglass(&self) -> Result<(), Refusal> {
        match self.mode() {
            KeyboardMode::Keyglass => Ok(()),
            KeyboardMode::NeedsConversion { found } => Err(Refusal::ConversionRequired { found }),
        }
    }

    fn installed_node(&self, c: Component) -> Option<Node> {
        match c {
            Component::Host => Some(Node {
                version: Some(self.profile.version.clone()),
                interfaces: self.profile.interfaces.clone(),
                release: None,
            }),
            _ => match self.inventory.get(c)? {
                InstalledSoftware::Keyglass {
                    version,
                    interfaces,
                } => Some(Node {
                    version: Some(version.clone()),
                    interfaces: interfaces.clone(),
                    release: None,
                }),
                _ => None,
            },
        }
    }

    /// Newer offered releases of `c`, collapsing consecutive runs with identical
    /// interface declarations (keeping the newest release in each run). Non-consecutive
    /// releases with the same declarations are kept so a transitional release below a
    /// target isn't discarded in favour of a higher release above the target.
    /// With no installed version, every offered release counts as newer.
    fn newer_candidates(&self, c: Component, installed: Option<&Version>) -> Vec<Node> {
        let mut indices: Vec<usize> = self
            .catalog
            .releases
            .iter()
            .enumerate()
            .filter(|(_, r)| r.component == c && self.prefs.offers(r))
            .filter(|(_, r)| installed.is_none_or(|v| &r.version > v))
            .map(|(i, _)| i)
            .collect();
        indices.sort_by(|&a, &b| {
            self.catalog.releases[a]
                .version
                .cmp(&self.catalog.releases[b].version)
        });

        let mut out: Vec<Node> = Vec::new();
        let mut last_key: Option<String> = None;
        for i in indices {
            let r = &self.catalog.releases[i];
            let key = serde_json::to_string(&r.interfaces).unwrap_or_default();
            let node = Node {
                version: Some(r.version.clone()),
                interfaces: r.interfaces.clone(),
                release: Some(i),
            };
            if last_key.as_deref() == Some(key.as_str()) {
                if let Some(prev) = out.last_mut() {
                    *prev = node;
                }
            } else {
                last_key = Some(key);
                out.push(node);
            }
        }
        out
    }

    fn newest_offered(&self, c: Component, installed: &Option<Version>) -> Option<&Release> {
        self.catalog
            .releases
            .iter()
            .filter(|r| r.component == c && self.prefs.offers(r))
            .filter(|r| installed.as_ref().is_none_or(|v| &r.version > v))
            .max_by(|a, b| a.version.cmp(&b.version))
    }

    fn planner(&self, movable: &BTreeSet<Component>) -> Planner<'_> {
        let cands = Component::ALL.map(|c| match self.installed_node(c) {
            None => Vec::new(),
            Some(n) => {
                let mut v = vec![n.clone()];
                if movable.contains(&c) {
                    v.extend(self.newer_candidates(c, n.version.as_ref()));
                }
                v
            }
        });
        Planner {
            engine: self,
            cands,
        }
    }

    fn check_reachable(&self, c: Component) -> Result<(), Refusal> {
        let p = self.planner(&BTreeSet::new());
        if !p.transport_ok(&[0; 4], c) {
            return Err(Refusal::NoTransport {
                component: c,
                interfaces: self.profile.transport(c).to_vec(),
            });
        }
        Ok(())
    }

    /// The host update offered while the keyboard needs conversion: the
    /// newest offered host release. No reasoning about the keyboard happens
    /// in this mode; the conversion module deals with it.
    fn host_only_plan(&self) -> Result<Plan, Refusal> {
        let current = Some(self.profile.version.clone());
        let release = self
            .newest_offered(Component::Host, &current)
            .ok_or(Refusal::NothingToUpdate)?;
        Ok(Plan {
            steps: vec![Step {
                component: Component::Host,
                from: self.profile.version.clone(),
                to: release.version.clone(),
                reason: StepReason::Requested,
                transport: Vec::new(),
                temporarily_unmet: Vec::new(),
                release: release.clone(),
            }],
            remaining_unmet: Vec::new(),
            held_back: Vec::new(),
        })
    }

    /// Plan updates for the requested components.
    ///
    /// Keyboard components outside the request may be updated as companions
    /// when the requested versions need them. The host is updated only when
    /// it is part of the request. Among all safe outcomes the engine prefers
    /// newer requested components (host, then bridge, display, firmware),
    /// then fewer companion updates, then shorter paths.
    ///
    /// On a keyboard that needs conversion only the host can be updated; a
    /// request that includes a keyboard component is refused with
    /// [`Refusal::ConversionRequired`].
    pub fn plan(&self, request: &PlanRequest) -> Result<Plan, Refusal> {
        if let KeyboardMode::NeedsConversion { found } = self.mode() {
            if request.components.iter().any(|c| *c != Component::Host) {
                return Err(Refusal::ConversionRequired { found });
            }
            if request.components.contains(&Component::Host) {
                return self.host_only_plan();
            }
            return Err(Refusal::NothingToUpdate);
        }
        let mut requested: Vec<Component> = Vec::new();
        for &c in &request.components {
            if c != Component::Host {
                self.check_reachable(c)?;
            }
            requested.push(c);
        }
        let mut movable: BTreeSet<Component> = Component::DEVICE.into_iter().collect();
        if request.components.contains(&Component::Host) {
            movable.insert(Component::Host);
        }
        self.planner(&movable).plan(&requested)
    }

    /// Plan an explicit downgrade of one keyboard component to `version`.
    ///
    /// The host is not a keyboard component and is refused with
    /// [`Refusal::NotAKeyboardComponent`]. Nothing else changes. The downgrade is refused if it would cost the
    /// host its update transport, break a requirement that is met before it, or
    /// make an area unavailable or read-only.
    pub fn plan_downgrade(&self, component: Component, version: &Version) -> Result<Plan, Refusal> {
        self.require_keyglass()?;
        if component == Component::Host {
            return Err(Refusal::NotAKeyboardComponent { component });
        }
        let Some((i, r)) = self
            .catalog
            .releases
            .iter()
            .enumerate()
            .find(|(_, r)| r.component == component && &r.version == version && !r.withdrawn)
        else {
            return Err(Refusal::NoSuchRelease {
                component,
                version: version.clone(),
            });
        };
        let installed = self
            .installed_node(component)
            .expect("an Keyglass keyboard has every node");
        let from = installed
            .version
            .clone()
            .expect("installed releases have versions");
        if version >= &from {
            return Err(Refusal::NotADowngrade {
                component,
                version: version.clone(),
            });
        }
        self.check_reachable(component)?;
        let mut planner = self.planner(&BTreeSet::new());
        planner.cands[component.index()].push(Node {
            version: Some(r.version.clone()),
            interfaces: r.interfaces.clone(),
            release: Some(i),
        });
        let start = [0; 4];
        let mut target = start;
        target[component.index()] = 1;
        let problems = planner.problems(&start, &target);
        if !problems.is_empty() {
            return Err(Refusal::UnsafeDowngrade {
                component,
                version: version.clone(),
                problems,
            });
        }
        let end = planner.eval(&target);
        Ok(Plan {
            steps: vec![Step {
                component,
                from,
                to: r.version.clone(),
                reason: StepReason::Downgrade,
                transport: self.profile.transport(component).to_vec(),
                temporarily_unmet: Vec::new(),
                release: r.clone(),
            }],
            remaining_unmet: end.unmet,
            held_back: Vec::new(),
        })
    }

    /// Propose the releases for converting this keyboard to Keyglass.
    ///
    /// Only for a keyboard that needs conversion. The proposal is the newest
    /// offered firmware, bridge and display that work together and with the
    /// running host, judged purely between those Keyglass releases: what the
    /// keyboard runs before the conversion plays no part.
    pub fn propose_conversion(&self) -> Result<ConversionPlan, Refusal> {
        if self.mode() == KeyboardMode::Keyglass {
            return Err(Refusal::AlreadyKeyglass);
        }
        let host = self.installed_node(Component::Host).expect("host node");
        let mut cands: [Vec<Node>; 4] = [vec![host], Vec::new(), Vec::new(), Vec::new()];
        for c in Component::DEVICE {
            cands[c.index()] = self.newer_candidates(c, None);
            if cands[c.index()].is_empty() {
                return Err(Refusal::NoRelease { component: c });
            }
        }
        let planner = Planner {
            engine: self,
            cands,
        };
        let mut best: Option<State> = None;
        let sizes = Component::ALL.map(|c| planner.cands[c.index()].len());
        let version_at =
            |s: &State, c: Component| planner.node(s, c).and_then(|n| n.version.clone());
        for f in 0..sizes[1] {
            for b in 0..sizes[2] {
                for d in 0..sizes[3] {
                    let s: State = [0, f, b, d];
                    let e = planner.eval(&s);
                    let works = e.unmet.is_empty()
                        && e.gaps.is_empty()
                        && Component::DEVICE
                            .into_iter()
                            .all(|c| planner.transport_ok(&s, c));
                    if !works {
                        continue;
                    }
                    let better = best.is_none_or(|old| {
                        PREFERENCE_ORDER
                            .iter()
                            .map(|c| cmp_opt_version(&version_at(&s, *c), &version_at(&old, *c)))
                            .find(|o| o.is_ne())
                            == Some(Ordering::Greater)
                    });
                    if better {
                        best = Some(s);
                    }
                }
            }
        }
        let Some(end) = best else {
            let newest: State = [0, sizes[1] - 1, sizes[2] - 1, sizes[3] - 1];
            let e = planner.eval(&newest);
            let blockers = Component::DEVICE
                .into_iter()
                .map(|c| {
                    let mut problems: Vec<Problem> = e
                        .unmet
                        .iter()
                        .filter(|u| u.consumer == c || u.provider == Some(c))
                        .map(|u| Problem::Breaks { unmet: u.clone() })
                        .collect();
                    problems.extend(
                        e.gaps
                            .iter()
                            .filter(|g| g.component == c)
                            .map(|g| Problem::NewerThanHost { gap: g.clone() }),
                    );
                    if !planner.transport_ok(&newest, c) {
                        problems.push(Problem::LosesTransport {
                            component: c,
                            interfaces: self.profile.transport(c).to_vec(),
                        });
                    }
                    Blocker {
                        component: c,
                        version: version_at(&newest, c).expect("candidates are releases"),
                        problems,
                    }
                })
                .collect();
            return Err(Refusal::NoSafePath { blockers });
        };
        Ok(ConversionPlan {
            found: Component::DEVICE
                .into_iter()
                .filter_map(|c| self.inventory.get(c).map(|s| (c, s.clone())))
                .collect(),
            install: Component::DEVICE
                .into_iter()
                .map(|c| {
                    let node = planner.node(&end, c).expect("candidate");
                    self.catalog.releases[node.release.expect("candidates are releases")].clone()
                })
                .collect(),
        })
    }

    /// What to tell the user about every component, host first.
    pub fn status(&self) -> Vec<ComponentStatus> {
        Component::ALL
            .into_iter()
            .map(|c| self.component_status(c))
            .collect()
    }

    pub fn component_status(&self, c: Component) -> ComponentStatus {
        let installed = self.installed_node(c).and_then(|n| n.version);
        let installed_withdrawn = installed
            .as_ref()
            .and_then(|v| self.catalog.find(c, v))
            .is_some_and(|r| r.withdrawn);
        let mut status = ComponentStatus {
            component: c,
            installed: installed.clone(),
            state: ComponentState::UpToDate,
            after_host_update: None,
            installed_withdrawn,
            unmet: Vec::new(),
        };
        if let KeyboardMode::NeedsConversion { found } = self.mode() {
            match c {
                Component::Host => {
                    if let Ok(plan) = self.host_only_plan() {
                        let to = plan.steps[0].to.clone();
                        status.state = ComponentState::UpdateAvailable {
                            to,
                            required: false,
                        };
                    }
                }
                _ => {
                    status.state = ComponentState::ConversionRequired {
                        found: found.get(&c).copied().unwrap_or(SoftwareKind::Unknown),
                    }
                }
            }
            return status;
        }

        let planner = self.planner(&BTreeSet::new());
        let now = planner.eval(&[0; 4]);
        status.unmet = now
            .unmet
            .iter()
            // The host's own status is about host updates; a keyboard
            // component that does not meet the host's needs reports it.
            .filter(|u| u.provider == Some(c) || (c != Component::Host && u.consumer == c))
            .cloned()
            .collect();
        let gaps: Vec<Gap> = now
            .gaps
            .iter()
            .filter(|g| g.component == c)
            .cloned()
            .collect();
        if !gaps.is_empty() {
            status.state = ComponentState::NewerThanHost { gaps };
            return status;
        }

        let newest = self
            .newest_offered(c, &installed)
            .map(|r| r.version.clone());
        let with_host = || -> Option<HostGate> {
            if c == Component::Host {
                return None;
            }
            let mut req = PlanRequest::only(c);
            req.components.insert(Component::Host);
            let plan = self.plan(&req).ok()?;
            let targets = plan.targets();
            Some(HostGate {
                to: targets.get(&c)?.clone(),
                host: targets.get(&Component::Host)?.clone(),
            })
        };

        match self.plan(&PlanRequest::only(c)) {
            Ok(plan) => {
                let to = plan.targets().remove(&c).expect("plan for c changes c");
                if c != Component::Host && newest.as_ref().is_some_and(|n| n > &to) {
                    status.after_host_update = with_host().filter(|g| g.to > to);
                }
                status.state = ComponentState::UpdateAvailable {
                    to,
                    required: !status.unmet.is_empty(),
                };
            }
            Err(Refusal::NothingToUpdate) => {
                if !status.unmet.is_empty() {
                    status.state = ComponentState::Incompatible {
                        unmet: status.unmet.clone(),
                    };
                }
            }
            Err(refusal) => {
                if let Some(gate) = with_host() {
                    status.state = ComponentState::NeedsNewerHost {
                        to: gate.to,
                        host: gate.host,
                    };
                } else if let Some(to) = newest {
                    status.state = ComponentState::UpdateBlocked { to, refusal };
                } else if !status.unmet.is_empty() {
                    status.state = ComponentState::Incompatible {
                        unmet: status.unmet.clone(),
                    };
                }
            }
        }
        status
    }
}

impl<'e> Planner<'e> {
    fn node(&self, s: &State, c: Component) -> Option<&Node> {
        self.cands[c.index()].get(s[c.index()])
    }

    /// Nodes in `s`, other than `except`, that provide `interface`.
    fn providers(&self, s: &State, interface: &str, except: Component) -> Vec<(Component, &Node)> {
        Component::ALL
            .into_iter()
            .filter(|c| *c != except)
            .filter_map(|c| self.node(s, c).map(|n| (c, n)))
            .filter(|(_, n)| !n.interfaces.provided(interface).is_empty())
            .collect()
    }

    fn eval(&self, s: &State) -> Eval {
        let profile = self.engine.profile;
        let mut unmet = Vec::new();
        for c in Component::ALL {
            let Some(node) = self.node(s, c) else {
                continue;
            };
            for (iface, reqs) in &node.interfaces.requires {
                let providers = self.providers(s, iface, c);
                if providers
                    .iter()
                    .any(|(_, p)| satisfies(p.interfaces.provided(iface), reqs))
                {
                    continue;
                }
                let expected = profile.providers.get(iface).copied();
                let (provider, provided) = match providers.first() {
                    Some((pc, p)) => (Some(*pc), p.interfaces.provided(iface).to_vec()),
                    None => (expected, Vec::new()),
                };
                unmet.push(Unmet {
                    consumer: c,
                    interface: iface.clone(),
                    requires: reqs.clone(),
                    provider,
                    provided,
                });
            }
        }
        unmet.sort();

        let mut gaps = Vec::new();
        if let Some(host) = self.node(s, Component::Host) {
            for (iface, reqs) in &host.interfaces.requires {
                for (pc, p) in self.providers(s, iface, Component::Host) {
                    let provided = p.interfaces.provided(iface);
                    let understands = host.interfaces.understands(iface);
                    let max_req_major = reqs.iter().map(|r| r.major).max().unwrap_or(0);
                    if !satisfies(provided, reqs) {
                        if let Some(&newest) = provided.iter().max() {
                            if newest.major > max_req_major {
                                gaps.push(Gap {
                                    component: pc,
                                    interface: iface.clone(),
                                    device: newest,
                                    host_understands: understands,
                                    effect: GapEffect::Unavailable,
                                });
                            }
                        }
                        continue;
                    }
                    let kind = profile
                        .kinds
                        .get(iface)
                        .copied()
                        .unwrap_or(InterfaceKind::Protocol);
                    if kind != InterfaceKind::Data {
                        continue;
                    }
                    let Some(&used) = provided
                        .iter()
                        .filter(|v| reqs.iter().any(|r| r.accepts(**v)))
                        .max()
                    else {
                        continue;
                    };
                    if let Some(u) = understands {
                        if used.major == u.major && used.minor > u.minor {
                            gaps.push(Gap {
                                component: pc,
                                interface: iface.clone(),
                                device: used,
                                host_understands: Some(u),
                                effect: GapEffect::ReadOnly,
                            });
                        }
                    }
                }
            }
        }
        gaps.sort();
        Eval { unmet, gaps }
    }

    /// The host can install to and recover `c` in state `s`.
    fn transport_ok(&self, s: &State, c: Component) -> bool {
        if c == Component::Host {
            return true;
        }
        let Some(host) = self.node(s, Component::Host) else {
            return false;
        };
        if self.node(s, c).is_none() {
            return false;
        }
        let ifaces = self.engine.profile.transport(c);
        !ifaces.is_empty()
            && ifaces.iter().all(|i| {
                let reqs = host.interfaces.required(i);
                !reqs.is_empty()
                    && self
                        .providers(s, i, Component::Host)
                        .iter()
                        .any(|(_, p)| satisfies(p.interfaces.provided(i), reqs))
            })
    }

    fn transports(&self, s: &State) -> BTreeSet<Component> {
        Component::DEVICE
            .into_iter()
            .filter(|c| self.transport_ok(s, *c))
            .collect()
    }

    /// Problems with moving straight from `start` to `target`, judged as a
    /// final state.
    fn problems(&self, start: &State, target: &State) -> Vec<Problem> {
        let before = self.eval(start);
        let after = self.eval(target);
        let mut out = Vec::new();
        let kept = self.transports(target);
        for c in self.transports(start) {
            if !kept.contains(&c) {
                out.push(Problem::LosesTransport {
                    component: c,
                    interfaces: self.engine.profile.transport(c).to_vec(),
                });
            }
        }
        let unmet_before = before.unmet_keys();
        for u in after.unmet {
            if !unmet_before.contains(&u.key()) {
                out.push(Problem::Breaks { unmet: u });
            }
        }
        let gaps_before = before.gap_keys();
        for g in after.gaps {
            if !gaps_before.contains(&g.key()) {
                out.push(Problem::NewerThanHost { gap: g });
            }
        }
        out
    }

    fn plan(&self, requested: &[Component]) -> Result<Plan, Refusal> {
        let start: State = [0; 4];
        let start_eval = self.eval(&start);
        let start_unmet = start_eval.unmet_keys();
        let start_gaps = start_eval.gap_keys();
        let required_transports = self.transports(&start);

        let degraded = |e: &Eval| {
            !e.unmet_keys().is_subset(&start_unmet) || !e.gap_keys().is_subset(&start_gaps)
        };

        // Shortest paths, preferring paths whose intermediate states keep
        // everything working. Cost is (degraded intermediate states, steps,
        // components in step order); the last part settles ties in favour of
        // firmware, then bridge, then display, so a component goes after the
        // ones that carry its data and transport.
        let mut dist: BTreeMap<State, (Cost, Option<(State, Component)>)> = BTreeMap::new();
        let mut evals: BTreeMap<State, Eval> = BTreeMap::new();
        let mut heap = BinaryHeap::new();
        dist.insert(start, ((0, 0, Vec::new()), None));
        heap.push(Reverse(((0usize, 0usize, Vec::new()), start)));
        evals.insert(start, start_eval);
        while let Some(Reverse((cost, s))) = heap.pop() {
            if dist.get(&s).is_some_and(|(d, _)| *d < cost) {
                continue;
            }
            let penalty = if s != start && degraded(&evals[&s]) {
                1
            } else {
                0
            };
            for c in Component::ALL {
                let ci = c.index();
                let Some(cur) = self.node(&s, c) else {
                    continue;
                };
                if !self.transport_ok(&s, c) {
                    continue;
                }
                for next in (s[ci] + 1)..self.cands[ci].len() {
                    if cmp_opt_version(&self.cands[ci][next].version, &cur.version)
                        != Ordering::Greater
                    {
                        continue;
                    }
                    let mut t = s;
                    t[ci] = next;
                    if !self.transport_ok(&t, c) {
                        continue;
                    }
                    if !required_transports
                        .iter()
                        .all(|d| self.transport_ok(&t, *d))
                    {
                        continue;
                    }
                    let mut order = cost.2.clone();
                    order.push(ci);
                    let nc = (cost.0 + penalty, cost.1 + 1, order);
                    if dist.get(&t).is_none_or(|(d, _)| nc < *d) {
                        dist.insert(t, (nc.clone(), Some((s, c))));
                        evals.entry(t).or_insert_with(|| self.eval(&t));
                        heap.push(Reverse((nc, t)));
                    }
                }
            }
        }

        let version_at = |s: &State, c: Component| self.node(s, c).and_then(|n| n.version.clone());
        let mut best: Option<State> = None;
        for (s, (cost, _)) in &dist {
            let e = &evals[s];
            if *s == start || degraded(e) {
                continue;
            }
            if !requested.iter().any(|c| s[c.index()] != start[c.index()]) {
                continue;
            }
            let better = match best {
                None => true,
                Some(b) => {
                    let be = &evals[&b];
                    let companions = |x: &State| {
                        Component::ALL
                            .iter()
                            .filter(|c| !requested.contains(c) && x[c.index()] != start[c.index()])
                            .count()
                    };
                    let order = PREFERENCE_ORDER
                        .iter()
                        .filter(|c| requested.contains(c))
                        .map(|c| cmp_opt_version(&version_at(s, *c), &version_at(&b, *c)))
                        .find(|o| o.is_ne())
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| companions(s).cmp(&companions(&b)).reverse())
                        .then_with(|| e.unmet.len().cmp(&be.unmet.len()).reverse())
                        .then_with(|| {
                            PREFERENCE_ORDER
                                .iter()
                                .filter(|c| !requested.contains(c))
                                .map(|c| cmp_opt_version(&version_at(s, *c), &version_at(&b, *c)))
                                .find(|o| o.is_ne())
                                .unwrap_or(Ordering::Equal)
                        })
                        .then_with(|| cost.cmp(&dist[&b].0).reverse());
                    order == Ordering::Greater
                }
            };
            if better {
                best = Some(*s);
            }
        }

        let held_back = |reached: Option<&State>| -> Vec<Blocker> {
            let mut out = Vec::new();
            for &c in requested {
                let ci = c.index();
                let Some(top) = self.cands[ci].len().checked_sub(1).filter(|i| *i > 0) else {
                    continue;
                };
                if reached.is_some_and(|r| r[ci] == top) {
                    continue;
                }
                let mut alone = start;
                alone[ci] = top;
                let mut problems = self.problems(&start, &alone);
                let reports_lost_transport = problems.iter().any(
                    |p| matches!(p, Problem::LosesTransport { component, .. } if *component == c),
                );
                if !self.transport_ok(&alone, c) && !reports_lost_transport {
                    let interfaces = self.engine.profile.transport(c).to_vec();
                    problems.insert(
                        0,
                        Problem::LosesTransport {
                            component: c,
                            interfaces,
                        },
                    );
                }
                out.push(Blocker {
                    component: c,
                    version: self.cands[ci][top]
                        .version
                        .clone()
                        .expect("candidates are releases"),
                    problems,
                });
            }
            out
        };

        let Some(end) = best else {
            let any_newer = requested.iter().any(|c| self.cands[c.index()].len() > 1);
            if !any_newer {
                return Err(Refusal::NothingToUpdate);
            }
            return Err(Refusal::NoSafePath {
                blockers: held_back(None),
            });
        };

        let mut path = Vec::new();
        let mut cur = end;
        while let Some((prev, c)) = dist[&cur].1 {
            path.push((prev, cur, c));
            cur = prev;
        }
        path.reverse();

        let mut steps = Vec::new();
        for (i, (prev, next, c)) in path.iter().enumerate() {
            let from = version_at(prev, *c).expect("Keyglass components have versions");
            let node = self.node(next, *c).expect("step target exists");
            let release = self.engine.catalog.releases
                [node.release.expect("step target is a release")]
            .clone();
            let moves_again = path[i + 1..].iter().any(|(_, _, d)| d == c);
            let reason = if moves_again {
                StepReason::Transitional
            } else if requested.contains(c) {
                StepReason::Requested
            } else {
                StepReason::Companion
            };
            let temporarily_unmet = if *next == end {
                Vec::new()
            } else {
                evals[next]
                    .unmet
                    .iter()
                    .filter(|u| !start_unmet.contains(&u.key()))
                    .cloned()
                    .collect()
            };
            steps.push(Step {
                component: *c,
                from,
                to: release.version.clone(),
                reason,
                transport: self.engine.profile.transport(*c).to_vec(),
                temporarily_unmet,
                release,
            });
        }
        let end_eval = &evals[&end];
        Ok(Plan {
            steps,
            remaining_unmet: end_eval.unmet.clone(),
            held_back: held_back(Some(&end)),
        })
    }
}
