//! Package mutations. Install/uninstall delegate to the owning agent tool's native
//! CLI so downloads, dependencies and install records stay agent tool-managed;
//! enable/disable uses the native toggle and checks the discovery revision.

use super::{dsh, files, Origin, PackageEntry, PackageInventory, Roots};
use crate::domain::AgentTool;
use crate::error::{other, Result};
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

pub const CLI_TIMEOUT_SECS: u64 = 180;
static DSH_MUTATION: Mutex<()> = Mutex::new(());

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

/// CLI argument mapping per agent tool. Pure: tests pin the contract without
/// launching anything. Every package command is user-scope in this iteration.
pub fn cli_args(agent: AgentTool, action: PackageAction, target: &str) -> Result<Vec<String>> {
    validate_target(target)?;
    let linked;
    let args: Vec<&str> = match (agent, action) {
        (AgentTool::Pi, PackageAction::Install) => vec!["install", target],
        // The plugin subcommand forwards to pnpm in the desktop profile; the
        // local directory is referenced (link:), never copied.
        (AgentTool::Dsh, PackageAction::Install) => {
            linked = format!("link:{target}");
            vec!["plugin", "--profile", "desktop", "add", &linked]
        }
        (AgentTool::Antigravity, PackageAction::Install) => vec!["plugin", "install", target],
        (AgentTool::Codex, PackageAction::Uninstall) => vec!["plugin", "remove", target, "--json"],
        (AgentTool::ClaudeCode, PackageAction::Uninstall) => {
            vec!["plugin", "uninstall", target, "--scope", "user"]
        }
        (AgentTool::Pi, PackageAction::Uninstall) => vec!["remove", target],
        (AgentTool::Dsh, PackageAction::Uninstall) => {
            vec!["plugin", "--profile", "desktop", "remove", target]
        }
        (AgentTool::Antigravity, PackageAction::Uninstall) => vec!["plugin", "uninstall", target],
        (AgentTool::ClaudeCode, PackageAction::Enable) => {
            vec!["plugin", "enable", target, "--json"]
        }
        (AgentTool::ClaudeCode, PackageAction::Disable) => {
            vec!["plugin", "disable", target, "--json"]
        }
        (AgentTool::Antigravity, PackageAction::Enable) => vec!["plugin", "enable", target],
        (AgentTool::Antigravity, PackageAction::Disable) => vec!["plugin", "disable", target],
        _ => {
            return Err(other(
                "不支持的插件包操作；本地目录安装仅支持 Pi、DSH 和 Antigravity",
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
        Some(market)
            if entry.agent != Some(AgentTool::Pi) && entry.agent != Some(AgentTool::Dsh) =>
        {
            format!("{}@{market}", entry.name)
        }
        _ => entry.name.clone(),
    }
}

fn run_dsh_cli(roots: &Roots, args: &[String]) -> Result<()> {
    let home = roots
        .dsh
        .to_str()
        .ok_or_else(|| other("DSH 数据目录包含无法传给 CLI 的字符"))?;
    run_cli(
        AgentTool::Dsh,
        args,
        &[(
            crate::platform::paths::agent_env_override(AgentTool::Dsh),
            home,
        )],
    )
}

fn run_cli(agent: AgentTool, args: &[String], env: &[(&str, &str)]) -> Result<()> {
    let installation = crate::platform::exec_resolver::resolve(agent)?;
    let output = crate::platform::exec_runner::run_with_env(
        Path::new(&installation.executable_path),
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
        CLI_TIMEOUT_SECS,
        env,
    )?;
    if !output.success {
        let reason = if output.exit_code.is_none() {
            "操作超时，请检查 运行工具 的安装状态".to_string()
        } else {
            format!("{}\n{}", output.stderr.trim(), output.stdout.trim())
        };
        return Err(other(reason.chars().take(5000).collect::<String>()));
    }
    Ok(())
}

pub fn uninstall(
    roots: &Roots,
    inventory: &PackageInventory,
    id: &str,
) -> Result<PackageOperationResult> {
    let entry = locate(inventory, id)?;
    let agent = entry
        .agent
        .ok_or_else(|| other("该插件包没有所属 运行工具，无法卸载"))?;
    let args = cli_args(agent, PackageAction::Uninstall, &cli_target(entry))?;
    if agent == AgentTool::Dsh {
        let _guard = DSH_MUTATION
            .lock()
            .map_err(|_| other("DSH 插件操作锁不可用"))?;
        dsh::uninstall_bundle(roots, &entry.name, || run_dsh_cli(roots, &args))?;
    } else {
        run_cli(agent, &args, &[])?;
    }
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
        Some(AgentTool::Codex) => {
            // Each discovered entry carries exactly one scope.
            let scope = entry
                .scopes
                .first()
                .ok_or_else(|| other("该插件包没有可启停的配置"))?;
            let raw = files::read(Path::new(&scope.config_path))?;
            let text = codex_toggle(&raw, entry, enable)?;
            files::write_atomic(Path::new(&scope.config_path), &text)?;
        }
        Some(AgentTool::ClaudeCode) => {
            let args = cli_args(AgentTool::ClaudeCode, action, &cli_target(entry))?;
            run_cli(AgentTool::ClaudeCode, &args, &[])?;
        }
        Some(AgentTool::Antigravity) => {
            // agy's enable/disable keys on the bare plugin name, matching
            // `plugin list --json`.
            let args = cli_args(AgentTool::Antigravity, action, &entry.name)?;
            run_cli(AgentTool::Antigravity, &args, &[])?;
        }
        _ => return Err(other("该 运行工具 未提供插件启停开关")),
    }
    Ok(PackageOperationResult {
        message: "启停配置已保存，下次启动 运行工具 生效".into(),
    })
}

fn locate<'a>(inventory: &'a PackageInventory, id: &str) -> Result<&'a PackageEntry> {
    let entry = inventory
        .packages
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| other("插件包已不存在，请刷新"))?;
    // Codex's bundled markets are ordinary config.toml entries — writing
    // `enabled` there is exactly how the agent tool itself toggles them, so
    // Bundled is not blocked here (capability flags already gate the UI).
    // NoEnding's own built-ins stay app-managed.
    if entry.origin == Origin::NoEndingManaged {
        return Err(other("NoEnding 内置插件包由应用管理，不能在这里操作"));
    }
    if !entry.capabilities.toggle && !entry.capabilities.uninstall {
        return Err(other("该 运行工具 未提供插件包管理命令"));
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
    agent: AgentTool,
    plugin: &str,
    market_path: &str,
    market_name: &str,
) -> Result<Vec<String>> {
    validate_target(plugin)?;
    validate_target(market_name)?;
    let args: Vec<String> = match agent {
        // Claude installs straight from a marketplace source path; the
        // marketplace is registered automatically on first use.
        AgentTool::ClaudeCode => [
            "plugin",
            "install",
            plugin,
            "--marketplace",
            market_path,
            "--json",
        ],
        // Codex needs the marketplace registered first and selects by
        // `name@market`.
        AgentTool::Codex => {
            return Ok(vec![
                "plugin".into(),
                "add".into(),
                format!("{plugin}@{market_name}"),
            ])
        }
        _ => return Err(other("该 运行工具 不支持从本地市场安装")),
    }
    .iter()
    .map(|s| s.to_string())
    .collect();
    Ok(args)
}

/// Installs a local marketplace directory's plugins into the selected
/// Agents. Unlike the single-package flow this REGISTERS the directory as a
/// marketplace in each agent tool's config, so the source path must stay in
/// place; every requested plugin must be listed by the marketplace manifest.
pub fn install_local_marketplace(
    raw_path: &str,
    selections: &[(AgentTool, Vec<String>)],
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
        if *agent == AgentTool::Codex {
            run_cli(AgentTool::Codex, &marketplace_add_args(&path), &[])?;
        }
        for plugin in plugins {
            let args = marketplace_install_args(*agent, plugin, &path, &market_name)?;
            run_cli(*agent, &args, &[])?;
            installed += 1;
        }
    }
    Ok(PackageOperationResult {
        message: format!(
            "已安装 {installed} 个插件；本地市场已注册到 运行工具，目录需保留在原处；已有终端请重新加载或重新打开"
        ),
    })
}

/// Installs a local package directory through each selected agent tool's CLI.
pub fn install_local(raw_path: &str, agents: &[AgentTool]) -> Result<PackageOperationResult> {
    let canonical = super::imported::canonical_dir(raw_path)?;
    let path = canonical.to_string_lossy().into_owned();
    if agents.is_empty() {
        return Err(other("请选择要安装到的 运行工具"));
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
        let outcome = cli_args(*agent, PackageAction::Install, &path).and_then(|args| {
            if *agent == AgentTool::Dsh {
                install_dsh(&canonical, &args)
            } else {
                run_cli(*agent, &args, &[])
            }
        });
        match outcome {
            Ok(()) => installed += 1,
            Err(error) => failures.push(format!("{}：{error}", agent.display_name())),
        }
    }
    if installed == 0 {
        return Err(other(format!(
            "未能安装到任何 运行工具：{}",
            failures.join("；")
        )));
    }
    let mut message = format!("已安装到 {installed} 个 运行工具；已有终端请重新加载或重新打开");
    if !failures.is_empty() {
        message.push_str(&format!("；失败：{}", failures.join("；")));
    }
    Ok(PackageOperationResult { message })
}

fn install_dsh(source: &Path, args: &[String]) -> Result<()> {
    let (name, _) = super::imported::read_manifest_meta(source);
    if name.is_empty() {
        return Err(other("源目录 package.json 缺少 name，无法声明插件包"));
    }
    let roots = Roots::current()?;
    let _guard = DSH_MUTATION
        .lock()
        .map_err(|_| other("DSH 插件操作锁不可用"))?;
    run_dsh_cli(&roots, args)?;
    dsh::declare_bundle(&roots, &name)
}
