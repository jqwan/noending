//! Agent Runtime Configuration v0.1 — override semantics.
//!
//! Invariant under test: NoEnding stores explicit overrides only. `None`
//! means *Agent default* and must survive persistence as "no argument",
//! never as a value NoEnding invented.

use noending::agent_runtime::{
    capabilities_of, get_runtime_overrides, runtime_exec_options, set_runtime_overrides,
    validate_runtime_overrides, AgentRuntimeOverrides,
};
use noending::domain::Agent;
use noending::storage::{new_id, Db};
use noending::sync::extractor::CliExtractor;

fn temp_db() -> Db {
    let dir = std::env::temp_dir().join(format!("noending-runtime-test-{}", new_id()));
    Db::open(&dir.join("test.db")).expect("temp db")
}

fn overrides(
    model: Option<&str>,
    provider: Option<&str>,
    effort: Option<&str>,
) -> AgentRuntimeOverrides {
    AgentRuntimeOverrides {
        model: model.map(String::from),
        provider: provider.map(String::from),
        effort: effort.map(String::from),
    }
}

#[test]
fn no_override_resolves_to_no_exec_options_at_all() {
    let db = temp_db();
    for agent in Agent::all() {
        let opts = runtime_exec_options(&db, *agent).expect("defaults always resolve");
        assert_eq!(
            (opts.model, opts.provider, opts.effort),
            (None, None, None),
            "{agent:?} with no stored override must pass no runtime argument"
        );
    }
}

#[test]
fn a_missing_row_and_an_empty_blob_mean_the_same_thing() {
    let db = temp_db();
    assert!(get_runtime_overrides(&db, Agent::Codex)
        .expect("read")
        .is_default());
    db.set_setting("agent.runtime.codex", "{}").unwrap();
    assert!(get_runtime_overrides(&db, Agent::Codex)
        .expect("read")
        .is_default());
}

#[test]
fn codex_model_and_effort_override_roundtrip() {
    let db = temp_db();
    set_runtime_overrides(
        &db,
        Agent::Codex,
        &overrides(Some("gpt-5.6-sol"), None, Some("high")),
    )
    .expect("codex supports model + effort");
    let opts = runtime_exec_options(&db, Agent::Codex).unwrap();
    assert_eq!(opts.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(opts.effort.as_deref(), Some("high"));
    assert_eq!(opts.provider, None, "provider was never overridden");
}

#[test]
fn claude_effort_override_is_accepted_and_forwarded() {
    let db = temp_db();
    set_runtime_overrides(
        &db,
        Agent::ClaudeCode,
        &overrides(Some("sonnet"), None, Some("high")),
    )
    .expect("claude supports model + effort");
    let opts = runtime_exec_options(&db, Agent::ClaudeCode).unwrap();
    assert_eq!(opts.model.as_deref(), Some("sonnet"));
    assert_eq!(opts.effort.as_deref(), Some("high"));
}

#[test]
fn pi_provider_model_and_thinking_override() {
    let db = temp_db();
    set_runtime_overrides(
        &db,
        Agent::Pi,
        &overrides(Some("qwen/qwen3.8-27b"), Some("lmstudio"), Some("low")),
    )
    .expect("pi supports all three fields");
    let opts = runtime_exec_options(&db, Agent::Pi).unwrap();
    assert_eq!(opts.provider.as_deref(), Some("lmstudio"));
    assert_eq!(opts.model.as_deref(), Some("qwen/qwen3.8-27b"));
    assert_eq!(opts.effort.as_deref(), Some("low"));
}

#[test]
fn unsupported_fields_are_rejected_on_write_and_on_read() {
    let db = temp_db();
    for agent in [Agent::Codex, Agent::ClaudeCode] {
        let bad = overrides(None, Some("openai"), None);
        let err = validate_runtime_overrides(agent, &bad).unwrap_err();
        assert!(
            err.to_string().contains("provider"),
            "names the field: {err}"
        );
        assert!(
            set_runtime_overrides(&db, agent, &bad).is_err(),
            "{agent:?} must refuse to persist an unsupported override"
        );
        assert!(
            get_runtime_overrides(&db, agent)
                .expect("nothing was written")
                .is_default(),
            "a rejected write must leave no trace"
        );
    }

    // A row that bypassed validation (hand-edited settings) fails loudly at
    // launch instead of being silently ignored.
    db.set_setting("agent.runtime.claude_code", r#"{"provider":"openai"}"#)
        .unwrap();
    assert!(runtime_exec_options(&db, Agent::ClaudeCode).is_err());

    // A legitimate re-write replaces the whole row, so the Settings UI can
    // repair it without a delete step.
    set_runtime_overrides(
        &db,
        Agent::ClaudeCode,
        &overrides(Some("sonnet"), None, None),
    )
    .unwrap();
    let repaired = runtime_exec_options(&db, Agent::ClaudeCode).unwrap();
    assert_eq!(repaired.model.as_deref(), Some("sonnet"));
    assert_eq!(repaired.provider, None);
}

#[test]
fn overrides_never_leak_between_agents() {
    let db = temp_db();
    set_runtime_overrides(
        &db,
        Agent::Codex,
        &overrides(Some("gpt-5.6-sol"), None, Some("high")),
    )
    .unwrap();
    set_runtime_overrides(&db, Agent::Pi, &overrides(None, Some("lmstudio"), None)).unwrap();

    let claude = runtime_exec_options(&db, Agent::ClaudeCode).unwrap();
    assert_eq!(
        (claude.model, claude.provider, claude.effort),
        (None, None, None),
        "an untouched Agent keeps its own defaults"
    );
    let pi = runtime_exec_options(&db, Agent::Pi).unwrap();
    assert_eq!(
        pi.model, None,
        "codex's model override must not become pi's"
    );
    assert_eq!(
        pi.effort, None,
        "codex's effort must not become pi's thinking"
    );
}

#[test]
fn blank_input_is_agent_default_and_clears_the_row() {
    let db = temp_db();
    set_runtime_overrides(
        &db,
        Agent::Codex,
        &overrides(Some("gpt-5.6-sol"), None, None),
    )
    .unwrap();
    assert!(db.get_setting("agent.runtime.codex").unwrap().is_some());

    // "Use Agent default" for the only overridden field: row goes away.
    set_runtime_overrides(&db, Agent::Codex, &overrides(Some("   "), None, Some(""))).unwrap();
    assert!(db.get_setting("agent.runtime.codex").unwrap().is_none());
    assert!(runtime_exec_options(&db, Agent::Codex)
        .unwrap()
        .model
        .is_none());
}

#[test]
fn a_corrupt_override_row_is_an_error_not_a_silent_default() {
    let db = temp_db();
    db.set_setting("agent.runtime.pi", "not json").unwrap();
    let err = get_runtime_overrides(&db, Agent::Pi).unwrap_err();
    assert!(err.to_string().contains("Pi"), "names the agent: {err}");
    assert!(runtime_exec_options(&db, Agent::Pi).is_err());
}

#[test]
fn overrides_serialise_without_inventing_defaults() {
    let json = serde_json::to_string(&overrides(Some("sonnet"), None, None).normalized()).unwrap();
    assert_eq!(json, r#"{"model":"sonnet","provider":null,"effort":null}"#);
}

/// The UI reads these spellings literally, so a rename is a breaking change.
#[test]
fn serialized_vocabulary_is_the_ui_contract() {
    let caps = serde_json::to_value(capabilities_of(Agent::ClaudeCode)).unwrap();
    assert_eq!(
        caps,
        serde_json::json!({ "model": "suggested", "provider": "unsupported", "effort": "suggested" })
    );
    assert_eq!(
        serde_json::to_value(Agent::ClaudeCode).unwrap(),
        serde_json::json!("claude_code")
    );
    assert_eq!(
        noending::agent_runtime::discovery::effort_levels_for(Agent::Pi)
            .first()
            .map(String::as_str),
        Some("off"),
        "pi can switch thinking off"
    );
    assert!(
        noending::agent_runtime::discovery::effort_levels_for(Agent::Codex)
            .iter()
            .all(|e| e != "off")
    );
}

#[test]
fn extractor_names_what_was_actually_passed() {
    let db = temp_db();
    assert_eq!(
        CliExtractor::try_for_agent(&db, "codex")
            .unwrap()
            .unwrap()
            .name(),
        "cli:codex:agent-default",
        "no override must read as Agent default, never as a model NoEnding chose"
    );

    set_runtime_overrides(
        &db,
        Agent::Codex,
        &overrides(Some("gpt-5.6-luna"), None, Some("low")),
    )
    .unwrap();
    let name = CliExtractor::try_for_agent(&db, "codex")
        .unwrap()
        .unwrap()
        .name();
    assert_eq!(name, "cli:codex:override:model=gpt-5.6-luna,effort=low");
    // The Preview badge and the launch audit both render this one string.
    assert_eq!(
        get_runtime_overrides(&db, Agent::Codex)
            .unwrap()
            .intent_summary(),
        "model=gpt-5.6-luna,effort=low"
    );
    assert!(
        CliExtractor::try_for_agent(&db, "none").unwrap().is_none(),
        "Assistant without an Agent stays retrieval-only"
    );
}

#[test]
fn an_invalid_override_errors_for_the_assistant_instead_of_downgrading() {
    let db = temp_db();
    db.set_setting("agent.runtime.claude_code", r#"{"provider":"openai"}"#)
        .unwrap();
    // an invalid override must surface, not downgrade
    assert!(CliExtractor::try_for_agent(&db, "claude_code").is_err());
}

#[test]
fn a_corrupt_assistant_agent_is_not_mistaken_for_retrieval_only() {
    let db = temp_db();
    db.set_setting("assistant.agent", "no_such_agent").unwrap();
    // `none` is the ONLY value that means "no model on purpose".
    assert!(CliExtractor::try_for_agent(&db, "none").unwrap().is_none());
    let err = CliExtractor::try_for_agent(&db, "no_such_agent")
        .err()
        .expect("an unresolvable Agent must not become retrieval-only");
    assert!(
        err.to_string().contains("no_such_agent"),
        "names the value: {err}"
    );
    assert!(
        CliExtractor::try_from_settings(&db).is_err(),
        "Assistant reports an unresolvable Agent instead of quietly answering from retrieval"
    );
}
