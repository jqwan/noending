pub fn seed_summary(
    db: &noending_lib::storage::Db,
    kind: &str,
    id: &str,
    revision: i64,
    summary: noending_lib::domain::Summary,
) -> noending_lib::error::Result<noending_lib::domain::Summary> {
    let summary = summary.resolve_ids()?;
    let generation = if kind == "session" {
        db.get_session_ingest_state(id)?.0
    } else {
        0
    };
    let conn = db.write();
    match kind {
        "session" => {
            conn.execute("INSERT INTO session_contexts(session_id,summary,revision,ingest_generation,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(session_id) DO UPDATE SET summary=?2,revision=?3",rusqlite::params![id,summary,revision,generation,noending_lib::storage::now()])?;
        }
        "task" => {
            conn.execute("INSERT INTO workstream_context_state(workstream_id,summary,context_revision) VALUES(?1,?2,?3) ON CONFLICT(workstream_id) DO UPDATE SET summary=?2,context_revision=?3",rusqlite::params![id,summary,revision])?;
        }
        "project" => {
            conn.execute("INSERT INTO project_context_state(project_id,summary,context_revision) VALUES(?1,?2,?3) ON CONFLICT(project_id) DO UPDATE SET summary=?2,context_revision=?3",rusqlite::params![id,summary,revision])?;
        }
        _ => panic!("invalid fixture kind"),
    }
    Ok(summary)
}
