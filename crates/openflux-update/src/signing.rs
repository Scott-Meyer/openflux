//! Key generation and signing, used by the release pipeline.
//!
//! Hosts never sign anything; this module exists for `openflux-release` and
//! for tests. Release signing runs only in the release workflow, where the
//! release key is available as an environment secret.

use std::io::Read;

use crate::trust::{KeyId, PublicKey, Signature};
use crate::version::ParseError;

/// A minisign secret key.
pub struct SecretKey {
    inner: minisign::SecretKey,
    public: PublicKey,
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretKey({})", self.public.id())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SigningError {
    #[error("{0}")]
    Key(String),
    #[error("signing failed: {0}")]
    Sign(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl SecretKey {
    /// Parse a minisign secret key: the two-line `.key` file or its base64
    /// line alone. A password-protected key needs `password`.
    pub fn parse(text: &str, password: Option<String>) -> Result<Self, SigningError> {
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
            .ok_or_else(|| SigningError::Key("secret key is empty".to_string()))?;
        let boxed = minisign::SecretKeyBox::from_string(&format!(
            "untrusted comment: openflux secret key\n{line}\n"
        ))
        .map_err(|e| SigningError::Key(format!("not a minisign secret key: {e}")))?;
        let inner = match password {
            Some(p) => minisign::SecretKey::from_box(boxed, Some(p)),
            None => minisign::SecretKey::from_unencrypted_box(boxed),
        }
        .map_err(|e| {
            SigningError::Key(format!(
                "secret key could not be opened ({e}); a password-protected key needs its password"
            ))
        })?;
        let pk = minisign::PublicKey::from_secret_key(&inner)
            .map_err(|e| SigningError::Key(e.to_string()))?;
        let public =
            PublicKey::parse(&pk.to_base64()).map_err(|ParseError(e)| SigningError::Key(e))?;
        Ok(Self { inner, public })
    }

    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }

    pub fn id(&self) -> KeyId {
        self.public.id()
    }

    /// Sign everything `data` yields. Returns the `.sig` file contents.
    pub fn sign<R: Read>(&self, data: R, trusted_comment: &str) -> Result<String, SigningError> {
        let untrusted = format!("signature from openflux key {}", self.id());
        let pk = minisign::PublicKey::from_base64(&self.public.to_base64())
            .map_err(|e| SigningError::Key(e.to_string()))?;
        let sig = minisign::sign(
            Some(&pk),
            &self.inner,
            data,
            Some(trusted_comment),
            Some(&untrusted),
        )
        .map_err(|e| SigningError::Sign(e.to_string()))?;
        let text = sig.into_string();
        Signature::parse(&text).map_err(|e| SigningError::Sign(e.to_string()))?;
        Ok(text)
    }

    pub fn sign_bytes(&self, data: &[u8], trusted_comment: &str) -> Result<String, SigningError> {
        self.sign(data, trusted_comment)
    }
}

/// A freshly generated, unencrypted key pair.
pub struct GeneratedKey {
    pub secret: SecretKey,
    /// `.key` file contents. Keep offline.
    pub secret_text: String,
    /// `.pub` file contents.
    pub public_text: String,
}

pub fn generate() -> Result<GeneratedKey, SigningError> {
    let pair = minisign::KeyPair::generate_unencrypted_keypair()
        .map_err(|e| SigningError::Key(e.to_string()))?;
    let secret_text = pair
        .sk
        .to_box(Some("openflux secret key"))
        .map_err(|e| SigningError::Key(e.to_string()))?
        .into_string();
    let secret = SecretKey::parse(&secret_text, None)?;
    let public_text = secret.public_key().to_file_text();
    Ok(GeneratedKey {
        secret,
        secret_text,
        public_text,
    })
}
