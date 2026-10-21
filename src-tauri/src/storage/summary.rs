//! SQLite representation of structured summary bodies. Format-gated by schema.
use crate::domain::Summary;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

impl FromSql for Summary {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let summary: Self =
            serde_json::from_str(value.as_str()?).map_err(|e| FromSqlError::Other(Box::new(e)))?;
        if summary.is_empty() {
            return Ok(summary);
        }
        summary
            .validated()
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}
impl ToSql for Summary {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let value = if self.is_empty() {
            self.clone()
        } else {
            self.clone()
                .validated()
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
        };
        serde_json::to_string(&value)
            .map(|s| ToSqlOutput::Owned(s.into()))
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    }
}
