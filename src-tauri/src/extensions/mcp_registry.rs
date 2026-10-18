//! NoEnding-owned MCP definitions (B model). A definition is scope-neutral:
//! enabling it writes the same logical server into user or project config
//! files, and identical configurations discovered in native files collapse into
//! the same row.
use super::{Inventory, Location, OperationResult, Resource, ResourceDetail, Roots};
use crate::{
    domain::Agent,
    error::{other, Result},
    storage::Db,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::path::PathBuf;

pub(super) const KEY: &str = "extensions.mcp_definitions";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpRegistry {
    #[serde(default)]
    pub definitions: Vec<McpDefinition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpDefinition {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub config: Value,
}

pub fn load(db: &Db) -> Result<McpRegistry> {
    match db.get_setting(KEY)? {
        Some(raw) => decode(&raw),
        None => Ok(McpRegistry::default()),
    }
}

fn decode(raw: &str) -> Result<McpRegistry> {
    let mut registry: McpRegistry =
        serde_json::from_str(raw).map_err(|e| other(format!("MCP 定义清单无法读取：{e}")))?;
    let mut ids = std::collections::HashSet::new();
    for definition in &mut registry.definitions {
        definition.config = canonical(&definition.name, &definition.config)["config"].take();
        definition.id = identity(&definition.name, &definition.config);
    }
    registry
        .definitions
        .retain(|definition| ids.insert(definition.id.clone()));
    Ok(registry)
}

/// Read and edit the current registry under the database writer transaction.
/// File operations must finish before entering this short transaction.
fn edit<T>(db: &Db, change: impl FnOnce(&mut McpRegistry) -> Result<T>) -> Result<T> {
    db.tx(|tx| {
        let raw: Option<String> = tx
            .query_row("SELECT value FROM settings WHERE key = ?1", [KEY], |row| {
                row.get(0)
            })
            .optional()?;
        let mut registry = match raw {
            Some(raw) => decode(&raw)?,
            None => McpRegistry::default(),
        };
        let result = change(&mut registry)?;
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [KEY, &serde_json::to_string(&registry)?],
        )?;
        Ok(result)
    })
}

pub fn store(db: &Db, name: &str, description: &str, config: Value) -> Result<String> {
    edit(db, |registry| {
        Ok(register(registry, name, description, config)?.id.clone())
    })
}

/// Remove only the uninstalled definition, preserving registrations made while
/// its native configuration files were being removed.
pub fn forget(db: &Db, id: &str) -> Result<()> {
    edit(db, |registry| {
        remove(registry, id);
        Ok(())
    })
}

/// Logical identity: transport spellings and native enable flags are Agent
/// details, not different MCP configurations.
pub fn identity(name: &str, server: &Value) -> String {
    super::key(format!("mcp:{}", canonical(name, server)))
}

pub fn canonical(name: &str, server: &Value) -> Value {
    let mut config = server.clone();
    if let Some(map) = config.as_object_mut() {
        if !map.contains_key("url") {
            if let Some(url) = map.remove("serverUrl") {
                map.insert("url".into(), url);
            }
        } else {
            map.remove("serverUrl");
        }
        if let Some(headers) = map.remove("http_headers") {
            // Static HTTP headers have different native names in Codex and
            // the JSON-based Agents, but represent the same configuration.
            let mut headers = headers;
            if let (Some(native), Some(portable)) = (
                headers.as_object_mut(),
                map.get("headers").and_then(Value::as_object),
            ) {
                native.extend(portable.clone());
            }
            map.insert("headers".into(), headers);
        }
        if matches!(
            map.get("type").and_then(Value::as_str),
            Some("http" | "stdio")
        ) {
            map.remove("type");
        }
        map.remove("enabled");
        map.remove("disabled");
    }
    json!({ "name": name, "config": config })
}

pub fn find<'a>(registry: &'a McpRegistry, id: &str) -> Option<&'a McpDefinition> {
    registry
        .definitions
        .iter()
        .find(|definition| definition.id == id)
}

pub fn register<'a>(
    registry: &'a mut McpRegistry,
    name: &str,
    description: &str,
    config: Value,
) -> Result<&'a McpDefinition> {
    super::inventory::validate_name(name)?;
    if !config.is_object() {
        return Err(other("服务器配置必须是 JSON 对象"));
    }
    let canonical_config = canonical(name, &config);
    let config = canonical_config
        .get("config")
        .cloned()
        .ok_or_else(|| other("MCP 配置无法规范化"))?;
    for agent in [
        Agent::Codex,
        Agent::ClaudeCode,
        Agent::Pi,
        Agent::Antigravity,
    ] {
        super::inventory::validate_mcp(agent, &for_agent(agent, &config))?;
    }
    let id = identity(name, &config);
    if registry
        .definitions
        .iter()
        .any(|definition| definition.id == id)
    {
        return Err(other("相同配置已存在"));
    }
    registry.definitions.push(McpDefinition {
        id,
        name: name.into(),
        description: description.into(),
        config,
    });
    Ok(registry
        .definitions
        .last()
        .expect("definition was just pushed"))
}

pub fn remove(registry: &mut McpRegistry, id: &str) -> Option<McpDefinition> {
    let index = registry
        .definitions
        .iter()
        .position(|definition| definition.id == id)?;
    Some(registry.definitions.remove(index))
}

pub fn revision(definition: &McpDefinition) -> String {
    super::key(format!(
        "mcp-definition:{}",
        canonical(&definition.name, &definition.config)
    ))
}

/// Mark matching native configurations as definition-backed and list inactive
/// definitions so a disabled MCP stays manageable.
pub fn merge(registry: &McpRegistry, inventory: &mut Inventory, include_inactive: bool) {
    if registry.definitions.is_empty() {
        return;
    }
    for row in inventory
        .resources
        .iter_mut()
        .filter(|row| row.kind == "mcp")
    {
        let Some(definition) = find(registry, &row.id) else {
            continue;
        };
        row.registered = true;
        if row.name.is_empty() {
            row.name = definition.name.clone();
        }
        if row.description.is_empty() {
            row.description = definition.description.clone();
        }
    }
    if include_inactive {
        for definition in &registry.definitions {
            if !inventory
                .resources
                .iter()
                .any(|row| row.id == definition.id)
            {
                inventory.resources.push(inactive(definition));
            }
        }
    }
}

fn for_agent(agent: Agent, config: &Value) -> Value {
    let mut server = config.clone();
    let Some(map) = server.as_object_mut() else {
        return server;
    };
    if agent == Agent::Codex {
        if let Some(headers) = map.remove("headers") {
            map.insert("http_headers".into(), headers);
        }
    } else if agent == Agent::Antigravity {
        if let Some(url) = map.remove("url") {
            map.insert("serverUrl".into(), url);
        }
    } else if agent == Agent::ClaudeCode && map.contains_key("url") && !map.contains_key("type") {
        map.insert("type".into(), json!("http"));
    }
    server
}

fn targets(roots: &Roots, cwd: Option<&Path>) -> Vec<Location> {
    super::inventory::locations(roots, cwd)
        .into_iter()
        .filter(|location| {
            location.kind == "mcp" && cwd.map_or(true, |_| location.scope == "project")
        })
        .collect()
}

fn ensure(location: &Location, definition: &McpDefinition) -> Result<bool> {
    let agent = location
        .agent
        .ok_or_else(|| other("MCP 配置位置缺少 Agent"))?;
    let server = for_agent(agent, &definition.config);
    super::inventory::validate_mcp(agent, &server)?;
    let (value, revision) = super::inventory::config(location)?;
    if let Some(existing) = super::inventory::section(&value, location)
        .and_then(|section| section.get(&definition.name))
    {
        if identity(&definition.name, existing) == definition.id {
            return Ok(false);
        }
        return Err(other(format!(
            "{} 中已有同名但不同配置的 MCP",
            location.path
        )));
    }
    super::inventory::mutate_mcp(location, &revision, &definition.name, Some(server), true)?;
    Ok(true)
}

fn clear(location: &Location, definition: &McpDefinition) -> Result<bool> {
    let (value, revision) = super::inventory::config(location)?;
    let Some(existing) = super::inventory::section(&value, location)
        .and_then(|section| section.get(&definition.name))
    else {
        return Ok(false);
    };
    if identity(&definition.name, existing) != definition.id {
        return Ok(false);
    }
    super::inventory::mutate_mcp(location, &revision, &definition.name, None, false)?;
    Ok(true)
}

fn contains(location: &Location, id: &str) -> Result<bool> {
    let (value, _) = super::inventory::config(location)?;
    Ok(super::inventory::section(&value, location)
        .and_then(Value::as_object)
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|(name, server)| identity(name, server) == id)
        }))
}

/// Project-level configurations of the same identity are redundant once the
/// definition is user-level enabled.
fn clear_projects(
    roots: &Roots,
    cwds: &[PathBuf],
    definition: &McpDefinition,
) -> (usize, Vec<String>) {
    let mut removed = 0;
    let mut failures = Vec::new();
    for cwd in cwds {
        for location in targets(roots, Some(cwd)) {
            match clear(&location, definition) {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(error) => failures.push(format!("{}：{error}", location.path)),
            }
        }
    }
    (removed, failures)
}

fn clear_identity(location: &Location, id: &str) -> Result<bool> {
    let (value, revision) = super::inventory::config(location)?;
    let Some(section) = super::inventory::section(&value, location) else {
        return Ok(false);
    };
    let Some(entries) = section.as_object() else {
        return Ok(false);
    };
    let Some(name) = entries
        .iter()
        .find(|(name, server)| identity(name, server) == id)
        .map(|(name, _)| name.clone())
    else {
        return Ok(false);
    };
    super::inventory::mutate_mcp(location, &revision, &name, None, false)?;
    Ok(true)
}

fn report(
    written: usize,
    failures: Vec<String>,
    done: &str,
    nothing: &str,
) -> Result<OperationResult> {
    if written == 0 && !failures.is_empty() {
        return Err(other(format!(
            "未能{done}任何位置：{}",
            failures.join("；")
        )));
    }
    let message = if failures.is_empty() {
        if written == 0 {
            nothing.into()
        } else {
            format!("{done} {written} 个位置")
        }
    } else {
        format!("{done} {written} 个位置；失败：{}", failures.join("；"))
    };
    Ok(OperationResult { message })
}

pub fn enable_user(
    roots: &Roots,
    registry: &McpRegistry,
    id: &str,
    cwds: &[PathBuf],
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let definition = find(registry, id).ok_or_else(|| other("MCP 定义已不存在"))?;
    let mut written = 0;
    let mut failures = Vec::new();
    for location in targets(roots, None) {
        match ensure(&location, definition) {
            Ok(true) => written += 1,
            Ok(false) => {}
            Err(error) => failures.push(format!("{}：{error}", location.path)),
        }
    }
    let complete = failures.is_empty();
    let mut result = report(written, failures, "已启用", "用户级已全部启用")?;
    if complete {
        let (removed, cleanup_failures) = clear_projects(roots, cwds, definition);
        if removed > 0 {
            result
                .message
                .push_str(&format!("；已清理 {removed} 个项目配置"));
        }
        if !cleanup_failures.is_empty() {
            result.message.push_str(&format!(
                "；项目配置清理失败：{}",
                cleanup_failures.join("；")
            ));
        }
    }
    Ok(result)
}

pub fn disable_user(roots: &Roots, registry: &McpRegistry, id: &str) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let definition = find(registry, id).ok_or_else(|| other("MCP 定义已不存在"))?;
    let mut removed = 0;
    let mut failures = Vec::new();
    for location in targets(roots, None) {
        match clear(&location, definition) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(error) => failures.push(format!("{}：{error}", location.path)),
        }
    }
    report(removed, failures, "已停用", "当前未在用户级启用")
}

pub fn enable_projects(
    roots: &Roots,
    registry: &McpRegistry,
    id: &str,
    cwds: &[PathBuf],
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let definition = find(registry, id).ok_or_else(|| other("MCP 定义已不存在"))?;
    for location in targets(roots, None) {
        if contains(&location, id)? {
            return Err(other("该 MCP 已由用户级启用，不能再配置项目级入口"));
        }
    }
    let mut written = 0;
    let mut failures = Vec::new();
    for cwd in cwds {
        for location in targets(roots, Some(cwd)) {
            match ensure(&location, definition) {
                Ok(true) => written += 1,
                Ok(false) => {}
                Err(error) => failures.push(format!("{}：{error}", location.path)),
            }
        }
    }
    report(written, failures, "已启用", "项目已全部启用")
}

pub fn disable_projects(
    roots: &Roots,
    registry: &McpRegistry,
    id: &str,
    cwds: &[PathBuf],
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let definition = find(registry, id).ok_or_else(|| other("MCP 定义已不存在"))?;
    let mut removed = 0;
    let mut failures = Vec::new();
    for cwd in cwds {
        for location in targets(roots, Some(cwd)) {
            match clear(&location, definition) {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(error) => failures.push(format!("{}：{error}", location.path)),
            }
        }
    }
    report(removed, failures, "已停用", "当前未在项目中启用")
}

/// Remove every native entry matching this configuration, then the definition.
/// A failed file keeps the definition so the operation can be retried.
pub fn uninstall(
    registry: &mut McpRegistry,
    roots: &Roots,
    id: &str,
    cwds: &[PathBuf],
) -> Result<OperationResult> {
    let _guard = super::MUTATION
        .lock()
        .map_err(|_| other("配置操作锁不可用"))?;
    let mut removed = 0;
    let mut failures = Vec::new();
    for location in targets(roots, None) {
        match clear_identity(&location, id) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(error) => failures.push(format!("{}：{error}", location.path)),
        }
    }
    for cwd in cwds {
        for location in targets(roots, Some(cwd)) {
            match clear_identity(&location, id) {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(error) => failures.push(format!("{}：{error}", location.path)),
            }
        }
    }
    let definition = remove(registry, id);
    if !failures.is_empty() {
        if let Some(definition) = definition {
            registry.definitions.push(definition);
        }
        return Err(other(format!("部分位置未能移除：{}", failures.join("；"))));
    }
    if removed == 0 && definition.is_none() {
        return Err(other("MCP 配置已不存在"));
    }
    Ok(OperationResult {
        message: if removed == 0 {
            "MCP 定义已删除".into()
        } else {
            format!("已删除 MCP 配置及 {removed} 个文件中的入口")
        },
    })
}

pub fn detail(
    roots: &Roots,
    cwd: Option<&Path>,
    registry: &McpRegistry,
    id: &str,
) -> Result<ResourceDetail> {
    let definition = find(registry, id).ok_or_else(|| other("MCP 定义已不存在"))?;
    let mut inventory = super::list(roots, cwd);
    merge(registry, &mut inventory, true);
    let mut resource = inventory
        .resources
        .into_iter()
        .find(|row| row.id == id && row.kind == "mcp")
        .unwrap_or_else(|| inactive(definition));
    resource.revision = revision(definition);
    Ok(ResourceDetail {
        resource,
        content: serde_json::to_string_pretty(&definition.config)?,
        config: Some(definition.config.clone()),
    })
}

fn inactive(definition: &McpDefinition) -> Resource {
    Resource {
        id: definition.id.clone(),
        kind: "mcp".into(),
        name: definition.name.clone(),
        description: definition.description.clone(),
        path: String::new(),
        locations: Vec::new(),
        entry_paths: Vec::new(),
        entry_links: Vec::new(),
        revision: revision(definition),
        registered: true,
        error: None,
    }
}
