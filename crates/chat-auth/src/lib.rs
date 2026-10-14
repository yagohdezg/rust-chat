//! Authentication: Argon2 password hashing and JWT session tokens.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine as _;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::rngs::OsRng;
use ring::digest::{digest, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use chat_core::{ChatError, Result};

pub mod breached;
pub mod permissions;
pub use breached::is_breached;
pub use permissions::{permissions_for_role, role_allows, Action, Permission, Scope};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub role: String,
    pub iat: i64,
    pub exp: i64,
}

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| ChatError::Internal(anyhow::anyhow!("hash error: {e}")))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| ChatError::Internal(anyhow::anyhow!("invalid hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub fn issue_token(secret: &str, user_id: Uuid, role: &str, ttl_seconds: i64) -> Result<String> {
    let now = Utc::now();
    let claims = Claims {
        sub: user_id.to_string(),
        role: role.to_string(),
        iat: now.timestamp(),
        exp: (now + Duration::seconds(ttl_seconds)).timestamp(),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| ChatError::Internal(anyhow::anyhow!("jwt encode error: {e}")))
}

pub fn verify_token(secret: &str, token: &str) -> Result<Claims> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|_| ChatError::Unauthorized)
}

/// 256 bits of CSPRNG output, URL-safe base64 without padding.
fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| ChatError::Internal(anyhow::anyhow!("failed to generate token")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Generate an opaque 256-bit refresh token. The plaintext is returned to the
/// client once and stored (hashed) server-side.
pub fn generate_refresh_token() -> Result<String> {
    random_token()
}

/// Generate an opaque single-use token for an email flow (address verification
/// or password reset). Stored hashed, like every other credential.
pub fn generate_auth_token() -> Result<String> {
    random_token()
}

/// Hash an auth token for storage, using the same SHA-256 scheme as refresh
/// tokens and API keys.
pub fn hash_auth_token(token: &str) -> String {
    sha256_hex(token)
}

/// Reject passwords that are too short or that appear in the bundled list of
/// common breached passwords. Applied wherever a password is set:
/// registration, admin creation and password reset.
pub fn validate_password(password: &str) -> Result<()> {
    const MIN_LEN: usize = 8;
    if password.chars().count() < MIN_LEN {
        return Err(ChatError::BadRequest(format!(
            "password must be at least {MIN_LEN} characters"
        )));
    }
    if is_breached(password) {
        return Err(ChatError::BadRequest(
            "password is one of the most commonly breached passwords; choose another".into(),
        ));
    }
    Ok(())
}

/// Prefix on every API key, so a bearer token can be told from a JWT.
pub const API_KEY_PREFIX: &str = "rck_";

fn sha256_hex(value: &str) -> String {
    digest(&SHA256, value.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Hash a refresh token for storage: SHA-256, lowercase hex. Storing only the
/// hash means a database leak does not yield usable tokens, and lookup stays a
/// direct unique-index hit.
pub fn hash_refresh_token(token: &str) -> String {
    sha256_hex(token)
}

/// Generate an opaque API key. The plaintext is returned to the client once and
/// stored (hashed) server-side.
pub fn generate_api_key() -> Result<String> {
    Ok(format!("{API_KEY_PREFIX}{}", random_token()?))
}

/// Hash an API key for storage, using the same SHA-256 scheme as refresh tokens.
pub fn hash_api_key(key: &str) -> String {
    sha256_hex(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_tokens_are_unique_and_hashed() {
        let a = generate_refresh_token().unwrap();
        let b = generate_refresh_token().unwrap();
        assert_ne!(a, b);
        assert_ne!(hash_refresh_token(&a), a);
        assert_ne!(hash_refresh_token(&a), hash_refresh_token(&b));
        // 32 bytes base64url => 43 chars; hash is 64 hex chars.
        assert_eq!(a.len(), 43);
        assert_eq!(hash_refresh_token(&a).len(), 64);
    }

    #[test]
    fn password_hash_round_trip() {
        let hash = hash_password("correct horse").unwrap();
        assert!(verify_password("correct horse", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }
}
