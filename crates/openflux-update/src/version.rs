//! Interface versions, requirements and component names.
//!
//! An interface version is `MAJOR.MINOR`. A minor bump is additive: a provider
//! at `1.4` serves every consumer that needs `1.0` through `1.4`. A major bump
//! is breaking. Requirements are therefore always bounded by one major:
//! `>=1.2, <2.0`. A consumer that works with two majors lists two requirements.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One of the four independently released parts of OpenFlux.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    /// Desktop software: daemon, Studio, CLI.
    Host,
    /// RP2040 key-scanner firmware.
    Firmware,
    /// Keyboard USB/UART bridge daemon.
    Bridge,
    /// Keyboard display application.
    Display,
}

impl Component {
    /// Every component, host first.
    pub const ALL: [Component; 4] = [
        Component::Host,
        Component::Firmware,
        Component::Bridge,
        Component::Display,
    ];
    /// The components that run on the keyboard.
    pub const DEVICE: [Component; 3] = [Component::Firmware, Component::Bridge, Component::Display];

    pub fn as_str(self) -> &'static str {
        match self {
            Component::Host => "host",
            Component::Firmware => "firmware",
            Component::Bridge => "bridge",
            Component::Display => "display",
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Component::Host => 0,
            Component::Firmware => 1,
            Component::Bridge => 2,
            Component::Display => 3,
        }
    }
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Component {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "host" => Ok(Component::Host),
            "firmware" => Ok(Component::Firmware),
            "bridge" => Ok(Component::Bridge),
            "display" => Ok(Component::Display),
            _ => Err(ParseError(format!(
                "unknown component {s:?} (expected host, firmware, bridge or display)"
            ))),
        }
    }
}

/// A parse failure with a message meant for the person who wrote the value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(pub String);

/// `MAJOR.MINOR` version of an interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InterfaceVersion {
    pub major: u32,
    pub minor: u32,
}

impl InterfaceVersion {
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for InterfaceVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

fn parse_number(s: &str, whole: &str) -> Result<u32, ParseError> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return Err(ParseError(format!(
            "invalid interface version {whole:?} (expected MAJOR.MINOR, for example 1.0)"
        )));
    }
    s.parse()
        .map_err(|_| ParseError(format!("interface version {whole:?} is out of range")))
}

impl FromStr for InterfaceVersion {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        let (major, minor) = t.split_once('.').ok_or_else(|| {
            ParseError(format!(
                "invalid interface version {s:?} (expected MAJOR.MINOR, for example 1.0)"
            ))
        })?;
        Ok(Self::new(parse_number(major, s)?, parse_number(minor, s)?))
    }
}

impl Serialize for InterfaceVersion {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for InterfaceVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A requirement on one major of an interface: `>=MAJOR.MIN_MINOR, <MAJOR+1.0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Requirement {
    pub major: u32,
    pub min_minor: u32,
}

impl Requirement {
    pub const fn new(major: u32, min_minor: u32) -> Self {
        Self { major, min_minor }
    }

    /// The lowest version that satisfies this requirement.
    pub fn minimum(&self) -> InterfaceVersion {
        InterfaceVersion::new(self.major, self.min_minor)
    }

    pub fn accepts(&self, v: InterfaceVersion) -> bool {
        v.major == self.major && v.minor >= self.min_minor
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            ">={}.{}, <{}.0",
            self.major,
            self.min_minor,
            u64::from(self.major) + 1
        )
    }
}

impl FromStr for Requirement {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || {
            ParseError(format!(
                "invalid requirement {s:?} (expected a range within one major, for example \">=1.2, <2.0\")"
            ))
        };
        let (lower, upper) = s.split_once(',').ok_or_else(bad)?;
        let lower = lower.trim().strip_prefix(">=").ok_or_else(bad)?;
        let upper = upper.trim().strip_prefix('<').ok_or_else(bad)?;
        let lower: InterfaceVersion = lower.parse().map_err(|_| bad())?;
        let upper: InterfaceVersion = upper.parse().map_err(|_| bad())?;
        let Some(next_major) = lower.major.checked_add(1) else {
            return Err(ParseError(format!(
                "requirement {s:?} has no upper bound: major {} is the largest there is",
                lower.major
            )));
        };
        if upper.minor != 0 || upper.major != next_major {
            return Err(ParseError(format!(
                "requirement {s:?} must stay within one major: write \">={lower}, <{next_major}.0\""
            )));
        }
        Ok(Self::new(lower.major, lower.minor))
    }
}

impl Serialize for Requirement {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Requirement {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// True when any of `provided` satisfies any of `required`.
pub fn satisfies(provided: &[InterfaceVersion], required: &[Requirement]) -> bool {
    required
        .iter()
        .any(|r| provided.iter().any(|p| r.accepts(*p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_round_trips_and_is_major_bounded() {
        let r: Requirement = ">=1.2, <2.0".parse().unwrap();
        assert_eq!(r, Requirement::new(1, 2));
        assert_eq!(r.to_string(), ">=1.2, <2.0");
        assert!(">=1.2,<2.0".parse::<Requirement>().is_ok());
        assert!(">=1.2, <3.0".parse::<Requirement>().is_err());
        assert!(">=1.2, <2.1".parse::<Requirement>().is_err());
        assert!("1.2".parse::<Requirement>().is_err());
        assert!(r.accepts(InterfaceVersion::new(1, 2)));
        assert!(r.accepts(InterfaceVersion::new(1, 9)));
        assert!(!r.accepts(InterfaceVersion::new(1, 1)));
        assert!(!r.accepts(InterfaceVersion::new(2, 0)));
    }

    #[test]
    fn requirement_at_the_largest_major_is_refused_and_still_displays() {
        let max = u32::MAX;
        for s in [
            format!(">={max}.0, <0.0"),
            format!(">={max}.0, <{max}.0"),
            format!(">={max}.0, <4294967296.0"),
        ] {
            assert!(s.parse::<Requirement>().is_err(), "{s} should not parse");
        }
        let last = max - 1;
        let r: Requirement = format!(">={last}.3, <{max}.0").parse().unwrap();
        assert_eq!(r, Requirement::new(last, 3));
        assert_eq!(
            Requirement::new(max, 1).to_string(),
            format!(">={max}.1, <4294967296.0")
        );
    }

    #[test]
    fn interface_version_parsing_is_strict() {
        assert_eq!(
            "1.0".parse::<InterfaceVersion>().unwrap(),
            InterfaceVersion::new(1, 0)
        );
        for bad in ["1", "1.", ".1", "1.0.0", "01.0", "a.b", "-1.0"] {
            assert!(
                bad.parse::<InterfaceVersion>().is_err(),
                "{bad} should not parse"
            );
        }
    }
}
