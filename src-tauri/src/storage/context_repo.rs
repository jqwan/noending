//! The v1 Context + ingest-state repository.
//!
//! Two shapes of state, deliberately kept apart:
//!
//! * **facts** — `session_ingest_state` (the fact generation of the current
//!   conversation) and `session_message_projection` (which messages ARE that
//!   conversation, in order). Only ingestion writes these.
//! * **NoEnding-owned Context** — `session_contexts` /
//!   `session_context_revisions` (the system-derived Session summary) and
//!   `workstream_context_state` / `workstream_session_frontiers` (the two
//!   revision counters and what a Workstream has already consumed). Only an
//!   explicit user action writes these.
//!
//! Every read of the conversation goes through the projection, so a message a
//! source rewrite retired can never reappear. Conversations and Context are
//! deliberately kept apart: ingestion never writes Context.

use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::{
    SessionContextFields, SessionContextRecord, SessionContextRevision, SessionIngestState,
    WorkstreamContextState, WorkstreamSessionFrontier,
};
use crate::error::{other, Result};
use crate::storage::{now, Db};

// Fact generation and the current-message projection

/// What maintaining the projection did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionOutcome {
    /// Nothing changed (an unchanged re-scan, or an append that deduped away).
    Unchanged,
    /// New messages were appended to the current generation.
    Appended,
    /// The whole conversation was replaced and the generation was raised.
    NewGeneration,
    /// An incomplete full re-scan: projection, generation and cursor are kept.
    Incomplete,
    /// The member is not the root, so it contributes no conversation.
    NotRoot,
}

pub fn get_ingest_state_conn(conn: &Connection, session_id: &str) -> Result<SessionIngestState> {
    Ok(conn
        .query_row(
            "SELECT session_id, generation, latest_message_seq FROM session_ingest_state
             WHERE session_id = ?1",
            params![session_id],
            |r| {
                Ok(SessionIngestState {
                    session_id: r.get(0)?,
                    generation: r.get(1)?,
                    latest_message_seq: r.get(2)?,
                })
            },
        )
        .optional()?
        .unwrap_or(SessionIngestState {
            session_id: session_id.to_string(),
            generation: 0,
            latest_message_seq: 0,
        }))
}

/// The ordered message ids of the CURRENT conversation.
pub fn projection_ids_conn(conn: &Connection, session_id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT session_message_id FROM session_message_projection
         WHERE session_id = ?1 ORDER BY ordinal",
    )?;
    let rows = st
        .query_map(params![session_id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn set_ingest_state_conn(
    conn: &Connection,
    session_id: &str,
    generation: i64,
    latest_message_seq: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO session_ingest_state (session_id, generation, latest_message_seq)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(session_id) DO UPDATE SET generation = ?2, latest_message_seq = ?3",
        params![session_id, generation, latest_message_seq],
    )?;
    Ok(())
}

fn append_projection_conn(
    conn: &Connection,
    session_id: &str,
    ids: &[String],
    start_ordinal: i64,
) -> Result<i64> {
    let mut ordinal = start_ordinal;
    {
        let mut ins = conn.prepare(
            "INSERT OR IGNORE INTO session_message_projection (session_id, ordinal, session_message_id)
             VALUES (?1, ?2, ?3)",
        )?;
        for id in ids {
            ordinal += 1;
            ins.execute(params![session_id, ordinal, id])?;
        }
    }
    Ok(ordinal)
}

/// Maintain the current-message projection inside the member-ingest
/// transaction.
///
/// * `current_ids` — the ordered message ids of THIS read, including messages
///   that deduped onto existing rows (they are still part of the conversation).
/// * `full_rescan` — the read started at byte 0.
/// * `complete_snapshot` — every frame of that read was whole.
pub fn apply_projection_conn(
    conn: &Connection,
    session_id: &str,
    is_root: bool,
    current_ids: &[String],
    full_rescan: bool,
    complete_snapshot: bool,
) -> Result<ProjectionOutcome> {
    if !is_root {
        return Ok(ProjectionOutcome::NotRoot);
    }
    let state = get_ingest_state_conn(conn, session_id)?;

    if !full_rescan {
        // Incremental and source-continuous: extend the current generation.
        if current_ids.is_empty() {
            return Ok(ProjectionOutcome::Unchanged);
        }
        let existing: std::collections::HashSet<String> =
            projection_ids_conn(conn, session_id)?.into_iter().collect();
        let fresh: Vec<String> = current_ids
            .iter()
            .filter(|id| !existing.contains(*id))
            .cloned()
            .collect();
        if fresh.is_empty() {
            return Ok(ProjectionOutcome::Unchanged);
        }
        let end = append_projection_conn(conn, session_id, &fresh, state.latest_message_seq)?;
        set_ingest_state_conn(conn, session_id, state.generation, end)?;
        return Ok(ProjectionOutcome::Appended);
    }

    // A full re-scan. An incomplete one must not touch the current generation.
    if !complete_snapshot {
        return Ok(ProjectionOutcome::Incomplete);
    }

    let existing = projection_ids_conn(conn, session_id)?;
    if existing == current_ids {
        return Ok(ProjectionOutcome::Unchanged);
    }
    if !existing.is_empty() && current_ids.starts_with(&existing[..]) {
        let fresh: Vec<String> = current_ids[existing.len()..].to_vec();
        let end = append_projection_conn(conn, session_id, &fresh, state.latest_message_seq)?;
        set_ingest_state_conn(conn, session_id, state.generation, end)?;
        return Ok(ProjectionOutcome::Appended);
    }

    // The conversation itself changed (rewrite / truncate / reorder): raise the
    // fact generation and atomically replace the whole projection. The old
    // `session_messages` rows stay for provenance audit.
    conn.execute(
        "DELETE FROM session_message_projection WHERE session_id = ?1",
        params![session_id],
    )?;
    let generation = state.generation + 1;
    let end = append_projection_conn(conn, session_id, current_ids, 0)?;
    set_ingest_state_conn(conn, session_id, generation, end)?;
    Ok(ProjectionOutcome::NewGeneration)
}

/// Resolve the given identity hashes back to their stored message ids, in the
/// order given. Used to build `current_ids` for a full re-scan, where an
/// unchanged message dedups onto an existing row.
pub fn message_ids_for_hashes_conn(
    conn: &Connection,
    member_id: &str,
    hashes: &[String],
) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT id FROM session_messages WHERE member_id = ?1 AND source_identity_hash = ?2",
    )?;
    let mut out = Vec::with_capacity(hashes.len());
    for h in hashes {
        let id: Option<String> = st
            .query_row(params![member_id, h], |r| r.get(0))
            .optional()?;
        if let Some(id) = id {
            out.push(id);
        }
    }
    Ok(out)
}

// Session Context

fn decode_list(v: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(v).unwrap_or_default()
}

fn encode_list(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

pub fn get_session_context_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionContextRecord>> {
    Ok(conn
        .query_row(
            "SELECT session_id, summary_current_state, decisions, open_questions, next_steps,
                    revision, ingest_generation, processed_through_seq, updated_at
             FROM session_contexts WHERE session_id = ?1",
            params![session_id],
            |r| {
                Ok(SessionContextRecord {
                    session_id: r.get(0)?,
                    fields: SessionContextFields {
                        summary_current_state: r.get(1)?,
                        decisions: decode_list(&r.get::<_, String>(2)?),
                        open_questions: decode_list(&r.get::<_, String>(3)?),
                        next_steps: decode_list(&r.get::<_, String>(4)?),
                    },
                    revision: r.get(5)?,
                    ingest_generation: r.get(6)?,
                    processed_through_seq: r.get(7)?,
                    updated_at: r.get(8)?,
                })
            },
        )
        .optional()?)
}

fn get_session_context_revision_conn(
    conn: &Connection,
    session_id: &str,
    revision: i64,
) -> Result<Option<SessionContextRevision>> {
    Ok(conn
        .query_row(
            "SELECT session_id, revision, summary_current_state, decisions, open_questions,
                    next_steps, ingest_generation, processed_through_seq, created_at
             FROM session_context_revisions WHERE session_id = ?1 AND revision = ?2",
            params![session_id, revision],
            |r| {
                Ok(SessionContextRevision {
                    session_id: r.get(0)?,
                    revision: r.get(1)?,
                    fields: SessionContextFields {
                        summary_current_state: r.get(2)?,
                        decisions: decode_list(&r.get::<_, String>(3)?),
                        open_questions: decode_list(&r.get::<_, String>(4)?),
                        next_steps: decode_list(&r.get::<_, String>(5)?),
                    },
                    ingest_generation: r.get(6)?,
                    processed_through_seq: r.get(7)?,
                    created_at: r.get(8)?,
                })
            },
        )
        .optional()?)
}

/// Commit one Session Context inside a caller-held transaction. The CAS is on
/// `expected_revision`: a mismatch is a concurrency conflict, never a silent
/// overwrite. Returns the new revision.
#[allow(clippy::too_many_arguments)]
pub fn commit_session_context_conn(
    conn: &Connection,
    session_id: &str,
    fields: &SessionContextFields,
    expected_revision: i64,
    ingest_generation: i64,
    processed_through_seq: i64,
) -> Result<i64> {
    let current = get_session_context_conn(conn, session_id)?;
    let current_revision = current.as_ref().map(|c| c.revision).unwrap_or(0);
    if current_revision != expected_revision {
        return Err(other(format!(
            "会话摘要已被并发更新（期望 revision {expected_revision}，当前 {current_revision}）"
        )));
    }
    let revision = current_revision + 1;
    let updated_at = now();
    conn.execute(
        "INSERT INTO session_contexts
           (session_id, summary_current_state, decisions, open_questions, next_steps,
            revision, ingest_generation, processed_through_seq, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(session_id) DO UPDATE SET
           summary_current_state = ?2, decisions = ?3, open_questions = ?4, next_steps = ?5,
           revision = ?6, ingest_generation = ?7, processed_through_seq = ?8, updated_at = ?9",
        params![
            session_id,
            fields.summary_current_state,
            encode_list(&fields.decisions),
            encode_list(&fields.open_questions),
            encode_list(&fields.next_steps),
            revision,
            ingest_generation,
            processed_through_seq,
            updated_at,
        ],
    )?;
    conn.execute(
        "INSERT INTO session_context_revisions
           (session_id, revision, summary_current_state, decisions, open_questions, next_steps,
            ingest_generation, processed_through_seq, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            session_id,
            revision,
            fields.summary_current_state,
            encode_list(&fields.decisions),
            encode_list(&fields.open_questions),
            encode_list(&fields.next_steps),
            ingest_generation,
            processed_through_seq,
            updated_at,
        ],
    )?;
    Ok(revision)
}

// Workstream Context revisions and per-Session frontiers

pub fn get_workstream_context_state_conn(
    conn: &Connection,
    workstream_id: &str,
) -> Result<WorkstreamContextState> {
    Ok(conn
        .query_row(
            "SELECT workstream_id, context_revision, input_revision, consumed_input_revision
             FROM workstream_context_state WHERE workstream_id = ?1",
            params![workstream_id],
            |r| {
                Ok(WorkstreamContextState {
                    workstream_id: r.get(0)?,
                    context_revision: r.get(1)?,
                    input_revision: r.get(2)?,
                    consumed_input_revision: r.get(3)?,
                })
            },
        )
        .optional()?
        .unwrap_or(WorkstreamContextState {
            workstream_id: workstream_id.to_string(),
            context_revision: 0,
            input_revision: 0,
            consumed_input_revision: 0,
        }))
}

pub fn ensure_workstream_state_conn(conn: &Connection, workstream_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO workstream_context_state
           (workstream_id, context_revision, input_revision, consumed_input_revision)
         VALUES (?1, 0, 1, 0)",
        params![workstream_id],
    )?;
    Ok(())
}

/// Bump `context_revision` — called by every path that writes a ContextItem
/// (manual edit, AI mutation, conflict resolution).
pub fn bump_context_revision_conn(conn: &Connection, workstream_id: &str) -> Result<i64> {
    ensure_workstream_state_conn(conn, workstream_id)?;
    conn.execute(
        "UPDATE workstream_context_state SET context_revision = context_revision + 1
         WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    Ok(get_workstream_context_state_conn(conn, workstream_id)?.context_revision)
}

/// Bump `input_revision` — called by paths that change what the Workstream
/// needs to RE-SYNTHESISE from: manual Context edits, Owner set, title /
/// description change, Session Trash / Restore.
pub fn bump_input_revision_conn(conn: &Connection, workstream_id: &str) -> Result<i64> {
    ensure_workstream_state_conn(conn, workstream_id)?;
    conn.execute(
        "UPDATE workstream_context_state SET input_revision = input_revision + 1
         WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    Ok(get_workstream_context_state_conn(conn, workstream_id)?.input_revision)
}

/// Record that the AI has consumed the current `input_revision`, so a
/// successful update does not leave the Workstream looking stale.
pub fn consume_input_revision_conn(conn: &Connection, workstream_id: &str) -> Result<()> {
    ensure_workstream_state_conn(conn, workstream_id)?;
    conn.execute(
        "UPDATE workstream_context_state
           SET consumed_input_revision = input_revision
         WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    Ok(())
}

pub fn set_workstream_frontier_conn(
    conn: &Connection,
    f: &WorkstreamSessionFrontier,
) -> Result<()> {
    conn.execute(
        "INSERT INTO workstream_session_frontiers
           (workstream_id, session_id, session_context_revision, ingest_generation, consumed_through_seq)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(workstream_id, session_id) DO UPDATE SET
           session_context_revision = ?3, ingest_generation = ?4, consumed_through_seq = ?5",
        params![
            f.workstream_id,
            f.session_id,
            f.session_context_revision,
            f.ingest_generation,
            f.consumed_through_seq,
        ],
    )?;
    Ok(())
}

pub fn delete_workstream_frontier_conn(
    conn: &Connection,
    workstream_id: &str,
    session_id: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM workstream_session_frontiers WHERE workstream_id = ?1 AND session_id = ?2",
        params![workstream_id, session_id],
    )?;
    Ok(())
}

/// Delete frontiers only for Sessions that no longer belong to this Workstream.
/// Sessions that are unchanged or omitted from a partial update still own
/// their frontier and must not have their consumed message prefix replayed.
pub fn delete_unowned_workstream_frontiers_conn(
    conn: &Connection,
    workstream_id: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM workstream_session_frontiers
         WHERE workstream_id = ?1
           AND NOT EXISTS (
             SELECT 1 FROM sessions s
             WHERE s.id = workstream_session_frontiers.session_id
               AND s.owner_workstream_id = ?1
           )",
        params![workstream_id],
    )?;
    Ok(())
}

pub fn frontiers_for_workstream_conn(
    conn: &Connection,
    workstream_id: &str,
) -> Result<Vec<WorkstreamSessionFrontier>> {
    let mut st = conn.prepare(
        "SELECT workstream_id, session_id, session_context_revision, ingest_generation, consumed_through_seq
         FROM workstream_session_frontiers WHERE workstream_id = ?1",
    )?;
    let rows = st
        .query_map(params![workstream_id], |r| {
            Ok(WorkstreamSessionFrontier {
                workstream_id: r.get(0)?,
                session_id: r.get(1)?,
                session_context_revision: r.get(2)?,
                ingest_generation: r.get(3)?,
                consumed_through_seq: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

// Db-level wrappers

impl Db {
    pub fn get_session_ingest_state(&self, session_id: &str) -> Result<SessionIngestState> {
        let conn = self.read();
        get_ingest_state_conn(&conn, session_id)
    }

    pub fn get_session_context(&self, session_id: &str) -> Result<Option<SessionContextRecord>> {
        let conn = self.read();
        get_session_context_conn(&conn, session_id)
    }

    pub fn get_session_context_revision(
        &self,
        session_id: &str,
        revision: i64,
    ) -> Result<Option<SessionContextRevision>> {
        let conn = self.read();
        get_session_context_revision_conn(&conn, session_id, revision)
    }

    pub fn get_workstream_context_state(
        &self,
        workstream_id: &str,
    ) -> Result<WorkstreamContextState> {
        let conn = self.read();
        get_workstream_context_state_conn(&conn, workstream_id)
    }

    pub fn workstream_frontiers(
        &self,
        workstream_id: &str,
    ) -> Result<Vec<WorkstreamSessionFrontier>> {
        let conn = self.read();
        frontiers_for_workstream_conn(&conn, workstream_id)
    }

    /// The ordered message ids of a Session's CURRENT conversation.
    pub fn message_projection_ids(&self, session_id: &str) -> Result<Vec<String>> {
        let conn = self.read();
        projection_ids_conn(&conn, session_id)
    }
}
