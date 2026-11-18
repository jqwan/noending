use super::*;
use crate::{
    agent_runtime::{self, AgentRuntimeOverrides},
    domain::*,
    storage::{assistant_profiles as repo, new_id, now},
    workspace::project,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn lock(home: &NoEndingHome) -> Result<std::fs::File> {
    fs::create_dir_all(&home.runtime_dir)?;
    Ok(crate::platform::file_lock::exclusive_wait(
        &home.runtime_dir.join("assistant-profiles.lock"),
        std::time::Duration::from_secs(10),
    )?)
}
fn validated_agent(agent: &str, runtime: Option<&AgentRuntimeOverrides>) -> Result<Agent> {
    let agent = Agent::parse(agent).ok_or_else(|| other("未知运行工具"))?;
    if !crate::adapters::adapter_for(agent).has_terminal_cli() {
        return Err(other("此运行工具不支持智能体终端"));
    }
    if let Some(runtime) = runtime {
        agent_runtime::validate_runtime_overrides(agent, runtime)?;
    }
    Ok(agent)
}
fn directory_of(db: &Db, p: &AssistantProfile) -> Result<WorkspacePath> {
    db.get_workspace_path(&p.workspace_path_id)?
        .ok_or_else(|| other("智能体主目录未登记"))
}
fn ensure_profile_parent(home: &NoEndingHome) -> Result<()> {
    fs::create_dir_all(directory(home))?;
    let actual = fs::canonicalize(directory(home))?;
    let expected = fs::canonicalize(&home.root)?.join("assistant");
    if crate::workspace::path_identity_of(&actual.to_string_lossy())
        != crate::workspace::path_identity_of(&expected.to_string_lossy())
    {
        return Err(other("智能体空间目录指向其他位置，未修改目录"));
    }
    Ok(())
}
fn validate_directory(
    home: &NoEndingHome,
    path: &WorkspacePath,
    p: &AssistantProfile,
) -> Result<PathBuf> {
    let expected = directory(home).join(&p.id);
    if !expected.is_dir() {
        return Err(other("智能体主目录不存在，请先恢复目录"));
    }
    let canonical = fs::canonicalize(&expected)?;
    let parent = fs::canonicalize(directory(home))?;
    let home_parent = fs::canonicalize(&home.root)?.join("assistant");
    if crate::workspace::path_identity_of(&parent.to_string_lossy())
        != crate::workspace::path_identity_of(&home_parent.to_string_lossy())
    {
        return Err(other("智能体空间目录指向其他位置"));
    }
    let registered = fs::canonicalize(&path.canonical_path)?;
    if canonical.parent() != Some(parent.as_path())
        || crate::workspace::path_identity_of(&canonical.to_string_lossy())
            != crate::workspace::path_identity_of(&registered.to_string_lossy())
    {
        return Err(other("智能体目录位置已变化或不属于当前 Home"));
    }
    Ok(canonical)
}
fn prepare_default_skill(
    db: &Db,
    home: &NoEndingHome,
    roots: &crate::skills::Roots,
    cwd: &Path,
) -> Result<()> {
    crate::skills::prepare_noending_skill(db, home, &crate::cli::executable()?)?;
    let registry = crate::skills::load_skill_registry(db)?;
    let source = registry
        .sources
        .iter()
        .find(|s| s.name == "noending")
        .ok_or_else(|| other("NoEnding 技能未安装，请在技能管理中恢复"))?;
    let inventory = crate::skills::list(roots, None);
    let global = inventory.resources.iter().any(|row| {
        row.id == source.id
            && row.locations.iter().any(|id| {
                inventory
                    .locations
                    .iter()
                    .any(|l| &l.id == id && l.scope == "user")
            })
    });
    if !global {
        crate::skills::enable_skill_project(
            roots,
            &[cwd.to_path_buf()],
            &source.id,
            Some(&source.path),
        )?;
    }
    crate::skills::invalidate_catalog();
    Ok(())
}
pub fn create_profile(
    db: &Db,
    home: &NoEndingHome,
    layer: &WorkspaceLayer,
    input: AssistantCreate,
) -> Result<AssistantDetail> {
    create_profile_with_roots(db, home, layer, input, &crate::skills::Roots::current()?)
}
/// Explicit roots keep native loading locations isolated in tests and local QA.
pub fn create_profile_with_roots(
    db: &Db,
    home: &NoEndingHome,
    layer: &WorkspaceLayer,
    input: AssistantCreate,
    roots: &crate::skills::Roots,
) -> Result<AssistantDetail> {
    let _access = home.acquire_access()?;
    let _lock = lock(home)?;
    crate::storage::ensure_not_empty("智能体名称", &input.name)?;
    let agent = validated_agent(&input.agent, input.runtime_override.as_ref())?;
    let id = new_id();
    ensure_profile_parent(home)?;
    let root = directory(home).join(&id);
    let staging = home.runtime_dir.join(format!("assistant-create-{id}"));
    fs::create_dir(&staging)?;
    let stamp = now();
    let mut p = AssistantProfile {
        id,
        name: input.name.trim().into(),
        avatar: input.avatar,
        description: input.description,
        instructions: input.instructions,
        agent,
        runtime_override: input.runtime_override.map(|r| r.normalized()),
        workspace_path_id: String::new(),
        visibility: "normal".into(),
        revision: 1,
        created_at: stamp.clone(),
        updated_at: stamp,
    };
    let preparation = (|| {
        files::prepare(&staging, &p)?;
        prepare_default_skill(db, home, roots, &staging)
    })();
    if let Err(error) = preparation {
        // This unpublished staging directory was allocated by this call in the
        // application's runtime area and contains only its generated entries.
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if root.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err(other("智能体目录已存在，未覆盖原内容"));
    }
    if let Err(error) = fs::rename(&staging, &root) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error.into());
    }
    let projection = layer.projection();
    let observation = projection.observer().observe(&root.to_string_lossy());
    db.tx(|tx|{
        let path=project::ensure_workspace_path_conn(tx,&observation,projection.policy())?.path;
        p.workspace_path_id=path.id;
        crate::storage::workspace::set_project_name_conn(tx,&path.project_id,&p.name,false)?;
        repo::save(tx,&p,None)?;
        // Exact matches can already exist after a recovered source scan.
        tx.execute("INSERT OR IGNORE INTO assistant_sessions(session_id,assistant_id) SELECT id,?1 FROM sessions WHERE workspace_path_id=?2",rusqlite::params![p.id,p.workspace_path_id])?;
        Ok(())
    })?;
    detail(db, &p.id)
}
pub fn detail(db: &Db, id: &str) -> Result<AssistantDetail> {
    let (profile, projects, tasks, session_count, last_activity_at) = {
        let conn = db.read();
        let p = repo::get(&conn, id)?;
        let projects = repo::project_ids(&conn, id)?
            .iter()
            .map(|id| {
                crate::storage::workspace::get_project_conn(&conn, id)?
                    .ok_or_else(|| other("负责项目已不存在"))
            })
            .collect::<Result<Vec<_>>>()?;
        let tasks = repo::task_ids(&conn, id)?
            .iter()
            .map(|id| {
                crate::storage::get_workstream_conn(&conn, id)?
                    .ok_or_else(|| other("负责任务已不存在"))
            })
            .collect::<Result<Vec<_>>>()?;
        let (count, activity) = repo::counts(&conn, id)?;
        (p, projects, tasks, count, activity)
    };
    let path = directory_of(db, &profile)?;
    let project = db
        .get_project(&path.project_id)?
        .ok_or_else(|| other("智能体项目不存在"))?;
    let state = files::state(Path::new(&path.canonical_path), &profile)?;
    Ok(AssistantDetail {
        profile,
        project,
        workspace_path: path,
        projects,
        tasks,
        session_count,
        last_activity_at,
        instructions_state: state,
    })
}
pub fn update_profile(
    db: &Db,
    home: &NoEndingHome,
    id: &str,
    revision: i64,
    patch: AssistantPatch,
) -> Result<AssistantDetail> {
    let _lock = lock(home)?;
    db.tx(|tx| {
        let mut p = repo::get(tx, id)?;
        repo::check_revision(&p, revision)?;
        if let Some(name) = patch.name {
            crate::storage::ensure_not_empty("智能体名称", &name)?;
            p.name = name.trim().into();
        }
        if let Some(value) = patch.avatar {
            p.avatar = value;
        }
        if let Some(value) = patch.description {
            p.description = value;
        }
        if let Some(value) = patch.instructions {
            p.instructions = value;
        }
        if let Some(agent) = patch.agent {
            if agent != p.agent.as_str() {
                p.runtime_override = None;
            }
            p.agent = validated_agent(&agent, None)?;
        }
        if let Some(value) = patch.runtime_override {
            p.runtime_override = value.map(|r| r.normalized());
        }
        validated_agent(p.agent.as_str(), p.runtime_override.as_ref())?;
        p.revision += 1;
        p.updated_at = now();
        repo::save(tx, &p, Some(revision))
    })?;
    detail(db, id)
}
pub fn set_responsibilities(
    db: &Db,
    home: &NoEndingHome,
    id: &str,
    revision: i64,
    projects: &[String],
    tasks: &[String],
) -> Result<AssistantDetail> {
    let _lock = lock(home)?;
    db.tx(|tx| {
        let mut p = repo::get(tx, id)?;
        repo::check_revision(&p, revision)?;
        if p.visibility != "normal" {
            return Err(other("请先恢复智能体再调整责任范围"));
        }
        repo::set_responsibilities(tx, id, projects, tasks)?;
        p.revision += 1;
        p.updated_at = now();
        repo::save(tx, &p, Some(revision))
    })?;
    detail(db, id)
}
pub fn set_archived(
    db: &Db,
    home: &NoEndingHome,
    id: &str,
    revision: i64,
    archived: bool,
) -> Result<AssistantDetail> {
    let _lock = lock(home)?;
    db.tx(|tx| {
        let mut p = repo::get(tx, id)?;
        repo::check_revision(&p, revision)?;
        p.visibility = if archived { "archived" } else { "normal" }.into();
        p.revision += 1;
        p.updated_at = now();
        repo::save(tx, &p, Some(revision))
    })?;
    detail(db, id)
}
pub fn prepare_profile_files(
    db: &Db,
    home: &NoEndingHome,
    id: &str,
    revision: i64,
) -> Result<AssistantDetail> {
    let _lock = lock(home)?;
    let p = repo::get(&db.read(), id)?;
    repo::check_revision(&p, revision)?;
    let path = directory_of(db, &p)?;
    let expected = directory(home).join(&p.id);
    if !expected.exists() {
        if crate::workspace::path_identity_of(&expected.to_string_lossy()).as_deref()
            != Some(path.id.as_str())
            || fs::symlink_metadata(&expected).is_ok()
        {
            return Err(other("智能体目录位置已变化，未重新创建"));
        }
        ensure_profile_parent(home)?;
        fs::create_dir(&expected)?;
    }
    let root = validate_directory(home, &path, &p)?;
    files::prepare(&root, &p)?;
    detail(db, id)
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    query: Option<String>,
    scope: String,
    after: String,
    id: String,
    kind: String,
}
fn page<T>(
    mut rows: Vec<(String, String, T)>,
    query: AssistantListQuery,
    kind: &str,
) -> Result<AssistantPage<T>> {
    let scope = query.scope.as_deref().unwrap_or("unarchived");
    if !["all", "unarchived", "archived"].contains(&scope) {
        return Err(other("scope 无效"));
    }
    let limit = query.limit.unwrap_or(20);
    if limit == 0 || limit > 50 {
        return Err(other("limit 必须在 1～50 之间"));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    if let Some(cursor) = query.cursor {
        let cursor: Cursor = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| other("游标无效"))?,
        )?;
        if cursor.kind != kind || cursor.query != query.query || cursor.scope != scope {
            return Err(other("游标与查询范围不匹配"));
        }
        rows.retain(|r| r.0 < cursor.after || (r.0 == cursor.after && r.1 > cursor.id));
    }
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next_cursor = if more {
        rows.last().map(|r| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&Cursor {
                    query: query.query,
                    scope: scope.into(),
                    after: r.0.clone(),
                    id: r.1.clone(),
                    kind: kind.into(),
                })
                .unwrap(),
            )
        })
    } else {
        None
    };
    Ok(AssistantPage {
        items: rows.into_iter().map(|r| r.2).collect(),
        next_cursor,
        end: !more,
    })
}
pub fn list_profiles(db: &Db, query: AssistantListQuery) -> Result<AssistantPage<AssistantCard>> {
    let scope = query.scope.as_deref().unwrap_or("unarchived");
    if !["all", "archived", "unarchived"].contains(&scope) {
        return Err(other("scope 无效"));
    }
    let limit = query.limit.unwrap_or(20);
    if limit == 0 || limit > 50 {
        return Err(other("limit 必须在 1～50 之间"));
    }
    let cursor = if let Some(raw) = query.cursor.as_deref() {
        let cursor: Cursor = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(raw)
                .map_err(|_| other("游标无效"))?,
        )?;
        if cursor.kind != "profiles" || cursor.scope != scope || cursor.query != query.query {
            return Err(other("游标与查询范围不匹配"));
        }
        Some(cursor)
    } else {
        None
    };
    let rows = repo::card_page(
        &db.read(),
        scope,
        query.query.as_deref().unwrap_or(""),
        cursor.as_ref().map(|c| (c.after.as_str(), c.id.as_str())),
        limit + 1,
    )?;
    page(rows, query, "profiles")
}
pub fn list_profile_sessions(
    db: &Db,
    id: &str,
    query: AssistantListQuery,
) -> Result<AssistantPage<SessionListItem>> {
    repo::get(&db.read(), id)?;
    let scope = match query.scope.as_deref().unwrap_or("unarchived") {
        "all" => SessionListScope::All,
        "archived" => SessionListScope::Archived,
        "unarchived" => SessionListScope::Unarchived,
        _ => return Err(other("scope 无效")),
    };
    let kind = format!("sessions:{id}");
    let cursor = if let Some(raw) = query.cursor.as_deref() {
        let cursor: Cursor = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(raw)
                .map_err(|_| other("游标无效"))?,
        )?;
        if cursor.kind != kind
            || cursor.scope != query.scope.as_deref().unwrap_or("unarchived")
            || cursor.query != query.query
        {
            return Err(other("游标与查询范围不匹配"));
        }
        Some(cursor)
    } else {
        None
    };
    let limit = query.limit.unwrap_or(20);
    if limit == 0 || limit > 50 {
        return Err(other("limit 必须在 1～50 之间"));
    }
    let scope_name = match scope {
        SessionListScope::All => "all",
        SessionListScope::Archived => "archived",
        SessionListScope::Unarchived => "unarchived",
    };
    let rows = repo::session_page(
        &db.read(),
        id,
        scope_name,
        cursor.as_ref().map(|c| (c.after.as_str(), c.id.as_str())),
        limit + 1,
    )?
    .into_iter()
    .map(|s| {
        (
            s.last_conversation_at
                .clone()
                .or(s.last_activity_at.clone())
                .or(s.started_at.clone())
                .unwrap_or_default(),
            s.id.clone(),
            s,
        )
    })
    .collect();
    page(rows, query, &format!("sessions:{id}"))
}
/// All launch paths share this validation; the authoritative cwd is never client supplied.
pub fn profile_launch(
    db: &Db,
    home: &NoEndingHome,
    id: &str,
    tasks: &[String],
    runtime: Option<&AgentRuntimeOverrides>,
) -> Result<(
    AssistantProfile,
    WorkspacePath,
    Option<AgentRuntimeOverrides>,
)> {
    let _lock = lock(home)?;
    let p = repo::get(&db.read(), id)?;
    if p.visibility != "normal" {
        return Err(other("已归档智能体不能开始新会话"));
    }
    let path = directory_of(db, &p)?;
    let root = validate_directory(home, &path, &p)?;
    let project = db
        .get_project(&path.project_id)?
        .ok_or_else(|| other("智能体项目不存在"))?;
    if project.visibility != "normal" {
        return Err(other("智能体项目已归档，请先恢复项目"));
    }
    for task in tasks {
        let conn = db.read();
        let assigned = repo::task_ids(&conn, id)?;
        let record =
            crate::storage::get_workstream_conn(&conn, task)?.ok_or_else(|| other("任务不存在"))?;
        if !assigned.contains(task) || record.visibility != "normal" {
            return Err(other("智能体未负责该任务或任务已归档"));
        }
    }
    let runtime = runtime.cloned().or_else(|| p.runtime_override.clone());
    validated_agent(p.agent.as_str(), runtime.as_ref())?;
    files::prepare(&root, &p)?;
    Ok((p, path, runtime))
}
