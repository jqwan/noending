//! Read evidence for associating a newly launched terminal with an ingested root.

use rusqlite::params;

use crate::domain::Agent;
use crate::error::Result;

use super::Db;

#[derive(Debug, Clone)]
pub(crate) struct TerminalBindingCandidate {
    pub session_id: String,
    pub agent: Agent,
    pub source_kind: String,
    pub cwd: Option<String>,
    pub started_at: Option<String>,
    pub first_user_message: Option<String>,
}

impl Db {
    /// All active roots that may belong to an unbound launch for this Agent.
    /// The first user message comes from the CURRENT conversation projection,
    /// in full: title previews and retired source generations are not evidence.
    ///
    /// No candidate cap: omitting an otherwise matching root would turn an
    /// ambiguous association into a seemingly unique one. Missing/unparseable
    /// timestamps stay in the pool; the matcher decides whether they establish
    /// enough evidence. SQLite's date comparison handles RFC3339 offsets; the
    /// matcher still checks the exact timestamp for each terminal.
    pub(crate) fn terminal_binding_candidates(
        &self,
        agent: Agent,
        earliest_started_at: &str,
    ) -> Result<Vec<TerminalBindingCandidate>> {
        let conn = self.read();
        let mut st = conn.prepare(
            "SELECT s.id, s.source_kind, s.cwd, s.started_at,
                    (SELECT m.content
                       FROM session_message_projection p
                       JOIN session_messages m ON m.id = p.session_message_id
                      WHERE p.session_id = s.id AND m.role = 'user'
                      ORDER BY p.ordinal LIMIT 1) AS first_user_message
               FROM sessions s
              WHERE s.agent = ?1 AND s.trashed_at IS NULL
                AND (julianday(s.started_at) IS NULL
                     OR julianday(s.started_at) >= julianday(?2))
              ORDER BY s.id",
        )?;
        let rows = st
            .query_map(params![agent.as_str(), earliest_started_at], |r| {
                Ok(TerminalBindingCandidate {
                    session_id: r.get(0)?,
                    agent,
                    source_kind: r.get(1)?,
                    cwd: r.get(2)?,
                    started_at: r.get(3)?,
                    first_user_message: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{ParsedSessionMessage, SessionMessageRole, SourceCursorUpdate};
    use crate::storage::new_id;

    use super::*;

    const START: &str = "2026-10-07T12:00:00Z";

    fn db() -> Db {
        let dir = std::env::temp_dir().join(format!("noending-terminal-binding-{}", new_id()));
        Db::open(&dir.join("test.db")).unwrap()
    }

    fn session(db: &Db, agent: Agent, root: &str, started_at: Option<&str>) -> String {
        db.upsert_logical_session(
            agent,
            root,
            None,
            None,
            Some("/repo"),
            None,
            None,
            started_at,
            started_at,
            "test_root",
            "/tmp/source",
            &serde_json::json!({}),
        )
        .unwrap()
        .0
    }

    fn message(id: &str, role: SessionMessageRole, content: &str) -> ParsedSessionMessage {
        ParsedSessionMessage {
            source_message_id: Some(id.into()),
            source_position: id.into(),
            ts: None,
            role,
            content: content.into(),
        }
    }

    fn source(generation: i64) -> SourceCursorUpdate {
        SourceCursorUpdate {
            file_identity: "test-source".into(),
            generation,
            byte_offset: 100,
            last_seen_size: 100,
            mtime: None,
            start_byte_offset: 0,
            prefix_hash: String::new(),
        }
    }

    #[test]
    fn terminal_binding_candidates_keep_the_full_first_projected_user_message() {
        let db = db();
        let id = session(&db, Agent::Codex, "root", Some(START));
        let prompt = format!("{}\n最后一行不能被截断", "完整的首条消息".repeat(100));
        db.commit_ingest(
            &id,
            &[
                message("assistant", SessionMessageRole::Assistant, "欢迎"),
                message("first-user", SessionMessageRole::User, &prompt),
                message("second-user", SessionMessageRole::User, "第二条消息"),
            ],
            &source(0),
        )
        .unwrap();

        let candidates = db.terminal_binding_candidates(Agent::Codex, START).unwrap();
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.session_id, id);
        assert_eq!(candidate.agent, Agent::Codex);
        assert_eq!(candidate.source_kind, "test_root");
        assert_eq!(candidate.cwd.as_deref(), Some("/repo"));
        assert_eq!(candidate.started_at.as_deref(), Some(START));
        assert_eq!(
            candidate.first_user_message.as_deref(),
            Some(prompt.as_str())
        );
    }

    #[test]
    fn terminal_binding_candidates_ignore_retired_user_messages_after_rewrite() {
        let db = db();
        let id = session(&db, Agent::Codex, "root", Some(START));
        db.commit_ingest(
            &id,
            &[message("old", SessionMessageRole::User, "旧提示词")],
            &source(0),
        )
        .unwrap();
        db.commit_ingest(
            &id,
            &[
                message("assistant", SessionMessageRole::Assistant, "前言"),
                message("new", SessionMessageRole::User, "新提示词"),
            ],
            &source(1),
        )
        .unwrap();

        let stored_count: i64 = db
            .read()
            .query_row(
                "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stored_count, 3,
            "retired messages remain in the audit store"
        );
        let candidates = db.terminal_binding_candidates(Agent::Codex, START).unwrap();
        assert_eq!(
            candidates[0].first_user_message.as_deref(),
            Some("新提示词")
        );
    }

    #[test]
    fn terminal_binding_candidates_filter_agent_time_and_trash_without_hiding_unknowns() {
        let db = db();
        session(&db, Agent::Codex, "old", Some("2026-10-07T11:59:59Z"));
        session(&db, Agent::Pi, "other-agent", Some(START));
        let trash = session(&db, Agent::Codex, "trash", Some(START));
        db.write()
            .execute(
                "UPDATE sessions SET trashed_at = ?2 WHERE id = ?1",
                params![trash, START],
            )
            .unwrap();
        let offset = session(
            &db,
            Agent::Codex,
            "offset",
            Some("2026-10-07T07:00:01-05:00"),
        );
        let missing = session(&db, Agent::Codex, "missing-time", None);
        let invalid = session(&db, Agent::Codex, "invalid-time", Some("invalid"));

        let candidates = db.terminal_binding_candidates(Agent::Codex, START).unwrap();
        assert_eq!(candidates.len(), 3);
        for id in [offset, missing, invalid] {
            assert!(candidates
                .iter()
                .any(|candidate| candidate.session_id == id));
        }
        assert!(candidates
            .iter()
            .all(|candidate| candidate.first_user_message.is_none()));
    }

    #[test]
    fn terminal_binding_candidates_have_no_candidate_count_cap() {
        let db = db();
        for index in 0..130 {
            session(&db, Agent::Codex, &format!("root-{index}"), Some(START));
        }
        assert_eq!(
            db.terminal_binding_candidates(Agent::Codex, START)
                .unwrap()
                .len(),
            130,
        );
    }
}
