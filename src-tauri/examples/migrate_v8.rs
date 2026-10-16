//! Explicit, offline, one-time format 8 -> 11 conversion. Not a startup path.
//! Uses the application's current schema and search projection writers.
use noending_lib::{
    error::{other, Result},
    storage::{new_id, schema, Db, DATABASE_APPLICATION_ID, DATABASE_FORMAT_VERSION},
};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const FROM: i64 = 8;
const TO: i64 = 11;
const RETIRED: [&str; 2] = ["assistant_sessions", "assistant_messages"];

fn tables(conn: &Connection, schema: &str) -> Result<BTreeSet<String>> {
    let mut st = conn.prepare(&format!("PRAGMA {schema}.table_list"))?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    let mut result = BTreeSet::new();
    for row in rows {
        let (name, kind) = row?;
        if matches!(kind.as_str(), "table" | "virtual") && !name.starts_with("sqlite_") {
            result.insert(name);
        }
    }
    Ok(result)
}
fn columns(conn: &Connection, schema: &str, table: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(&format!("PRAGMA {schema}.table_info({})", quote(table)))?;
    let rows = st.query_map([], |r| r.get(1))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn scalar(conn: &Connection, sql: &str) -> Result<i64> {
    Ok(conn.query_row(sql, [], |r| r.get(0))?)
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
fn equal(conn: &Connection, table: &str, cols: &str, source_condition: &str) -> Result<()> {
    let left = format!("SELECT {cols} FROM main.{}", quote(table));
    let right = format!("SELECT {cols} FROM old.{} {source_condition}", quote(table));
    if scalar(
        conn,
        &format!("SELECT EXISTS({left} EXCEPT {right}) OR EXISTS({right} EXCEPT {left})"),
    )? != 0
    {
        return Err(other(format!("数据核对失败：{table}")));
    }
    Ok(())
}

fn build(backup: &Path, candidate: &Path) -> Result<(i64, i64, i64)> {
    let db = Db::open(candidate)?;
    {
        let conn = db.write();
        conn.execute(
            "ATTACH DATABASE ?1 AS old",
            [backup.to_string_lossy().as_ref()],
        )?;
        let target = tables(&conn, "main")?;
        let mut expected = target.clone();
        expected.remove("search_documents");
        expected.extend(RETIRED.map(str::to_owned));
        if tables(&conn, "old")? != expected {
            return Err(other("格式 8 的表集合不符合预期，拒绝丢弃未知数据"));
        }
        for table in RETIRED {
            if scalar(&conn, &format!("SELECT COUNT(*) FROM old.{table}"))? != 0 {
                return Err(other(format!("旧表 {table} 含数据，需单独保留后再迁移")));
            }
        }
        let tx = conn.unchecked_transaction()?;
        // Copy cyclic references in one transaction, checking them before commit.
        tx.execute_batch("PRAGMA defer_foreign_keys=ON; DELETE FROM ingest_sources;")?;
        for table in &target {
            if matches!(
                table.as_str(),
                "search_index" | "search_documents" | "session_message_projection"
            ) {
                continue;
            }
            let cols = columns(&tx, "main", table)?;
            if columns(&tx, "old", table)? != cols {
                return Err(other(format!("旧表 {table} 列结构不符合预期")));
            }
            let cols = cols.iter().map(|c| quote(c)).collect::<Vec<_>>().join(",");
            let condition = if table == "session_messages" {
                "WHERE id IN (SELECT session_message_id FROM old.session_message_projection)"
            } else {
                ""
            };
            tx.execute(
                &format!(
                    "INSERT INTO main.{}({cols}) SELECT {cols} FROM old.{} {condition}",
                    quote(table),
                    quote(table)
                ),
                [],
            )?;
            equal(&tx, table, &cols, condition)?;
        }
        if columns(&tx, "old", "session_message_projection")?
            != ["session_id", "ordinal", "session_message_id"]
        {
            return Err(other("旧消息投影结构不符合格式 8"));
        }
        tx.execute("INSERT INTO session_message_projection(session_id,ordinal,session_message_id,part_end)
            SELECT p.session_id,p.ordinal,p.session_message_id,
              SUM(MAX(1,(LENGTH(m.content)+1999)/2000)) OVER (PARTITION BY p.session_id ORDER BY p.ordinal ROWS UNBOUNDED PRECEDING)
            FROM old.session_message_projection p JOIN session_messages m ON m.id=p.session_message_id AND m.session_id=p.session_id",[])?;
        equal(
            &tx,
            "session_message_projection",
            "session_id,ordinal,session_message_id",
            "",
        )?;
        // The current reader relies on consecutive ordinals and an accurate tail.
        if scalar(&tx,"SELECT COUNT(*) FROM sessions s WHERE latest_message_seq != COALESCE((SELECT MAX(ordinal) FROM session_message_projection p WHERE p.session_id=s.id),0) OR latest_message_seq != (SELECT COUNT(*) FROM session_message_projection p WHERE p.session_id=s.id)")?!=0 {
            return Err(other("会话消息投影不连续或尾部标记不一致"));
        }
        if tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
            return Err(other("迁移后的外键检查失败"));
        }
        tx.commit()?;
    }
    // Reuse the actual production writers; no legacy summary index is copied.
    db.backfill_search_index()?;
    let counts = {
        let conn = db.read();
        schema::open_or_create(&conn)?;
        checked(&conn)?;
        let sessions = scalar(&conn, "SELECT COUNT(*) FROM sessions")?;
        let messages = scalar(&conn, "SELECT COUNT(*) FROM session_messages")?;
        if scalar(
            &conn,
            "SELECT COUNT(*) FROM search_documents WHERE kind='session'",
        )? != sessions
            || scalar(
                &conn,
                "SELECT COUNT(*) FROM search_documents WHERE kind='message'",
            )? != messages
        {
            return Err(other("搜索文档数量核对失败"));
        }
        (sessions, messages)
    };
    {
        let conn = db.write();
        conn.execute(
            "INSERT INTO search_index(search_index,rank) VALUES('integrity-check',1)",
            [],
        )?;
        conn.execute_batch("DETACH DATABASE old;")?;
    }
    drop(db);
    // Journal mode changes require every WAL reader to be closed first.
    let prepared = Connection::open_with_flags(candidate, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    prepared.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
    prepared.close().map_err(|(_, e)| e)?;
    let old = Connection::open_with_flags(backup, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    checked(&old)?;
    Ok((
        counts.0,
        counts.1,
        scalar(&old, "SELECT COUNT(*) FROM session_messages")? - counts.1,
    ))
}

fn migrate(path: &Path, backup_dir: &Path, apply: bool) -> Result<serde_json::Value> {
    if DATABASE_FORMAT_VERSION != TO {
        return Err(other("一次性工具仅针对格式 11，不能用于后续格式"));
    }
    let path = path.canonicalize()?;
    let source = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let application: i32 = source.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = source.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if application != DATABASE_APPLICATION_ID || version != FROM {
        return Err(other(format!(
            "只接受 NoEnding 格式 8 数据库，实际 application_id={application}, version={version}"
        )));
    }
    source.busy_timeout(std::time::Duration::from_secs(1))?;
    // Retain an exclusive SQLite lock while validating and preparing the copy.
    // A running app/reader makes this fail rather than migrating live data.
    source.execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; COMMIT;")?;
    checked(&source)?;
    std::fs::create_dir_all(backup_dir)?;
    let token = new_id();
    let backup = backup_dir.join(format!("noending-format-8-{token}.db"));
    source.execute("VACUUM main INTO ?1", [backup.to_string_lossy().as_ref()])?;
    std::fs::set_permissions(&backup, std::fs::metadata(&path)?.permissions())?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&backup)?
        .sync_all()?;
    let candidate = path.with_file_name(format!("noending-format-11-{token}.prepared.db"));
    let (sessions, messages, retired_messages) = build(&backup, &candidate)?;
    std::fs::set_permissions(&candidate, std::fs::metadata(&path)?.permissions())?;
    if apply {
        // Checkpoint/remove old sidecars before replacing the main file. SQLite
        // must release handles before rename on Windows. Keep NoEnding closed.
        source.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
        source.close().map_err(|(_, e)| e)?;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&candidate)?
            .sync_all()?;
        std::fs::rename(&candidate, &path)?;
        let final_db = Db::open(&path)?;
        checked(&final_db.read())?;
    }
    Ok(
        json!({"applied":apply,"from":FROM,"to":TO,"database":path,"backup":backup,"prepared":if apply {None} else {Some(candidate)},"sessions":sessions,"current_messages":messages,"retired_messages":retired_messages}),
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
                "用法：migrate_v8 --database PATH --backup-dir DIR [--apply]；请先退出 NoEnding",
            )),
        }
    }
    let database = database.ok_or_else(|| other("缺少 --database"))?;
    let backups = backups.ok_or_else(|| other("缺少 --backup-dir"))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&migrate(&database, &backups, apply)?)?
    );
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
            let path = std::env::temp_dir().join(format!("noending-migrate-{}", new_id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn db(&self) -> PathBuf {
            self.0.join("source.db")
        }
        fn backups(&self) -> PathBuf {
            self.0.join("backups")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn legacy(f: &Fixture) {
        let conn = Connection::open(f.db()).unwrap();
        conn.execute_batch(include_str!("../tests/fixtures/database-v8.sql"))
            .unwrap();
        conn.pragma_update(None, "application_id", DATABASE_APPLICATION_ID)
            .unwrap();
        conn.pragma_update(None, "user_version", FROM).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;
          INSERT INTO projects VALUES('p','CustomProject',NULL,1,'normal','a','b');
          INSERT INTO workstreams VALUES('w','TrackedTask','User description','normal','a','b');
          INSERT INTO workspace_paths VALUES('wp','/work/main','p','none',NULL,1,'a','b');
          INSERT INTO workstream_projects VALUES('w','p');
          INSERT INTO sessions(id,agent,root_agent_session_id,title,cwd,workspace_path_id,source_kind,source_path,fact_generation,latest_message_seq) VALUES('s','codex','native','Session title','/work/main','wp','fixture','/history',3,2);
          INSERT INTO sessions(id,agent,root_agent_session_id,forked_from_session_id,archived_at,source_kind,source_path) VALUES('fork','codex','native-fork','s','archived','fixture','/fork');
          INSERT INTO session_messages(id,session_id,sequence,source_identity_hash,role,content) VALUES('empty','s',1,'hash1','user','');
          INSERT INTO session_messages(id,session_id,sequence,source_identity_hash,role,content) VALUES('live','s',2,'hash2','assistant','placeholder');
          INSERT INTO session_messages(id,session_id,sequence,source_identity_hash,role,content) VALUES('retired','s',3,'hash3','user','retiredposting');
          INSERT INTO session_message_projection VALUES('s',1,'live'),('s',2,'empty');
          INSERT INTO session_workstreams VALUES('s','w');
          INSERT INTO session_contexts VALUES('s','Session summary',5,3,2,'now');
          INSERT INTO workstream_context_state VALUES('w',7,9,9,'Task summary','now');
          INSERT INTO workstream_session_frontiers VALUES('w','s',5);
          INSERT INTO project_context_state VALUES('p',8,'fingerprint','Project summary','now');
          INSERT INTO project_session_frontiers VALUES('p','s',5);
          INSERT INTO preferences VALUES('pref','p','Keep this preference',1,6);
          INSERT INTO settings VALUES('pinned_sessions','[\"s\"]');
          INSERT INTO ingest_sources VALUES('source','codex','/history',1,'user','now');
          INSERT INTO search_index(kind,ref_id,parent_id,title,body) VALUES('message','retired','s','','retiredposting');
          INSERT INTO search_index(kind,ref_id,parent_id,title,body) VALUES('project','p','','CustomProject','Project summary');").unwrap();
        conn.execute(
            "UPDATE session_messages SET content=?1 WHERE id='live'",
            [format!("liveposting {}", "😀".repeat(2001))],
        )
        .unwrap();
    }
    #[test]
    fn apply_preserves_owned_data_and_current_message_identity_and_rebuilds_search() {
        let f = Fixture::new();
        legacy(&f);
        let result = migrate(&f.db(), &f.backups(), true).unwrap();
        assert_eq!(result["sessions"], 2);
        assert_eq!(result["current_messages"], 2);
        assert_eq!(result["retired_messages"], 1);
        let db = Db::open(&f.db()).unwrap();
        {
            let conn = db.read();
            assert_eq!(scalar(&conn, "PRAGMA user_version").unwrap(), TO);
            assert_eq!(scalar(&conn,"SELECT part_end FROM session_message_projection WHERE session_id='s' AND ordinal=1").unwrap(),2);
            assert_eq!(scalar(&conn,"SELECT part_end FROM session_message_projection WHERE session_id='s' AND ordinal=2").unwrap(),3);
            assert_eq!(
                scalar(
                    &conn,
                    "SELECT sequence FROM session_messages WHERE id='live'"
                )
                .unwrap(),
                2
            );
            assert_eq!(
                scalar(
                    &conn,
                    "SELECT COUNT(*) FROM session_messages WHERE id='retired'"
                )
                .unwrap(),
                0
            );
            let context:(String,i64,i64,i64)=conn.query_row("SELECT summary,revision,ingest_generation,processed_through_seq FROM session_contexts WHERE session_id='s'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
            assert_eq!(context, ("Session summary".into(), 5, 3, 2));
            let parts =
                noending_lib::storage::message_query::parts(&conn, "s", 0, 2, 0, 8, None).unwrap();
            assert_eq!(parts.len(), 3);
            assert_eq!(parts[2]["record"]["message_id"], "empty");
            assert_eq!(parts[2]["record"]["content"], "");
        }
        let filter = noending_lib::search::sessions::SessionFilter {
            scope: "all".into(),
            ..Default::default()
        };
        let hits = noending_lib::search::sessions::page(&db, "liveposting", &filter, 20, 0, None)
            .unwrap()
            .0;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].hit.ref_id, "live");
        assert_eq!(hits[0].sequence, Some(1));
        assert!(
            noending_lib::search::sessions::page(&db, "retiredposting", &filter, 20, 0, None)
                .unwrap()
                .0
                .is_empty()
        );
        let hits = noending_lib::search::sessions::page(&db, "TrackedTask", &filter, 20, 0, None)
            .unwrap()
            .0;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].hit.ref_id, "s");
        let backup = Connection::open(result["backup"].as_str().unwrap()).unwrap();
        checked(&backup).unwrap();
        assert_eq!(scalar(&backup, "PRAGMA user_version").unwrap(), FROM);
        assert_eq!(
            scalar(&backup, "SELECT COUNT(*) FROM session_messages").unwrap(),
            3
        );
        // Compare the actual persisted owner data independently against backup.
        let conn = db.read();
        conn.execute(
            "ATTACH DATABASE ?1 AS old",
            [result["backup"].as_str().unwrap()],
        )
        .unwrap();
        for table in [
            "projects",
            "workstreams",
            "sessions",
            "session_workstreams",
            "workstream_projects",
            "preferences",
            "settings",
            "session_contexts",
            "workstream_context_state",
            "project_context_state",
            "workstream_session_frontiers",
            "project_session_frontiers",
        ] {
            equal(&conn, table, "*", "").unwrap();
        }
        drop(conn);
        drop(db);
        assert!(migrate(&f.db(), &f.backups(), true)
            .unwrap_err()
            .to_string()
            .contains("version=11"));
    }
    #[test]
    fn prepare_and_validation_failure_leave_original_data_in_format_eight() {
        let f = Fixture::new();
        legacy(&f);
        let prepared = migrate(&f.db(), &f.backups(), false).unwrap();
        assert!(Path::new(prepared["prepared"].as_str().unwrap()).is_file());
        let conn = Connection::open(f.db()).unwrap();
        assert_eq!(scalar(&conn, "PRAGMA user_version").unwrap(), FROM);
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM session_messages").unwrap(),
            3
        );
        conn.execute(
            "UPDATE session_message_projection SET ordinal=4 WHERE ordinal=2",
            [],
        )
        .unwrap();
        drop(conn);
        assert!(migrate(&f.db(), &f.backups(), true)
            .unwrap_err()
            .to_string()
            .contains("不连续"));
        let conn = Connection::open(f.db()).unwrap();
        assert_eq!(scalar(&conn, "PRAGMA user_version").unwrap(), FROM);
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM session_messages").unwrap(),
            3
        );
    }
    #[test]
    fn refuses_an_active_reader_and_nonempty_retired_owned_data() {
        let f = Fixture::new();
        legacy(&f);
        let conn = Connection::open(f.db()).unwrap();
        conn.execute_batch("BEGIN; SELECT * FROM sessions;")
            .unwrap();
        assert!(migrate(&f.db(), &f.backups(), true).is_err());
        assert!(!f.backups().exists());
        conn.execute_batch(
            "ROLLBACK; INSERT INTO assistant_sessions VALUES('owned','Legacy chat','now');",
        )
        .unwrap();
        drop(conn);
        assert!(migrate(&f.db(), &f.backups(), true)
            .unwrap_err()
            .to_string()
            .contains("assistant_sessions"));
        let conn = Connection::open(f.db()).unwrap();
        assert_eq!(scalar(&conn, "PRAGMA user_version").unwrap(), FROM);
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM assistant_sessions").unwrap(),
            1
        );
    }
}
