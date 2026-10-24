//! Thread identity and immutable rollout versions are separate. A history base
//! names a rollout and an exclusive native-ordinal prefix, not a forked thread.
use super::*;
use crate::adapters::{file_identity, mtime_secs, StableFile};
use crate::error::other;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct HistoryBase {
    pub thread_id: String,
    pub end_ordinal_exclusive: u64,
    pub end_byte_offset: Option<u64>,
}
#[derive(Clone)]
pub(super) struct Rollout {
    pub path: PathBuf,
    pub thread_id: String,
    pub rollout_id: String,
    pub base: Option<HistoryBase>,
    pub started_at: Option<String>,
    pub metadata: Value,
}

pub(super) fn filename_ids(path: &Path) -> Option<(String, String)> {
    let stem = rollout_stem(path)?.strip_prefix("rollout-")?;
    let ids = stem.get(20..)?;
    let (thread, rollout) = ids.split_once('_').unwrap_or((ids, ids));
    uuid::Uuid::parse_str(thread).ok()?;
    uuid::Uuid::parse_str(rollout).ok()?;
    Some((thread.into(), rollout.into()))
}
pub(super) fn same_path(a: &Path, b: &Path) -> bool {
    a == b || matches!((std::fs::canonicalize(a),std::fs::canonicalize(b)),(Ok(a),Ok(b)) if a == b)
}
fn header(path: &Path) -> Result<Option<Value>> {
    let file = std::fs::File::open(path)?;
    let input: Box<dyn Read> = if is_compressed(path) {
        Box::new(zstd::stream::read::Decoder::new(file)?)
    } else {
        Box::new(file)
    };
    let mut reader = BufReader::new(input);
    let mut line = String::new();
    for _ in 0..10 {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if crate::adapters::fingerprint_line(&v) == Some(Agent::Codex)
            && v["type"] == "session_meta"
        {
            return Ok(Some(v["payload"].clone()));
        }
    }
    Ok(None)
}
impl Rollout {
    fn open(path: PathBuf) -> Result<Option<Self>> {
        let Some(meta) = header(&path)? else {
            return Ok(None);
        };
        let names = filename_ids(&path);
        let thread_id = meta["id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| names.as_ref().map(|v| v.0.clone()))
            .or_else(|| meta["session_id"].as_str().map(str::to_owned))
            .ok_or_else(|| other("Codex 来源缺少线程身份"))?;
        let rollout_id = names.map(|v| v.1).unwrap_or_else(|| thread_id.clone());
        let base = meta
            .get("history_base")
            .filter(|v| !v.is_null())
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?;
        Ok(Some(Self {
            path,
            thread_id,
            rollout_id,
            base,
            started_at: meta["timestamp"].as_str().map(str::to_owned),
            metadata: Value::Object(
                meta.as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(key, _)| {
                        matches!(
                            key.as_str(),
                            "id" | "session_id"
                                | "cwd"
                                | "timestamp"
                                | "thread_source"
                                | "parent_thread_id"
                                | "forked_from_id"
                                | "history_base"
                        )
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        }))
    }
}

/// Search only the Agent's transcript roots. Explicit/custom transcript roots
/// use their own directory; standard date subdirectories share the Agent root.
pub(super) fn roots_for(path: &Path) -> Vec<PathBuf> {
    for parent in path.ancestors() {
        if parent
            .file_name()
            .is_some_and(|n| n == "sessions" || n == "archived_sessions")
        {
            if let Some(root) = parent.parent() {
                return vec![root.join("sessions"), root.join("archived_sessions")];
            }
        }
    }
    vec![path.parent().unwrap_or(path).to_path_buf()]
}
fn data_roots(roots: &[PathBuf]) -> HashSet<PathBuf> {
    roots
        .iter()
        .flat_map(|root| {
            let mut result = vec![root.clone()];
            for ancestor in root.ancestors() {
                if ancestor
                    .file_name()
                    .is_some_and(|n| n == "sessions" || n == "archived_sessions")
                {
                    if let Some(parent) = ancestor.parent() {
                        result.push(parent.to_path_buf());
                    }
                    break;
                }
            }
            result
        })
        .collect()
}
/// Read the native index without opening or modifying Codex's database for writes.
/// Missing indexes are supported for exported transcripts and older installations.
pub(super) fn active_paths(roots: &[PathBuf]) -> HashMap<String, PathBuf> {
    let mut result = HashMap::new();
    for root in data_roots(roots) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut databases: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name();
                let version = name
                    .to_str()?
                    .strip_prefix("state_")?
                    .strip_suffix(".sqlite")?
                    .parse::<u64>()
                    .ok()?;
                Some((version, e.path()))
            })
            .collect();
        databases.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in databases {
            let Ok(conn) = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            ) else {
                continue;
            };
            let _ = conn.busy_timeout(std::time::Duration::from_millis(100));
            let Ok(mut stmt) = conn.prepare("SELECT id, rollout_path FROM threads") else {
                continue;
            };
            let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) else {
                continue;
            };
            for row in rows.flatten() {
                result
                    .entry(row.0)
                    .or_insert_with(|| resolve_rollout(Path::new(&row.1)));
            }
            break;
        }
    }
    result
}

pub(super) struct Catalog {
    pub rollouts: Vec<Rollout>,
    active: HashMap<String, PathBuf>,
}
impl Catalog {
    pub fn load(roots: &[PathBuf]) -> Result<Self> {
        let mut rollouts = Vec::new();
        let mut stack = roots.to_vec();
        let mut seen = HashSet::new();
        while let Some(dir) = stack.pop() {
            let canonical = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
            if !seen.insert(canonical) {
                continue;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in entries {
                let path = entry?.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if rollout_stem(&path).is_none() || resolve_rollout(&path) != path {
                    continue;
                }
                if let Some(rollout) = Rollout::open(path)? {
                    rollouts.push(rollout);
                }
            }
        }
        rollouts.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Self {
            rollouts,
            active: active_paths(roots),
        })
    }
    pub fn selected(&self, native: Option<&str>) -> Result<Vec<&Rollout>> {
        let mut groups: HashMap<&str, Vec<&Rollout>> = HashMap::new();
        for r in &self.rollouts {
            if native.is_some_and(|id| id != r.thread_id) {
                continue;
            }
            groups.entry(&r.thread_id).or_default().push(r);
        }
        let mut selected = Vec::new();
        for (id, group) in groups {
            if let Some(path) = self.active.get(id) {
                let current = group
                    .iter()
                    .find(|r| same_path(&r.path, path))
                    .ok_or_else(|| {
                        other(format!(
                            "Codex 当前转录不可读取或身份不符：{}",
                            path.display()
                        ))
                    })?;
                selected.push(*current);
                continue;
            }
            let referenced: HashSet<_> = group
                .iter()
                .filter_map(|r| r.base.as_ref().map(|b| b.thread_id.as_str()))
                .collect();
            let heads: Vec<_> = group
                .into_iter()
                .filter(|r| !referenced.contains(r.rollout_id.as_str()))
                .collect();
            if heads.len() != 1 {
                return Err(other(format!(
                    "Codex 线程 {id} 有多个或循环的转录版本，缺少有效当前版本索引"
                )));
            }
            selected.push(heads[0]);
        }
        selected.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
        Ok(selected)
    }
    fn referenced(&self, id: &str) -> Result<&Rollout> {
        let found: Vec<_> = self
            .rollouts
            .iter()
            .filter(|r| r.rollout_id == id)
            .collect();
        if found.len() != 1 {
            return Err(other(format!("Codex 历史引用 {id} 缺失或不唯一")));
        }
        Ok(found[0])
    }
    fn dependencies(&self, head: &Rollout) -> Result<Vec<(&Rollout, HistoryBase)>> {
        let mut result = Vec::new();
        let mut visited = HashSet::from([head.rollout_id.clone()]);
        let mut next = head.base.clone();
        while let Some(base) = next {
            if result.len() >= 128 || !visited.insert(base.thread_id.clone()) {
                return Err(other("Codex 历史引用存在循环或过深"));
            }
            let parent = self.referenced(&base.thread_id)?;
            next = parent.base.clone();
            result.push((parent, base));
        }
        Ok(result)
    }
    pub fn identity(&self, head: &Rollout) -> Result<String> {
        if head.base.is_none() {
            return Ok(file_identity(&head.path));
        }
        let mut hash = Sha256::new();
        hash.update(head.path.to_string_lossy().as_bytes());
        hash.update(file_identity(&head.path));
        for (r, base) in self.dependencies(head)? {
            let meta = std::fs::metadata(&r.path)?;
            hash.update(serde_json::to_vec(&(
                r.path.clone(),
                file_identity(&r.path),
                meta.len(),
                mtime_secs(&meta),
                base,
            ))?);
        }
        Ok(format!("codex-chain:{:x}", hash.finalize()))
    }
    /// Convert the adapter's compound cursor to the leaf's physical cursor.
    pub fn physical_cursor(&self, head: &Rollout, cursor: &SourceCursor) -> Result<SourceCursor> {
        let mut physical = cursor.clone();
        if cursor.source_file_identity == self.identity(head)? {
            physical.source_file_identity = file_identity(&head.path);
        }
        Ok(physical)
    }
    pub fn prepend(&self, head: &Rollout, delta: &mut MemberReadDelta) -> Result<()> {
        let Some(source) = delta.source.as_mut() else {
            return Ok(());
        };
        if source.start_byte_offset == 0 {
            if let Some(base) = &head.base {
                let mut prefix = self.prefix(
                    self.referenced(&base.thread_id)?,
                    base,
                    &mut HashSet::from([head.rollout_id.clone()]),
                )?;
                prefix.append(&mut delta.messages);
                delta.messages = prefix;
            }
        }
        source.file_identity = self.identity(head)?;
        Ok(())
    }
    fn prefix(
        &self,
        rollout: &Rollout,
        limit: &HistoryBase,
        visited: &mut HashSet<String>,
    ) -> Result<Vec<crate::adapters::ParsedSessionMessage>> {
        if visited.len() >= 128 || !visited.insert(rollout.rollout_id.clone()) {
            return Err(other("Codex 历史引用存在循环或过深"));
        }
        let mut messages = Vec::new();
        if let Some(base) = &rollout.base {
            let inherited = HistoryBase {
                end_ordinal_exclusive: base.end_ordinal_exclusive.min(limit.end_ordinal_exclusive),
                end_byte_offset: (base.end_ordinal_exclusive <= limit.end_ordinal_exclusive)
                    .then_some(base.end_byte_offset)
                    .flatten(),
                ..base.clone()
            };
            messages = self.prefix(self.referenced(&base.thread_id)?, &inherited, visited)?;
        }
        let source = StableFile::open(&rollout.path)?;
        let input: Box<dyn Read + '_> = if is_compressed(&rollout.path) {
            Box::new(zstd::stream::read::Decoder::new(&source.file)?)
        } else {
            Box::new(&source.file)
        };
        let mut reader = BufReader::new(input.take(limit.end_byte_offset.unwrap_or(u64::MAX)));
        let mut line = Vec::new();
        let mut bytes = 0u64;
        let mut index = 0u64;
        let mut boundary = limit.end_ordinal_exclusive == 0
            || rollout
                .base
                .as_ref()
                .is_some_and(|base| limit.end_ordinal_exclusive <= base.end_ordinal_exclusive);
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if !line.ends_with(b"\n") {
                return Err(other("Codex 历史引用在帧中间结束"));
            }
            bytes += line.len() as u64;
            let v: Value = serde_json::from_slice(&line)?;
            let ordinal = v["ordinal"].as_u64().unwrap_or(index);
            index += 1;
            if ordinal >= limit.end_ordinal_exclusive {
                boundary = true;
                break;
            }
            boundary = ordinal + 1 == limit.end_ordinal_exclusive;
            if let Some(mut message) = parse_line(&v, true).and_then(|p| p.message) {
                if message.ts.is_none() {
                    message.ts = v["timestamp"].as_str().map(str::to_owned);
                }
                messages.push(message);
            }
        }
        if !boundary || limit.end_byte_offset.is_some_and(|end| end != bytes) {
            return Err(other(format!(
                "Codex 历史引用边界无效：{}",
                rollout.path.display()
            )));
        }
        source.validate()?;
        Ok(messages)
    }
}
