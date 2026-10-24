//! Reconcile physical Codex rollout versions with their canonical thread.
//! Only identities verified against the discovered source descriptors qualify.
use super::*;

fn versions(metadata: &serde_json::Value) -> impl Iterator<Item = (&str, &str)> {
    metadata["rollout_versions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| Some((v["id"].as_str()?, v["path"].as_str()?)))
}
fn same_source(a: &str, b: &str) -> bool {
    let name = |p: &str| {
        std::path::Path::new(p)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.strip_suffix(".zst").unwrap_or(s).to_owned())
    };
    a.strip_suffix(".zst").unwrap_or(a) == b.strip_suffix(".zst").unwrap_or(b)
        || name(a).is_some() && name(a) == name(b)
}
fn aliases(conn: &Connection, native: &str, metadata: &serde_json::Value) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for (rollout, path) in versions(metadata).filter(|(id, _)| *id != native) {
        let found: Option<(String, String)> = conn
            .query_row(
                "SELECT id,source_path FROM sessions
             WHERE agent='codex' AND source_kind='codex_rollout' AND root_agent_session_id=?1",
                [rollout],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, _)) = found.filter(|(_, source)| same_source(source, path)) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}
impl Db {
    /// If the real thread has not been stored yet, preserve an existing app ID
    /// by correcting its source identity before normal discovery upserts it.
    pub(crate) fn canonicalize_codex_identity(
        &self,
        native: &str,
        metadata: &serde_json::Value,
    ) -> Result<()> {
        self.tx(|tx| {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE agent='codex' AND root_agent_session_id=?1)",
                [native],
                |r| r.get(0),
            )?;
            if exists {
                return Ok(());
            }
            if let Some(id) = aliases(tx, native, metadata)?.first() {
                tx.execute(
                    "UPDATE sessions SET root_agent_session_id=?2,forked_from_session_id=NULL WHERE id=?1",
                    params![id, native],
                )?;
            }
            Ok(())
        })
    }
    /// Run only after a successful ingest of the verified effective history.
    /// Derived messages are rebuilt from source; app-owned content is retained.
    pub(crate) fn reconcile_codex_rollouts(&self, session: &Session) -> Result<()> {
        if session.agent != Agent::Codex {
            return Ok(());
        }
        self.tx(|tx| {
            let duplicates = aliases(tx, &session.root_agent_session_id, &session.metadata)?;
            if duplicates.is_empty() {
                return Ok(());
            }
            let mut combined: Option<(Summary, i64)> = tx.query_row(
                "SELECT summary,revision FROM session_contexts WHERE session_id=?1",
                [&session.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?;
            let mut summary_changed = false;
            for id in duplicates {
                if id == session.id {
                    continue;
                }
                tx.execute(
                    "INSERT OR IGNORE INTO session_workstreams(session_id,workstream_id)
                     SELECT ?1,workstream_id FROM session_workstreams WHERE session_id=?2",
                    params![session.id, id],
                )?;
                let summary: Option<(Summary, i64)> = tx.query_row(
                    "SELECT summary,revision FROM session_contexts WHERE session_id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                ).optional()?;
                if let Some((summary, revision)) = summary {
                    let (target, version) = combined.get_or_insert_with(|| (Summary::default(), 0));
                    *version = (*version).max(revision);
                    for item in summary.items {
                        if !target.items.contains(&item) {
                            target.items.push(item);
                        }
                    }
                    summary_changed = true;
                }
                // Genuine forks of a historical source version still belong
                // to this thread, not to the absorbed physical-file record.
                tx.execute(
                    "UPDATE sessions SET forked_from_session_id=?1 WHERE forked_from_session_id=?2 AND id!=?1",
                    params![session.id, id],
                )?;
                session_lifecycle::purge_session_data_conn(tx, &id)?;
            }
            if summary_changed {
                if let Some((summary, revision)) = combined {
                    tx.execute(
                        "INSERT INTO session_contexts(
                            session_id,summary,revision,ingest_generation,processed_through_seq,updated_at,refreshed_at
                         ) VALUES(?1,?2,?3,?4,0,?5,NULL)
                         ON CONFLICT(session_id) DO UPDATE SET
                            summary=excluded.summary,revision=excluded.revision,
                            ingest_generation=excluded.ingest_generation,processed_through_seq=0,
                            updated_at=excluded.updated_at,refreshed_at=NULL",
                        params![session.id, summary, revision + 1, session.fact_generation, now()],
                    )?;
                }
            }
            index_session_conn(tx, &session.id)?;
            Ok(())
        })
    }
}
