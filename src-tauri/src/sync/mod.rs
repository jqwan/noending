//! Workspace Assistant Core — Background Sync.
//!
//! A SyncJob reads the event delta after the *processed* cursor, extracts
//! candidate context mutations, classifies to workstreams, and merges —
//! all database writes of one run commit in a single SQLite transaction
//! together with the SyncRun row and the processed-cursor advance. Any
//! failure rolls the whole run back; retries are idempotent via the delta
//! fingerprint.
//!
//! Locking model (non-blocking UI): a run is split into three phases.
//! `prepare` and `commit` each take the DB lock briefly; `extract` — which
//! may run the user's agent CLI for minutes — holds NO lock, so concurrent
//! UI commands interleave freely. `prepare` snapshots everything extraction
//! needs.
//!
//! Policy (docs §13/§26, Issue #3/#4):
//! - read_cursor (events durably ingested) and processed_cursor (events
//!   consumed by a committed SyncRun) are separate;
//! - every mutation passes the unified AuthorityPolicy; user authority is
//!   never silently overridden — disagreement becomes a ContextConflict;
//! - every mutation leaves a full source trail.

pub mod extractor;
pub mod merge;
pub mod policy;

use sha2::{Digest, Sha256};
use serde::Serialize;
use std::sync::{Mutex, MutexGuard};

use crate::domain::{ContextItem, ContextItemRevision, Session, SessionEvent, SyncRun};
use crate::error::{other, Result};
use crate::storage::{now, new_id, Db};

/// A proposed change to a workstream's context, produced by the Assistant.
/// `source_refs` are stable event references ("session-event:<id>"); a
/// mutation may cite several events. Empty = no resolvable source.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ContextMutation {
    Add {
        workstream_id: String,
        item_kind: String,
        title: String,
        content: String,
        source_refs: Vec<String>,
        authority: String,
    },
    Update {
        item_id: String,
        title: String,
        content: String,
        source_refs: Vec<String>,
        authority: String,
    },
    Supersede {
        item_id: String,
        title: String,
        content: String,
        source_refs: Vec<String>,
        authority: String,
    },
    Resolve {
        item_id: String,
        source_refs: Vec<String>,
    },
    CreateWorkstream {
        /// Workstreams may be discovered without a Project home; None is valid.
        project_id: Option<String>,
        title: String,
        reason: String,
    },
    Conflict {
        workstream_id: String,
        item_id: String,
        title: String,
        content: String,
        source_refs: Vec<String>,
        reason: String,
    },
}

/// Result of one extraction run: mutations plus diagnostics about dropped
/// refs / invalid entries (surfaced in the SyncRun summary).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExtractOutput {
    pub mutations: Vec<ContextMutation>,
    pub diagnostics: Vec<String>,
}

impl ExtractOutput {
    pub fn mutations(mutations: Vec<ContextMutation>) -> Self {
        Self { mutations, diagnostics: vec![] }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncJobOutput {
    pub run_id: String,
    pub status: String,
    pub applied: usize,
    pub skipped: usize,
    pub unclassified: usize,
    pub summary: String,
}

/// The extraction seam between raw session deltas and the deterministic
/// merge engine. `inputs` are the prompt-building reads snapshotted while
/// the DB lock was held — extraction itself must not touch the database.
pub trait ContextExtractor: Send + Sync {
    fn name(&self) -> String;

    fn extract(
        &self,
        session: &Session,
        events: &[&SessionEvent],
        candidate_workstream_ids: &[String],
        inputs: &extractor::PromptInputs,
    ) -> Result<ExtractOutput>;
}

/// Everything needed to run one sync run. Produced by `prepare` while the
/// DB lock is held; consumed by `extract` (lock-free) and `commit` (locked).
#[derive(Debug, Clone)]
pub struct PreparedSync {
    pub run_id: String,
    pub fingerprint: String,
    pub source_generation: i64,
    pub from_sequence: i64,
    pub to_sequence: i64,
    pub candidates: Vec<String>,
    pub meaningful: Vec<SessionEvent>,
    pub meaningful_count: usize,
    pub unclassified: usize,
    pub inputs: extractor::PromptInputs,
}

/// Everything the merge engine needs to know about the run it writes for.
#[derive(Debug, Clone)]
pub struct MergeContext {
    pub run_id: String,
    /// extractor runtime name; recorded as `created_by = sync:<runtime>`.
    pub runtime: String,
}

pub struct SyncEngine {
    pub heuristic: extractor::HeuristicExtractor,
    pub merger: merge::MergeEngine,
    /// When configured, tried first; any failure falls back to heuristic
    /// and is reported via SyncRun.runtime = "<llm-name>->heuristic".
    pub llm: Option<extractor::CliExtractor>,
}

impl Default for SyncEngine {
    fn default() -> Self {
        Self {
            heuristic: extractor::HeuristicExtractor,
            merger: merge::MergeEngine,
            llm: None,
        }
    }
}

/// Lock a shared Db handle, tolerating poisoning (a panicked command must
/// not take the whole app down with it).
pub fn lock_db(db_lock: &Mutex<Db>) -> Result<MutexGuard<'_, Db>> {
    db_lock.lock().map_err(|_| other("db lock poisoned"))
}

impl SyncEngine {
    /// Build an engine using the assistant configuration stored in settings.
    /// Falls back to heuristic-only when the configured agent CLI is missing.
    pub fn from_settings(db: &Db) -> Self {
        let cfg = extractor::CliExtractor::from_settings(db);
        match cfg {
            Some(cli) if crate::adapters::adapter_for(cli.agent).detect().is_some() => {
                Self {
                    heuristic: extractor::HeuristicExtractor,
                    merger: merge::MergeEngine,
                    llm: Some(cli),
                }
            }
            _ => Self::default(),
        }
    }

    /// Phase A (DB lock held): idempotency check, candidate classification,
    /// pre-filter, prompt-input snapshot. Returns None when this delta was
    /// already processed by a committed run.
    pub fn prepare(
        &self,
        db: &Db,
        session: &Session,
        events: &[SessionEvent],
        from_sequence: i64,
        to_sequence: i64,
    ) -> Result<Option<PreparedSync>> {
        let run_id = new_id();
        let fingerprint = delta_fingerprint(&session.id, events);
        let source_generation = events
            .iter()
            .map(|e| e.source_generation)
            .max()
            .unwrap_or(0);

        // Idempotent retries: this exact delta already committed once.
        if !events.is_empty() && db.has_completed_run(&session.id, &fingerprint)? {
            return Ok(None);
        }

        // 1. candidate workstreams: bound ones, plus keyword-matched ones
        let mut candidates: Vec<String> = db
            .bindings_for_session(&session.id)?
            .into_iter()
            .map(|b| b.workstream_id)
            .collect();
        if candidates.is_empty() {
            candidates = self.auto_classify(db, session, events);
        }

        // 2. pre-filter: only meaningful message kinds
        let meaningful: Vec<SessionEvent> = events
            .iter()
            .filter(|e| matches!(e.kind.as_str(), "user_message" | "assistant_message"))
            .filter(|e| e.text.as_deref().map(|t| t.len() > 30).unwrap_or(false))
            .cloned()
            .collect();

        let unclassified = if candidates.is_empty() && !meaningful.is_empty() {
            meaningful.len()
        } else {
            0
        };

        // 3. snapshot everything extraction needs while the lock is held
        let inputs = if self.llm.is_some() {
            extractor::collect_prompt_inputs(db, &candidates)?
        } else {
            extractor::PromptInputs::default()
        };

        Ok(Some(PreparedSync {
            run_id,
            fingerprint,
            source_generation,
            from_sequence,
            to_sequence,
            candidates,
            meaningful_count: meaningful.len(),
            meaningful,
            unclassified,
            inputs,
        }))
    }

    /// Phase B (NO DB lock): extraction. The heuristic is instant; the CLI
    /// extractor may run the user's agent CLI for minutes. An extractor
    /// failure falls back to the heuristic; a heuristic failure surfaces so
    /// the commit phase (and processed cursor) is skipped for retry.
    pub fn extract(
        &self,
        session: &Session,
        pre: &PreparedSync,
    ) -> Result<(Vec<ContextMutation>, String, Vec<String>)> {
        if pre.meaningful.is_empty() || pre.candidates.is_empty() {
            return Ok((Vec::new(), "none".to_string(), Vec::new()));
        }
        let refs: Vec<&SessionEvent> = pre.meaningful.iter().collect();
        if let Some(cli) = &self.llm {
            match cli.extract(session, &refs, &pre.candidates, &pre.inputs) {
                Ok(o) => Ok((o.mutations, cli.name(), o.diagnostics)),
                Err(e) => {
                    eprintln!("[sync] cli extractor failed, falling back: {}", e);
                    let o = self.heuristic.extract(session, &refs, &pre.candidates, &pre.inputs)?;
                    Ok((o.mutations, format!("{}->heuristic", cli.name()), o.diagnostics))
                }
            }
        } else {
            let o = self.heuristic.extract(session, &refs, &pre.candidates, &pre.inputs)?;
            Ok((o.mutations, "heuristic".to_string(), o.diagnostics))
        }
    }

    /// Phase C (DB lock held): apply mutations, record the SyncRun and
    /// advance the processed cursor — all inside ONE transaction. Any error
    /// rolls the entire run back and the batch is retried later.
    pub fn commit(
        &self,
        db: &Db,
        session: &Session,
        pre: &PreparedSync,
        mutations: Vec<ContextMutation>,
        runtime: &str,
        diagnostics: Vec<String>,
    ) -> Result<SyncJobOutput> {
        let ctx = MergeContext {
            run_id: pre.run_id.clone(),
            runtime: runtime.to_string(),
        };
        let (applied, skipped, summary) = db.tx(|tx| {
            let mut applied = 0usize;
            let mut skipped = 0usize;
            for m in &mutations {
                if self.merger.apply(tx, m, &ctx)? {
                    applied += 1;
                } else {
                    skipped += 1;
                }
            }

            let mut summary = format!(
                "同步 {} 条新增消息：新增/更新 {} 项，跳过 {} 项{}",
                pre.meaningful_count,
                applied,
                skipped,
                if pre.unclassified > 0 {
                    format!("；{} 条消息暂未能归类到 Workstream", pre.unclassified)
                } else {
                    String::new()
                }
            );
            if !diagnostics.is_empty() {
                summary.push_str(&format!("；诊断: {}", diagnostics.join("；")));
            }

            let run = SyncRun {
                id: pre.run_id.clone(),
                session_id: session.id.clone(),
                from_sequence: pre.from_sequence,
                to_sequence: pre.to_sequence,
                status: "ok".into(),
                mutations: serde_json::to_value(&mutations)?,
                summary: summary.clone(),
                error: None,
                created_at: now(),
                runtime: runtime.to_string(),
                delta_fingerprint: Some(pre.fingerprint.clone()),
                source_generation: pre.source_generation,
            };
            crate::storage::insert_sync_run_conn(tx, &run)?;
            crate::storage::set_processed_sequence_conn(tx, &session.id, pre.to_sequence)?;
            Ok((applied, skipped, summary))
        })
        .map_err(|e| {
            eprintln!("[sync] run {} rolled back, processed cursor unchanged: {}", pre.run_id, e);
            e
        })?;

        Ok(SyncJobOutput {
            run_id: pre.run_id.clone(),
            status: "ok".into(),
            applied,
            skipped,
            unclassified: pre.unclassified,
            summary,
        })
    }

    /// Convenience for callers that already hold the DB lock (tests and
    /// interactive single-session flows, which are heuristic-speed).
    pub fn run_session_sync(
        &self,
        db: &Db,
        session: &Session,
        events: &[SessionEvent],
        from_sequence: i64,
        to_sequence: i64,
    ) -> Result<SyncJobOutput> {
        let Some(pre) = self.prepare(db, session, events, from_sequence, to_sequence)? else {
            return Ok(SyncJobOutput {
                run_id: new_id(),
                status: "ok".into(),
                applied: 0,
                skipped: 0,
                unclassified: 0,
                summary: "该批次事件已由先前的 SyncRun 处理（幂等跳过）。".into(),
            });
        };
        let (mutations, runtime, diagnostics) = self.extract(session, &pre)?;
        self.commit(db, session, &pre, mutations, &runtime, diagnostics)
    }

    /// Non-blocking path used by background reconcile: takes the DB lock
    /// per phase, so concurrent UI commands interleave. Processes every
    /// pending event (sequence > processed cursor) of this session.
    pub fn run_pending_sync_nonblocking(
        &self,
        db_lock: &Mutex<Db>,
        session: &Session,
    ) -> Result<usize> {
        let pre = {
            let guard = lock_db(db_lock)?;
            let processed = guard.get_processed_sequence(&session.id)?;
            let pending = guard.get_events(&session.id, Some(processed), 10_000)?;
            if pending.is_empty() {
                return Ok(0);
            }
            let to = pending.last().map(|e| e.sequence).unwrap_or(processed);
            self.prepare(&guard, session, &pending, processed, to)?
        };
        let Some(pre) = pre else { return Ok(0) };

        // extraction runs WITHOUT the lock (may take minutes)
        let (mutations, runtime, diagnostics) = self.extract(session, &pre)?;

        let out = {
            let guard = lock_db(db_lock)?;
            self.commit(&guard, session, &pre, mutations, &runtime, diagnostics)?
        };
        Ok(out.applied)
    }

    /// Keyword-based workstream classification.
    /// Title / description tokens are matched against event text.
    fn auto_classify(&self, db: &Db, _session: &Session, events: &[SessionEvent]) -> Vec<String> {
        let workstreams = match db.list_workstreams(None) {
            Ok(v) => v,
            Err(_) => return vec![],
        };
        let text = events
            .iter()
            .filter_map(|e| e.text.as_deref())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if text.is_empty() {
            return vec![];
        }
        let mut scored: Vec<(String, usize)> = Vec::new();
        for w in workstreams {
            let mut score = 0usize;
            for token in tokenize(&format!("{} {}", w.title, w.description)) {
                if token.len() >= 3 && text.contains(&token) {
                    score += token.len();
                }
            }
            if score > 0 {
                scored.push((w.id, score));
            }
        }
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().take(2).map(|(id, _)| id).collect()
    }
}

/// Stable fingerprint of a processed delta: ordered event ids. A completed
/// SyncRun with the same fingerprint means "this batch is already merged".
pub fn delta_fingerprint(session_id: &str, events: &[SessionEvent]) -> String {
    let mut h = Sha256::new();
    h.update(session_id);
    for e in events {
        h.update([0x1f]);
        h.update(e.id.as_bytes());
    }
    let d = h.finalize();
    d.iter().map(|b| format!("{:02x}", b)).collect::<String>()
}

fn tokenize(s: &str) -> Vec<String> {
    // split on whitespace/punct; keep CJK bigrams for Chinese titles
    let mut out = Vec::new();
    for word in s.split(|c: char| c.is_whitespace() || ",.;:!?()[]{}\"'/|".contains(c)) {
        let w = word.trim();
        if w.is_empty() {
            continue;
        }
        if w.chars().any(|c| c.is_ascii()) {
            out.push(w.to_lowercase());
        }
        let chars: Vec<char> = w.chars().collect();
        if chars.len() >= 2 && chars.iter().all(|c| !c.is_ascii()) {
            for pair in chars.windows(2) {
                out.push(pair.iter().collect());
            }
        }
    }
    out.dedup();
    out
}

/// Create a new item with its first revision (shared by merge + manual UI).
#[allow(clippy::too_many_arguments)]
pub fn create_item(
    db: &Db,
    workstream_id: &str,
    kind: &str,
    title: &str,
    content: &str,
    authority: &str,
    source_type: &str,
    source_refs: &[String],
    sync_run_id: Option<&str>,
    created_by: &str,
) -> Result<ContextItem> {
    db.tx(|tx| {
        create_item_conn(
            tx,
            workstream_id,
            kind,
            title,
            content,
            authority,
            source_type,
            source_refs,
            sync_run_id,
            created_by,
        )
    })
}

/// Connection-level twin used inside transactions.
#[allow(clippy::too_many_arguments)]
pub fn create_item_conn(
    conn: &rusqlite::Connection,
    workstream_id: &str,
    kind: &str,
    title: &str,
    content: &str,
    authority: &str,
    source_type: &str,
    source_refs: &[String],
    sync_run_id: Option<&str>,
    created_by: &str,
) -> Result<ContextItem> {
    let item = ContextItem {
        id: new_id(),
        workstream_id: workstream_id.to_string(),
        kind: kind.to_string(),
        status: "active".into(),
        authority: authority.to_string(),
        created_by: created_by.to_string(),
        current_revision_id: None,
        supersedes_item_id: None,
        created_at: now(),
        updated_at: now(),
    };
    let rev = ContextItemRevision {
        id: new_id(),
        item_id: item.id.clone(),
        title: title.to_string(),
        content: content.to_string(),
        metadata: if source_refs.len() > 1 {
            serde_json::json!({ "source_refs": source_refs })
        } else {
            serde_json::json!({})
        },
        source_type: Some(source_type.to_string()),
        source_ref: source_refs.first().cloned(),
        sync_run_id: sync_run_id.map(|s| s.to_string()),
        created_at: now(),
    };
    crate::storage::insert_item_conn(conn, &item, &rev)?;
    Ok(item)
}
