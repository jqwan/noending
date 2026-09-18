//! Pi model catalog: `pi --list-models` prints a fixed-width table whose
//! columns are `provider model context max-out thinking images`.
//!
//! Pi is the only Agent here with a provider dimension, so this is also where
//! the Provider → Model cascade gets its data.

use super::{run_cli, warn, ModelCatalog, ModelOption};
use crate::domain::Agent;

/// `pi --help`: "Set thinking level: off, minimal, low, medium, high, xhigh, max".
pub const EFFORT_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

const THINKING_CAPABLE: &str = "yes";

pub(crate) fn discover() -> (ModelCatalog, Vec<String>, Vec<String>) {
    let levels = EFFORT_LEVELS.iter().map(|s| s.to_string()).collect();
    match run_cli(Agent::Pi, &["--list-models"]) {
        Ok(raw) => {
            let models = parse(&raw);
            if models.is_empty() {
                (
                    ModelCatalog::Unavailable,
                    levels,
                    warn("Pi 模型列表为空或格式无法识别"),
                )
            } else {
                (ModelCatalog::Dynamic(models), levels, vec![])
            }
        }
        Err(e) => (
            ModelCatalog::Unavailable,
            levels,
            warn(format!("Pi 模型列表获取失败: {}", e)),
        ),
    }
}

fn parse(raw: &str) -> Vec<ModelOption> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let columns: Vec<&str> = line.split_whitespace().collect();
        // Header, blank separators and truncated rows carry no usable pair.
        if columns.len() < 5 || columns[0] == "provider" {
            continue;
        }
        let (provider, model) = (columns[0], columns[1]);
        let thinking = columns.get(4).copied().unwrap_or("");
        let supported_efforts = if thinking.eq_ignore_ascii_case(THINKING_CAPABLE) {
            EFFORT_LEVELS.iter().map(|s| s.to_string()).collect()
        } else {
            vec!["off".to_string()]
        };
        out.push(ModelOption {
            id: model.to_string(),
            display_name: None,
            provider: Some(provider.to_string()),
            supported_efforts,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_real_table_shape_and_keeps_provider_pairing() {
        let raw = "provider      model                         context  max-out  thinking  images
openai-codex  gpt-5.6-luna                  272K     128K     yes       yes
lmstudio      qwen/qwen3.8-27b              208.4K   100K     yes       yes
deepseek      deepseek-v4-pro               1M       384K     no        no
";
        let models = parse(raw);
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].provider.as_deref(), Some("openai-codex"));
        assert_eq!(models[1].id, "qwen/qwen3.8-27b");
        assert_eq!(
            models[1].supported_efforts.first().map(String::as_str),
            Some("off")
        );
        assert_eq!(
            models[2].supported_efforts,
            vec!["off".to_string()],
            "a model without thinking only accepts off"
        );
    }

    #[test]
    fn a_header_only_or_garbled_table_yields_no_models() {
        assert!(parse("").is_empty());
        assert!(parse("provider  model  context  max-out  thinking  images\n").is_empty());
        assert!(parse("nope\n").is_empty());
    }
}
