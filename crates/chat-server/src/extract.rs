use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use std::sync::Arc;
use uuid::Uuid;

use chrono::{Duration, Utc};

use chat_auth::verify_token;
use chat_core::ChatError;

use crate::error::ApiError;
use crate::state::AppState;

/// How stale `last_seen_at` may get before the next request refreshes it.
const LAST_SEEN_REFRESH_SECONDS: i64 = 300;

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

        // Re-read the row so a disabled account or a role change takes effect
        // immediately, instead of waiting for the token to expire.
        let user = state
            .store
            .get_user(id)
            .await?
            .ok_or(ApiError(ChatError::Unauthorized))?;
        if user.disabled {
            return Err(ApiError(ChatError::Forbidden));
        }

        // Best-effort activity tracking: refresh in the background only when the
        // previous timestamp is old, so it costs no write on most requests.
        let stale = user
            .last_seen_at
            .map(|t| {
                Utc::now().signed_duration_since(t) > Duration::seconds(LAST_SEEN_REFRESH_SECONDS)
            })
            .unwrap_or(true);
        if stale {
            let store = state.store.clone();
            tokio::spawn(async move {
                if let Err(err) = store.touch_user_last_seen(id).await {
                    tracing::debug!(%id, error = %err, "failed to update last_seen_at");
                }
            });
        }

        Ok(AuthUser {
            id,
            role: user.role,
        })
    }
}

/// An authenticated caller that must hold the `admin` role. Use in handlers
/// under `/api/admin/*` (or any admin-only action).
pub struct AdminUser(pub AuthUser);

impl FromRequestParts<Arc<AppState>> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if user.role != "admin" {
            return Err(ApiError(ChatError::Forbidden));
        }
        Ok(AdminUser(user))
    }
}
