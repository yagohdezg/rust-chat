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

    #[error("conflict: {0}")]
    Conflict(String),

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
            ChatError::Conflict(_) => 409,
            ChatError::Config(_) | ChatError::Database(_) | ChatError::Internal(_) => 500,
        }
    }
}
