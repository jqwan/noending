//! Workspace Assistant Core — Background Sync.
//!
//! A SyncJob reads the CONVERSATION messages after the Context frontier
//! (`session_context_state.processed_message_sequence`), extracts context
//! mutations routed to the Session's single Owner Workstream, and merges —
//! all database writes of one run commit in a single SQLite transaction
//! together with the SyncRun row and the frontier advance. Any failure rolls
//! the whole run back; retries are idempotent via the delta fingerprint.
//!
//! The input is exactly `session_messages`: the Conversation is already
//! curated at ingestion (only root user/assistant prose is stored), so there
//! is no second kind-based filter and no length heuristic here — a short
//! message can be the constraint that matters. Adapter and Extractor
//! own the two judgment calls the design names: the adapter decides what is
//! real conversation text, the extractor decides what carries Context value.
//!
//! Routing has exactly one input: `session.owner_workstream_id`.
//! There is no automatic classification and no candidate set.
//!
//! Locking model (non-blocking UI): a run is split into three phases.
//! `prepare` and `commit` each take the DB lock briefly; `extract` — which
//! may run the user's agent CLI for minutes — holds NO lock, so concurrent
//! UI commands interleave freely. `prepare` snapshots everything extraction
//! needs.
//!
//! Policy (docs, Issue #3/#4):
//! - the member cursors (read position) and the session's context frontier
//!   (processed position) are separate lifecycles;
//! - every mutation passes the unified AuthorityPolicy; user authority is
//!   never silently overridden — disagreement becomes a ContextConflict;
//! - every mutation leaves a full source trail (`session-message:<id>`).

pub mod extractor;
pub mod merge;
pub mod policy;

use rusqlite::OptionalExtension;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::domain::{ContextItem, ContextItemRevision, Session, SessionMessage, SyncRun};
use crate::error::Result;
use crate::storage::{new_id, now, Db};

/// A proposed change to a workstream's context, produced by the Assistant.
/// `source_refs` are stable message references ("session-message:<id>"); a
/// mutation may cite several messages. Empty = no resolvable source.
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

/// The extraction seam between conversation messages and the deterministic
/// merge engine. `inputs` are the prompt-building reads snapshotted while
/// the DB lock was held — extraction itself must not touch the database.
pub trait ContextExtractor: Send + Sync {
    fn name(&self) -> String;

    /// `workstream_id` is the Session's Owner Workstream — the single routing
    /// target of this extraction.
    fn extract(
        &self,
        session: &Session,
        messages: &[&SessionMessage],
        workstream_id: &str,
        inputs: &extractor::PromptInputs,
    ) -> Result<ExtractOutput>;
}

/// Everything needed to run one sync run. Produced by `prepare` while the
/// DB lock is held; consumed by `extract` (lock-free) and `commit` (locked).
#[derive(Debug, Clone)]
pub struct PreparedSync {
    pub run_id: String,
    pub fingerprint: String,
    pub from_sequence: i64,
    pub to_sequence: i64,
    /// Snapshot of `sessions.owner_workstream_id` taken at prepare time. It is
    /// both the routing target and the commit-phase CAS.
    pub owner_workstream_id: Option<String>,
    /// The conversation messages this run processes — already exactly the
    /// user/assistant prose the store curates (no pre-filter here).
    pub messages: Vec<SessionMessage>,
    pub inputs: extractor::PromptInputs,
}

/// Everything the merge engine needs to know about the run it writes for.
#[derive(Debug, Clone)]
pub struct MergeContext {
    pub run_id: String,
    /// extractor runtime name; recorded as `created_by = sync:<runtime>`.
    pub runtime: String,
    /// The ONE Workstream this run may write. Every mutation is
    /// checked against it before anything is stored: a Session has a single
    /// Owner, so a SyncRun that wrote another Workstream's Context would make
    /// routing a suggestion again. The check lives here rather than only in the
    /// extractor because model output is untrusted input — an `item_id` the
    /// transcript happened to contain must not become a write into a Workstream
    /// this Session does not belong to.
    pub workstream_id: String,
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

    /// Phase A (DB lock held): idempotency check, Owner snapshot, prompt-input
    /// snapshot. Returns None when this delta was already processed by a
    /// committed run, or when the Session has no Owner Workstream (—
    /// ownerless: messages keep ingesting, the frontier stays put, and a later
    /// owner assignment re-processes from here).
    pub fn prepare(
        &self,
        db: &Db,
        session: &Session,
        messages: &[SessionMessage],
        from_sequence: i64,
        to_sequence: i64,
    ) -> Result<Option<PreparedSync>> {
        let run_id = new_id();
        let fingerprint = delta_fingerprint(&session.id, messages);

        // Idempotent retries: this exact delta already committed once.
        if !messages.is_empty() && db.has_completed_run(&session.id, &fingerprint)? {
            return Ok(None);
        }

        // 1. routing: the Session's single Owner Workstream, read fresh from
        //    the DB. No Owner means no Context processing at all:
        //    messages keep ingesting, the frontier stays put, and a later
        //    owner assignment re-processes from here.
        let owner_workstream_id: Option<String> = db
            .get_session(&session.id)?
            .and_then(|s| s.owner_workstream_id);
        if owner_workstream_id.is_none() {
            return Ok(None);
        }

        // 2. snapshot everything extraction needs while the lock is held.
        let inputs = if self.llm.is_some() {
            extractor::collect_prompt_inputs(db, owner_workstream_id.as_deref().unwrap_or(""))?
        } else {
            extractor::PromptInputs::default()
        };

        Ok(Some(PreparedSync {
            run_id,
            fingerprint,
            from_sequence,
            to_sequence,
            owner_workstream_id,
            messages: messages.to_vec(),
            inputs,
        }))
    }

    /// Phase B (NO DB lock): extraction. The heuristic is instant; the CLI
    /// extractor may run the user's agent CLI for minutes. An extractor
    /// failure falls back to the heuristic; a heuristic failure surfaces so
    /// the commit phase (and frontier) is skipped for retry.
    pub fn extract(
        &self,
        session: &Session,
        pre: &PreparedSync,
    ) -> Result<(Vec<ContextMutation>, String, Vec<String>)> {
        let Some(ws_id) = pre.owner_workstream_id.as_deref() else {
            return Ok((Vec::new(), "none".to_string(), Vec::new()));
        };
        if pre.messages.is_empty() {
            return Ok((Vec::new(), "none".to_string(), Vec::new()));
        }
        let refs: Vec<&SessionMessage> = pre.messages.iter().collect();
        if let Some(cli) = &self.llm {
            match cli.extract(session, &refs, ws_id, &pre.inputs) {
                Ok(o) => Ok((o.mutations, cli.name(), o.diagnostics)),
                Err(e) => {
                    eprintln!("[sync] cli extractor failed, falling back: {}", e);
                    let o = self.heuristic.extract(session, &refs, ws_id, &pre.inputs)?;
                    Ok((
                        o.mutations,
                        format!("{}->heuristic", cli.name()),
                        o.diagnostics,
                    ))
                }
            }
        } else {
            let o = self.heuristic.extract(session, &refs, ws_id, &pre.inputs)?;
            Ok((o.mutations, "heuristic".to_string(), o.diagnostics))
        }
    }

    /// Phase C (DB lock held): apply mutations, record the SyncRun and
    /// advance the Context frontier — all inside ONE transaction. Any error
    /// rolls the entire run back and the batch is retried later.
    ///
    /// CAS inside the transaction: `prepare` snapshotted `from_sequence`
    /// while holding the lock, but extraction runs WITHOUT it and may take
    /// minutes. If another run committed a newer delta for this session in
    /// the meantime, our mutations are stale — applying them would duplicate
    /// context and move the frontier BACKWARDS. The run is discarded
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
        // the Owner IS the routing target. `prepare` refuses to prepare an
        // ownerless Session, so arriving here without one means a caller built a
        // `PreparedSync` by hand: never write Context, a SyncRun or a frontier
        // advance for a Session that has no Workstream to route into.
        let Some(run_workstream) = pre.owner_workstream_id.clone() else {
            return Ok(SyncJobOutput {
                run_id: pre.run_id.clone(),
                status: "no_owner".into(),
                applied: 0,
                skipped: 0,
                unclassified: 0,
                summary: "该 Session 没有 Owner Workstream，本次同步不处理 Context。".into(),
            });
        };
        let ctx = MergeContext {
            run_id: pre.run_id.clone(),
            runtime: runtime.to_string(),
            workstream_id: run_workstream,
        };
        db.tx(|tx| {
            // commit-time trash guard, FIRST of the re-checks: a run
            // prepared against a session that was trashed while its
            // extraction ran must not write message-derived context, a
            // SyncRun, or a frontier advance. The session keeps its
            // data frozen at the moment of trashing; a Restore re-syncs from
            // the unchanged frontier.
            //
            // Type note: the turbofish pins the column reader to Option<String>
            // so `.optional()`'s outer Option means ROW PRESENCE — Some(None)
            // is an existing, Normal session; None is a vanished row.
            let trashed: Option<Option<String>> = tx
                .query_row(
                    "SELECT trashed_at FROM sessions WHERE id = ?1",
                    rusqlite::params![session.id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?;
            if !matches!(trashed, Some(None)) {
                eprintln!(
                    "[sync] run {} rejected: session {} is trashed or gone, discarding",
                    pre.run_id, session.id
                );
                return Ok(SyncJobOutput {
                    run_id: pre.run_id.clone(),
                    status: "trashed".into(),
                    applied: 0,
                    skipped: 0,
                    unclassified: 0,
                    summary: "该会话已移入回收站，本次同步结果作废。".into(),
                });
            }

            let current_processed: i64 = tx.query_row(
                "SELECT COALESCE((SELECT processed_message_sequence FROM session_context_state WHERE session_id = ?1), 0)",
                rusqlite::params![session.id],
                |r| r.get(0),
            )?;
            if current_processed != pre.from_sequence {
                eprintln!(
                    "[sync] run {} stale: frontier moved {} → {} during extraction, discarding",
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

            // CAS on the routing target: the Session's Owner
            // Workstream IS the Context routing decision. If the user changed
            // the Owner while the extractor ran without the lock, the prepared
            // mutations target a routing that no longer exists — discard the
            // run WITHOUT advancing the frontier, so the next sync
            // re-prepares against the new Owner.
            let current_owner: Option<Option<String>> = tx
                .query_row(
                    "SELECT owner_workstream_id FROM sessions WHERE id = ?1",
                    rusqlite::params![session.id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?;
            if !matches!(&current_owner, Some(owner) if *owner == pre.owner_workstream_id) {
                eprintln!(
                    "[sync] run {} stale: session owner changed during extraction, discarding",
                    pre.run_id
                );
                return Ok(SyncJobOutput {
                    run_id: pre.run_id.clone(),
                    status: "stale".into(),
                    applied: 0,
                    skipped: 0,
                    unclassified: 0,
                    summary:
                        "提取期间该 Session 的所属任务发生了变化，本次结果作废，待下次同步按新归属重新准备。"
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

            let mut summary = format!(
                "同步 {} 条新增消息：新增/更新 {} 项，跳过 {} 项",
                pre.messages.len(),
                applied,
                skipped
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
            };
            crate::storage::insert_sync_run_conn(tx, &run)?;
            crate::storage::set_processed_message_sequence_conn(tx, &session.id, pre.to_sequence)?;
            Ok(SyncJobOutput {
                run_id: pre.run_id.clone(),
                status: "ok".into(),
                applied,
                skipped,
                unclassified: 0,
                summary,
            })
        })
        .map_err(|e| {
            eprintln!("[sync] run {} rolled back, frontier unchanged: {}", pre.run_id, e);
            e
        })
    }

    /// Convenience for callers that already hold the DB lock (tests and
    /// interactive single-session flows, which are heuristic-speed).
    pub fn run_session_sync(
        &self,
        db: &Db,
        session: &Session,
        messages: &[SessionMessage],
        from_sequence: i64,
        to_sequence: i64,
    ) -> Result<SyncJobOutput> {
        let Some(pre) = self.prepare(db, session, messages, from_sequence, to_sequence)? else {
            return Ok(SyncJobOutput {
                run_id: new_id(),
                status: "ok".into(),
                applied: 0,
                skipped: 0,
                unclassified: 0,
                summary: "该批次消息已由先前的 SyncRun 处理（幂等跳过）。".into(),
            });
        };
        let (mutations, runtime, diagnostics) = self.extract(session, &pre)?;
        self.commit(db, session, &pre, mutations, &runtime, diagnostics)
    }
}

/// Stable fingerprint of a processed delta: ordered message ids. A completed
/// SyncRun with the same fingerprint means "this batch is already merged".
pub fn delta_fingerprint(session_id: &str, messages: &[SessionMessage]) -> String {
    let mut h = Sha256::new();
    h.update(session_id);
    for m in messages {
        h.update([0x1f]);
        h.update(m.id.as_bytes());
    }
    let d = h.finalize();
    d.iter().map(|b| format!("{:02x}", b)).collect::<String>()
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
                || source_type == "session_message"
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
