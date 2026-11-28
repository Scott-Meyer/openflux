//! Hierarchical topic validation and MQTT-style wildcard matching (`+` and `#`).

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TopicError {
    #[error("topic must not be empty")]
    Empty,
    #[error("topic must not start or end with '/' or contain empty segments: {0:?}")]
    EmptySegment(String),
    #[error("publish topic must not contain wildcards '+' or '#': {0:?}")]
    WildcardInPublishTopic(String),
    #[error("multi-level wildcard '#' must be the last segment and occupy a full segment: {0:?}")]
    InvalidMultiLevelWildcard(String),
    #[error("single-level wildcard '+' must occupy an entire segment: {0:?}")]
    InvalidSingleLevelWildcard(String),
    #[error("topic contains invalid whitespace or control character: {0:?}")]
    InvalidCharacter(String),
}

fn check_basic_chars(s: &str) -> Result<(), TopicError> {
    if s.is_empty() {
        return Err(TopicError::Empty);
    }
    if s.starts_with('/') || s.ends_with('/') {
        return Err(TopicError::EmptySegment(s.to_string()));
    }
    if s.bytes().any(|b| b <= 0x20 || b == 0x7F) {
        return Err(TopicError::InvalidCharacter(s.to_string()));
    }
    Ok(())
}

/// Validates a concrete publish topic (no wildcards allowed).
pub fn validate_topic(topic: &str) -> Result<(), TopicError> {
    check_basic_chars(topic)?;
    for seg in topic.split('/') {
        if seg.is_empty() {
            return Err(TopicError::EmptySegment(topic.to_string()));
        }
        if seg.contains('+') || seg.contains('#') {
            return Err(TopicError::WildcardInPublishTopic(topic.to_string()));
        }
    }
    Ok(())
}

/// Validates a subscription pattern (`+` matches one segment, `#` matches zero or more trailing segments).
pub fn validate_pattern(pattern: &str) -> Result<(), TopicError> {
    check_basic_chars(pattern)?;
    let segments: Vec<&str> = pattern.split('/').collect();
    for (idx, seg) in segments.iter().enumerate() {
        if seg.is_empty() {
            return Err(TopicError::EmptySegment(pattern.to_string()));
        }
        if seg.contains('#') && (*seg != "#" || idx + 1 != segments.len()) {
            return Err(TopicError::InvalidMultiLevelWildcard(pattern.to_string()));
        }
        if seg.contains('+') && *seg != "+" {
            return Err(TopicError::InvalidSingleLevelWildcard(pattern.to_string()));
        }
    }
    Ok(())
}

/// Returns true if a concrete `topic` matches a validated MQTT-style `pattern`.
///
/// Examples:
/// - `input/keys/analog` matches `input/keys/analog`, `input/+/analog`, `input/#`, `#`
/// - `input/keys` matches `input/keys/#` (MQTT zero-or-more trailing semantics)
pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    if pattern == "#" {
        return !topic.is_empty();
    }
    let mut pat_iter = pattern.split('/');
    let mut top_iter = topic.split('/');

    loop {
        match (pat_iter.next(), top_iter.next()) {
            (Some("#"), _) => return true,
            (Some("+"), Some(_)) => continue,
            (Some(p), Some(t)) if p == t => continue,
            (None, None) => return true,
            // Support MQTT parent-level match: `a/b/#` matches `a/b`
            (Some(p), None) => return p == "#",
            _ => return false,
        }
    }
}

/// Parsed and validated topic pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TopicPattern(String);

impl TopicPattern {
    pub fn parse(pattern: impl Into<String>) -> Result<Self, TopicError> {
        let s = pattern.into();
        validate_pattern(&s)?;
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches(&self, topic: &str) -> bool {
        topic_matches(&self.0, topic)
    }
}
