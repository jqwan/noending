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
    let mut models: Vec<ModelOption> = Vec::new();
    const EFFORT_SUFFIXES: &[(&str, &str, &str)] = &[
        ("-low", "(Low)", "low"),
        ("-medium", "(Medium)", "medium"),
        ("-high", "(High)", "high"),
        ("-xhigh", "(xHigh)", "xhigh"),
        ("-max", "(Max)", "max"),
    ];

    for line in raw.lines() {
        let Some((id, name)) = line.split_once('\t') else {
            continue;
        };
        let (id, name) = (id.trim(), name.trim());
        if id.is_empty() || name.is_empty() || id.chars().any(char::is_whitespace) {
            continue;
        }

        let mut base_id = id;
        let mut clean_name = name;
        let mut effort: Option<&str> = None;

        for &(id_sfx, name_sfx, eff) in EFFORT_SUFFIXES {
            if base_id.ends_with(id_sfx) {
                base_id = &base_id[..base_id.len() - id_sfx.len()];
                effort = Some(eff);
                let trimmed = clean_name.trim_end();
                if trimmed.ends_with(name_sfx) {
                    clean_name = trimmed[..trimmed.len() - name_sfx.len()].trim_end();
                }
                break;
            }
        }

        let trimmed_name = clean_name.trim_end();
        if trimmed_name.ends_with("(Thinking)") {
            clean_name = trimmed_name[..trimmed_name.len() - "(Thinking)".len()].trim_end();
        }

        if let Some(existing) = models.iter_mut().find(|m| m.id == base_id) {
            if let Some(eff) = effort {
                if !existing.supported_efforts.iter().any(|e| e == eff) {
                    existing.supported_efforts.push(eff.to_string());
                }
            }
        } else {
            let mut supported_efforts = Vec::new();
            if let Some(eff) = effort {
                supported_efforts.push(eff.to_string());
            }
            models.push(ModelOption {
                id: base_id.to_string(),
                display_name: Some(clean_name.to_string()),
                provider: None,
                supported_efforts,
            });
        }
    }

    let effort_order = ["low", "medium", "high", "xhigh", "max"];
    for m in &mut models {
        m.supported_efforts.sort_by_key(|e| {
            effort_order
                .iter()
                .position(|&x| x == e.as_str())
                .unwrap_or(usize::MAX)
        });
    }

    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tab_separated_catalog_and_groups_efforts() {
        let raw = "\
Fetching available models...
gemini-3.8-flash-high\tGemini 3.8 Flash (High)
gemini-3.8-flash-medium\tGemini 3.8 Flash (Medium)
gemini-3.8-flash-low\tGemini 3.8 Flash (Low)
gemini-3.1-pro-high\tGemini 3.1 Pro (High)
gemini-3.1-pro-low\tGemini 3.1 Pro (Low)
claude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)
claude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)
gemini-3.8-flash-high\tduplicate
\tmissing id
invalid id\tbad
";
        let models = parse(raw);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec![
                "gemini-3.8-flash",
                "gemini-3.1-pro",
                "claude-sonnet-4-6",
                "claude-opus-4-6-thinking"
            ]
        );
        assert_eq!(models[0].display_name.as_deref(), Some("Gemini 3.8 Flash"));
        assert_eq!(models[0].supported_efforts, vec!["low", "medium", "high"]);
        assert_eq!(models[1].display_name.as_deref(), Some("Gemini 3.1 Pro"));
        assert_eq!(models[1].supported_efforts, vec!["low", "high"]);
        assert_eq!(models[2].display_name.as_deref(), Some("Claude Sonnet 4.6"));
        assert!(models[2].supported_efforts.is_empty());
        assert_eq!(models[3].display_name.as_deref(), Some("Claude Opus 4.6"));
        assert!(models[3].supported_efforts.is_empty());
        assert!(parse("Fetching available models...\n").is_empty());
    }
}
