//! Session Launcher — New / Resume flows (技术实现方案 §23/§24).
//!
//! Flow: sync stale sessions → build bundle → write context file →
//! adapter builds command → PlatformLauncher executes → bindings recorded.

use std::path::PathBuf;

use serde::Serialize;

use crate::adapters::AgentCommand;
use crate::domain::{Agent, Session, SessionWorkstreamBinding};
use crate::error::{other, Result};
use crate::storage::{now, Db};

#[derive(Debug, Clone, Serialize)]
pub struct LaunchResult {
    pub launched_via: String,
    pub command_line: String,
    pub context_file: String,
    pub bundle: crate::context::SessionContextBundle,
    pub note: String,
}

pub struct SessionLauncher {
    pub app_data_dir: PathBuf,
}

impl SessionLauncher {
    fn write_context_file(&self, name_hint: &str, markdown: &str) -> Result<PathBuf> {
        let dir = self.app_data_dir.join("context-bundles");
        std::fs::create_dir_all(&dir)?;
        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let safe: String = name_hint
            .chars()
            .take(40)
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let path = dir.join(format!("{}-{}.md", ts, safe));
        std::fs::write(&path, markdown)?;
        Ok(path)
    }

    /// Sync Point: before New Session, sync stale sessions of the workstreams.
    pub fn sync_stale_for_workstreams(&self, db: &Db, workstream_ids: &[String]) -> Result<usize> {
        let engine = crate::sync::SyncEngine::from_settings(db);
        let mut synced = 0;
        for ws_id in workstream_ids {
            let stale = crate::ingestion::stale_sessions(db, ws_id)?;
            for s in stale {
                let n = sync_one_session_with_engine(db, &engine, &s)?;
                synced += n;
            }
        }
        Ok(synced)
    }

    pub fn new_session(
        &self,
        db: &Db,
        agent: Agent,
        workstream_ids: &[String],
        cwd: Option<&str>,
    ) -> Result<LaunchResult> {
        // 1. sync stale sessions that share these workstreams (no-op when empty)
        self.sync_stale_for_workstreams(db, workstream_ids)?;

        // 2. build context bundle; zero contexts → plain launch, no injection.
        //    (UX rule: 关联 Workstream 永远是可选项。)
        let bundle = crate::context::build_bundle(db, "new", None, workstream_ids, 4000)?;
        let ctx_file = if workstream_ids.is_empty() {
            None
        } else {
            let title_hint = workstream_ids
                .first()
                .and_then(|id| db.get_workstream(id).ok().flatten())
                .map(|w| w.title)
                .unwrap_or_else(|| "bundle".into());
            Some(self.write_context_file(&title_hint, &bundle.markdown)?)
        };

        // 3. resolve agent CLI
        let install = match db.get_installation(agent)? {
            Some(i) if std::path::Path::new(&i.executable_path).exists() => i,
            _ => crate::platform::exec_resolver::resolve(agent)
                .map(|i| {
                    let _ = db.save_installation(&i);
                    i
                })?,
        };
        let adapter = crate::adapters::adapter_for(agent);
        let cwd_path = cwd.map(PathBuf::from);

        // 4. adapter builds command, platform launches it
        let cmd: AgentCommand = adapter.build_new_command(&install, ctx_file.as_deref(), cwd_path.as_deref())?;
        let outcome = crate::platform::launcher::launch(&cmd)?;

        // 5. bindings will be created on first discovery of the new session;
        //    a context-less session gets associated later via Sync.
        Ok(LaunchResult {
            launched_via: outcome.launched_via,
            command_line: outcome.command_line,
            context_file: ctx_file
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            bundle,
            note: if ctx_file.is_some() {
                "新 Session 启动后会在下次同步时自动发现并建立 Workstream 绑定。".into()
            } else {
                "已直接启动（未携带 Workstream Context）；同步后会自动尝试关联 Workstream。".into()
            },
        })
    }

    pub fn resume_session(
        &self,
        db: &Db,
        session_id: &str,
        extra_workstream_ids: &[String],
    ) -> Result<LaunchResult> {
        let session = db
            .get_session(session_id)?
            .ok_or_else(|| other("Session 不存在"))?;

        // 1. sync this session's own new messages first
        let engine = crate::sync::SyncEngine::from_settings(db);
        let adapter = crate::adapters::adapter_for(session.agent);
        sync_one_session_with_engine(db, &engine, &session)?;

        // 2. resolve workstreams: existing bindings + user-added extras.
        //    An empty set is valid: resume natively without context injection.
        let mut ws_ids: Vec<String> = db
            .bindings_for_session(session_id)?
            .into_iter()
            .map(|b| b.workstream_id)
            .collect();
        for extra in extra_workstream_ids {
            if !ws_ids.contains(extra) {
                ws_ids.push(extra.clone());
                self.record_binding(db, session_id, extra, "related")?;
            }
        }

        // 3. delta bundle (empty → no injection)
        let bundle = crate::context::build_bundle(db, "resume", Some(&session), &ws_ids, 3000)?;
        let ctx_file = if ws_ids.is_empty() {
            None
        } else {
            Some(self.write_context_file("resume", &bundle.markdown)?)
        };

        // 4. launch
        let install = match db.get_installation(session.agent)? {
            Some(i) if std::path::Path::new(&i.executable_path).exists() => i,
            _ => crate::platform::exec_resolver::resolve(session.agent)
                .map(|i| {
                    let _ = db.save_installation(&i);
                    i
                })?,
        };
        let cwd_path = session.cwd.clone().map(PathBuf::from);
        let cmd = adapter.build_resume_command(
            &install,
            &session.agent_session_id,
            ctx_file.as_deref(),
            cwd_path.as_deref(),
        )?;
        let outcome = crate::platform::launcher::launch(&cmd)?;

        // touch binding usage
        for ws_id in &ws_ids {
            self.record_binding(db, session_id, ws_id, "related")?;
        }
        Ok(LaunchResult {
            launched_via: outcome.launched_via,
            command_line: outcome.command_line,
            context_file: ctx_file
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            bundle,
            note: if ctx_file.is_some() {
                "已同步最新消息并生成增量上下文。".into()
            } else {
                "该 Session 未关联 Workstream：已同步自身消息后直接恢复，未注入上下文。".into()
            },
        })
    }

    pub fn record_binding(&self, db: &Db, session_id: &str, workstream_id: &str, role: &str) -> Result<()> {
        let existing = db.bindings_for_session(session_id)?;
        if let Some(b) = existing.iter().find(|b| b.workstream_id == workstream_id) {
            let mut b = b.clone();
            b.last_used_at = now();
            db.bind(&b)?;
            return Ok(());
        }
        let b = SessionWorkstreamBinding {
            session_id: session_id.to_string(),
            workstream_id: workstream_id.to_string(),
            role: role.to_string(),
            last_seen_revision: None,
            last_sync_cursor: 0,
            created_at: now(),
            last_used_at: now(),
        };
        db.bind(&b)?;
        Ok(())
    }
}

/// Single-path ingest+sync: reads delta from the stored cursor, persists
/// events, runs the sync engine, advances the cursor on success.
/// Returns (events_ingested, mutations_applied).
pub fn ingest_and_sync_session(
    db: &Db,
    engine: &crate::sync::SyncEngine,
    session: &Session,
) -> Result<(i64, usize)> {
    let adapter = crate::adapters::adapter_for(session.agent);
    let from = db.get_cursor(&session.id)?;
    let delta = adapter.read_delta(session, from)?;
    if delta.events.is_empty() {
        db.set_cursor(&session.id, delta.last_sequence, delta.file_size)?;
        return Ok((0, 0));
    }
    db.append_events(&delta.events)?;
    if let Err(e) = db.index_events(&delta.events) {
        eprintln!("[sync] index_events failed: {}", e);
    }
    let out = engine.run_session_sync(db, session, &delta.events, from, delta.last_sequence)?;
    Ok((delta.events.len() as i64, out.applied))
}

/// Ingest + sync a single session delta. Returns number of applied mutations.
pub fn sync_one_session(db: &Db, engine: &crate::sync::SyncEngine, session: &Session) -> Result<usize> {
    sync_one_session_with_engine(db, engine, session)
}

pub fn sync_one_session_with_engine(
    db: &Db,
    engine: &crate::sync::SyncEngine,
    session: &Session,
) -> Result<usize> {
    Ok(ingest_and_sync_session(db, engine, session)?.1)
}
