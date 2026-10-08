use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use std::sync::Arc;
use uuid::Uuid;

use chat_auth::verify_token;

use crate::error::ApiError;
use crate::state::AppState;

/// Authenticated caller, extracted from the `Authorization: Bearer <jwt>` header.
pub struct AuthUser {
    pub id: Uuid,
    pub role: String,
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(ApiError(chat_core::ChatError::Unauthorized))?;

        let claims = verify_token(&state.cfg.jwt_secret, header)?;
        let id = claims
            .sub
            .parse::<Uuid>()
            .map_err(|_| ApiError(chat_core::ChatError::Unauthorized))?;

        Ok(AuthUser {
            id,
            role: claims.role,
        })
    }
}
