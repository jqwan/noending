//! Explicit offline format 12 -> 13 conversion, never called by app startup.
use noending_lib::{
    error::{other, Result},
    storage::{new_id, schema, DATABASE_APPLICATION_ID, DATABASE_FORMAT_VERSION},
};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const FROM: i64 = 12;
const TO: i64 = 13;
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn scalar(conn: &Connection, sql: &str) -> Result<i64> {
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
}
fn tables(conn: &Connection, db: &str) -> Result<BTreeSet<String>> {
    let mut st = conn.prepare(&format!("PRAGMA {db}.table_list"))?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    let mut out = BTreeSet::new();
    for row in rows {
        let (name, kind) = row?;
        if matches!(kind.as_str(), "table" | "virtual") && !name.starts_with("sqlite_") {
            out.insert(name);
        }
    }
    Ok(out)
}
fn columns(conn: &Connection, db: &str, table: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(&format!("PRAGMA {db}.table_info({})", quote(table)))?;
    let rows = st.query_map([], |r| r.get(1))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}
fn checked(conn: &Connection) -> Result<()> {
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(other(format!("数据库完整性检查失败：{integrity}")));
    }
    if conn.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(other("数据库存在外键损坏"));
    }
    Ok(())
}
fn build(backup: &Path, candidate: &Path) -> Result<serde_json::Value> {
    std::fs::copy(backup, candidate)?;
    let conn = Connection::open_with_flags(candidate, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    // Check the exact supported schema delta before modifying the copy.
    let expected = Connection::open_in_memory()?;
    schema::open_or_create(&expected)?;
    if tables(&conn, "main")? != tables(&expected, "main")? {
        return Err(other("格式 12 的表集合不符合预期"));
    }
    for table in tables(&expected, "main")? {
        if columns(&conn, "main", &table)? != columns(&expected, "main", &table)? {
            return Err(other(format!("旧表 {table} 列结构不符合预期")));
        }
    }
    conn.execute(
        "ATTACH DATABASE ?1 AS old",
        [backup.to_string_lossy().as_ref()],
    )?;
    // Recreate only summary bodies from the current schema; all other tables
    // and IDs remain untouched. No old Markdown is parsed or retained.
    conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("DELETE FROM workstream_session_frontiers; DELETE FROM project_session_frontiers;
        DROP TABLE session_contexts; DROP TABLE workstream_context_state; DROP TABLE project_context_state;")?;
    for table in [
        "session_contexts",
        "workstream_context_state",
        "project_context_state",
    ] {
        let ddl: String = expected.query_row(
            "SELECT sql FROM sqlite_master WHERE name=?1",
            [table],
            |r| r.get(0),
        )?;
        tx.execute_batch(&ddl)?;
    }
    // Retain revision guards and mark task/project inputs pending, but reset
    // all summary consumption. Sessions start extraction at their first message.
    tx.execute_batch("INSERT INTO workstream_context_state(workstream_id,context_revision,input_revision,consumed_input_revision)
        SELECT w.id,COALESCE(c.context_revision,0)+1,COALESCE(c.input_revision,0)+1,0
        FROM workstreams w LEFT JOIN old.workstream_context_state c ON c.workstream_id=w.id;
        INSERT INTO project_context_state(project_id,context_revision,input_fingerprint)
        SELECT project_id,context_revision+1,'' FROM old.project_context_state;")?;
    tx.pragma_update(None, "user_version", TO)?;
    tx.commit()?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    schema::open_or_create(&conn)?;
    // Check every non-summary table in both directions, including FTS data.
    for table in tables(&conn, "main")? {
        if matches!(
            table.as_str(),
            "session_contexts"
                | "workstream_context_state"
                | "project_context_state"
                | "workstream_session_frontiers"
                | "project_session_frontiers"
        ) {
            continue;
        }
        let cols = columns(&conn, "main", &table)?
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(",");
        let main = format!("SELECT {cols} FROM main.{}", quote(&table));
        let old = format!("SELECT {cols} FROM old.{}", quote(&table));
        if scalar(
            &conn,
            &format!("SELECT EXISTS({main} EXCEPT {old}) OR EXISTS({old} EXCEPT {main})"),
        )? != 0
            || scalar(
                &conn,
                &format!("SELECT COUNT(*) FROM main.{}", quote(&table)),
            )? != scalar(
                &conn,
                &format!("SELECT COUNT(*) FROM old.{}", quote(&table)),
            )?
        {
            return Err(other(format!("数据核对失败：{table}")));
        }
    }
    checked(&conn)?;
    conn.execute(
        "INSERT INTO search_index(search_index,rank) VALUES('integrity-check',1)",
        [],
    )?;
    let counts = json!({
        "sessions": scalar(&conn, "SELECT COUNT(*) FROM sessions")?,
        "messages": scalar(&conn, "SELECT COUNT(*) FROM session_messages")?,
        "sources": scalar(&conn, "SELECT COUNT(*) FROM ingest_sources")?,
        "projects": scalar(&conn, "SELECT COUNT(*) FROM projects")?,
        "tasks": scalar(&conn, "SELECT COUNT(*) FROM workstreams")?,
        "settings": scalar(&conn, "SELECT COUNT(*) FROM settings")?
    });
    conn.execute_batch("DETACH DATABASE old; PRAGMA journal_mode=DELETE;")?;
    conn.close().map_err(|(_, e)| e)?;
    Ok(counts)
}
fn sync_file(path: &Path) -> Result<()> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(())
}
fn migrate(path: &Path, backup_dir: &Path, apply: bool) -> Result<serde_json::Value> {
    if DATABASE_FORMAT_VERSION != TO {
        return Err(other("一次性工具仅适用于目标格式 13"));
    }
    let path = path.canonicalize()?;
    let source = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let app = scalar(&source, "PRAGMA application_id")?;
    let version = scalar(&source, "PRAGMA user_version")?;
    if app != i64::from(DATABASE_APPLICATION_ID) || version != FROM {
        return Err(other(format!(
            "只接受 NoEnding 格式 12，实际 application_id={app}, version={version}"
        )));
    }
    source.busy_timeout(std::time::Duration::from_secs(1))?;
    // Refuse a live app or reader; retain the exclusive lock while preparing.
    source.execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; COMMIT;")?;
    checked(&source)?;
    std::fs::create_dir_all(backup_dir)?;
    let token = new_id();
    let backup = backup_dir.join(format!("noending-format-12-{token}.db"));
    source.execute("VACUUM main INTO ?1", [backup.to_string_lossy().as_ref()])?;
    let permissions = std::fs::metadata(&path)?.permissions();
    std::fs::set_permissions(&backup, permissions.clone())?;
    sync_file(&backup)?;
    let candidate = path.with_file_name(format!("noending-format-13-{token}.prepared.db"));
    let counts = build(&backup, &candidate)?;
    std::fs::set_permissions(&candidate, permissions)?;
    sync_file(&candidate)?;
    if apply {
        // Close SQLite handles before replacement, including on Windows.
        // NoEnding must remain closed until replacement and verification finish.
        source.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
        source.close().map_err(|(_, e)| e)?;
        std::fs::rename(&candidate, &path)?;
        let final_conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        schema::open_or_create(&final_conn)?;
        checked(&final_conn)?;
    }
    Ok(
        json!({"applied":apply,"from":FROM,"to":TO,"database":path,"backup":backup,"prepared":if apply {None} else {Some(candidate)},"counts":counts}),
    )
}
fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut database, mut backups) = (None, None);
    let mut apply = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--database" => database = args.next().map(PathBuf::from),
            "--backup-dir" => backups = args.next().map(PathBuf::from),
            "--apply" => apply = true,
            _ => return Err(other(
                "用法：migrate_v12 --database PATH --backup-dir DIR [--apply]；请先退出 NoEnding",
            )),
        }
    }
    let result = migrate(
        &database.ok_or_else(|| other("缺少 --database"))?,
        &backups.ok_or_else(|| other("缺少 --backup-dir"))?,
        apply,
    )?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("迁移未完成：{e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("noending-v12-migration-{}", new_id()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn db(&self) -> PathBuf {
            self.0.join("source.db")
        }
        fn backups(&self) -> PathBuf {
            self.0.join("backups")
        }
        fn legacy(&self) {
            let conn = Connection::open(self.db()).unwrap();
            conn.execute_batch(include_str!("../tests/fixtures/database-v12.sql"))
                .unwrap();
            conn.pragma_update(None, "application_id", DATABASE_APPLICATION_ID)
                .unwrap();
            conn.execute_batch("INSERT INTO projects VALUES('p','Custom project',NULL,1,'normal','a','b');
                INSERT INTO workstreams VALUES('w','Task','Description','normal','a','b');
                INSERT INTO sessions(id,agent,root_agent_session_id,title,source_kind,source_path,latest_message_seq) VALUES('s','codex','native','Title','fixture','/absent',1);
                INSERT INTO session_messages(id,session_id,sequence,source_identity_hash,role,content) VALUES('m','s',1,'hash','user','保留消息');
                INSERT INTO session_message_projection VALUES('s',1,'m',1);
                INSERT INTO session_workstreams VALUES('s','w');
                INSERT INTO session_contexts VALUES('s','old markdown',5,3,1,'now');
                INSERT INTO workstream_context_state VALUES('w',8,4,4,'old task markdown','now');
                INSERT INTO project_context_state VALUES('p',9,'fingerprint','old project markdown','now');
                INSERT INTO workstream_session_frontiers VALUES('w','s',5);
                INSERT INTO project_session_frontiers VALUES('p','s',5);
                INSERT INTO preferences VALUES('pref','p','Keep preference',1,6);
                INSERT INTO settings VALUES('pinned_sessions','[\"s\"]');
                INSERT INTO search_documents(kind,ref_id,parent_id,title) VALUES('session','s','','Title'),('message','m','s','');
                PRAGMA journal_mode=WAL;").unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn migration_preserves_all_non_summary_data_and_resets_summary_consumption() {
        let f = Fixture::new();
        f.legacy();
        let result = migrate(&f.db(), &f.backups(), true).unwrap();
        let backup = Connection::open(result["backup"].as_str().unwrap()).unwrap();
        assert_eq!(scalar(&backup, "PRAGMA user_version").unwrap(), 12);
        assert_eq!(
            backup
                .query_row("SELECT summary FROM session_contexts", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "old markdown"
        );
        let db = noending_lib::storage::Db::open(&f.db()).unwrap();
        assert_eq!(
            db.get_messages("s", None, 10).unwrap()[0].content,
            "保留消息"
        );
        assert_eq!(
            db.get_setting("pinned_sessions").unwrap().as_deref(),
            Some("[\"s\"]")
        );
        let session = noending_lib::context::session_context_view(&db, "s").unwrap();
        assert!(session.fields.is_none());
        assert!(session.pending);
        assert_eq!(session.processed_through_seq, 0);
        let task = noending_lib::context::workstream_context_view(&db, "w").unwrap();
        assert!(task.summary.is_none());
        assert!(task.pending);
        assert_eq!(task.context_revision, 9);
        let conn = db.read();
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM project_session_frontiers").unwrap(),
            0
        );
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM workstream_session_frontiers").unwrap(),
            0
        );
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM preferences").unwrap(),
            1
        );
        // Current constraints reject a return to unstructured summary storage.
        assert!(conn
            .execute("UPDATE workstream_context_state SET summary='old text'", [])
            .is_err());
    }
    #[test]
    fn prepare_keeps_the_original_untouched_and_rejects_repeated_upgrade() {
        let f = Fixture::new();
        f.legacy();
        let result = migrate(&f.db(), &f.backups(), false).unwrap();
        assert_eq!(
            scalar(&Connection::open(f.db()).unwrap(), "PRAGMA user_version").unwrap(),
            12
        );
        let prepared = PathBuf::from(result["prepared"].as_str().unwrap());
        assert!(noending_lib::storage::Db::open(&prepared).is_ok());
        assert!(migrate(&prepared, &f.backups(), true).is_err());
    }
}
