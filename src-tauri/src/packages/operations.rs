//! Package mutations. Install/uninstall delegate to the owning Agent's native
//! CLI so downloads, dependencies and install records stay Agent-managed;
//! enable/disable writes the native toggle the Agent itself reads back. All
//! writes are revision-checked against the discovery snapshot.

use super::{files, Origin, PackageEntry, PackageInventory};
use crate::domain::Agent;
use crate::error::{other, Result};
use serde::Serialize;
use std::path::Path;

pub const CLI_TIMEOUT_SECS: u64 = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageAction {
    Install,
    Uninstall,
    Enable,
    Disable,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageOperationResult {
    pub message: String,
}

/// CLI argument mapping per Agent. Pure: tests pin the contract without
/// launching anything. Every package command is user-scope in this iteration.
pub fn cli_args(agent: Agent, action: PackageAction, target: &str) -> Result<Vec<String>> {
    validate_target(target)?;
    let linked;
    let args: Vec<&str> = match (agent, action) {
        (Agent::Pi, PackageAction::Install) => vec!["install", target],
        // The plugin subcommand forwards to pnpm in the desktop profile; the
        // local directory is referenced (link:), never copied.
        (Agent::Dsh, PackageAction::Install) => {
            linked = format!("link:{target}");
            vec!["plugin", "--profile", "desktop", "add", &linked]
        }
        (Agent::Antigravity, PackageAction::Install) => vec!["plugin", "install", target],
        (Agent::Codex, PackageAction::Uninstall) => vec!["plugin", "remove", target, "--json"],
        (Agent::ClaudeCode, PackageAction::Uninstall) => {
            vec!["plugin", "uninstall", target, "--scope", "user"]
        }
        (Agent::Pi, PackageAction::Uninstall) => vec!["remove", target],
        (Agent::Dsh, PackageAction::Uninstall) => {
            vec!["plugin", "--profile", "desktop", "remove", target]
        }
        (Agent::Antigravity, PackageAction::Uninstall) => vec!["plugin", "uninstall", target],
        (Agent::ClaudeCode, PackageAction::Enable) => vec!["plugin", "enable", target, "--json"],
        (Agent::ClaudeCode, PackageAction::Disable) => vec!["plugin", "disable", target, "--json"],
        (Agent::Antigravity, PackageAction::Enable) => vec!["plugin", "enable", target],
        (Agent::Antigravity, PackageAction::Disable) => vec!["plugin", "disable", target],
        _ => {
            return Err(other(
                "不支持的插件包安装方式；本地目录安装仅支持 Pi 和 Antigravity",
            ))
        }
    };
    Ok(args.into_iter().map(str::to_string).collect())
}

/// Rejects option-injection and control characters; local paths are fine.
fn validate_target(target: &str) -> Result<()> {
    if target.trim().is_empty() || target.starts_with('-') || target.contains(['\0', '\n', '\r']) {
        return Err(other("插件目标无效"));
    }
    Ok(())
}

/// `name@market` for market agents; the raw source for Pi and the bare npm
/// name for DSH (its marketplace field is the profile label, not a market).
fn cli_target(entry: &PackageEntry) -> String {
    match entry.marketplace.as_deref() {
        Some(market) if entry.agent != Some(Agent::Pi) && entry.agent != Some(Agent::Dsh) => {
            format!("{}@{market}", entry.name)
        }
        _ => entry.name.clone(),
    }
}

fn run_cli(agent: Agent, args: &[String]) -> Result<()> {
    let installation = crate::platform::exec_resolver::resolve(agent)?;
    let output = crate::platform::exec_runner::run(
        Path::new(&installation.executable_path),
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
        CLI_TIMEOUT_SECS,
    )?;
    if !output.success {
        let reason = if output.exit_code.is_none() {
            "操作超时，请检查 Agent 的安装状态".to_string()
        } else {
            format!("{}\n{}", output.stderr.trim(), output.stdout.trim())
        };
        return Err(other(reason.chars().take(5000).collect::<String>()));
    }
    Ok(())
}

/// Market-name install was removed: NoEnding never triggers a marketplace
/// download through Codex/Claude selectors. Install args only carry local
/// directories (Pi/Antigravity imports), see [`install_imported`].
pub fn uninstall(inventory: &PackageInventory, id: &str) -> Result<PackageOperationResult> {
    let entry = locate(inventory, id)?;
    let agent = entry
        .agent
        .ok_or_else(|| other("该插件包没有所属 Agent，无法卸载"))?;
    let args = cli_args(agent, PackageAction::Uninstall, &cli_target(entry))?;
    if agent == Agent::Dsh {
        // Undeclare first: a failed removal then leaves an installed-but-
        // undeclared package (benign), never a declared-but-missing one.
        super::dsh::undeclare_bundle(&crate::skills::Roots::current()?, &entry.name)?;
    }
    run_cli(agent, &args)?;
    Ok(PackageOperationResult {
        message: "插件包已卸载；已有终端请重新加载或重新打开".into(),
    })
}

/// Applies the enable/disable switch. Claude and Antigravity expose native
/// CLI commands, so their toggles ride those (the CLI owns validation and
/// file layout); Codex has no such subcommand and writes config.toml
/// directly. The revision guard still rejects stale snapshots for all three.
pub fn set_enabled(
    inventory: &PackageInventory,
    id: &str,
    expected_revision: &str,
    enable: bool,
) -> Result<PackageOperationResult> {
    let entry = locate(inventory, id)?;
    files::check_revision(&entry.revision, expected_revision)?;
    let action = if enable {
        PackageAction::Enable
    } else {
        PackageAction::Disable
    };
    match entry.agent {
        Some(Agent::Codex) => {
            // Each discovered entry carries exactly one scope.
            let scope = entry
                .scopes
                .first()
                .ok_or_else(|| other("该插件包没有可启停的配置"))?;
            let raw = files::read(Path::new(&scope.config_path))?;
            let text = codex_toggle(&raw, entry, enable)?;
            files::write_atomic(Path::new(&scope.config_path), &text)?;
        }
        Some(Agent::ClaudeCode) => {
            let args = cli_args(Agent::ClaudeCode, action, &cli_target(entry))?;
            run_cli(Agent::ClaudeCode, &args)?;
        }
        Some(Agent::Antigravity) => {
            // agy's enable/disable keys on the bare plugin name, matching
            // `plugin list --json`.
            let args = cli_args(Agent::Antigravity, action, &entry.name)?;
            run_cli(Agent::Antigravity, &args)?;
        }
        _ => return Err(other("该 Agent 未提供插件启停开关")),
    }
    Ok(PackageOperationResult {
        message: "启停配置已保存，下次启动 Agent 生效".into(),
    })
}

fn locate<'a>(inventory: &'a PackageInventory, id: &str) -> Result<&'a PackageEntry> {
    let entry = inventory
        .packages
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| other("插件包已不存在，请刷新"))?;
    // Codex's bundled markets are ordinary config.toml entries — writing
    // `enabled` there is exactly how the Agent itself toggles them, so
    // Bundled is not blocked here (capability flags already gate the UI).
    // NoEnding's own built-ins stay app-managed.
    if entry.origin == Origin::NoEndingManaged {
        return Err(other("NoEnding 内置插件包由应用管理，不能在这里操作"));
    }
    if !entry.capabilities.toggle && !entry.capabilities.uninstall {
        return Err(other("该 Agent 未提供插件包管理命令"));
    }
    Ok(entry)
}

fn codex_toggle(raw: &str, entry: &PackageEntry, enable: bool) -> Result<String> {
    let mut doc = raw
        .parse::<toml_edit::Document>()
        .map_err(|e| other(format!("config.toml 无法解析：{e}")))?;
    let selector = cli_target(entry);
    doc["plugins"][&selector]["enabled"] = toml_edit::value(enable);
    Ok(doc.to_string())
}

/// Pure marketplace command mapping; tests pin the contract without
/// launching anything.
pub fn marketplace_add_args(market_path: &str) -> Vec<String> {
    ["plugin", "marketplace", "add", market_path]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

pub fn marketplace_install_args(
    agent: Agent,
    plugin: &str,
    market_path: &str,
    market_name: &str,
) -> Result<Vec<String>> {
    validate_target(plugin)?;
    validate_target(market_name)?;
    let args: Vec<String> = match agent {
        // Claude installs straight from a marketplace source path; the
        // marketplace is registered automatically on first use.
        Agent::ClaudeCode => [
            "plugin",
            "install",
            plugin,
            "--marketplace",
            market_path,
            "--json",
        ],
        // Codex needs the marketplace registered first and selects by
        // `name@market`.
        Agent::Codex => {
            return Ok(vec![
                "plugin".into(),
                "add".into(),
                format!("{plugin}@{market_name}"),
            ])
        }
        _ => return Err(other("该 Agent 不支持从本地市场安装")),
    }
    .iter()
    .map(|s| s.to_string())
    .collect();
    Ok(args)
}

/// Installs a local marketplace directory's plugins into the selected
/// Agents. Unlike the single-package flow this REGISTERS the directory as a
/// marketplace in each Agent's config, so the source path must stay in
/// place; every requested plugin must be listed by the marketplace manifest.
pub fn install_local_marketplace(
    raw_path: &str,
    selections: &[(Agent, Vec<String>)],
) -> Result<PackageOperationResult> {
    let canonical = super::imported::canonical_dir(raw_path)?;
    let path = canonical.to_string_lossy().into_owned();
    if selections.is_empty() {
        return Err(other("请选择要安装的插件"));
    }
    let markets = super::imported::installable_marketplaces(&canonical);
    let mut installed = 0;
    for (agent, plugins) in selections {
        if plugins.is_empty() {
            return Err(other("请选择要安装的插件"));
        }
        let market = markets
            .iter()
            .find(|(_, kind)| kind.agent() == *agent)
            .ok_or_else(|| {
                other(format!(
                    "该目录没有 {} 格式的市场清单",
                    agent.display_name()
                ))
            })?;
        let (market_name, _, listed) = super::imported::read_marketplace_meta(&market.0);
        if market_name.is_empty() {
            return Err(other("市场清单缺少 name，无法注册"));
        }
        for plugin in plugins {
            if !listed.iter().any(|entry| entry.name == *plugin) {
                return Err(other(format!("市场清单里没有插件「{plugin}」")));
            }
        }
        if *agent == Agent::Codex {
            run_cli(Agent::Codex, &marketplace_add_args(&path))?;
        }
        for plugin in plugins {
            let args = marketplace_install_args(*agent, plugin, &path, &market_name)?;
            run_cli(*agent, &args)?;
            installed += 1;
        }
    }
    Ok(PackageOperationResult {
        message: format!(
            "已安装 {installed} 个插件；本地市场已注册到 Agent，目录需保留在原处；已有终端请重新加载或重新打开"
        ),
    })
}

/// Installs a local package directory into the requested Agents. Nothing is
/// registered: each requested Agent must have a matching root manifest, and
/// the native CLI is the only thing that records the install.
pub fn install_local(raw_path: &str, agents: &[Agent]) -> Result<PackageOperationResult> {
    let canonical = super::imported::canonical_dir(raw_path)?;
    let path = canonical.to_string_lossy().into_owned();
    if agents.is_empty() {
        return Err(other("请选择要安装到的 Agent"));
    }
    let supported = super::imported::installable_manifests(&canonical)?;
    for agent in agents {
        let ok = supported
            .iter()
            .any(|(_, kind)| matches!(kind.install_target(), Ok(target) if target == *agent));
        if !ok {
            return Err(other(format!(
                "该目录没有 {} 格式的清单，不能安装到 {}",
                agent.display_name(),
                agent.display_name()
            )));
        }
    }
    let mut installed = 0;
    let mut failures = Vec::new();
    for agent in agents {
        let outcome =
            cli_args(*agent, PackageAction::Install, &path).and_then(|args| run_cli(*agent, &args));
        if outcome.is_ok() && *agent == Agent::Dsh {
            // The dependency landed; declare the bundle so the loader picks
            // it up. The declared name is the source package's npm name.
            let (source_name, _) = super::imported::read_manifest_meta(&canonical);
            if source_name.is_empty() {
                failures.push("DSH：源目录 package.json 缺少 name，无法声明插件包".into());
                continue;
            }
            if let Err(error) =
                super::dsh::declare_bundle(&crate::skills::Roots::current()?, &source_name)
            {
                failures.push(format!("DSH：{error}"));
                continue;
            }
        }
        match outcome {
            Ok(()) => installed += 1,
            Err(error) => failures.push(format!("{}：{error}", agent.display_name())),
        }
    }
    if installed == 0 {
        return Err(other(format!(
            "未能安装到任何 Agent：{}",
            failures.join("；")
        )));
    }
    let mut message = format!("已安装到 {installed} 个 Agent；已有终端请重新加载或重新打开");
    if !failures.is_empty() {
        message.push_str(&format!("；失败：{}", failures.join("；")));
    }
    Ok(PackageOperationResult { message })
}
