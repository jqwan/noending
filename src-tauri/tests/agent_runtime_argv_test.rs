//! The invariant where it actually matters: argv.
//!
//! `None` must mean the argument is ABSENT. A rendered `-m ""` or a guessed
//! default would hand the Agent a value NoEnding invented, which is exactly
//! what the runtime model forbids.

use noending::adapters::{adapter_for, ExecOptions};
use noending::domain::Agent;
use noending::platform::exec_resolver::AgentInstallation;

fn install(agent: Agent) -> AgentInstallation {
    AgentInstallation {
        agent,
        executable_path: format!("/usr/local/bin/{}", agent.as_str()),
        version: Some("test".into()),
        source: "test".into(),
        last_verified_at: String::new(),
    }
}

fn exec_args(agent: Agent, opts: &ExecOptions) -> Vec<String> {
    adapter_for(agent)
        .build_exec_command(&install(agent), opts, "prompt text")
        .expect("build")
        .args
}

fn new_args(agent: Agent, opts: &ExecOptions) -> Vec<String> {
    adapter_for(agent)
        .build_new_command(&install(agent), opts, None, None)
        .expect("build")
        .args
}

fn resume_args(agent: Agent, opts: &ExecOptions) -> Vec<String> {
    adapter_for(agent)
        .build_resume_command(&install(agent), opts, "agent-session-id", None, None)
        .expect("build")
        .args
}

/// New / Resume go to a real interactive terminal, so an invented default
/// there is the most visible way to break "Agent owns defaults".
#[test]
fn interactive_launches_pass_no_runtime_flag_without_an_override() {
    for agent in Agent::all() {
        for args in [
            new_args(agent, &ExecOptions::default()),
            resume_args(agent, &ExecOptions::default()),
        ] {
            for flag in RUNTIME_FLAGS {
                assert!(
                    !args.iter().any(|a| a.contains(flag)),
                    "{agent:?} interactive launch with no override passed {flag}: {args:?}"
                );
            }
        }
    }
}

#[test]
fn resume_keeps_its_session_selector() {
    for (agent, selector) in [
        (Agent::Codex, "resume"),
        (Agent::ClaudeCode, "--resume"),
        (Agent::Pi, "--session"),
    ] {
        let args = resume_args(agent, &ExecOptions::default());
        let at = args.iter().position(|a| a == selector).expect(selector);
        assert_eq!(
            args.get(at + 1).map(|s| s.as_str()),
            Some("agent-session-id"),
            "{agent:?} resume argv: {args:?}"
        );
    }
}

#[test]
fn every_consumer_renders_the_same_override_the_same_way() {
    // One runtime semantics: the flags an interactive session gets are the
    // flags the headless run gets — New / Resume / exec cannot diverge.
    for agent in Agent::all() {
        let opts = ExecOptions {
            model: Some("some-model".into()),
            provider: Some("some-provider".into()),
            effort: Some("medium".into()),
        };
        let runtime = |args: &[String]| -> Vec<String> {
            RUNTIME_FLAGS
                .iter()
                .filter(|f| args.iter().any(|a| a.contains(**f)))
                .map(|f| (*f).to_string())
                .collect()
        };
        let exec = runtime(&exec_args(agent, &opts));
        assert_eq!(runtime(&new_args(agent, &opts)), exec, "{agent:?} new");
        assert_eq!(
            runtime(&resume_args(agent, &opts)),
            exec,
            "{agent:?} resume"
        );
        assert!(!exec.is_empty(), "{agent:?} passed nothing at all");
    }
}

/// Every runtime flag any of the three CLIs understands.
const RUNTIME_FLAGS: &[&str] = &[
    "-m",
    "--model",
    "--provider",
    "--effort",
    "--thinking",
    "model_reasoning_effort",
];

#[test]
fn no_override_leaves_the_cli_to_its_own_defaults() {
    for agent in Agent::all() {
        let args = exec_args(agent, &ExecOptions::default());
        for flag in RUNTIME_FLAGS {
            assert!(
                !args.iter().any(|a| a.contains(flag)),
                "{agent:?} with no override passed {flag}: {args:?}"
            );
        }
        assert!(
            args.last().map(|a| a.as_str()) == Some("prompt text"),
            "{agent:?} still receives its prompt: {args:?}"
        );
    }
}

#[test]
fn codex_renders_model_and_reasoning_effort() {
    let args = exec_args(
        Agent::Codex,
        &ExecOptions {
            model: Some("gpt-5.6-sol".into()),
            provider: None,
            effort: Some("high".into()),
        },
    );
    let pos = args.iter().position(|a| a == "-m").expect("-m");
    assert_eq!(args[pos + 1], "gpt-5.6-sol");
    // effort travels as one literal argv element, never as shell syntax
    assert!(args.iter().any(|a| a == "model_reasoning_effort=\"high\""));
}

#[test]
fn claude_renders_model_and_effort() {
    let args = exec_args(
        Agent::ClaudeCode,
        &ExecOptions {
            model: Some("sonnet".into()),
            provider: None,
            effort: Some("high".into()),
        },
    );
    assert!(args
        .windows(2)
        .any(|w| w[0] == "--model" && w[1] == "sonnet"));
    assert!(
        args.windows(2)
            .any(|w| w[0] == "--effort" && w[1] == "high"),
        "claude effort used to be dropped silently: {args:?}"
    );
}

#[test]
fn pi_renders_provider_model_and_thinking() {
    let args = exec_args(
        Agent::Pi,
        &ExecOptions {
            model: Some("qwen/qwen3.8-27b".into()),
            provider: Some("lmstudio".into()),
            effort: Some("low".into()),
        },
    );
    assert!(args
        .windows(2)
        .any(|w| w[0] == "--provider" && w[1] == "lmstudio"));
    assert!(args
        .windows(2)
        .any(|w| w[0] == "--model" && w[1] == "qwen/qwen3.8-27b"));
    assert!(args
        .windows(2)
        .any(|w| w[0] == "--thinking" && w[1] == "low"));
}

#[test]
fn an_override_value_is_passed_literally_as_one_argv_element() {
    // Values come from user input (Custom model ID): the adapter must not
    // split, interpolate or quote them — quoting is the Platform layer's job.
    let value = "$(id) 'quote' two words";
    let args = exec_args(
        Agent::Pi,
        &ExecOptions {
            model: Some(value.into()),
            provider: None,
            effort: None,
        },
    );
    assert!(args.iter().any(|a| a == value), "{args:?}");
    assert_eq!(args.iter().filter(|a| a.as_str() == "--model").count(), 1);
    assert_eq!(args.last().map(|a| a.as_str()), Some("prompt text"));
}
