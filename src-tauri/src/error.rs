/// Structured, privacy-safe failure returned only by explicit Context updates.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ContextUpdateFailure {
    pub code: String,
    pub message: String,
    pub operation_id: String,
}

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
    #[error("context update failed")]
    ContextUpdateFailed(ContextUpdateFailure),
}

impl AppError {
    pub fn context(err: crate::domain::ContextUpdateError) -> Self {
        AppError::Context(err)
    }
}

impl serde::Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            AppError::ContextUpdateFailed(err) => serde::Serialize::serialize(err, s),
            other => s.serialize_str(&other.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

pub fn other<T: std::fmt::Display>(msg: T) -> AppError {
    AppError::Other(msg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_update_failure_serializes_as_a_structured_safe_payload() {
        let error = AppError::ContextUpdateFailed(ContextUpdateFailure {
            code: "stale_snapshot".into(),
            message: "内容已变化，请重新更新。".into(),
            operation_id: "123e4567-e89b-12d3-a456-426614174000".into(),
        });
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            serde_json::json!({
                "code": "stale_snapshot",
                "message": "内容已变化，请重新更新。",
                "operation_id": "123e4567-e89b-12d3-a456-426614174000"
            })
        );
    }
}
