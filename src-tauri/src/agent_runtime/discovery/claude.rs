//! Claude Code runtime options.
//!
//! v0.1 does not attempt to list the account's real models, so the catalog is
//! `Suggested`: model ALIASES the CLI documents (`--model` accepts an alias
//! such as `sonnet`, or a full name like `claude-sonnet-4-6`), not a claim
//! about what this account can currently run. Display names follow the
//! user's own alias mapping in `settings.json` (`ANTHROPIC_DEFAULT_<ALIAS>_MODEL`),
//! the same source the CLI's `/model` picker renders as "Custom Opus model".
//! Custom input stays available.

use std::path::PathBuf;

use super::{ModelCatalog, ModelOption};

/// `claude --help`: "Effort level for the current session (low, medium, high, xhigh, max)".
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

const ALIASES: &[(&str, &str)] = &[("sonnet", "Sonnet"), ("opus", "Opus"), ("haiku", "Haiku")];

pub(crate) fn discover() -> (ModelCatalog, Vec<String>) {
    let env = settings_env().unwrap_or_default();
    let models = ALIASES
        .iter()
        .map(|(alias, label)| ModelOption {
            id: alias.to_string(),
            display_name: Some(
                alias_display_name(&env, alias).unwrap_or_else(|| label.to_string()),
            ),
            provider: None,
            // Aliases do not declare which effort levels they accept.
            supported_efforts: vec![],
        })
        .collect();
    (ModelCatalog::Suggested(models), vec![])
}

fn settings_env() -> Option<serde_json::Map<String, serde_json::Value>> {
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))?;
    let raw = std::fs::read_to_string(root.join("settings.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value.get("env")?.as_object().cloned()
}

fn alias_display_name(
    env: &serde_json::Map<String, serde_json::Value>,
    alias: &str,
) -> Option<String> {
    let key = |suffix: &str| format!("ANTHROPIC_DEFAULT_{}_{}", alias.to_uppercase(), suffix);
    let value = |name: &str| {
        env.get(name)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    value(&key("MODEL_NAME")).or_else(|| value(&key("MODEL")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env(entries: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect()
    }

    #[test]
    fn prefers_the_explicit_display_name_over_the_model_id() {
        let e = env(&[
            ("ANTHROPIC_DEFAULT_OPUS_MODEL_NAME", "My Opus"),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", "qwen-x"),
        ]);
        assert_eq!(alias_display_name(&e, "opus").as_deref(), Some("My Opus"));
    }

    #[test]
    fn falls_back_to_the_mapped_model_id() {
        let e = env(&[("ANTHROPIC_DEFAULT_SONNET_MODEL", "qwen-x")]);
        assert_eq!(alias_display_name(&e, "sonnet").as_deref(), Some("qwen-x"));
    }

    #[test]
    fn ignores_empty_and_unmapped_aliases() {
        let e = env(&[
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME", ""),
            ("ANTHROPIC_MODEL", "qwen-x"),
        ]);
        assert_eq!(alias_display_name(&e, "haiku"), None);
        assert_eq!(alias_display_name(&e, "opus"), None);
    }

    #[test]
    fn unknown_alias_yields_nothing() {
        assert_eq!(
            alias_display_name(&serde_json::Map::new(), "nonsense"),
            None
        );
    }
}
