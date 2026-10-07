//! The invariant where it actually matters: argv.
//!
//! `None` must mean the argument is ABSENT. A rendered `-m ""` or a guessed
//! default would hand the Agent a value NoEnding invented, which is exactly
//! what the runtime model forbids.

use noending::adapters::{adapter_for, ExecOptions};
use noending::domain::Agent;
use noending::platform::exec_resolver::{cli_names, AgentInstallation};

/// The argv contract only exists for Agents that HAVE a CLI. Qoder is
/// IDE-hosted: there is no command line to render, and its builders must fail
/// instead of inventing one.
fn launchable() -> Vec<Agent> {
    Agent::all()
        .iter()
        .copied()
        .filter(|a| !cli_names(*a).is_empty())
        .collect()
}

/// Agents with a wired headless exec (`build_exec_command`). Antigravity
/// launches interactively but has no one-shot integration yet, so its exec
/// builder still refuses.
fn headless_exec() -> Vec<Agent> {
    vec![Agent::Codex, Agent::ClaudeCode, Agent::Pi]
}

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
        .build_new_command(&install(agent), opts, None)
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
fn every_launch_flow_leaves_runtime_defaults_to_the_cli() {
    for agent in launchable() {
        let opts = ExecOptions::default();
        let mut flows = vec![new_args(agent, &opts), resume_args(agent, &opts)];
        if headless_exec().contains(&agent) {
            let exec = exec_args(agent, &opts);
            assert_eq!(exec.last().map(String::as_str), Some("prompt text"));
            flows.push(exec);
        }
        for args in flows {
            for flag in RUNTIME_FLAGS {
                assert!(
                    !args.iter().any(|a| a.contains(flag)),
                    "{agent:?} launch with no override passed {flag}: {args:?}"
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
        (Agent::Antigravity, "--conversation"),
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
fn codex_renders_model_and_reasoning_effort() {
    let opts = ExecOptions {
        model: Some("gpt-5.6-sol".into()),
        provider: None,
        effort: Some("high".into()),
        root_session_id: None,
    };
    for args in [
        exec_args(Agent::Codex, &opts),
        new_args(Agent::Codex, &opts),
        resume_args(Agent::Codex, &opts),
    ] {
        let pos = args.iter().position(|a| a == "-m").expect("-m");
        assert_eq!(args[pos + 1], "gpt-5.6-sol");
        // effort travels as one literal argv element, never as shell syntax
        assert!(args.iter().any(|a| a == "model_reasoning_effort=\"high\""));
    }
}

#[test]
fn claude_renders_model_and_effort() {
    let opts = ExecOptions {
        model: Some("sonnet".into()),
        provider: None,
        effort: Some("high".into()),
        root_session_id: None,
    };
    for args in [
        exec_args(Agent::ClaudeCode, &opts),
        new_args(Agent::ClaudeCode, &opts),
        resume_args(Agent::ClaudeCode, &opts),
    ] {
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--model" && w[1] == "sonnet"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--effort" && w[1] == "high"),
            "claude effort used to be dropped silently: {args:?}"
        );
    }
}

#[test]
fn pi_renders_provider_model_and_thinking() {
    let opts = ExecOptions {
        model: Some("qwen/qwen3.8-27b".into()),
        provider: Some("lmstudio".into()),
        effort: Some("low".into()),
        root_session_id: None,
    };
    for args in [
        exec_args(Agent::Pi, &opts),
        new_args(Agent::Pi, &opts),
        resume_args(Agent::Pi, &opts),
    ] {
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
            root_session_id: None,
        },
    );
    assert!(args.iter().any(|a| a == value), "{args:?}");
    assert_eq!(args.iter().filter(|a| a.as_str() == "--model").count(), 1);
    assert_eq!(args.last().map(|a| a.as_str()), Some("prompt text"));
}

/// An Agent without a CLI must fail loudly. Returning an empty argv (or a
/// guessed program) would look like a successful launch and hand the user a
/// terminal that never opens — the failure mode "Agent owns defaults" exists
/// to prevent.
#[test]
fn agents_without_a_cli_refuse_to_build_a_command() {
    let cli_less: Vec<Agent> = Agent::all()
        .iter()
        .copied()
        .filter(|a| cli_names(*a).is_empty())
        .collect();
    assert!(
        !cli_less.is_empty(),
        "the roster is expected to contain at least one history-only Agent"
    );

    for agent in cli_less {
        for built in [
            adapter_for(agent).build_new_command(&install(agent), &ExecOptions::default(), None),
            adapter_for(agent).build_resume_command(
                &install(agent),
                &ExecOptions::default(),
                "as-1",
                None,
                None,
            ),
            adapter_for(agent).build_exec_command(&install(agent), &ExecOptions::default(), "p"),
        ] {
            assert!(built.is_err(), "{agent:?} invented a command line");
        }
        // And detection never claims an installation either.
        assert!(adapter_for(agent).detect().is_none(), "{agent:?}");
    }
}
