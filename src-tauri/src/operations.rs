//! Ephemeral operation presence, shared by desktop, CLI and MCP for one database.
//! OS locks own liveness: a crashed process never leaves a permanent busy flag.
use crate::{
    domain::Agent,
    error::{other, Result},
    storage::Db,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Target {
    pub kind: String,
    pub id: String,
}
impl Target {
    pub fn new(kind: &str, id: &str) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Sync,
    Summary,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Running,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationStatus {
    pub id: String,
    pub kind: Kind,
    pub state: State,
    pub targets: Vec<Target>,
    pub agent: Option<Agent>,
}
impl OperationStatus {
    pub fn sync(kind: &str, id: &str, agent: Option<Agent>) -> Self {
        Self {
            id: String::new(),
            kind: Kind::Sync,
            state: State::Running,
            targets: vec![Target::new(kind, id)],
            agent,
        }
    }
    pub fn source(db: &Db, id: &str, agent: Option<Agent>) -> Result<Self> {
        let mut status = Self::sync("source", id, agent);
        if let Some(source) = db.get_ingest_source(id)? {
            let roots = crate::adapters::source_discovery_roots(&source);
            let conn = db.read();
            let mut statement =
                conn.prepare("SELECT id,source_path FROM sessions WHERE agent=?1")?;
            let rows = statement.query_map([source.agent.as_str()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (session, path) = row?;
                if roots
                    .iter()
                    .any(|root| crate::workspace::is_within(&path, &root.to_string_lossy()))
                {
                    status.targets.push(Target::new("session", &session));
                }
            }
        }
        Ok(status)
    }
    pub fn native(db: &Db, agent: Agent, native_id: &str) -> Result<Self> {
        Ok(match db.find_session_by_root_agent_id(agent, native_id)? {
            Some(session) => Self::sync("session", &session.id, Some(agent)),
            None => Self::sync("native", native_id, Some(agent)),
        })
    }
    pub fn summary(kind: &str, id: &str) -> Self {
        Self {
            id: String::new(),
            kind: Kind::Summary,
            state: State::Running,
            targets: vec![Target::new(kind, id)],
            agent: None,
        }
    }
}
fn directory(db: &Db) -> Result<PathBuf> {
    let conn = db.read();
    let path = conn.path().ok_or_else(|| other("运行状态需要文件数据库"))?;
    Ok(fs::canonicalize(path)?.with_extension("operations"))
}
pub struct Operation {
    status: OperationStatus,
    directory: PathBuf,
    owner: Option<File>,
}
impl Operation {
    pub fn start(db: &Db, mut status: OperationStatus) -> Result<Self> {
        let directory = directory(db)?;
        crate::cli::transport::private_dir(&directory).map_err(|e| other(e.to_string()))?;
        status.id = crate::storage::new_id();
        let owner =
            crate::platform::file_lock::exclusive(&directory.join(format!("{}.lock", status.id)))?;
        let result = Self {
            status,
            directory,
            owner: Some(owner),
        };
        result.publish()?;
        Ok(result)
    }
    pub fn add_sessions(&mut self, ids: impl IntoIterator<Item = String>) -> Result<()> {
        for id in ids {
            let target = Target::new("session", &id);
            if !self.status.targets.contains(&target) {
                self.status.targets.push(target);
            }
        }
        self.publish()
    }
    fn publish(&self) -> Result<()> {
        let temp = self.directory.join(format!("{}.tmp", self.status.id));
        let output = self.directory.join(format!("{}.json", self.status.id));
        crate::cli::transport::private_write(&temp, &serde_json::to_string(&self.status)?)
            .map_err(|e| other(e.to_string()))?;
        fs::rename(temp, output)?;
        Ok(())
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        drop(self.owner.take());
        cleanup(&self.directory, &self.status.id);
    }
}
fn cleanup(directory: &Path, id: &str) {
    for extension in ["json", "tmp", "lock"] {
        let _ = fs::remove_file(directory.join(format!("{id}.{extension}")));
    }
}
pub fn list(db: &Db) -> Result<Vec<OperationStatus>> {
    let root = directory(db)?;
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut result: Vec<OperationStatus> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if uuid::Uuid::parse_str(id).is_err() {
            continue;
        }
        match crate::platform::file_lock::exclusive(&root.join(format!("{id}.lock"))) {
            Ok(probe) => {
                drop(probe);
                cleanup(&root, id);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => match fs::read(&path) {
                Ok(bytes) => {
                    let mut status: OperationStatus = serde_json::from_slice(&bytes)?;
                    // Discovery may add/rebind a session after the operation began.
                    if let Some(target) = status.targets.first() {
                        if target.kind == "source" {
                            status.targets =
                                OperationStatus::source(db, &target.id, status.agent)?.targets;
                        } else if target.kind == "native" {
                            if let Some(agent) = status.agent {
                                status.targets =
                                    OperationStatus::native(db, agent, &target.id)?.targets;
                            }
                        }
                    }
                    result.push(status);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}
