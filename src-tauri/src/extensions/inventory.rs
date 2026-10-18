use super::{
    files, key, location, Inventory, Location, OperationResult, Resource, ResourceDetail, Roots,
};
use crate::{
    domain::Agent,
    error::{other, Result},
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

/// Agent configuration destinations only. Skill sources, including the
/// NoEnding source library, are never scan roots or Locations.
pub(super) fn locations(roots: &Roots, cwd: Option<&Path>) -> Vec<Location> {
    let mut rows = vec![
        location(
            "skill",
            None,
            "user",
            roots.home.join(".agents/skills"),
            "共享 Skills",
        ),
        location(
            "skill",
            Some(Agent::Codex),
            "user",
            roots.codex.join("skills"),
            "Codex Skills",
        ),
        location(
            "skill",
            Some(Agent::ClaudeCode),
            "user",
            roots.claude.join("skills"),
            "Claude Code Skills",
        ),
        location(
            "skill",
            Some(Agent::Pi),
            "user",
            roots.pi.join("skills"),
            "Pi Skills",
        ),
        location(
            "skill",
            Some(Agent::Antigravity),
            "user",
            roots.home.join(".gemini/antigravity-cli/skills"),
            "Antigravity Skills",
        ),
        location(
            "skill",
            Some(Agent::Antigravity),
            "user",
            roots.home.join(".gemini/config/skills"),
            "Antigravity 兼容目录",
        ),
    ];
    let claude_json = if roots.claude == roots.home.join(".claude") {
        roots.home.join(".claude.json")
    } else {
        roots.claude.join(".claude.json")
    };
    for (agent, file) in [
        (Agent::Codex, roots.codex.join("config.toml")),
        (Agent::ClaudeCode, claude_json.clone()),
        (Agent::Pi, roots.pi.join("mcp.json")),
        (
            Agent::Antigravity,
            roots.home.join(".gemini/config/mcp_config.json"),
        ),
    ] {
        let mut row = location("mcp", Some(agent), "user", file, agent.display_name());
        row.section = vec![if agent == Agent::Codex {
            "mcp_servers"
        } else {
            "mcpServers"
        }
        .into()];
        rows.push(row);
    }
    if let Some(cwd) = cwd {
        // These are explicit configuration destinations, not an invented
        // NoEnding inheritance system. Native CLI trust/ancestor rules still apply.
        for (agent, rel, label) in [
            (None, ".agents/skills", "共享 Skills"),
            (
                Some(Agent::ClaudeCode),
                ".claude/skills",
                "Claude Code Skills",
            ),
            (Some(Agent::Pi), ".pi/skills", "Pi Skills"),
        ] {
            rows.push(location("skill", agent, "project", cwd.join(rel), label));
        }
        for (agent, rel) in [
            (Agent::Codex, ".codex/config.toml"),
            (Agent::ClaudeCode, ".mcp.json"),
            (Agent::Pi, ".pi/mcp.json"),
            (Agent::Antigravity, ".agents/mcp_config.json"),
        ] {
            let mut row = location(
                "mcp",
                Some(agent),
                "project",
                cwd.join(rel),
                agent.display_name(),
            );
            row.section = vec![if agent == Agent::Codex {
                "mcp_servers"
            } else {
                "mcpServers"
            }
            .into()];
            rows.push(row);
        }
    }
    rows
}

/// Project-level status only depends on the project's own skill destinations.
/// Scanning user skills or MCP configs here multiplies the cost by the number
/// of registered working directories.
pub(super) fn list_project_skills(roots: &Roots, cwd: &Path) -> Inventory {
    let mut result = Inventory::default();
    result.locations.extend(
        locations(roots, Some(cwd))
            .into_iter()
            .filter(|loc| loc.kind == "skill" && loc.scope == "project"),
    );
    let skill_locations = result.locations.clone();
    let mut seen = HashSet::new();
    for loc in &skill_locations {
        super::skills::scan(loc, &mut seen, &mut result);
    }
    result.resources.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    result
}

/// One lightweight catalog across the user scope and every registered working
/// directory. Project scans only touch project-level Skill and MCP locations;
/// user skills and unrelated Agent files are read once.
pub fn list_catalog(roots: &Roots, cwds: &[PathBuf]) -> Inventory {
    let mut inventories = vec![list(roots, None)];
    for cwd in cwds {
        let mut inventory = list_project_skills(roots, cwd);
        let mut mcp = Inventory::default();
        mcp.locations = locations(roots, Some(cwd))
            .into_iter()
            .filter(|loc| loc.kind == "mcp" && loc.scope == "project")
            .collect();
        let mcp_locations = mcp.locations.clone();
        for loc in &mcp_locations {
            if let Err(error) = scan_mcp(loc, &mut mcp) {
                mcp.warnings.push(format!("{}：{error}", loc.path));
            }
        }
        inventory.locations.extend(mcp.locations);
        inventory.resources.extend(mcp.resources);
        inventory.warnings.extend(mcp.warnings);
        inventories.push(inventory);
    }
    merge_inventories(inventories)
}

pub fn list(roots: &Roots, cwd: Option<&Path>) -> Inventory {
    let mut result = Inventory {
        locations: locations(roots, cwd),
        ..Default::default()
    };
    let locations = result.locations.clone();
    let mut seen = HashSet::new();
    for loc in &locations {
        match loc.kind.as_str() {
            "skill" => super::skills::scan(loc, &mut seen, &mut result),
            "mcp" => {
                if let Err(e) = scan_mcp(loc, &mut result) {
                    result.warnings.push(format!("{}：{e}", loc.path));
                }
            }
            _ => {}
        }
    }
    let mut merged: Vec<Resource> = Vec::new();
    for row in result.resources.drain(..) {
        let Some(existing) = merged
            .iter_mut()
            .find(|existing| existing.id == row.id && existing.kind == row.kind)
        else {
            merged.push(row);
            continue;
        };
        existing.registered |= row.registered;
        if existing.error.is_none() {
            existing.error = row.error;
        }
        for (index, location) in row.locations.iter().enumerate() {
            if existing.locations.contains(location) {
                continue;
            }
            let Some(entry) = row.entry_paths.get(index) else {
                continue;
            };
            existing.locations.push(location.clone());
            existing.entry_paths.push(entry.clone());
            existing
                .entry_links
                .push(row.entry_links.get(index).copied().unwrap_or(false));
        }
    }
    result.resources = merged;
    result.resources.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    result
}

/// One inventory for a project spanning several working directories. Resources
/// keep one row per source; entries from every directory stay aligned with their
/// location ids.
pub fn merge_inventories(inventories: Vec<Inventory>) -> Inventory {
    let mut result = Inventory::default();
    for inventory in inventories {
        for loc in inventory.locations {
            if !result
                .locations
                .iter()
                .any(|existing| existing.id == loc.id)
            {
                result.locations.push(loc);
            }
        }
        for warning in inventory.warnings {
            if !result.warnings.contains(&warning) {
                result.warnings.push(warning);
            }
        }
        for row in inventory.resources {
            let Some(existing) = result.resources.iter_mut().find(|r| r.id == row.id) else {
                result.resources.push(row);
                continue;
            };
            existing.registered |= row.registered;
            if existing.error.is_none() {
                existing.error = row.error;
            }
            for (index, location) in row.locations.iter().enumerate() {
                if existing.locations.contains(location) {
                    continue;
                }
                let Some(entry) = row.entry_paths.get(index) else {
                    continue;
                };
                existing.locations.push(location.clone());
                existing.entry_paths.push(entry.clone());
                existing
                    .entry_links
                    .push(row.entry_links.get(index).copied().unwrap_or(false));
            }
        }
    }
    result.resources.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    result
}

pub(super) fn config(loc: &Location) -> Result<(Value, String)> {
    let raw = files::read(Path::new(&loc.path))?;
    let value = if loc.path.ends_with(".toml") {
        if raw.trim().is_empty() {
            json!({})
        } else {
            toml_edit::de::from_str(&raw).map_err(other)?
        }
    } else {
        files::json(&raw)?
    };
    Ok((value, key(raw)))
}

pub(super) fn section<'a>(value: &'a Value, loc: &Location) -> Option<&'a Value> {
    let mut value = value;
    for name in &loc.section {
        value = value.get(name)?;
    }
    Some(value)
}

fn scan_mcp(loc: &Location, inventory: &mut Inventory) -> Result<()> {
    let (value, revision) = config(loc)?;
    let Some(section) = section(&value, loc) else {
        return Ok(());
    };
    let entries = section
        .as_object()
        .ok_or_else(|| other("MCP 列表必须是对象"))?;
    for (name, server) in entries {
        let error = validate_mcp(loc.agent.unwrap(), server)
            .err()
            .map(|e| e.to_string());
        inventory.resources.push(Resource {
            id: super::mcp_registry::identity(name, server),
            kind: "mcp".into(),
            name: name.clone(),
            description: server
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            path: loc.path.clone(),
            locations: vec![loc.id.clone()],
            entry_paths: vec![loc.path.clone()],
            entry_links: Vec::new(),
            revision: revision.clone(),
            registered: false,
            error,
        });
    }
    Ok(())
}

pub fn detail(roots: &Roots, cwd: Option<&Path>, id: &str) -> Result<ResourceDetail> {
    let list = list(roots, cwd);
    let resource = list
        .resources
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| other("资源已不存在，请刷新"))?;
    if resource.kind == "skill" {
        return Ok(ResourceDetail {
            content: files::read(Path::new(&resource.path))?,
            config: None,
            resource,
        });
    }
    let loc = list
        .locations
        .iter()
        .find(|l| resource.locations.contains(&l.id))
        .ok_or_else(|| other("资源来源已不存在"))?;
    if resource.kind == "mcp" {
        let (document, revision) = config(loc)?;
        let value = section(&document, loc)
            .and_then(|s| s.get(&resource.name))
            .cloned()
            .ok_or_else(|| other("配置已变化，请刷新"))?;
        let mut resource = resource;
        resource.revision = revision;
        return Ok(ResourceDetail {
            content: serde_json::to_string_pretty(&value)?,
            config: Some(value),
            resource,
        });
    }
    Err(other("资源类型未知"))
}

/// Detail for one resource taken from an already-merged catalog. Callers that
/// also need the catalog (the detail page) reuse a single scan.
pub fn detail_in_catalog(
    inventory: &Inventory,
    mcp_registry: &super::McpRegistry,
    id: &str,
) -> Result<Option<ResourceDetail>> {
    let Some(mut resource) = inventory.resources.iter().find(|row| row.id == id).cloned() else {
        return Ok(None);
    };
    if resource.kind == "skill" {
        return Ok(Some(ResourceDetail {
            content: files::read(Path::new(&resource.path))?,
            config: None,
            resource,
        }));
    }
    if resource.kind == "mcp" {
        if let Some(definition) = super::mcp_registry::find(mcp_registry, id) {
            resource.revision = super::mcp_registry::revision(definition);
            return Ok(Some(ResourceDetail {
                content: serde_json::to_string_pretty(&definition.config)?,
                config: Some(definition.config.clone()),
                resource,
            }));
        }
        let location = resource
            .locations
            .iter()
            .find_map(|id| inventory.locations.iter().find(|loc| &loc.id == id))
            .ok_or_else(|| other("配置来源已不存在"))?;
        let (document, revision) = config(location)?;
        let value = section(&document, location)
            .and_then(|section| section.get(&resource.name))
            .cloned()
            .ok_or_else(|| other("配置已变化，请刷新"))?;
        resource.revision = revision;
        return Ok(Some(ResourceDetail {
            content: serde_json::to_string_pretty(&value)?,
            config: Some(value),
            resource,
        }));
    }
    Err(other("资源类型未知"))
}

pub(super) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 200
        || name.starts_with('-')
        || name
            .chars()
            .any(|c| !(c.is_alphanumeric() || "_- .@".contains(c)))
    {
        return Err(other(
            "名称只能包含文字、数字、空格及 _ - . @，且不能以 - 开头",
        ));
    }
    Ok(())
}

pub(super) fn validate_mcp(agent: Agent, value: &Value) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| other("服务器配置必须是 JSON 对象"))?;
    // Pi explicitly permits sparse project enable/exposure overrides.
    if agent == Agent::Pi
        && !object.contains_key("command")
        && !object.contains_key("url")
        && !object.contains_key("type")
        && !object.is_empty()
        && object
            .keys()
            .all(|k| ["enabled", "exposure", "toolExposure"].contains(&k.as_str()))
    {
        return Ok(());
    }
    let command = object
        .get("command")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    let url_key = if agent == Agent::Antigravity {
        "serverUrl"
    } else {
        "url"
    };
    let url = object
        .get(url_key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    if command.is_some() == url.is_some() {
        return Err(other(format!("必须且只能配置 command 或 {url_key}")));
    }
    if let Some(url) = url {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(other("MCP 地址必须是 HTTP 或 HTTPS"));
        }
    }
    if agent == Agent::ClaudeCode
        && url.is_some()
        && !matches!(
            object.get("type").and_then(Value::as_str),
            Some("http" | "streamable-http" | "sse" | "ws")
        )
    {
        return Err(other("Claude 远程 MCP 需要明确设置 type，例如 http"));
    }
    if let Some(args) = object.get("args") {
        if !args
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string))
        {
            return Err(other("args 必须是字符串数组"));
        }
    }
    for field in ["env", "headers", "http_headers"] {
        if let Some(v) = object.get(field) {
            if !v
                .as_object()
                .is_some_and(|m| m.values().all(Value::is_string))
            {
                return Err(other(format!("{field} 必须是字符串键值对象")));
            }
        }
    }
    for field in ["enabled", "disabled"] {
        if object.get(field).is_some_and(|v| !v.is_boolean()) {
            return Err(other(format!("{field} 必须是布尔值")));
        }
    }
    if agent == Agent::Pi && object.get("type").and_then(Value::as_str) == Some("sse") {
        return Err(other("Pi 不支持旧版 SSE，请使用 Streamable HTTP"));
    }
    Ok(())
}

pub(super) fn mutate_mcp(
    loc: &Location,
    expected: &str,
    name: &str,
    server: Option<Value>,
    create: bool,
) -> Result<()> {
    let (mut value, revision) = config(loc)?;
    super::check_revision(&revision, expected)?;
    let mut section = &mut value;
    for part in &loc.section {
        let map = section
            .as_object_mut()
            .ok_or_else(|| other("配置结构不是对象，无法安全修改"))?;
        section = map.entry(part).or_insert_with(|| json!({}));
    }
    let map = section
        .as_object_mut()
        .ok_or_else(|| other("MCP 配置不是对象"))?;
    if create && map.contains_key(name) {
        return Err(other("同名配置已存在，请编辑已有项"));
    }
    if !create && !map.contains_key(name) {
        return Err(other("配置已不存在，请刷新"));
    }
    match &server {
        Some(v) => {
            map.insert(name.into(), v.clone());
        }
        None => {
            map.remove(name);
        }
    }
    let text = if loc.agent == Some(Agent::Codex) {
        let raw = files::read(Path::new(&loc.path))?;
        let mut doc = raw.parse::<toml_edit::Document>().map_err(other)?;
        if let Some(server) = server {
            let table = toml_edit::ser::to_document(&server).map_err(other)?;
            if doc.get("mcp_servers").is_none() {
                doc["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
            }
            let entries = doc["mcp_servers"]
                .as_table_like_mut()
                .ok_or_else(|| other("mcp_servers 不是表"))?;
            entries.insert(name, toml_edit::Item::Table(table.as_table().clone()));
        } else if let Some(entries) = doc
            .get_mut("mcp_servers")
            .and_then(toml_edit::Item::as_table_like_mut)
        {
            entries.remove(name);
        }
        doc.to_string()
    } else {
        format!("{}\n", serde_json::to_string_pretty(&value)?)
    };
    files::write(Path::new(&loc.path), expected, &text)
}

pub fn save_mcp(
    roots: &Roots,
    cwd: Option<&Path>,
    location_id: &str,
    expected: &str,
    name: &str,
    server: Value,
    create: bool,
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    validate_name(name)?;
    let loc = locations(roots, cwd)
        .into_iter()
        .find(|l| l.kind == "mcp" && l.id == location_id)
        .ok_or_else(|| other("配置来源无效"))?;
    validate_mcp(loc.agent.unwrap(), &server)?;
    mutate_mcp(&loc, expected, name, Some(server), create)?;
    Ok(OperationResult {
        message: "MCP 配置已保存；已有终端请按 Agent 的方式重新加载或重新打开".into(),
    })
}

pub fn remove_mcp(
    roots: &Roots,
    cwd: Option<&Path>,
    id: &str,
    expected: &str,
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let list = list(roots, cwd);
    let row = list
        .resources
        .iter()
        .find(|r| r.id == id && r.kind == "mcp")
        .ok_or_else(|| other("MCP 已不存在"))?;
    let loc = list
        .locations
        .iter()
        .find(|l| row.locations.contains(&l.id))
        .ok_or_else(|| other("来源已不存在"))?;
    mutate_mcp(loc, expected, &row.name, None, false)?;
    Ok(OperationResult {
        message: "MCP 配置已删除".into(),
    })
}

pub fn location_revision(roots: &Roots, cwd: Option<&Path>, id: &str) -> Result<String> {
    let loc = locations(roots, cwd)
        .into_iter()
        .find(|l| l.kind == "mcp" && l.id == id)
        .ok_or_else(|| other("配置来源无效"))?;
    files::revision(Path::new(&loc.path))
}
