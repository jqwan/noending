//! Optional live Agent smoke tests. Authenticated CLI/model prerequisites are explicit.
use noending::adapters::ExecOptions;
use noending::domain::Agent;
use noending::platform::exec_runner::{clean_exec_stdout, run_headless};
use noending_lib as noending;
// Real-model smoke tests (#[ignore]: need the authenticated agent CLIs)

#[test]
#[ignore = "requires Pi CLI and LM Studio serving qwen/qwen3.8-27b"]
fn real_pi_local_qwen_headless() {
    let install = noending::platform::exec_resolver::resolve(Agent::Pi).expect("pi CLI");
    let adapter = noending::adapters::adapter_for(Agent::Pi);
    let cmd = adapter
        .build_exec_command(
            &install,
            &ExecOptions {
                model: Some("qwen/qwen3.8-27b".into()),
                provider: Some("lmstudio".into()),
                effort: Some("low".into()),
                ..Default::default()
            },
            "只输出一个 JSON 对象，不要其他文字：{\"summary\":\"采用方案A\"}",
        )
        .unwrap();
    let out = run_headless(&cmd, 180).expect("pi headless run");
    let text = clean_exec_stdout(&out.stdout);
    println!("pi qwen output: {}", text);
    assert!(
        text.contains("方案A"),
        "pi+qwen should echo the fixture decision"
    );
}
