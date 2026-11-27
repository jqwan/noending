//! Live executable availability, separate from authentication and model access.
use crate::{
    domain::AgentTool,
    platform::exec_resolver::{self, AgentToolInstallation},
};
use serde_json::{json, Value};

pub(super) fn list() -> Value {
    list_with(|agent| exec_resolver::resolve(agent).ok())
}
fn list_with(mut resolve: impl FnMut(AgentTool) -> Option<AgentToolInstallation>) -> Value {
    let items: Vec<_> = AgentTool::all()
        .iter()
        .map(|agent| {
            let supports_cli = !exec_resolver::cli_names(*agent).is_empty();
            let installation = if supports_cli { resolve(*agent) } else { None };
            json!({
                "tool":agent, "name":agent.display_name(), "supports_cli":supports_cli,
                "cli_available":installation.is_some(),
                "executable_path":installation.as_ref().map(|i| &i.executable_path),
                "version":installation.as_ref().and_then(|i| i.version.as_ref()),
                "supports_extraction":crate::sync::extractor::supports_extraction(*agent),
                "runtime_capabilities":crate::agent_runtime::capabilities_of(*agent)
            })
        })
        .collect();
    json!({"items":items,"availability_scope":"local_executable"})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn availability_distinguishes_installed_missing_and_history_only_tools() {
        let result = list_with(|agent| {
            assert!(!exec_resolver::cli_names(agent).is_empty());
            (agent == AgentTool::Codex).then(|| AgentToolInstallation {
                agent,
                executable_path: "/test/codex".into(),
                version: Some("test-version".into()),
            })
        });
        let item = |id: &str| {
            result["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["tool"] == id)
                .unwrap()
        };
        assert_eq!(item("codex")["cli_available"], true);
        assert_eq!(item("codex")["executable_path"], "/test/codex");
        assert_eq!(item("codex")["version"], "test-version");
        assert_eq!(item("codex")["supports_extraction"], true);
        assert_eq!(item("pi")["supports_cli"], true);
        assert_eq!(item("pi")["cli_available"], false);
        assert_eq!(item("pi")["supports_extraction"], true);
        assert_eq!(item("qoder")["supports_cli"], false);
        assert_eq!(item("qoder")["cli_available"], false);
        assert_eq!(item("qoder")["supports_extraction"], false);
        assert_eq!(item("antigravity")["supports_extraction"], true);
    }
}
