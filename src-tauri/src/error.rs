#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("db: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
    /// A failed explicit Context update, carrying the distinguishable reason
    /// the UI needs to offer the right retry.
    #[error("context: {0:?}")]
    Context(crate::domain::ContextUpdateError),
}

impl AppError {
    pub fn context(err: crate::domain::ContextUpdateError) -> Self {
        AppError::Context(err)
    }
}

impl serde::Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

pub fn other<T: std::fmt::Display>(msg: T) -> AppError {
    AppError::Other(msg.to_string())
}
