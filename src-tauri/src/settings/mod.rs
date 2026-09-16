//! Application settings management.

use crate::context::ContextDeliveryLevel;
use crate::error::Result;
use crate::storage::Db;

pub const CONTEXT_DELIVERY_LEVEL_KEY: &str = "context.delivery_level";

pub fn context_delivery_level_of(db: &Db) -> Result<ContextDeliveryLevel> {
    if let Some(v) = db.get_setting(CONTEXT_DELIVERY_LEVEL_KEY)? {
        if let Some(lvl) = ContextDeliveryLevel::parse(&v) {
            return Ok(lvl);
        }
    }
    Ok(ContextDeliveryLevel::Balanced)
}

pub fn set_context_delivery_level(db: &Db, level: ContextDeliveryLevel) -> Result<()> {
    db.set_setting(CONTEXT_DELIVERY_LEVEL_KEY, level.as_str())
}
