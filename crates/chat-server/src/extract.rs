use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use std::sync::Arc;
use uuid::Uuid;

use chrono::{Duration, Utc};

use chat_auth::{hash_api_key, role_allows, verify_token, Action, Scope, API_KEY_PREFIX};
use chat_core::ChatError;

use crate::error::ApiError;
use crate::state::AppState;

/// How stale `last_seen_at` may get before the next request refreshes it.
const LAST_SEEN_REFRESH_SECONDS: i64 = 300;

/// Authenticated caller, extracted from the `Authorization: Bearer <token>`
/// header. The token is either a JWT session token or an API key.
pub struct AuthUser {
    pub id: Uuid,
    pub role: String,
    /// When the caller authenticated with an API key, the scopes it is limited
    /// to; `None` for a JWT session, which the role alone constrains.
    key_scopes: Option<Vec<Scope>>,
}

impl AuthUser {
    /// Whether this caller may perform `action` on `scope`: the role must allow
    /// it, and an API key must additionally carry the scope.
    pub fn allows(&self, scope: Scope, action: Action) -> bool {
        role_allows(&self.role, scope, action)
            && self
                .key_scopes
                .as_ref()
                .is_none_or(|scopes| scopes.contains(&scope))
    }

    /// Authenticate an API key: hash lookup, then the same account checks as a
    /// session token.
    async fn from_api_key(token: &str, state: &Arc<AppState>) -> Result<Self, ApiError> {
        let key = state
            .store
            .get_api_key_by_hash(&hash_api_key(token))
            .await?
            .ok_or(ApiError(ChatError::Unauthorized))?;
        let expired = key.expires_at.is_some_and(|at| at <= Utc::now());
        if key.revoked_at.is_some() || expired {
            return Err(ApiError(ChatError::Unauthorized));
        }

        let user = state
            .store
            .get_user(key.user_id)
            .await?
            .ok_or(ApiError(ChatError::Unauthorized))?;
        if user.disabled {
            return Err(ApiError(ChatError::Forbidden));
        }

        // Best-effort usage tracking, throttled like `last_seen_at` so most
        // requests cost no write.
        let stale = key
            .last_used_at
            .map(|t| {
                Utc::now().signed_duration_since(t) > Duration::seconds(LAST_SEEN_REFRESH_SECONDS)
            })
            .unwrap_or(true);
        if stale {
            let store = state.store.clone();
            let key_id = key.id;
            tokio::spawn(async move {
                if let Err(err) = store.touch_api_key_last_used(key_id).await {
                    tracing::debug!(%key_id, error = %err, "failed to update api key last_used_at");
                }
            });
        }
        touch_last_seen(state, &user);

        let scopes = key
            .scopes
            .0
            .iter()
            .filter_map(|name| Scope::parse(name))
            .collect();
        Ok(AuthUser {
            id: user.id,
            role: user.role,
            key_scopes: Some(scopes),
        })
    }
}

/// Refresh `last_seen_at` in the background when the previous timestamp is old.
fn touch_last_seen(state: &Arc<AppState>, user: &chat_store::User) {
    let stale = user
        .last_seen_at
        .map(|t| Utc::now().signed_duration_since(t) > Duration::seconds(LAST_SEEN_REFRESH_SECONDS))
        .unwrap_or(true);
    if !stale {
        return;
    }
    let store = state.store.clone();
    let id = user.id;
    tokio::spawn(async move {
        if let Err(err) = store.touch_user_last_seen(id).await {
            tracing::debug!(%id, error = %err, "failed to update last_seen_at");
        }
    });
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(ApiError(chat_core::ChatError::Unauthorized))?;

        if token.starts_with(API_KEY_PREFIX) {
            return Self::from_api_key(token, state).await;
        }

        let claims = verify_token(&state.cfg.jwt_secret, token)?;
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
        touch_last_seen(state, &user);

        Ok(AuthUser {
            id,
            role: user.role,
            key_scopes: None,
        })
    }
}

pub trait ScopedPermission {
    const SCOPE: Scope;
    const ACTION: Action;
}

pub struct Require<P: ScopedPermission>(pub AuthUser, std::marker::PhantomData<P>);

impl<P: ScopedPermission> std::ops::Deref for Require<P> {
    type Target = AuthUser;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<P: ScopedPermission> FromRequestParts<Arc<AppState>> for Require<P> {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if !user.allows(P::SCOPE, P::ACTION) {
            return Err(ApiError(ChatError::Forbidden));
        }
        Ok(Require(user, std::marker::PhantomData))
    }
}

macro_rules! permissions {
    ($($name:ident => $scope:ident / $action:ident),+ $(,)?) => {
        $(
            pub struct $name;
            impl ScopedPermission for $name {
                const SCOPE: Scope = Scope::$scope;
                const ACTION: Action = Action::$action;
            }
        )+
    };
}

permissions! {
    ConversationsView => Conversations / View,
    ConversationsManage => Conversations / Manage,
    MessagesView => Messages / View,
    AgentsView => Agents / View,
    AgentsManage => Agents / Manage,
    McpServersView => McpServers / View,
    McpServersManage => McpServers / Manage,
    ProvidersView => Providers / View,
    ProvidersManage => Providers / Manage,
    FilesView => Files / View,
    FilesManage => Files / Manage,
    ChatUse => Chat / Use,
    SandboxUse => Sandbox / Use,
    ComputersView => Computers / View,
    ComputersUse => Computers / Use,
    ComputersManage => Computers / Manage,
    ModelsView => Models / View,
    ApiKeysView => ApiKeys / View,
    ApiKeysManage => ApiKeys / Manage,
    AdminUsersView => AdminUsers / View,
    AdminUsersManage => AdminUsers / Manage,
    AdminAuditView => AdminAudit / View,
    AdminComputersView => AdminComputers / View,
    AdminProvidersManage => AdminProviders / Manage,
}
