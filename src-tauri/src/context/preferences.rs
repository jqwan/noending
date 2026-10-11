//! One preference result per Agent call. Invalid changes never discard summaries.
use crate::error::{other, Result};
use crate::storage::preferences as repo;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreferenceReply {
    Completed {},
    Failed { error: String },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    changes: Vec<Change>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Scope {
    User,
    Project,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Change {
    Add {
        scope: Scope,
        project_id: Option<String>,
        content: String,
    },
    Update {
        scope: Scope,
        project_id: Option<String>,
        id: String,
        content: String,
    },
    Delete {
        scope: Scope,
        project_id: Option<String>,
        id: String,
    },
}
#[derive(Serialize)]
pub struct Snapshot {
    pub user: Vec<repo::Preference>,
    pub projects: Vec<ProjectItems>,
}
#[derive(Serialize)]
pub struct ProjectItems {
    pub project_id: String,
    pub items: Vec<repo::Preference>,
}
impl Snapshot {
    pub fn read_conn(
        conn: &Connection,
        projects: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        let mut ids: Vec<_> = projects.into_iter().collect();
        ids.sort();
        ids.dedup();
        Ok(Self {
            user: repo::list(conn, None)?,
            projects: ids
                .into_iter()
                .map(|id| {
                    Ok(ProjectItems {
                        items: repo::list(conn, Some(&id))?,
                        project_id: id,
                    })
                })
                .collect::<Result<_>>()?,
        })
    }
}
pub struct Pending {
    projects: HashSet<String>,
    changes: Result<Vec<Change>>,
}
impl Pending {
    pub fn read(projects: Vec<String>, output: Result<String>, reply: PreferenceReply) -> Self {
        let changes = (|| match reply {
            PreferenceReply::Completed {} => Ok(serde_json::from_str::<Output>(&output?)?.changes),
            PreferenceReply::Failed { error } => Err(other(if error.trim().is_empty() {
                "偏好提取失败：未提供原因".into()
            } else {
                format!("偏好提取失败：{error}")
            })),
        })();
        Self {
            projects: projects.into_iter().collect(),
            changes,
        }
    }
    // A savepoint rolls back every preference change on validation failure while
    // the caller's valid summary transaction remains committable.
    pub fn apply(&self, conn: &Connection) -> Result<Option<String>> {
        let changes = match &self.changes {
            Ok(changes) => changes,
            Err(e) => return Ok(Some(e.to_string())),
        };
        if changes.is_empty() {
            return Ok(None);
        }
        let result = (|| -> Result<()> {
            conn.execute_batch("SAVEPOINT preference_changes")?;
            let mut seen = HashSet::new();
            for change in changes {
                let (scope, project, id) = match change {
                    Change::Add {
                        scope, project_id, ..
                    } => (scope, project_id, None),
                    Change::Update {
                        scope,
                        project_id,
                        id,
                        ..
                    }
                    | Change::Delete {
                        scope,
                        project_id,
                        id,
                    } => (scope, project_id, Some(id)),
                };
                match scope {
                    Scope::User if project.is_some() => return Err(other("用户偏好不能指定项目")),
                    Scope::Project
                        if !project
                            .as_ref()
                            .is_some_and(|id| self.projects.contains(id)) =>
                    {
                        return Err(other("项目不在本次提取范围内"))
                    }
                    _ => {}
                }
                if let Some(id) = id {
                    if !seen.insert(id) {
                        return Err(other("同一条目不能重复变更"));
                    }
                    let current = repo::get(conn, id)?.ok_or_else(|| other("偏好不存在"))?;
                    if current.project_id != *project {
                        return Err(other("偏好所属范围不匹配"));
                    }
                    if current.locked {
                        return Err(other("Agent 不能修改或删除锁定偏好"));
                    }
                }
                match change {
                    Change::Add {
                        project_id,
                        content,
                        ..
                    } => {
                        repo::insert(conn, project_id.as_deref(), content, false)?;
                    }
                    Change::Update { id, content, .. } => {
                        repo::update(conn, id, content, false)?;
                    }
                    Change::Delete { id, .. } => {
                        conn.execute("DELETE FROM preferences WHERE id=?1", [id])?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            conn.execute_batch("ROLLBACK TO preference_changes; RELEASE preference_changes")?;
            return Ok(Some(e.to_string()));
        }
        conn.execute_batch("RELEASE preference_changes")?;
        Ok(None)
    }
}
