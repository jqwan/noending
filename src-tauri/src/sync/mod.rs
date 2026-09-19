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

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::{Mutex, MutexGuard};

use crate::domain::{
    binding_source, ContextItem, ContextItemRevision, Session, SessionEvent,
    SessionWorkstreamBinding, SyncRun,
};
use crate::error::{other, Result};
use crate::storage::{new_id, now, Db};

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
        Self {
            mutations,
            diagnostics: vec![],
        }
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
    /// True when `candidates` came from keyword auto-classification rather
    /// than existing bindings; commit then persists them as
    /// `automatic_classification` bindings inside the run transaction, so
    /// the UI's session state matches what sync actually used.
    pub candidates_are_automatic: bool,
    /// CAS snapshot of the session's binding decision (strong bindings +
    /// removal tombstones) at prepare time. A binding decision IS a Context
    /// routing decision; commit re-computes it and discards the run when it
    /// changed during the lock-free extraction — one check that covers
    /// auto→strong, strong→none, strong A→strong B and tombstone changes.
    pub binding_decision_hash: String,
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
            Some(cli) if crate::adapters::adapter_for(cli.agent).detect().is_some() => Self {
                heuristic: extractor::HeuristicExtractor,
                merger: merge::MergeEngine,
                llm: Some(cli),
            },
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

        // 1. candidate workstreams: bound ones, plus keyword-matched ones.
        //     Auto-classified candidates are remembered as such so commit
        //     can persist the classification as a real binding.
        //     STRONG provenance (explicit launch / user assignment) freezes
        //     the candidates; automatic_classification is only a guess and
        //     must stay revisable — it never blocks re-classification when
        //     the evidence in new events points elsewhere.
        let bound: Vec<SessionWorkstreamBinding> = db.bindings_for_session(&session.id)?;
        let strong: Vec<String> = bound
            .iter()
            .filter(|b| {
                matches!(
                    b.source.as_str(),
                    binding_source::EXPLICIT_LAUNCH | binding_source::USER_ASSIGNED
                )
            })
            .map(|b| b.workstream_id.clone())
            .collect();
        let (candidates, candidates_are_automatic) = if strong.is_empty() {
            // Durable negative overrides filter BEFORE extraction: a
            // workstream the user explicitly removed must not even become an
            // extraction candidate. Filtering only at binding write-back
            // would still run the extractor against it and COMMIT context
            // mutations into a workstream the user rejected.
            let proposed = self.auto_classify(db, session, events);
            let mut kept = Vec::with_capacity(proposed.len());
            for ws in proposed {
                if !db.binding_removal_exists(&session.id, &ws)? {
                    kept.push(ws);
                }
            }
            (kept, true)
        } else {
            (strong, false)
        };

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

        // 3. snapshot everything extraction needs while the lock is held.
        // The binding decision hash rides along as the commit-phase CAS.
        let binding_decision = binding_decision_hash(&db.0, &session.id)?;
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
            candidates_are_automatic,
            binding_decision_hash: binding_decision,
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
                    let o = self
                        .heuristic
                        .extract(session, &refs, &pre.candidates, &pre.inputs)?;
                    Ok((
                        o.mutations,
                        format!("{}->heuristic", cli.name()),
                        o.diagnostics,
                    ))
                }
            }
        } else {
            let o = self
                .heuristic
                .extract(session, &refs, &pre.candidates, &pre.inputs)?;
            Ok((o.mutations, "heuristic".to_string(), o.diagnostics))
        }
    }

    /// Phase C (DB lock held): apply mutations, record the SyncRun and
    /// advance the processed cursor — all inside ONE transaction. Any error
    /// rolls the entire run back and the batch is retried later.
    ///
    /// CAS inside the transaction: `prepare` snapshotted `from_sequence`
    /// while holding the lock, but extraction runs WITHOUT it and may take
    /// minutes. If another run committed a newer delta for this session in
    /// the meantime, our mutations are stale — applying them would duplicate
    /// context and move the processed cursor BACKWARDS. The run is discarded
    /// (status "stale"); the next sync re-prepares from the current state.
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
        db.tx(|tx| {
            let current_processed: i64 = tx.query_row(
                "SELECT COALESCE((SELECT processed_sequence FROM session_cursors WHERE session_id = ?1), 0)",
                rusqlite::params![session.id],
                |r| r.get(0),
            )?;
            if current_processed != pre.from_sequence {
                eprintln!(
                    "[sync] run {} stale: processed moved {} → {} during extraction, discarding",
                    pre.run_id, pre.from_sequence, current_processed
                );
                return Ok(SyncJobOutput {
                    run_id: pre.run_id.clone(),
                    status: "stale".into(),
                    applied: 0,
                    skipped: 0,
                    unclassified: 0,
                    summary: format!(
                        "提取期间该 Session 已被其他同步推进（processed {} → {}），本次结果作废，待下次同步重新提取。",
                        pre.from_sequence, current_processed
                    ),
                });
            }

            // CAS on the binding decision (AGENTS.md: revalidate state before
            // committing work prepared while the lock was released). A
            // binding decision IS a Context routing decision: strong
            // bindings and removal tombstones decide where this session's
            // context may go. If either changed while the extractor ran
            // (user edit, launch selection, removal), the prepared mutations
            // target a routing that no longer exists — discard the run
            // WITHOUT advancing the processed cursor, so the next sync
            // re-prepares against the user's decision. One hash comparison
            // covers every transition (auto→strong, strong→none,
            // strong A→strong B, tombstone added/removed) instead of
            // case-by-case counters.
            let current_decision = binding_decision_hash(tx, &session.id)?;
            if current_decision != pre.binding_decision_hash {
                eprintln!(
                    "[sync] run {} stale: binding decision changed during extraction, discarding",
                    pre.run_id
                );
                return Ok(SyncJobOutput {
                    run_id: pre.run_id.clone(),
                    status: "stale".into(),
                    applied: 0,
                    skipped: 0,
                    unclassified: 0,
                    summary:
                        "提取期间该 Session 的 Workstream 绑定发生了变化，本次结果作废，待下次同步按新绑定重新准备。"
                            .into(),
                });
            }

            let mut applied = 0usize;
            let mut skipped = 0usize;
            for m in &mutations {
                if self.merger.apply(tx, m, &ctx)? {
                    applied += 1;
                } else {
                    skipped += 1;
                }
            }

            // Persist auto-classification so the session's UI state matches
            // what sync actually used. Binding precedence keeps any explicit
            // or user binding strictly stronger than this one.
            //
            // A fresh classification REPLACES the previous automatic guess
            // instead of accumulating stale auto bindings beside it — auto
            // bindings are revisable by design. When the new classification
            // is empty there is no signal, so the old guess is kept rather
            // than dropped. (Strong bindings are never touched here: prepare
            // only reaches the automatic path when none exist, and
            // bind_conn's precedence still guards a racing explicit bind.)
            // Candidates the user explicitly removed are skipped via the
            // durable removal tombstones — a rejected guess must not come
            // back.
            if pre.candidates_are_automatic && !pre.candidates.is_empty() {
                persist_auto_classification(tx, &session.id, &pre.candidates)?;
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
            Ok(SyncJobOutput {
                run_id: pre.run_id.clone(),
                status: "ok".into(),
                applied,
                skipped,
                unclassified: pre.unclassified,
                summary,
            })
        })
        .map_err(|e| {
            eprintln!("[sync] run {} rolled back, processed cursor unchanged: {}", pre.run_id, e);
            e
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
/// CAS snapshot of a session's binding decision: the sorted set of strong
/// bindings (workstream_id + role) plus removal tombstones. AUTO rows are
/// deliberately excluded — sync rewrites them on every classification run,
/// so they are not part of the *decision*. Accepts `&Connection` or
/// `&Transaction` (deref).
pub fn binding_decision_hash(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> crate::error::Result<String> {
    use sha2::Digest;

    let mut st = conn.prepare(
        "SELECT workstream_id, role FROM session_workstream_bindings
         WHERE session_id = ?1 AND source IN (?2, ?3)
         ORDER BY workstream_id",
    )?;
    let strong = st
        .query_map(
            rusqlite::params![
                session_id,
                binding_source::EXPLICIT_LAUNCH,
                binding_source::USER_ASSIGNED
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut st2 = conn.prepare(
        "SELECT workstream_id FROM session_binding_removals
         WHERE session_id = ?1 ORDER BY workstream_id",
    )?;
    let removed = st2
        .query_map(rusqlite::params![session_id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut h = sha2::Sha256::new();
    for (ws, role) in strong {
        h.update(b"B\x1f");
        h.update(ws.as_bytes());
        h.update(b"\x1f");
        h.update(role.as_bytes());
        h.update(b"\x1e");
    }
    for ws in removed {
        h.update(b"R\x1f");
        h.update(ws.as_bytes());
        h.update(b"\x1e");
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Testable core of the auto-classification persist step: replace the
/// previous AUTOMATIC guesses with the fresh candidates, skipping
/// workstreams the user explicitly removed (durable removal tombstones —
/// a rejected guess must not silently come back).
pub fn persist_auto_classification(
    tx: &rusqlite::Transaction,
    session_id: &str,
    candidates: &[String],
) -> crate::error::Result<()> {
    use crate::domain::{binding_source, SessionWorkstreamBinding};
    use crate::storage::{bind_conn, now};

    tx.execute(
        "DELETE FROM session_workstream_bindings WHERE session_id = ?1 AND source = ?2",
        rusqlite::params![session_id, binding_source::AUTO],
    )?;
    for ws_id in candidates {
        let removed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_binding_removals
             WHERE session_id = ?1 AND workstream_id = ?2)",
            rusqlite::params![session_id, ws_id],
            |r| r.get(0),
        )?;
        if removed {
            continue;
        }
        bind_conn(
            tx,
            &SessionWorkstreamBinding {
                session_id: session_id.to_string(),
                workstream_id: ws_id.clone(),
                role: "related".into(),
                source: binding_source::AUTO.into(),
                confidence: 0.6,
                workstream_path_id: None,
                last_seen_revision: None,
                last_sync_cursor: 0,
                created_at: now(),
                last_used_at: now(),
            },
        )?;
    }
    Ok(())
}

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
        metadata: {
            let actor = if authority.starts_with("user") || created_by == "user" {
                "user"
            } else if created_by == "agent"
                || created_by.starts_with("sync:")
                || sync_run_id.is_some()
                || authority.starts_with("agent")
                || source_type == "session_event"
            {
                "agent"
            } else {
                "system"
            };
            let mut meta = serde_json::json!({
                "provenance": {
                    "authority": authority,
                    "actor": actor,
                    "source_type": source_type,
                    "source_ref": source_refs.first(),
                }
            });
            if source_refs.len() > 1 {
                meta["source_refs"] = serde_json::json!(source_refs);
            }
            meta
        },
        source_type: Some(source_type.to_string()),
        source_ref: source_refs.first().cloned(),
        sync_run_id: sync_run_id.map(|s| s.to_string()),
        created_at: now(),
    };
    crate::storage::insert_item_conn(conn, &item, &rev)?;
    let mut item = item;
    item.current_revision_id = Some(rev.id);
    Ok(item)
}
