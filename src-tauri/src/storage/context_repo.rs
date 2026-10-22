//! Current summaries and conversation ingestion state.
//!
//! Two shapes of state, deliberately kept apart:
//!
//! * **facts** — `sessions.fact_generation` (the fact generation of the current
//!   conversation) and `session_message_projection` (which messages ARE that
//!   conversation, in order). Only ingestion writes these.
//! * **NoEnding-owned Context** — `session_contexts` (the current summary) and
//!   `workstream_context_state` / `workstream_session_frontiers` (the two
//!   revision counters and what a Workstream has already consumed). Only an
//!   explicit user action writes these.
//!
//! Every read of the conversation goes through the projection, so a message a
//! source rewrite retired can never reappear. Conversations and Context are
//! deliberately kept apart: ingestion never writes Context.

use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::{
    SessionContextFields, SessionContextRecord, WorkstreamContextState, WorkstreamSessionFrontier,
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
}

/// The session's fact frontier: `(fact_generation, latest_message_seq)`, read
/// straight off the flattened `sessions` row. No row is impossible (FK), but a
/// missing read still answers the zero frontier.
pub fn get_ingest_state_conn(conn: &Connection, session_id: &str) -> Result<(i64, i64)> {
    Ok(conn
        .query_row(
            "SELECT fact_generation, latest_message_seq FROM sessions WHERE id = ?1",
            params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, 0)))
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
        "UPDATE sessions SET fact_generation = ?2, latest_message_seq = ?3 WHERE id = ?1",
        params![session_id, generation, latest_message_seq],
    )?;
    Ok(())
}

/// Recompute the turn-final flag over the CURRENT conversation: an assistant
/// message is its turn's final reply exactly when the next projected message
/// is not another assistant message (nothing follows, or the user spoke
/// again). An append that continues a turn demotes the formerly-final reply,
/// so appends recheck the previous tail and the newly projected messages.
/// Replacements recompute the whole current conversation.
///
/// Projection ordinals are consecutive by construction (appends extend the
/// sequence; a replacement restarts it), so "next" is ordinal + 1.
fn recompute_turn_finals_conn(conn: &Connection, session_id: &str, from: i64) -> Result<()> {
    conn.execute(
        "UPDATE session_messages SET turn_final = 0
         WHERE session_id = ?1 AND turn_final != 0
           AND id IN (SELECT session_message_id FROM session_message_projection
                      WHERE session_id = ?1 AND ordinal >= ?2)",
        params![session_id, from],
    )?;
    conn.execute(
        "UPDATE session_messages SET turn_final = 1
         WHERE id IN (
           SELECT m.id
           FROM session_message_projection p
           JOIN session_messages m ON m.id = p.session_message_id
           LEFT JOIN session_message_projection n
             ON n.session_id = p.session_id AND n.ordinal = p.ordinal + 1
           LEFT JOIN session_messages nm ON nm.id = n.session_message_id
           WHERE p.session_id = ?1
             AND p.ordinal >= ?2
             AND m.role = 'assistant'
             AND (n.session_message_id IS NULL OR nm.role != 'assistant')
         )",
        params![session_id, from],
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
    let mut part_end = if start_ordinal == 0 {
        0
    } else {
        conn.query_row(
            "SELECT part_end FROM session_message_projection WHERE session_id=?1 AND ordinal=?2",
            params![session_id, start_ordinal],
            |r| r.get::<_, i64>(0),
        )?
    };
    {
        let mut ins = conn.prepare(
            "INSERT INTO session_message_projection (session_id, ordinal, session_message_id, part_end)
             VALUES (?1, ?2, ?3, ?4 + (SELECT MAX(1,(LENGTH(content)+1999)/2000)
                                     FROM session_messages WHERE id=?3 AND session_id=?1))
             RETURNING part_end",
        )?;
        for id in ids {
            ordinal += 1;
            part_end = ins.query_row(params![session_id, ordinal, id, part_end], |r| r.get(0))?;
        }
    }
    Ok(ordinal)
}

/// Maintain the current-message projection inside the ingest transaction.
///
/// * `current_ids` — the ordered message ids of THIS read, including messages
///   that deduped onto existing rows (they are still part of the conversation).
/// * `full_rescan` — the read started at byte 0.
/// The caller rejects incomplete full reads before inserting any facts.
pub fn apply_projection_conn(
    conn: &Connection,
    session_id: &str,
    current_ids: &[String],
    full_rescan: bool,
) -> Result<ProjectionOutcome> {
    let state = get_ingest_state_conn(conn, session_id)?;

    if !full_rescan {
        // Incremental and source-continuous: extend the current generation.
        if current_ids.is_empty() {
            return Ok(ProjectionOutcome::Unchanged);
        }
        let mut contains = conn.prepare(
            "SELECT EXISTS(SELECT 1 FROM session_message_projection
             WHERE session_id = ?1 AND session_message_id = ?2)",
        )?;
        let mut fresh = Vec::new();
        for id in current_ids {
            if !contains.query_row(params![session_id, id], |r| r.get::<_, bool>(0))? {
                fresh.push(id.clone());
            }
        }
        if fresh.is_empty() {
            return Ok(ProjectionOutcome::Unchanged);
        }
        let end = append_projection_conn(conn, session_id, &fresh, state.1)?;
        set_ingest_state_conn(conn, session_id, state.0, end)?;
        recompute_turn_finals_conn(conn, session_id, state.1.max(1))?;
        return Ok(ProjectionOutcome::Appended);
    }

    // A complete full re-scan.
    let existing = projection_ids_conn(conn, session_id)?;
    if existing == current_ids {
        return Ok(ProjectionOutcome::Unchanged);
    }
    if !existing.is_empty() && current_ids.starts_with(&existing[..]) {
        let fresh: Vec<String> = current_ids[existing.len()..].to_vec();
        let end = append_projection_conn(conn, session_id, &fresh, state.1)?;
        set_ingest_state_conn(conn, session_id, state.0, end)?;
        recompute_turn_finals_conn(conn, session_id, state.1.max(1))?;
        return Ok(ProjectionOutcome::Appended);
    }

    // The conversation itself changed (rewrite / truncate / reorder): raise the
    // fact generation and atomically replace the whole projection. The commit
    // then removes messages that are no longer part of this conversation.
    conn.execute(
        "DELETE FROM session_message_projection WHERE session_id = ?1",
        params![session_id],
    )?;
    let generation = state.0 + 1;
    let end = append_projection_conn(conn, session_id, current_ids, 0)?;
    set_ingest_state_conn(conn, session_id, generation, end)?;
    recompute_turn_finals_conn(conn, session_id, 1)?;
    Ok(ProjectionOutcome::NewGeneration)
}

// Session Context

pub fn get_session_context_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionContextRecord>> {
    Ok(conn
        .query_row(
            "SELECT session_id, summary,
                    revision, ingest_generation, processed_through_seq, updated_at
             FROM session_contexts WHERE session_id = ?1",
            params![session_id],
            |r| {
                Ok(SessionContextRecord {
                    session_id: r.get(0)?,
                    fields: SessionContextFields { summary: r.get(1)? },
                    revision: r.get(2)?,
                    ingest_generation: r.get(3)?,
                    processed_through_seq: r.get(4)?,
                    updated_at: r.get(5)?,
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
           (session_id, summary,
            revision, ingest_generation, processed_through_seq, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(session_id) DO UPDATE SET
           summary = ?2, revision = ?3, ingest_generation = ?4,
           processed_through_seq = ?5, updated_at = ?6",
        params![
            session_id,
            fields.summary,
            revision,
            ingest_generation,
            processed_through_seq,
            updated_at,
        ],
    )?;
    Ok(revision)
}

/// Persist the current task summary after the caller validates its snapshot.
pub fn commit_workstream_summary_conn(
    conn: &Connection,
    workstream_id: &str,
    summary: &crate::domain::Summary,
) -> Result<i64> {
    ensure_workstream_state_conn(conn, workstream_id)?;
    conn.execute("UPDATE workstream_context_state SET summary = ?2, updated_at = ?3, refreshed_at = ?3,
        context_revision = context_revision + 1, consumed_input_revision = input_revision WHERE workstream_id = ?1",
        params![workstream_id, summary, now()])?;

    Ok(get_workstream_context_state_conn(conn, workstream_id)?.context_revision)
}

// Workstream Context revisions and per-Session frontiers

pub fn get_workstream_context_state_conn(
    conn: &Connection,
    workstream_id: &str,
) -> Result<WorkstreamContextState> {
    Ok(conn
        .query_row(
            "SELECT workstream_id, context_revision, input_revision, consumed_input_revision, summary, updated_at
             FROM workstream_context_state WHERE workstream_id = ?1",
            params![workstream_id],
            |r| {
                Ok(WorkstreamContextState {
                    workstream_id: r.get(0)?,
                    context_revision: r.get(1)?,
                    input_revision: r.get(2)?,
                    consumed_input_revision: r.get(3)?,
                    summary: r.get(4)?,
                    updated_at: r.get(5)?,
                })
            },
        )
        .optional()?
        .unwrap_or(WorkstreamContextState {
            workstream_id: workstream_id.to_string(),
            context_revision: 0,
            input_revision: 0,
            consumed_input_revision: 0,
            summary: Default::default(),
            updated_at: None,
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

/// Bump `input_revision` — called by paths that change what the Workstream
/// has pending inputs: tracking set, projects, title /
/// description change or Session ownership change.
pub fn bump_input_revision_conn(conn: &Connection, workstream_id: &str) -> Result<i64> {
    ensure_workstream_state_conn(conn, workstream_id)?;
    conn.execute(
        "UPDATE workstream_context_state SET input_revision = input_revision + 1
         WHERE workstream_id = ?1",
        params![workstream_id],
    )?;
    Ok(get_workstream_context_state_conn(conn, workstream_id)?.input_revision)
}

pub fn set_workstream_frontier_conn(
    conn: &Connection,
    f: &WorkstreamSessionFrontier,
) -> Result<()> {
    conn.execute(
        "INSERT INTO workstream_session_frontiers
           (workstream_id, session_id, session_context_revision, ingest_generation, processed_through_seq)
         VALUES (?1, ?2, ?3,
          (SELECT ingest_generation FROM session_contexts WHERE session_id=?2 AND revision=?3),
          (SELECT processed_through_seq FROM session_contexts WHERE session_id=?2 AND revision=?3))
         ON CONFLICT(workstream_id, session_id) DO UPDATE SET
           session_context_revision = ?3, ingest_generation=excluded.ingest_generation, processed_through_seq=excluded.processed_through_seq",
        params![f.workstream_id, f.session_id, f.session_context_revision,],
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
               AND EXISTS (SELECT 1 FROM session_workstreams sw WHERE sw.session_id=s.id AND sw.workstream_id=?1)
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
        "SELECT workstream_id, session_id, session_context_revision
         FROM workstream_session_frontiers WHERE workstream_id = ?1",
    )?;
    let rows = st
        .query_map(params![workstream_id], |r| {
            Ok(WorkstreamSessionFrontier {
                workstream_id: r.get(0)?,
                session_id: r.get(1)?,
                session_context_revision: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

// Db-level wrappers

impl Db {
    /// The session's fact frontier: `(fact_generation, latest_message_seq)`.
    pub fn get_session_ingest_state(&self, session_id: &str) -> Result<(i64, i64)> {
        let conn = self.read();
        get_ingest_state_conn(&conn, session_id)
    }

    pub fn get_session_context(&self, session_id: &str) -> Result<Option<SessionContextRecord>> {
        let conn = self.read();
        get_session_context_conn(&conn, session_id)
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
