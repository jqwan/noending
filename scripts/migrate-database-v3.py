#!/usr/bin/env python3
"""Explicit, offline, one-time conversion of flattened NoEnding v1/v2 to v3.

Close NoEnding first. Run with --database PATH to prepare a backup and verified
v3 candidate; add --apply to install it. The application itself still accepts
only its current format. DDL is read exclusively from storage/schema.rs.
No Agent source files are read or modified. Python 3 with SQLite FTS5 required.
"""
import argparse
from contextlib import closing
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[1]
APPLICATION_ID = 0x4E6F456E
TARGET_VERSION = 3
REMOVED = {
    "projects": {"description"},
    "sessions": {"project_id", "trashed_at"},
    "session_messages": {"raw_ref"},
    "launch_intents": {"launch_type", "process_id"},
    "workstreams": {"lifecycle"},
}


def schema():
    source = (ROOT / "src-tauri/src/storage/schema.rs").read_text()
    version = int(re.search(r"pub const DATABASE_FORMAT_VERSION: i64 = (\d+);", source)[1])
    if version != TARGET_VERSION:
        raise ValueError("This one-time tool only targets database format 3")
    return re.search(r'const CURRENT_SCHEMA: &str = r#"(.*?)"#;', source, re.S)[1]


def quote(name):
    return '"' + name.replace('"', '""') + '"'


def tables(conn):
    return {row[0] for row in conn.execute(
        "SELECT name FROM sqlite_master WHERE type='table' "
        "AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'search_index_%'"
    )}


def columns(conn, table):
    return [row[1] for row in conn.execute("PRAGMA table_info(" + quote(table) + ")")]


def integrity(conn):
    if conn.execute("PRAGMA integrity_check").fetchall() != [("ok",)]:
        raise ValueError("Database integrity check failed")
    if conn.execute("PRAGMA foreign_key_check").fetchone() is not None:
        raise ValueError("Database has dangling foreign keys")


def read_only(path):
    conn = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=0)
    conn.execute("PRAGMA query_only=ON")
    return conn


def fingerprint(conn):
    """All original columns, including removed ones; detects intervening writes."""
    result = {}
    for table in sorted(tables(conn)):
        cols = columns(conn, table)
        result[table] = digest(conn.execute(
            "SELECT * FROM " + quote(table) + " ORDER BY " + ",".join(map(quote, cols))
        ))
    result["schema"] = digest(conn.execute(
        "SELECT type,name,sql FROM sqlite_master ORDER BY type,name"
    ))
    return result


def digest(rows):
    h = hashlib.sha256()
    count = 0
    for row in rows:
        h.update(json.dumps(list(row), ensure_ascii=False, separators=(",", ":")).encode())
        h.update(b"\n")
        count += 1
    return count, h.hexdigest()


def mapping(source, target, table):
    old = set(columns(source, table))
    new = columns(target, table)
    removed = old - set(new)
    if removed - REMOVED.get(table, set()):
        raise ValueError("Unrecognized columns in " + table + ": " + str(sorted(removed)))
    exprs = []
    for col in new:
        if table == "workstreams" and col == "visibility":
            bad = source.execute(
                "SELECT count(*) FROM workstreams WHERE visibility NOT IN ('normal','trashed','archived')"
            ).fetchone()[0]
            if bad:
                raise ValueError("Unknown task visibility; conversion refused")
            exprs.append("CASE WHEN visibility = 'trashed' THEN 'archived' ELSE visibility END")
        elif table == "sessions" and col == "archived_at" and col not in old:
            if "trashed_at" not in old:
                raise ValueError("Missing Session archive/trash authority")
            exprs.append('"trashed_at"')
        elif col in old:
            exprs.append(quote(col))
        else:
            raise ValueError("Missing source column " + table + "." + col)
    return new, exprs


def rebuild_search(conn):
    conn.execute("INSERT INTO search_index (kind,ref_id,parent_id,title,body) SELECT 'project',id,'',name,'' FROM projects")
    conn.execute("INSERT INTO search_index (kind,ref_id,parent_id,title,body) SELECT 'workstream',id,'',title,description FROM workstreams")
    conn.execute("""INSERT INTO search_index (kind,ref_id,parent_id,title,body)
        SELECT 'session',s.id,'',COALESCE(s.title,s.agent),
               COALESCE(p.name,'') || char(10) || COALESCE(w.title,'') || char(10) || COALESCE(s.cwd,'')
        FROM sessions s LEFT JOIN workspace_paths wp ON wp.id=s.workspace_path_id
        LEFT JOIN projects p ON p.id=wp.project_id LEFT JOIN workstreams w ON w.id=s.owner_workstream_id""")
    conn.execute("""INSERT INTO search_index (kind,ref_id,parent_id,title,body)
        SELECT 'message',m.id,m.session_id,'',m.content FROM session_message_projection p
        JOIN session_messages m ON m.id=p.session_message_id AND m.session_id=p.session_id""")
    conn.execute("""INSERT INTO search_index (kind,ref_id,parent_id,title,body)
        SELECT 'item',i.id,i.workstream_id,r.title,r.content FROM context_items i
        JOIN context_item_revisions r ON r.id=i.current_revision_id""")
    conn.execute("INSERT INTO search_index(search_index) VALUES ('optimize')")


def convert(source, destination):
    """Create a new current schema; validate every preserved/transformed field."""
    if destination.exists():
        raise ValueError("Destination already exists")
    with closing(read_only(source)) as old, closing(sqlite3.connect(destination)) as new:
        if old.execute("PRAGMA application_id").fetchone()[0] != APPLICATION_ID:
            raise ValueError("Not a NoEnding database")
        version = old.execute("PRAGMA user_version").fetchone()[0]
        if version not in (1, 2):
            raise ValueError("Only flattened v1/v2 databases are accepted")
        integrity(old)
        new.executescript(schema())
        if tables(old) != tables(new):
            raise ValueError("Unexpected source tables; no data will be silently dropped")
        if old.execute("SELECT count(*) FROM projects WHERE description <> ''").fetchone()[0]:
            raise ValueError("Nonempty Project descriptions need an explicit preservation decision")
        if old.execute("SELECT count(*) FROM launch_intents WHERE launch_type <> 'new'").fetchone()[0]:
            raise ValueError("Unexpected launch type")
        counts = {}
        with new:
            for table in sorted(tables(new) - {"search_index"}):
                cols, exprs = mapping(old, new, table)
                fields = ",".join(map(quote, cols))
                select = "SELECT " + ",".join(exprs) + " FROM " + quote(table)
                new.executemany(
                    "INSERT INTO " + quote(table) + " (" + fields + ") VALUES (" + ",".join("?" for _ in cols) + ")",
                    old.execute(select),
                )
                # Compare full rows, not only counts. Ordering includes all retained fields.
                order = " ORDER BY " + ",".join(str(i + 1) for i in range(len(cols)))
                expected = digest(old.execute(select + order))
                actual = digest(new.execute("SELECT " + fields + " FROM " + quote(table) + order))
                if expected != actual:
                    raise ValueError("Data verification failed for " + table)
                counts[table] = actual[0]
            rebuild_search(new)
            new.execute("PRAGMA application_id=" + str(APPLICATION_ID))
            new.execute("PRAGMA user_version=" + str(TARGET_VERSION))
        integrity(new)
        return {"from_version": version, "to_version": TARGET_VERSION, "rows": counts,
                "search_documents": new.execute("SELECT count(*) FROM search_index").fetchone()[0]}


def ensure_closed(path):
    # macOS/Linux: also refuse idle open handles, which SQLite transactions alone
    # cannot detect. Windows refuses replacing a file held open by SQLite.
    if sys.platform in ("darwin", "linux"):
        check = subprocess.run(["lsof", "-t", str(path), str(path) + "-wal", str(path) + "-shm"],
                               capture_output=True, text=True)
        if check.returncode not in (0, 1):
            raise ValueError("Could not check whether NoEnding is closed")
        if check.stdout.strip():
            raise ValueError("Close NoEnding and other database readers before migration")


def migrate(database, apply=False):
    path = database.expanduser().resolve(strict=True)
    ensure_closed(path)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
    backup = path.with_name(path.name + ".backup-" + stamp)
    candidate = path.with_name(path.name + ".v3-" + stamp)
    # SQLite backup includes committed WAL data, unlike copying the .db file.
    with closing(read_only(path)) as source, closing(sqlite3.connect(backup)) as saved:
        source.backup(saved)
        saved.commit()
    os.chmod(backup, 0o600)
    with closing(read_only(backup)) as saved:
        before = fingerprint(saved)
    report = convert(backup, candidate)
    os.chmod(candidate, path.stat().st_mode & 0o777)
    report.update(database=str(path), backup=str(backup), candidate=str(candidate),
                  old_bytes=path.stat().st_size, new_bytes=candidate.stat().st_size, installed=False)
    if apply:
        ensure_closed(path)
        with closing(sqlite3.connect(path, timeout=0)) as guard:
            # Fold any remaining WAL into the old file and acquire an exclusive
            # transaction. Never delete sidecars carrying uncheckpointed data.
            checkpoint = guard.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()
            if checkpoint and checkpoint[0] != 0:
                raise ValueError("Database is busy; backup/candidate kept, source not replaced")
            if guard.execute("PRAGMA journal_mode=DELETE").fetchone()[0] != "delete":
                raise ValueError("Could not leave WAL mode")
            guard.execute("BEGIN EXCLUSIVE")
            if fingerprint(guard) != before:
                raise ValueError("Source changed after backup; rerun migration")
            guard.rollback()
        # No application may be running throughout the offline operation.
        ensure_closed(path)
        for suffix in ("-wal", "-shm"):
            if Path(str(path) + suffix).exists():
                raise ValueError("Unexpected SQLite sidecar remains; source not replaced")
        os.replace(candidate, path)
        report["installed"] = True
    report_path = backup.with_name(backup.name + ".report.json")
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--database", type=Path, required=True)
    parser.add_argument("--apply", action="store_true", help="Install the verified candidate; NoEnding must be closed")
    args = parser.parse_args()
    try:
        print(json.dumps(migrate(args.database, args.apply), ensure_ascii=False, indent=2))
    except (ValueError, OSError, sqlite3.Error) as exc:
        parser.exit(1, "Migration refused: " + str(exc) + "\n")


if __name__ == "__main__":
    main()
