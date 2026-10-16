use super::{files, key, location, Inventory, Location, OperationResult, Resource, Roots};
use crate::{
    domain::Agent,
    error::{other, Result},
};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn read_json(path: &Path, inventory: &mut Inventory) -> Option<Value> {
    if !path.exists() {
        return None;
    }
    match files::read(path).and_then(|s| files::json(&s)) {
        Ok(value) => Some(value),
        Err(e) => {
            inventory.warnings.push(format!("{}：{e}", path.display()));
            None
        }
    }
}

fn add_package(
    inventory: &mut Inventory,
    loc: Location,
    name: &str,
    description: String,
    enabled: Option<bool>,
    root: Option<&Path>,
) {
    let managed = name == "noending-terminal-identity";
    let revision = files::revision(Path::new(&loc.path)).unwrap_or_default();
    let id = key(format!("package:{}:{name}", loc.id));
    inventory.resources.push(Resource {
        id: id.clone(),
        kind: "package".into(),
        name: name.into(),
        description,
        path: loc.path.clone(),
        locations: vec![loc.id.clone()],
        entry_paths: vec![loc.path.clone()],
        enabled,
        revision,
        managed,
        error: None,
        skill_controls: Vec::new(),
    });
    inventory.locations.push(loc.clone());
    let Some(root) = root.filter(|r| r.is_dir()) else {
        return;
    };
    let canonical_root = match fs::canonicalize(root) {
        Ok(p) => p,
        Err(_) => return,
    };
    let manifest_path = [
        "plugin.json",
        ".claude-plugin/plugin.json",
        ".codex-plugin/plugin.json",
        "package.json",
    ]
    .into_iter()
    .map(|p| root.join(p))
    .find(|p| p.is_file());
    let manifest = manifest_path
        .as_ref()
        .and_then(|p| read_json(p, inventory))
        .unwrap_or(json!({}));
    let allowed = |relative: &str| -> Option<std::path::PathBuf> {
        let path = root.join(relative);
        let real = fs::canonicalize(&path).ok()?;
        real.starts_with(&canonical_root).then_some(path)
    };
    let mut skill_paths = vec!["skills".to_string()];
    let pi = manifest.get("pi").unwrap_or(&manifest);
    match pi.get("skills") {
        Some(Value::String(s)) => skill_paths.push(s.clone()),
        Some(Value::Array(a)) => {
            skill_paths.extend(a.iter().filter_map(Value::as_str).map(str::to_string))
        }
        _ => {}
    }
    skill_paths.sort();
    skill_paths.dedup();
    for rel in skill_paths {
        if let Some(path) = allowed(&rel) {
            let mut child = location(
                "skill",
                loc.agent,
                &loc.scope,
                path,
                &format!("插件包 · {name}"),
            );
            child.id = key(format!("{}:package:{name}", child.id));
            child.writable = false;
            child.package = Some(name.into());
            child.package_enabled = enabled;
            inventory.locations.push(child);
        }
    }
    let mut mcps = vec![
        ".mcp.json".to_string(),
        "mcp.json".into(),
        "mcp_config.json".into(),
    ];
    if let Some(s) = manifest.get("mcpServers").and_then(Value::as_str) {
        mcps.push(s.into());
    }
    mcps.sort();
    mcps.dedup();
    for rel in mcps {
        if let Some(path) = allowed(&rel).filter(|p| p.is_file()) {
            let mut child = location(
                "mcp",
                loc.agent,
                &loc.scope,
                path,
                &format!("插件包 · {name}"),
            );
            child.id = key(format!("{}:package:{name}", child.id));
            child.writable = false;
            child.package = Some(name.into());
            child.package_enabled = enabled;
            child.section = vec!["mcpServers".into()];
            inventory.locations.push(child);
        }
    }
    if manifest.get("mcpServers").is_some_and(Value::is_object) {
        if let Some(path) = manifest_path {
            let mut child = location(
                "mcp",
                loc.agent,
                &loc.scope,
                path,
                &format!("插件包 · {name}"),
            );
            child.id = key(format!("{}:package:{name}", child.id));
            child.writable = false;
            child.package = Some(name.into());
            child.package_enabled = enabled;
            child.section = vec!["mcpServers".into()];
            inventory.locations.push(child);
        }
    }
}

pub(super) fn discover(roots: &Roots, cwd: Option<&Path>, inventory: &mut Inventory) {
    // Explicit system skills are visible but cannot be edited/uninstalled.
    let mut system = location(
        "skill",
        Some(Agent::Codex),
        "user",
        roots.codex.join("skills/.system"),
        "Codex 内置 Skills",
    );
    system.writable = false;
    inventory.locations.push(system);
    if let Some(cwd) = cwd {
        let mut assistant = location(
            "skill",
            None,
            "project",
            cwd.join("skills/noending-assistant"),
            "NoEnding 内置助手",
        );
        assistant.writable = false;
        if Path::new(&assistant.path).join("SKILL.md").exists() {
            inventory.locations.push(assistant);
        }
    }
    // Claude's install registry is authoritative; cached versions alone are
    // not evidence that a plugin is installed in this scope.
    let registry_path = roots.claude.join("plugins/installed_plugins.json");
    if let Some(registry) = read_json(&registry_path, inventory) {
        if let Some(plugins) = registry.get("plugins").and_then(Value::as_object) {
            for (name, installs) in plugins {
                for install in installs.as_array().into_iter().flatten() {
                    let scope = install
                        .get("scope")
                        .and_then(Value::as_str)
                        .unwrap_or("user");
                    let project = install.get("projectPath").and_then(Value::as_str);
                    if scope != "user"
                        && !cwd.is_some_and(|c| project.is_some_and(|p| Path::new(p) == c))
                    {
                        continue;
                    }
                    let root = install
                        .get("installPath")
                        .and_then(Value::as_str)
                        .map(Path::new);
                    let settings_path = if scope == "user" {
                        roots.claude.join("settings.json")
                    } else {
                        cwd.unwrap().join(if scope == "local" {
                            ".claude/settings.local.json"
                        } else {
                            ".claude/settings.json"
                        })
                    };
                    let enabled = read_json(&settings_path, inventory).and_then(|s| {
                        s.get("enabledPlugins")
                            .and_then(|p| p.get(name))
                            .and_then(Value::as_bool)
                    });
                    let mut loc = location(
                        "package",
                        Some(Agent::ClaudeCode),
                        if scope == "user" { "user" } else { "project" },
                        settings_path,
                        "Claude Code 插件包",
                    );
                    loc.id = key(format!("{}:{scope}:{name}", loc.id));
                    loc.section = vec![scope.into()];
                    add_package(
                        inventory,
                        loc,
                        name,
                        install
                            .get("version")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        enabled,
                        root,
                    );
                }
            }
        }
    }
    for (scope, config) in std::iter::once(("user", roots.codex.join("config.toml")))
        .chain(cwd.map(|p| ("project", p.join(".codex/config.toml"))))
    {
        let loc = location(
            "package",
            Some(Agent::Codex),
            scope,
            config,
            "Codex 插件配置",
        );
        match super::inventory::config(&loc) {
            Ok((value, _)) => {
                if let Some(plugins) = value.get("plugins").and_then(Value::as_object) {
                    for (name, settings) in plugins {
                        let mut loc = loc.clone();
                        loc.id = key(format!("{}:{name}", loc.id));
                        let root = codex_package_root(roots, name);
                        add_package(
                            inventory,
                            loc,
                            name,
                            "由 Codex 管理安装与版本".into(),
                            settings.get("enabled").and_then(Value::as_bool),
                            root.as_deref(),
                        );
                    }
                }
            }
            Err(e) => inventory.warnings.push(format!("{}：{e}", loc.path)),
        }
    }
    let agy_config =
        read_json(&roots.home.join(".gemini/config/config.json"), inventory).unwrap_or(json!({}));
    for base in [
        roots.home.join(".gemini/config/plugins"),
        roots.home.join(".gemini/antigravity-cli/plugins"),
    ] {
        if let Ok(entries) = fs::read_dir(base) {
            for entry in entries.flatten() {
                let manifest = entry.path().join("plugin.json");
                let Some(value) = read_json(&manifest, inventory) else {
                    continue;
                };
                let Some(name) = value.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let loc = location(
                    "package",
                    Some(Agent::Antigravity),
                    "user",
                    manifest,
                    "Antigravity 插件包",
                );
                let enabled = agy_config
                    .get("plugins")
                    .and_then(|p| p.get(name))
                    .and_then(|p| p.get("enabled"))
                    .and_then(Value::as_bool);
                add_package(
                    inventory,
                    loc,
                    name,
                    value
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    enabled,
                    Some(&entry.path()),
                );
            }
        }
    }
    for (scope, file) in std::iter::once(("user", roots.pi.join("settings.json")))
        .chain(cwd.map(|c| ("project", c.join(".pi/settings.json"))))
    {
        let Some(value) = read_json(&file, inventory) else {
            continue;
        };
        for package in value
            .get("packages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(source) = package
                .as_str()
                .or_else(|| package.get("source").and_then(Value::as_str))
            else {
                continue;
            };
            let mut loc = location("package", Some(Agent::Pi), scope, file.clone(), "Pi 包");
            loc.id = key(format!("{}:{source}", loc.id));
            // Local sources can be inspected without running npm/git. Remote
            // package resolution remains Pi's responsibility.
            let local = if source.starts_with("./")
                || source.starts_with("../")
                || Path::new(source).is_absolute()
            {
                Some(file.parent().unwrap().join(source))
            } else {
                None
            };
            add_package(
                inventory,
                loc,
                source,
                "Pi 包声明".into(),
                None,
                local.as_deref(),
            );
        }
    }
}

fn codex_package_root(roots: &Roots, selector: &str) -> Option<std::path::PathBuf> {
    let (name, market) = selector.rsplit_once('@')?;
    // Do not guess an active version when several cache versions coexist.
    let dir = roots.codex.join("plugins/cache").join(market).join(name);
    let versions: Vec<_> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    (versions.len() == 1).then(|| versions[0].clone())
}

pub fn package_action(
    roots: &Roots,
    cwd: Option<&Path>,
    agent: Agent,
    action: &str,
    target: &str,
    resource_id: Option<&str>,
    expected: Option<&str>,
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    if !super::AGENTS.contains(&agent) {
        return Err(other("该 Agent 尚不支持插件包管理"));
    }
    let project_override = matches!(action, "project_enable" | "project_disable");
    let action = match action {
        "project_enable" => "enable",
        "project_disable" => "disable",
        action => action,
    };
    if target.trim().is_empty() || target.starts_with('-') || target.contains(['\0', '\n', '\r']) {
        return Err(other("插件目标无效"));
    }
    let inventory = super::list(roots, cwd);
    let row = resource_id
        .map(|id| {
            inventory
                .resources
                .iter()
                .find(|r| r.id == id && r.kind == "package")
                .ok_or_else(|| other("插件包已不存在"))
        })
        .transpose()?;
    let mut native_scope = if cwd.is_some() { "project" } else { "user" };
    if action != "install" {
        let row = row.ok_or_else(|| other("请选择已有插件包"))?;
        if row.managed {
            return Err(other("NoEnding 内置插件由应用管理"));
        }
        if row.name != target {
            return Err(other("插件包目标已变化"));
        }
        super::check_revision(
            &row.revision,
            expected.ok_or_else(|| other("缺少配置版本"))?,
        )?;
        let loc = inventory
            .locations
            .iter()
            .find(|l| row.locations.contains(&l.id))
            .ok_or_else(|| other("插件包来源无效"))?;
        if loc.agent != Some(agent) {
            return Err(other("Agent 与插件包不匹配"));
        }
        native_scope = if loc.scope == "user" {
            "user"
        } else if loc.section.first().is_some_and(|s| s == "local") {
            "local"
        } else {
            "project"
        };
    }
    if project_override {
        if cwd.is_none() || !matches!(agent, Agent::Codex | Agent::ClaudeCode) {
            return Err(other("该 Agent 不支持此项目级插件开关"));
        }
        native_scope = "project";
    }
    if action == "inherit" {
        let row = row.ok_or_else(|| other("请选择已有插件包"))?;
        if native_scope == "user" || !matches!(agent, Agent::Codex | Agent::ClaudeCode) {
            return Err(other("只有项目级配置可以恢复继承"));
        }
        let path = Path::new(&row.path);
        let raw = files::read(path)?;
        let text = if agent == Agent::Codex {
            let mut doc = raw.parse::<toml_edit::Document>().map_err(other)?;
            let empty = if let Some(table) = doc
                .get_mut("plugins")
                .and_then(toml_edit::Item::as_table_like_mut)
                .and_then(|p| p.get_mut(target))
                .and_then(toml_edit::Item::as_table_like_mut)
            {
                table.remove("enabled");
                table.iter().next().is_none()
            } else {
                false
            };
            if empty {
                if let Some(table) = doc
                    .get_mut("plugins")
                    .and_then(toml_edit::Item::as_table_like_mut)
                {
                    table.remove(target);
                }
            }
            doc.to_string()
        } else {
            let mut config = files::json(&raw)?;
            if let Some(values) = config
                .get_mut("enabledPlugins")
                .and_then(Value::as_object_mut)
            {
                values.remove(target);
            }
            format!("{}\n", serde_json::to_string_pretty(&config)?)
        };
        files::write(path, &key(raw), &text)?;
        return Ok(OperationResult {
            message: "已移除项目级启停设置，按 Agent 原生规则继承".into(),
        });
    }
    if agent == Agent::Codex && matches!(action, "enable" | "disable") {
        let path = if native_scope == "user" {
            roots.codex.join("config.toml")
        } else {
            cwd.ok_or_else(|| other("缺少项目目录"))?
                .join(".codex/config.toml")
        };
        let raw = files::read(&path)?;
        let mut doc = raw.parse::<toml_edit::Document>().map_err(other)?;
        doc["plugins"][target]["enabled"] = toml_edit::value(action == "enable");
        files::write(&path, &key(raw), &doc.to_string())?;
        return Ok(OperationResult {
            message: "插件启用配置已保存，下次启动生效".into(),
        });
    }
    let args = action_args(agent, action, target, native_scope)?;
    let installation = crate::platform::exec_resolver::resolve(agent)?;
    let output = crate::platform::exec_runner::run(
        Path::new(&installation.executable_path),
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        cwd.or(Some(&roots.home)),
        180,
    )?;
    if !output.success {
        let reason = if output.exit_code.is_none() {
            "操作超时，请检查 Agent 的安装状态".into()
        } else {
            format!("{}\n{}", output.stderr, output.stdout)
        };
        return Err(other(reason.chars().take(5000).collect::<String>()));
    }
    Ok(OperationResult {
        message: "插件包操作已完成；已有终端请重新加载或重新打开".into(),
    })
}

fn action_args(agent: Agent, action: &str, target: &str, scope: &str) -> Result<Vec<String>> {
    let args: Vec<&str> = match (agent, action) {
        (Agent::Codex, "install") if scope == "user" => vec!["plugin", "add", target, "--json"],
        (Agent::Codex, "remove") if scope == "user" => vec!["plugin", "remove", target, "--json"],
        (Agent::ClaudeCode, "install" | "remove" | "enable" | "disable") => vec!["plugin", if action == "remove" { "uninstall" } else { action }, target, "--scope", scope],
        (Agent::Pi, "install" | "remove") => {
            let mut args = vec![action, target]; if scope != "user" { args.push("--local"); } args
        }
        (Agent::Antigravity, "install" | "remove" | "enable" | "disable") if scope == "user" => vec!["plugin", if action == "remove" { "uninstall" } else { action }, target],
        _ => return Err(other("该 Agent 不支持此作用域下的插件操作；Codex 和 Antigravity 安装/卸载使用用户级入口，Pi 包不提供统一启停")),
    };
    Ok(args.into_iter().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_package_commands_keep_project_scope_explicit_and_reject_unsupported_scopes() {
        assert_eq!(
            action_args(Agent::ClaudeCode, "install", "review@team", "project").unwrap(),
            ["plugin", "install", "review@team", "--scope", "project"]
        );
        assert_eq!(
            action_args(Agent::ClaudeCode, "remove", "review@team", "local").unwrap(),
            ["plugin", "uninstall", "review@team", "--scope", "local"]
        );
        assert_eq!(
            action_args(Agent::Pi, "install", "npm:team-tools", "project").unwrap(),
            ["install", "npm:team-tools", "--local"]
        );
        assert_eq!(
            action_args(Agent::Antigravity, "remove", "review", "user").unwrap(),
            ["plugin", "uninstall", "review"]
        );
        assert!(action_args(Agent::Codex, "install", "review@team", "project").is_err());
        assert!(action_args(Agent::Antigravity, "remove", "review", "project").is_err());
        assert!(action_args(Agent::Pi, "disable", "npm:team-tools", "user").is_err());
    }
}
