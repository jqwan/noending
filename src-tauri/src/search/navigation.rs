//! Object lookup for navigation. Intentionally independent of content search:
//! no message text, descriptions, summaries, source metadata or CLI detection.
use super::objects::{predicate, ObjectKind, ObjectQuery};
use crate::{domain::Agent, error::Result, storage::Db};
use rusqlite::params;
use serde::Serialize;

const RESULT_LIMIT: usize = 20;

#[derive(Debug, Serialize)]
pub struct NavigationItem {
    pub kind: ObjectKind,
    pub ref_id: String,
    pub title: String,
    pub hint: String,
    pub agent: Option<Agent>,
    pub archived: bool,
}

#[derive(Debug, Serialize)]
pub struct NavigationResults {
    pub items: Vec<NavigationItem>,
    pub has_more: bool,
}

struct RankedItem {
    item: NavigationItem,
    rank: i64,
    activity: String,
}

pub fn lookup(db: &Db, query: &str) -> Result<NavigationResults> {
    lookup_with_context(db, query, None, None)
}

pub fn lookup_with_context(
    db: &Db,
    query: &str,
    roots: Option<&crate::skills::Roots>,
    managed_root: Option<&std::path::Path>,
) -> Result<NavigationResults> {
    let query = query.trim();
    let parsed = ObjectQuery::new(query);
    let sql = if query.is_empty() {
        // Default suggestions are deliberately a preview, not an unbounded
        // preload of projects/tasks/sessions. All Agents are added below.
        "SELECT 'session',id,COALESCE(NULLIF(TRIM(title),''),'未命名会话'),
                COALESCE(cwd,''),agent,archived_at IS NOT NULL,
                COALESCE(last_conversation_at,last_activity_at,started_at,''),0
         FROM sessions
         ORDER BY COALESCE(last_conversation_at,last_activity_at,started_at) DESC,id LIMIT 8"
            .to_string()
    } else {
        format!("SELECT 'project' AS kind,p.id,p.name AS title,
                COALESCE((SELECT canonical_path FROM workspace_paths wp WHERE wp.project_id=p.id
                    ORDER BY (canonical_path LIKE ?1 ESCAPE '!') DESC,canonical_path LIMIT 1),'') AS hint,
                NULL AS agent,p.visibility='archived' AS archived,p.updated_at AS activity,
                CASE WHEN p.name LIKE ?2 ESCAPE '!' THEN 0 WHEN p.name LIKE ?3 ESCAPE '!' THEN 1 ELSE 2 END AS relevance
         FROM projects p WHERE {}
         UNION ALL
         SELECT 'workstream',w.id,w.title,'',NULL,w.visibility='archived',w.updated_at,
                CASE WHEN w.title LIKE ?2 ESCAPE '!' THEN 0 WHEN w.title LIKE ?3 ESCAPE '!' THEN 1 ELSE 2 END
         FROM workstreams w WHERE {}
         UNION ALL
         SELECT 'session',s.id,COALESCE(NULLIF(TRIM(s.title),''),'未命名会话'),COALESCE(s.cwd,''),s.agent,
                s.archived_at IS NOT NULL,COALESCE(s.last_conversation_at,s.last_activity_at,s.started_at,''),
                CASE WHEN s.title LIKE ?2 ESCAPE '!' THEN 0 WHEN s.title LIKE ?3 ESCAPE '!' THEN 1 ELSE 2 END
         FROM sessions s WHERE {}
         ORDER BY relevance,activity DESC,kind,id LIMIT ?4",
         predicate(ObjectKind::Project),predicate(ObjectKind::Workstream),predicate(ObjectKind::Session))
    };
    let mut candidates = {
        let conn = db.read();
        let mut statement = conn.prepare(&sql)?;
        let read = |r: &rusqlite::Row<'_>| {
            let kind = match r.get::<_, String>(0)?.as_str() {
                "project" => ObjectKind::Project,
                "workstream" => ObjectKind::Workstream,
                _ => ObjectKind::Session,
            };
            Ok(RankedItem {
                item: NavigationItem {
                    kind,
                    ref_id: r.get(1)?,
                    title: r.get(2)?,
                    hint: r.get(3)?,
                    agent: r
                        .get::<_, Option<String>>(4)?
                        .and_then(|s| Agent::parse(&s)),
                    archived: r.get(5)?,
                },
                activity: r.get(6)?,
                rank: r.get(7)?,
            })
        };
        let rows = if query.is_empty() {
            statement.query_map([], read)?
        } else {
            statement.query_map(
                params![
                    parsed.pattern,
                    parsed.exact,
                    parsed.prefix,
                    (RESULT_LIMIT + 1) as i64
                ],
                read,
            )?
        };
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for agent in Agent::all() {
        if let Some(rank) = parsed.agent_rank(*agent) {
            candidates.push(RankedItem {
                item: NavigationItem {
                    kind: ObjectKind::Agent,
                    ref_id: agent.as_str().to_string(),
                    title: agent.display_name().to_string(),
                    hint: "Agent 详情".into(),
                    agent: Some(*agent),
                    archived: false,
                },
                rank,
                activity: String::new(),
            });
        }
    }
    if !query.is_empty() {
        let skill_items: Vec<(String, String, String)> = if let Some(roots) = roots {
            let cwds = db
                .list_workspace_paths()?
                .into_iter()
                .map(|p| std::path::PathBuf::from(p.canonical_path))
                .filter(|p| p.is_dir())
                .collect::<Vec<_>>();
            let registry = crate::skills::load_skill_registry(db)?;
            let inventory = crate::skills::catalog(
                roots,
                &cwds,
                &registry,
                managed_root.unwrap_or_else(|| std::path::Path::new("")),
            );
            inventory
                .resources
                .into_iter()
                .map(|r| (r.id, r.name, r.description))
                .collect()
        } else {
            let registry = crate::skills::load_skill_registry(db)?;
            registry
                .sources
                .into_iter()
                .map(|s| (s.id, s.name, s.description))
                .collect()
        };

        for (id, name, description) in skill_items {
            if let Some(rank) = parsed.skill_rank(&name, &id, &description) {
                candidates.push(RankedItem {
                    item: NavigationItem {
                        kind: ObjectKind::Skill,
                        ref_id: id.clone(),
                        title: if name.is_empty() { id } else { name },
                        hint: if !description.is_empty() {
                            description.lines().next().unwrap_or("").trim().to_string()
                        } else {
                            "技能".into()
                        },
                        agent: None,
                        archived: false,
                    },
                    rank,
                    activity: String::new(),
                });
            }
        }
    }
    candidates.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| b.activity.cmp(&a.activity))
            .then_with(|| a.item.kind.cmp(&b.item.kind))
            .then_with(|| a.item.ref_id.cmp(&b.item.ref_id))
    });
    let has_more = candidates.len() > RESULT_LIMIT;
    candidates.truncate(RESULT_LIMIT);
    Ok(NavigationResults {
        items: candidates.into_iter().map(|c| c.item).collect(),
        has_more,
    })
}
