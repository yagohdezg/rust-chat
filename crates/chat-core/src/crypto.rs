//! Symmetric encryption for secrets stored at rest.
//!
//! Only provider API keys use this today, but the cipher is generic. Values are
//! encrypted with ChaCha20-Poly1305 (authenticated) under a 256-bit key and
//! stored as `enc.v1:<base64(nonce || ciphertext || tag)>`. The versioned,
//! self-describing prefix lets us detect legacy plaintext rows and rotate the
//! scheme later without guessing.

use base64::Engine as _;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use ring::hkdf::{Salt, HKDF_SHA256};
use ring::rand::{SecureRandom, SystemRandom};

use crate::error::ChatError;
use crate::Result;

/// Marker prefixed to every ciphertext so plaintext and encrypted rows can
/// coexist during migration.
pub const ENCRYPTED_PREFIX: &str = "enc.v1:";

/// HKDF salt/domain separator for keys derived from an existing secret.
const DERIVE_SALT: &[u8] = b"rust-chat/provider-encryption/v1";
const DERIVE_INFO: &[u8] = b"provider-api-key";

/// Authenticated encryption for small secrets at rest.
#[derive(Clone)]
pub struct SecretCipher {
    key: LessSafeKey,
}

impl SecretCipher {
    /// Build a cipher from a raw 256-bit key.
    pub fn from_key_bytes(key: &[u8]) -> Result<Self> {
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, key)
            .map_err(|_| ChatError::Config("encryption key must be 32 bytes".into()))?;
        Ok(Self {
            key: LessSafeKey::new(unbound),
        })
    }

    /// Build a cipher from a base64-encoded 256-bit key (`SECRET_ENCRYPTION_KEY`).
    pub fn from_base64(encoded: &str) -> Result<Self> {
        let trimmed = encoded.trim();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(trimmed)
            .map_err(|e| {
                ChatError::Config(format!("SECRET_ENCRYPTION_KEY is not valid base64: {e}"))
            })?;
        Self::from_key_bytes(&raw)
    }

    /// Derive a stable key from an existing secret (e.g. `JWT_SECRET`) via
    /// HKDF-SHA256. Used when no dedicated `SECRET_ENCRYPTION_KEY` is set.
    ///
    /// Rotating the source secret makes previously encrypted values
    /// undecryptable, so production deployments should set a dedicated key.
    pub fn derive_from_secret(secret: &str) -> Result<Self> {
        let salt = Salt::new(HKDF_SHA256, DERIVE_SALT);
        let prk = salt.extract(secret.as_bytes());
        let okm = prk
            .expand(&[DERIVE_INFO], HKDF_SHA256)
            .map_err(|_| ChatError::Config("failed to derive encryption key".into()))?;
        let mut key = [0u8; 32];
        okm.fill(&mut key)
            .map_err(|_| ChatError::Config("failed to derive encryption key".into()))?;
        Self::from_key_bytes(&key)
    }

    /// Encrypt a value into the self-describing storage format.
    pub fn encrypt(&self, plaintext: &str) -> Result<String> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| ChatError::Internal(anyhow::anyhow!("failed to generate nonce")))?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);

        let mut buf = plaintext.as_bytes().to_vec();
        self.key
            .seal_in_place_append_tag(nonce, Aad::empty(), &mut buf)
            .map_err(|_| ChatError::Internal(anyhow::anyhow!("failed to encrypt secret")))?;

        let mut out = nonce_bytes.to_vec();
        out.extend_from_slice(&buf);
        Ok(format!(
            "{ENCRYPTED_PREFIX}{}",
            base64::engine::general_purpose::STANDARD.encode(out)
        ))
    }

    /// Decrypt a stored value. Values without the [`ENCRYPTED_PREFIX`] are
    /// legacy plaintext and returned unchanged, so existing rows keep working
    /// until the backfill re-encrypts them.
    pub fn decrypt(&self, stored: &str) -> Result<String> {
        let Some(encoded) = stored.strip_prefix(ENCRYPTED_PREFIX) else {
            return Ok(stored.to_string());
        };
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| ChatError::Internal(anyhow::anyhow!("corrupt encrypted secret: {e}")))?;
        if raw.len() <= NONCE_LEN {
            return Err(ChatError::Internal(anyhow::anyhow!(
                "corrupt encrypted secret: too short"
            )));
        }
        let (nonce_bytes, ciphertext) = raw.split_at_mut(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce_bytes)
            .map_err(|_| ChatError::Internal(anyhow::anyhow!("corrupt encrypted secret: nonce")))?;
        let plaintext = self
            .key
            .open_in_place(nonce, Aad::empty(), ciphertext)
            .map_err(|_| ChatError::Internal(anyhow::anyhow!("failed to decrypt secret")))?;
        String::from_utf8(plaintext.to_vec())
            .map_err(|e| ChatError::Internal(anyhow::anyhow!("decrypted secret is not UTF-8: {e}")))
    }

    /// Decrypt an optional stored value.
    pub fn decrypt_opt(&self, stored: Option<&str>) -> Result<Option<String>> {
        stored.map(|s| self.decrypt(s)).transpose()
    }

    /// Whether a stored value is already encrypted by this scheme.
    pub fn is_encrypted(stored: &str) -> bool {
        stored.starts_with(ENCRYPTED_PREFIX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> SecretCipher {
        SecretCipher::derive_from_secret("test-secret").unwrap()
    }

    #[test]
    fn round_trip() {
        let c = cipher();
        let encrypted = c.encrypt("sk-super-secret").unwrap();
        assert!(SecretCipher::is_encrypted(&encrypted));
        assert_ne!(encrypted, "sk-super-secret");
        assert_eq!(c.decrypt(&encrypted).unwrap(), "sk-super-secret");
    }

    #[test]
    fn encryption_is_nondeterministic() {
        let c = cipher();
        assert_ne!(c.encrypt("same").unwrap(), c.encrypt("same").unwrap());
    }

    #[test]
    fn legacy_plaintext_passes_through() {
        let c = cipher();
        assert_eq!(c.decrypt("sk-legacy").unwrap(), "sk-legacy");
        assert!(!SecretCipher::is_encrypted("sk-legacy"));
    }

    #[test]
    fn wrong_key_fails_to_decrypt() {
        let encrypted = cipher().encrypt("secret").unwrap();
        let other = SecretCipher::derive_from_secret("other-secret").unwrap();
        assert!(other.decrypt(&encrypted).is_err());
    }

    #[test]
    fn base64_key_round_trip() {
        let raw = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let c = SecretCipher::from_base64(&raw).unwrap();
        assert_eq!(c.decrypt(&c.encrypt("value").unwrap()).unwrap(), "value");
        assert!(SecretCipher::from_base64("not-base64!!").is_err());
    }
}
