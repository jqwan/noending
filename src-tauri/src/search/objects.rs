//! Shared object identity matching for boards and navigation.
use crate::{domain::Agent, error::Result, storage::Db};
use rusqlite::{params_from_iter, types::Value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Agent,
    Project,
    Session,
    Workstream,
    Skill,
}

pub(super) struct ObjectQuery {
    pub pattern: String,
    pub exact: String,
    pub prefix: String,
    lower: String,
}
impl ObjectQuery {
    pub fn new(query: &str) -> Self {
        let query = query.trim();
        let exact = query
            .replace('!', "!!")
            .replace('%', "!%")
            .replace('_', "!_");
        Self {
            pattern: format!("%{exact}%"),
            prefix: format!("{exact}%"),
            exact,
            lower: query.to_lowercase(),
        }
    }
    pub fn agent_rank(&self, agent: Agent) -> Option<i64> {
        let label = agent.display_name().to_lowercase();
        let id = agent.as_str();
        if self.lower.is_empty() || label == self.lower || Agent::parse(&self.lower) == Some(agent)
        {
            Some(0)
        } else if label.starts_with(&self.lower) || id.starts_with(&self.lower) {
            Some(1)
        } else if label.contains(&self.lower) || id.contains(&self.lower) {
            Some(2)
        } else {
            None
        }
    }
    pub fn skill_rank(&self, name: &str, id: &str, description: &str) -> Option<i64> {
        if self.lower.is_empty() {
            return None;
        }
        let name_lower = name.to_lowercase();
        let id_lower = id.to_lowercase();
        if name_lower == self.lower || id_lower == self.lower {
            Some(0)
        } else if name_lower.starts_with(&self.lower) || id_lower.starts_with(&self.lower) {
            Some(1)
        } else if name_lower.contains(&self.lower)
            || id_lower.contains(&self.lower)
            || description.to_lowercase().contains(&self.lower)
        {
            Some(2)
        } else {
            None
        }
    }
}

/// Aliases are fixed by the query builders: p=projects, w=workstreams,
/// s=sessions. Exact ID lookup is intentional; IDs are never substring searched.
pub(super) fn predicate(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Project=>"(p.name LIKE ?1 ESCAPE '!' OR EXISTS(SELECT 1 FROM workspace_paths wp WHERE wp.project_id=p.id AND wp.canonical_path LIKE ?1 ESCAPE '!'))",
        ObjectKind::Workstream=>"w.title LIKE ?1 ESCAPE '!'",
        ObjectKind::Session=>"(s.title LIKE ?1 ESCAPE '!' OR s.cwd LIKE ?1 ESCAPE '!' OR s.id LIKE ?2 ESCAPE '!' OR s.root_agent_session_id LIKE ?2 ESCAPE '!')",
        ObjectKind::Agent=>unreachable!("Agent names come from the roster"),
        ObjectKind::Skill=>unreachable!("Skill names come from the catalog"),
    }
}

/// Complete match identities, without a preview limit or entity payloads.
/// The board supplies its complete filtered candidate set. The primary-key
/// constraint applies before text matching, so excluded objects are neither
/// searched nor transferred back to the UI. An empty set produces no matches.
pub fn matching_ids(
    db: &Db,
    kind: ObjectKind,
    query: &str,
    candidate_ids: &[String],
) -> Result<Vec<String>> {
    if candidate_ids.is_empty() {
        return Ok(Vec::new());
    }
    let parsed = ObjectQuery::new(query);
    if kind == ObjectKind::Agent {
        return Ok(Agent::all()
            .iter()
            .filter(|a| {
                candidate_ids.iter().any(|id| id == a.as_str()) && parsed.agent_rank(**a).is_some()
            })
            .map(|a| a.as_str().to_string())
            .collect());
    }
    if kind == ObjectKind::Skill {
        return Ok(candidate_ids
            .iter()
            .filter(|id| {
                let id_lower = id.to_lowercase();
                parsed.lower.is_empty() || id_lower.contains(&parsed.lower)
            })
            .cloned()
            .collect());
    }
    let (table, alias) = match kind {
        ObjectKind::Project => ("projects", "p"),
        ObjectKind::Workstream => ("workstreams", "w"),
        ObjectKind::Session => ("sessions", "s"),
        ObjectKind::Agent | ObjectKind::Skill => unreachable!(),
    };
    let empty = query.trim().is_empty();
    let mut values = Vec::<Value>::new();
    if !empty {
        values.push(parsed.pattern.into());
        if kind == ObjectKind::Session {
            values.push(parsed.exact.into());
        }
    }
    let mut sql = format!(
        "SELECT {alias}.id FROM {table} {alias} WHERE {}",
        if empty { "1" } else { predicate(kind) }
    );
    values.push(serde_json::to_string(candidate_ids)?.into());
    sql.push_str(&format!(
        " AND {alias}.id IN (SELECT value FROM json_each(?{}))",
        values.len()
    ));
    sql.push_str(&format!(" ORDER BY {alias}.id"));
    let conn = db.read();
    let mut statement = conn.prepare(&sql)?;
    let read = |r: &rusqlite::Row<'_>| r.get::<_, String>(0);
    let rows = statement.query_map(params_from_iter(values), read)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}
