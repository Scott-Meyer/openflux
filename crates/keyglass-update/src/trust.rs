//! Signatures, keys and index acceptance.
//!
//! Signatures use the minisign format (Ed25519 over a BLAKE2b-512 prehash, with
//! a signed trusted comment), so any published file can also be checked with
//! the standard `minisign` tool.
//!
//! Two kinds of keys exist:
//! - **Root keys** are kept offline and sign only [`KeyStatement`]s that add
//!   or revoke release keys. The host is built with the root public keys.
//! - **Release keys** sign `index.json` and every artifact. The host is built
//!   with the initial release public key and learns later ones from
//!   root-signed statements in the index.
//!
//! An index is accepted only if its signature comes from a trusted, unrevoked
//! release key, it has not expired, and its sequence is not lower than the
//! highest sequence accepted before. The caller provides persistence for
//! that sequence and for revocations through [`TrustStore`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Read, Seek, SeekFrom};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::manifest::{Artifact, Index, KeyStatement, ManifestError};
use crate::version::ParseError;

/// Minisign key identifier, shown as 16 uppercase hex digits exactly as the
/// `minisign` tool prints it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(pub [u8; 8]);

impl KeyId {
    fn from_slice(b: &[u8]) -> Option<Self> {
        b.try_into().ok().map(KeyId)
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016X}", u64::from_le_bytes(self.0))
    }
}

impl fmt::Debug for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyId({self})")
    }
}

impl FromStr for KeyId {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 16 {
            return Err(ParseError(format!("key id {s:?} must be 16 hex digits")));
        }
        let n = u64::from_str_radix(s, 16)
            .map_err(|_| ParseError(format!("key id {s:?} must be 16 hex digits")))?;
        Ok(KeyId(n.to_le_bytes()))
    }
}

impl Serialize for KeyId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for KeyId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A minisign public key.
#[derive(Clone)]
pub struct PublicKey {
    inner: minisign::PublicKey,
    id: KeyId,
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.id)
    }
}

impl PublicKey {
    /// Parse a minisign public key: either the two-line `.pub` file or its
    /// base64 line alone.
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
            .ok_or_else(|| ParseError("public key is empty".to_string()))?;
        let inner = minisign::PublicKey::from_base64(line)
            .map_err(|e| ParseError(format!("not a minisign public key: {e}")))?;
        let id = KeyId::from_slice(inner.keynum())
            .ok_or_else(|| ParseError("public key has no key id".to_string()))?;
        Ok(Self { inner, id })
    }

    pub fn id(&self) -> KeyId {
        self.id
    }

    /// The base64 line of the key, as stored in key statements.
    pub fn to_base64(&self) -> String {
        self.inner.to_base64()
    }

    /// The standard two-line `.pub` file contents.
    pub fn to_file_text(&self) -> String {
        format!(
            "untrusted comment: minisign public key {}\n{}\n",
            self.id,
            self.to_base64()
        )
    }

    /// Verify a minisign signature over the bytes read from `data`.
    pub fn verify<R: Read + Seek>(
        &self,
        signature: &Signature,
        data: R,
    ) -> Result<(), SignatureError> {
        if signature.key_id() != self.id {
            return Err(SignatureError::WrongKey {
                expected: self.id,
                signed_by: signature.key_id(),
            });
        }
        if !signature.inner.is_prehashed() {
            return Err(SignatureError::Legacy);
        }
        minisign::verify(&self.inner, &signature.inner, data, true, false, false)
            .map_err(|_| SignatureError::Invalid { key_id: self.id })
    }

    pub fn verify_bytes(&self, signature: &Signature, data: &[u8]) -> Result<(), SignatureError> {
        self.verify(signature, std::io::Cursor::new(data))
    }
}

/// A parsed minisign signature (the contents of a `.sig` file).
#[derive(Clone)]
pub struct Signature {
    inner: minisign::SignatureBox,
    key_id: KeyId,
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature(by {})", self.key_id)
    }
}

impl Signature {
    pub fn parse(text: &str) -> Result<Self, SignatureError> {
        let inner = minisign::SignatureBox::from_string(text.trim_start())
            .map_err(|e| SignatureError::Malformed(e.to_string()))?;
        let key_id = KeyId::from_slice(inner.keynum())
            .ok_or_else(|| SignatureError::Malformed("no key id".into()))?;
        Ok(Self { inner, key_id })
    }

    /// The key that made this signature, as recorded in the signature.
    pub fn key_id(&self) -> KeyId {
        self.key_id
    }

    /// The signed trusted comment. Authentic only after verification.
    pub fn trusted_comment(&self) -> String {
        self.inner.trusted_comment().unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SignatureError {
    #[error("signature is malformed: {0}")]
    Malformed(String),
    #[error("signature was made by key {signed_by}, expected {expected}")]
    WrongKey { expected: KeyId, signed_by: KeyId },
    #[error("signature was made by key {signed_by}, which is not trusted")]
    UnknownKey { signed_by: KeyId },
    #[error("signature uses the legacy non-prehashed minisign format")]
    Legacy,
    #[error("signature does not match the data for key {key_id}")]
    Invalid { key_id: KeyId },
}

/// The keys a host is built with.
#[derive(Clone, Debug, Default)]
pub struct TrustAnchors {
    /// Offline keys that may add or revoke release keys.
    pub root_keys: Vec<PublicKey>,
    /// Release keys trusted without a statement.
    pub release_keys: Vec<PublicKey>,
}

impl TrustAnchors {
    /// Keys trusted for artifacts that do not come with an index, such as the
    /// keyboard software bundled in a host installer.
    pub fn bundled_keys(&self, state: &TrustState) -> TrustedKeys {
        let mut keys = TrustedKeys::default();
        for k in &self.release_keys {
            if !state.revoked.contains(&k.id()) {
                keys.keys.insert(k.id(), k.clone());
            }
        }
        keys
    }
}

/// Release keys that may sign indexes and artifacts.
#[derive(Clone, Debug, Default)]
pub struct TrustedKeys {
    keys: BTreeMap<KeyId, PublicKey>,
}

impl TrustedKeys {
    pub fn ids(&self) -> impl Iterator<Item = KeyId> + '_ {
        self.keys.keys().copied()
    }

    pub fn get(&self, id: KeyId) -> Option<&PublicKey> {
        self.keys.get(&id)
    }

    /// Verify `signature` over `data` with whichever trusted key made it.
    pub fn verify<R: Read + Seek>(
        &self,
        signature: &Signature,
        data: R,
    ) -> Result<KeyId, SignatureError> {
        let key = self
            .keys
            .get(&signature.key_id())
            .ok_or(SignatureError::UnknownKey {
                signed_by: signature.key_id(),
            })?;
        key.verify(signature, data)?;
        Ok(key.id())
    }
}

/// What a host remembers between index fetches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustState {
    /// Highest index sequence accepted so far.
    pub highest_sequence: u64,
    /// Release keys revoked by verified root statements.
    #[serde(default)]
    pub revoked: BTreeSet<KeyId>,
}

/// Persistence for [`TrustState`]. `save` must be durable before it returns:
/// an index is reported as accepted only after its state is saved.
pub trait TrustStore {
    type Error: fmt::Display;
    fn load(&self) -> Result<TrustState, Self::Error>;
    fn save(&mut self, state: &TrustState) -> Result<(), Self::Error>;
}

/// An in-memory [`TrustStore`], for tests and tools.
#[derive(Clone, Debug, Default)]
pub struct MemoryTrustStore {
    pub state: TrustState,
}

impl TrustStore for MemoryTrustStore {
    type Error = std::convert::Infallible;
    fn load(&self) -> Result<TrustState, Self::Error> {
        Ok(self.state.clone())
    }
    fn save(&mut self, state: &TrustState) -> Result<(), Self::Error> {
        self.state = state.clone();
        Ok(())
    }
}

/// Why an index was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IndexError {
    #[error("{0}")]
    Malformed(String),
    #[error("index signature is malformed: {0}")]
    BadSignatureFormat(String),
    #[error("key statement {position} is invalid: {reason}")]
    InvalidStatement { position: usize, reason: String },
    #[error("index is signed by key {key_id}, which is not trusted")]
    UntrustedKey { key_id: KeyId },
    #[error("index is signed by key {key_id}, which has been revoked")]
    RevokedKey { key_id: KeyId },
    #[error("index declares key {declared} but is signed by {signed_by}")]
    KeyIdMismatch { declared: KeyId, signed_by: KeyId },
    #[error("index signature does not verify: {0}")]
    BadSignature(SignatureError),
    #[error("index expired at {expires}")]
    Expired {
        expires: OffsetDateTime,
        now: OffsetDateTime,
    },
    #[error("index sequence {sequence} is older than the accepted sequence {highest}")]
    Replayed { sequence: u64, highest: u64 },
    #[error("trust state could not be loaded or saved: {0}")]
    Store(String),
}

/// An index whose signature, freshness and sequence have been checked.
///
/// Only [`verify_index`] and [`verify_bundled_index`] create one, so holding a `VerifiedIndex` means the
/// releases in it are authentic.
#[derive(Clone, Debug)]
pub struct VerifiedIndex {
    index: Index,
    keys: TrustedKeys,
    signed_by: KeyId,
    bundled: bool,
}

impl VerifiedIndex {
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// Release keys trusted under this index, for verifying artifacts.
    pub fn trusted_keys(&self) -> &TrustedKeys {
        &self.keys
    }

    pub fn signed_by(&self) -> KeyId {
        self.signed_by
    }

    /// True for an index accepted with [`verify_bundled_index`].
    pub fn is_bundled(&self) -> bool {
        self.bundled
    }

    /// The factory reference recorded in this index, if any.
    pub fn factory(&self) -> Option<&crate::factory::FactoryReference> {
        self.index.factory.as_ref()
    }

    /// The press-refinement source recorded in this index, if any.
    pub fn press_refinement_source(&self) -> Option<&crate::factory::PressRefinementSource> {
        self.index.press_refinement_source.as_ref()
    }

    pub fn into_index(self) -> Index {
        self.index
    }
}

/// Verify a root-signed statement and return its contents.
pub fn verify_statement(
    statement: &crate::manifest::SignedStatement,
    roots: &[PublicKey],
) -> Result<KeyStatement, String> {
    let sig = Signature::parse(&statement.signature).map_err(|e| e.to_string())?;
    let root = roots
        .iter()
        .find(|r| r.id() == sig.key_id())
        .ok_or_else(|| format!("signed by {}, which is not a root key", sig.key_id()))?;
    root.verify_bytes(&sig, statement.statement.as_bytes())
        .map_err(|e| e.to_string())?;
    let parsed: KeyStatement = serde_json::from_str(&statement.statement)
        .map_err(|e| format!("unreadable statement: {e}"))?;
    if let KeyStatement::Trust {
        key_id, public_key, ..
    } = &parsed
    {
        let pk = PublicKey::parse(public_key).map_err(|e| e.to_string())?;
        if pk.id() != *key_id {
            return Err(format!(
                "statement names key {key_id} but carries key {}",
                pk.id()
            ));
        }
    }
    Ok(parsed)
}

/// Decide which release keys are trusted given the anchors, the statements
/// in an index and recorded revocations.
pub fn trusted_keys(
    anchors: &TrustAnchors,
    statements: &[crate::manifest::SignedStatement],
    state: &TrustState,
) -> Result<(TrustedKeys, BTreeSet<KeyId>), IndexError> {
    let mut revoked = state.revoked.clone();
    let mut candidates: BTreeMap<KeyId, PublicKey> = anchors
        .release_keys
        .iter()
        .map(|k| (k.id(), k.clone()))
        .collect();
    for (position, s) in statements.iter().enumerate() {
        match verify_statement(s, &anchors.root_keys)
            .map_err(|reason| IndexError::InvalidStatement { position, reason })?
        {
            KeyStatement::Trust { public_key, .. } => {
                let pk =
                    PublicKey::parse(&public_key).map_err(|e| IndexError::InvalidStatement {
                        position,
                        reason: e.to_string(),
                    })?;
                candidates.insert(pk.id(), pk);
            }
            KeyStatement::Revoke { key_id, .. } => {
                revoked.insert(key_id);
            }
        }
    }
    let keys = candidates
        .into_iter()
        .filter(|(id, _)| !revoked.contains(id))
        .collect();
    Ok((TrustedKeys { keys }, revoked))
}

fn check_signature(
    index_json: &[u8],
    signature: &str,
    anchors: &TrustAnchors,
    state: &TrustState,
) -> Result<(Index, TrustedKeys, BTreeSet<KeyId>, KeyId), IndexError> {
    let sig =
        Signature::parse(signature).map_err(|e| IndexError::BadSignatureFormat(e.to_string()))?;
    let index = Index::parse(index_json).map_err(|ManifestError(e)| IndexError::Malformed(e))?;
    let (keys, revoked) = trusted_keys(anchors, &index.key_statements, state)?;
    let signer = sig.key_id();
    if revoked.contains(&signer) {
        return Err(IndexError::RevokedKey { key_id: signer });
    }
    let key = keys
        .get(signer)
        .ok_or(IndexError::UntrustedKey { key_id: signer })?;
    key.verify_bytes(&sig, index_json)
        .map_err(IndexError::BadSignature)?;
    if index.key_id != signer {
        return Err(IndexError::KeyIdMismatch {
            declared: index.key_id,
            signed_by: signer,
        });
    }
    Ok((index, keys, revoked, signer))
}

/// Accept or refuse a fetched index.
///
/// On success the new highest sequence and any new revocations are saved to
/// `store` before this returns. An index with the same sequence as the
/// highest accepted one is accepted again (a repeated fetch of an unchanged
/// index).
pub fn verify_index<S: TrustStore>(
    index_json: &[u8],
    signature: &str,
    anchors: &TrustAnchors,
    store: &mut S,
    now: OffsetDateTime,
) -> Result<VerifiedIndex, IndexError> {
    let state = store.load().map_err(|e| IndexError::Store(e.to_string()))?;
    let (index, keys, revoked, signer) = check_signature(index_json, signature, anchors, &state)?;
    if now >= index.expires {
        return Err(IndexError::Expired {
            expires: index.expires,
            now,
        });
    }
    if index.sequence < state.highest_sequence {
        return Err(IndexError::Replayed {
            sequence: index.sequence,
            highest: state.highest_sequence,
        });
    }
    let new_state = TrustState {
        highest_sequence: index.sequence,
        revoked,
    };
    if new_state != state {
        store
            .save(&new_state)
            .map_err(|e| IndexError::Store(e.to_string()))?;
    }
    Ok(VerifiedIndex {
        index,
        keys,
        signed_by: signer,
        bundled: false,
    })
}

/// Accept the index bundled with a host installer.
///
/// The bundled index is checked with the same signature, key and revocation
/// rules as a fetched one. Expiry and sequence are not checked: the bundle
/// was frozen when the installer was built and is meant for offline installs
/// and recovery, so it is expected to be older than the newest index the host
/// has seen. For the same reason it never changes the stored trust state.
/// Combine it with a fetched index through
/// [`Catalog::add_index`](crate::engine::Catalog::add_index), which keeps any
/// withdrawal either index records.
pub fn verify_bundled_index(
    index_json: &[u8],
    signature: &str,
    anchors: &TrustAnchors,
    state: &TrustState,
) -> Result<VerifiedIndex, IndexError> {
    let (index, keys, _, signer) = check_signature(index_json, signature, anchors, state)?;
    Ok(VerifiedIndex {
        index,
        keys,
        signed_by: signer,
        bundled: true,
    })
}

/// An artifact whose size, hash and signature have been checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedArtifact {
    pub name: String,
    pub sha256: String,
    pub size: u64,
    pub signed_by: KeyId,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ArtifactError {
    #[error("artifact {name} could not be read: {reason}")]
    Io { name: String, reason: String },
    #[error("artifact {name} is {actual} bytes, expected {expected}")]
    Size {
        name: String,
        expected: u64,
        actual: u64,
    },
    #[error("artifact {name} has sha256 {actual}, expected {expected}")]
    Hash {
        name: String,
        expected: String,
        actual: String,
    },
    #[error("artifact {name} has no signature")]
    Unsigned { name: String },
    #[error(
        "artifact {name} is signed only by keys this host does not trust: {}",
        list_ids(key_ids)
    )]
    UntrustedKey { name: String, key_ids: Vec<KeyId> },
    #[error("artifact {name}: {source}")]
    Signature {
        name: String,
        source: SignatureError,
    },
}

fn list_ids(ids: &[KeyId]) -> String {
    ids.iter()
        .map(|k| k.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Lowercase hex SHA-256 and length of everything `reader` yields.
pub fn sha256_hex<R: Read>(mut reader: R) -> std::io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok((hex, total))
}

/// Check a downloaded artifact against its manifest entry: size, SHA-256 and
/// a signature by a trusted release key. The keyboard cannot check any of
/// this itself, so nothing reaches it without passing here.
pub fn verify_artifact<R: Read + Seek>(
    mut data: R,
    artifact: &Artifact,
    keys: &TrustedKeys,
) -> Result<VerifiedArtifact, ArtifactError> {
    let name = artifact.name.clone();
    let io = |e: std::io::Error| ArtifactError::Io {
        name: name.clone(),
        reason: e.to_string(),
    };
    let (sha, size) = sha256_hex(&mut data).map_err(io)?;
    if size != artifact.size {
        return Err(ArtifactError::Size {
            name,
            expected: artifact.size,
            actual: size,
        });
    }
    if sha != artifact.sha256 {
        return Err(ArtifactError::Hash {
            name,
            expected: artifact.sha256.clone(),
            actual: sha,
        });
    }
    if artifact.signatures.is_empty() {
        return Err(ArtifactError::Unsigned { name });
    }
    // Any signature by a trusted, unrevoked key is enough. A signature by a
    // trusted key that does not verify is reported in preference to
    // signatures by keys this host does not trust.
    let mut failure = None;
    let mut untrusted = Vec::new();
    for text in &artifact.signatures {
        let sig = match Signature::parse(text) {
            Ok(sig) => sig,
            Err(source) => {
                failure.get_or_insert(ArtifactError::Signature {
                    name: name.clone(),
                    source,
                });
                continue;
            }
        };
        let Some(key) = keys.get(sig.key_id()) else {
            untrusted.push(sig.key_id());
            continue;
        };
        data.seek(SeekFrom::Start(0)).map_err(io)?;
        match key.verify(&sig, &mut data) {
            Ok(()) => {
                return Ok(VerifiedArtifact {
                    name,
                    sha256: sha,
                    size,
                    signed_by: key.id(),
                })
            }
            Err(source) => {
                failure = Some(ArtifactError::Signature {
                    name: name.clone(),
                    source,
                });
            }
        }
    }
    Err(failure.unwrap_or(ArtifactError::UntrustedKey {
        name,
        key_ids: untrusted,
    }))
}
