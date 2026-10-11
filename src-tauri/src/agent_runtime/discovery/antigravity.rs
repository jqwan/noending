//! `agy models` emits tab-separated model IDs and display names.
use super::{run_cli, warn, ModelCatalog, ModelOption};
use crate::domain::Agent;

pub(crate) fn discover() -> (ModelCatalog, Vec<String>) {
    match run_cli(Agent::Antigravity, &["models"]) {
        Ok(raw) => {
            let models = parse(&raw);
            if models.is_empty() {
                (
                    ModelCatalog::Unavailable,
                    warn("Antigravity 模型列表为空或格式无法识别"),
                )
            } else {
                (ModelCatalog::Dynamic(models), vec![])
            }
        }
        Err(e) => (
            ModelCatalog::Unavailable,
            warn(format!("Antigravity 模型列表获取失败: {e}")),
        ),
    }
}

fn parse(raw: &str) -> Vec<ModelOption> {
    let mut models = Vec::new();
    for line in raw.lines() {
        let Some((id, name)) = line.split_once('\t') else {
            continue;
        };
        let (id, name) = (id.trim(), name.trim());
        if id.is_empty()
            || name.is_empty()
            || id.chars().any(char::is_whitespace)
            || models.iter().any(|m: &ModelOption| m.id == id)
        {
            continue;
        }
        models.push(ModelOption {
            id: id.into(),
            display_name: Some(name.into()),
            provider: None,
            supported_efforts: vec![],
        });
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_tab_separated_catalog_without_progress_or_duplicates() {
        let models = parse("Fetching available models...\ngemini-3.8-flash-high\tGemini 3.8 Flash (High)\nclaude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\ngemini-3.8-flash-high\tduplicate\n\tmissing id\ninvalid id\tbad\n");
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["gemini-3.8-flash-high", "claude-sonnet-4-6"]
        );
        assert_eq!(
            models[1].display_name.as_deref(),
            Some("Claude Sonnet 4.6 (Thinking)")
        );
        assert!(models[0].supported_efforts.is_empty());
        assert!(parse("Fetching available models...\n").is_empty());
    }
}
