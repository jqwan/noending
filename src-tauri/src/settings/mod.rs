//! Application settings management.
//!
//! `settings` is a bare key/value table: no typed row is ever seeded by a
//! migration except where a value has to be pinned explicitly (see
//! `storage::migrate` v11). So every product default lives here, and each key
//! must have exactly ONE accessor that decides what a missing row means.

use crate::context::ContextDeliveryLevel;
use crate::error::Result;
use crate::storage::Db;

pub const CONTEXT_DELIVERY_LEVEL_KEY: &str = "context.delivery_level";
pub const CONTEXT_INTELLIGENCE_ENABLED_KEY: &str = "context.intelligence_enabled";

/// Context Intelligence (extraction, classification, Context mutation,
/// conflict creation, review generation). A missing row — or any value that is
/// not an explicit opt-in — means OFF: the shipped product is Base Experience.
pub fn context_intelligence_enabled(db: &Db) -> Result<bool> {
    Ok(db
        .get_setting(CONTEXT_INTELLIGENCE_ENABLED_KEY)?
        .map(|v| matches!(v.trim().to_lowercase().as_str(), "true" | "1"))
        .unwrap_or(false))
}

pub fn set_context_intelligence_enabled(db: &Db, enabled: bool) -> Result<()> {
    db.set_setting(
        CONTEXT_INTELLIGENCE_ENABLED_KEY,
        if enabled { "true" } else { "false" },
    )
}

/// Outbound context injection level. Orthogonal to
/// [`context_intelligence_enabled`]: Off stops delivery only, and turning it
/// off must never stop ingestion, sync or extraction.
///
/// The default is Off. `ContextDeliveryLevel::default()` intentionally stays
/// Balanced — that is a type default for callers building a level value, not
/// the product default; reading the product default goes through this fn.
pub fn context_delivery_level_of(db: &Db) -> Result<ContextDeliveryLevel> {
    if let Some(v) = db.get_setting(CONTEXT_DELIVERY_LEVEL_KEY)? {
        if let Some(lvl) = ContextDeliveryLevel::parse(&v) {
            return Ok(lvl);
        }
    }
    Ok(ContextDeliveryLevel::Off)
}

pub fn set_context_delivery_level(db: &Db, level: ContextDeliveryLevel) -> Result<()> {
    db.set_setting(CONTEXT_DELIVERY_LEVEL_KEY, level.as_str())
}
