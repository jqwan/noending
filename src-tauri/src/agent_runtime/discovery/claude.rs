//! Claude Code runtime options.
//!
//! v0.1 does not attempt to list the account's real models, so the catalog is
//! `Suggested`: model ALIASES the CLI documents (`--model` accepts an alias
//! such as `sonnet`, or a full name like `claude-sonnet-4-6`), not a claim
//! about what this account can currently run. Custom input stays available.

use super::ModelCatalog;
use super::ModelOption;

/// `claude --help`: "Effort level for the current session (low, medium, high, xhigh, max)".
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

pub(crate) fn discover() -> (ModelCatalog, Vec<String>) {
    let models = [("sonnet", "Sonnet"), ("opus", "Opus"), ("haiku", "Haiku")]
        .iter()
        .map(|(alias, label)| ModelOption {
            id: alias.to_string(),
            display_name: Some(label.to_string()),
            provider: None,
            // Aliases do not declare which effort levels they accept.
            supported_efforts: vec![],
        })
        .collect();
    (ModelCatalog::Suggested(models), vec![])
}
