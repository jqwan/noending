//! Codex model catalog: `codex debug models` renders the raw catalog as JSON.
//!
//! Only `visibility: "list"` entries are offered — the catalog also carries
//! hidden internal models (`gpt-reserve`, `codex-auto-review`) that a user
//! must not be steered into.

use serde_json::Value;

use super::{run_cli, warn, ModelCatalog, ModelOption};
use crate::domain::Agent;

/// Codex documents no `off` thinking level; reasoning always runs.
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];

pub(crate) fn discover() -> (ModelCatalog, Vec<String>) {
    match run_cli(Agent::Codex, &["debug", "models"]) {
        Ok(raw) => match parse(&raw) {
            Ok(models) if !models.is_empty() => (ModelCatalog::Dynamic(models), vec![]),
            Ok(_) => (ModelCatalog::Unavailable, warn("Codex 返回了空的模型目录")),
            Err(e) => (
                ModelCatalog::Unavailable,
                warn(format!("Codex 模型目录解析失败: {}", e)),
            ),
        },
        Err(e) => (
            ModelCatalog::Unavailable,
            warn(format!("Codex 模型目录获取失败: {}", e)),
        ),
    }
}

fn parse(raw: &str) -> Result<Vec<ModelOption>, String> {
    let root: Value = serde_json::from_str(raw.trim()).map_err(|e| e.to_string())?;
    let entries = root
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "缺少 models 数组".to_string())?;

    let mut out = Vec::new();
    for entry in entries {
        if entry.get("visibility").and_then(Value::as_str) != Some("list") {
            continue;
        }
        let Some(id) = entry.get("slug").and_then(Value::as_str) else {
            continue;
        };
        let supported_efforts = entry
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .map(|levels| {
                levels
                    .iter()
                    .filter_map(|l| l.get("effort").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        out.push(ModelOption {
            id: id.to_string(),
            display_name: entry
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::to_string),
            provider: None,
            supported_efforts,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_only_visible_models_and_carries_their_reasoning_levels() {
        let raw = r#"{"models":[
            {"slug":"gpt-5.6-luna","display_name":"GPT-5.6-Luna","visibility":"list",
             "supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}]},
            {"slug":"codex-auto-review","visibility":"hide","supported_reasoning_levels":[]},
            {"visibility":"list","display_name":"entry without a slug"}
        ]}"#;
        let models = parse(raw).expect("catalog");
        assert_eq!(models.len(), 1, "hidden and slug-less entries are skipped");
        assert_eq!(models[0].id, "gpt-5.6-luna");
        assert_eq!(models[0].provider, None, "codex has no provider dimension");
        assert_eq!(
            models[0].supported_efforts,
            vec!["low".to_string(), "high".to_string()]
        );
    }

    #[test]
    fn unparsable_output_is_an_error_the_caller_turns_into_a_warning() {
        assert!(parse("").is_err());
        assert!(parse(r#"{"nope":[]}"#).is_err());
    }
}
