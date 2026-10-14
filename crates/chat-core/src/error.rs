use thiserror::Error;

pub type Result<T, E = ChatError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum ChatError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("not found")]
    NotFound,

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),

    /// The account exists but its email address has not been confirmed.
    #[error("email not verified")]
    EmailNotVerified,

    /// A per-account throttle, distinct from the per-IP request limiter.
    #[error("too many requests: {0}")]
    TooManyRequests(String),

    #[error("conflict: {0}")]
    Conflict(String),

    /// No model provider has been configured for this instance/user yet.
    #[error("no model provider configured")]
    ProviderNotConfigured,

    /// The upstream model provider returned an error.
    #[error("upstream provider error: {0}")]
    Upstream(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl ChatError {
    /// HTTP status code for this error, used by the API layer.
    pub fn status_code(&self) -> u16 {
        match self {
            ChatError::NotFound => 404,
            ChatError::Unauthorized => 401,
            ChatError::Forbidden => 403,
            ChatError::BadRequest(_) => 400,
            ChatError::EmailNotVerified => 403,
            ChatError::TooManyRequests(_) => 429,
            ChatError::Conflict(_) => 409,
            ChatError::ProviderNotConfigured => 428,
            ChatError::Upstream(_) => 502,
            ChatError::Config(_) | ChatError::Database(_) | ChatError::Internal(_) => 500,
        }
    }
}
