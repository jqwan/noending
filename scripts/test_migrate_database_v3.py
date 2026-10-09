"""Regression tests for the explicitly requested offline conversion."""
import importlib.util
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("migration", Path(__file__).with_name("migrate-database-v3.py"))
migration = importlib.util.module_from_spec(spec)
spec.loader.exec_module(migration)


class MigrationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.source = Path(self.temp.name) / "old.db"
        self.target = Path(self.temp.name) / "new.db"

    def fixture(self, version=1):
        # Reverse only the known v3 retirements; production has no alternate DDL.
        ddl = migration.schema()
        ddl = ddl.replace("      name TEXT NOT NULL,", "      name TEXT NOT NULL,\n      description TEXT NOT NULL DEFAULT '',", 1)
        ddl = ddl.replace("      workspace_path_id TEXT,", "      workspace_path_id TEXT,\n      project_id TEXT REFERENCES projects(id),")
        ddl = ddl.replace("      agent TEXT NOT NULL,\n      owner_workstream_id TEXT REFERENCES workstreams", "      launch_type TEXT NOT NULL DEFAULT 'new',\n      process_id INTEGER,\n      agent TEXT NOT NULL,\n      owner_workstream_id TEXT REFERENCES workstreams")
        ddl = ddl.replace("      content TEXT NOT NULL,", "      content TEXT NOT NULL,\n      raw_ref TEXT NOT NULL DEFAULT '',", 1)
        if version == 1:
            ddl = ddl.replace("      visibility TEXT NOT NULL DEFAULT 'normal',", "      lifecycle TEXT NOT NULL DEFAULT 'active',\n      visibility TEXT NOT NULL DEFAULT 'normal',")
            ddl = ddl.replace("      archived_at TEXT,", "      trashed_at TEXT,")
        ddl = ddl.replace("kind UNINDEXED, ref_id UNINDEXED, parent_id UNINDEXED", "kind, ref_id, parent_id")
        with sqlite3.connect(self.source) as c:
            c.executescript(ddl)
            c.execute("PRAGMA application_id=" + str(migration.APPLICATION_ID))
            c.execute("PRAGMA user_version=" + str(version))
            c.execute("INSERT INTO projects (id,name,name_customized,created_at,updated_at) VALUES ('p','Custom name',1,'t1','t2')")
            c.execute("INSERT INTO workspace_paths VALUES ('path','/work','p','none',NULL,1,'t1','t2')")
            state = 'trashed' if version == 1 else 'archived'
            c.execute("INSERT INTO workstreams (id,title,description,visibility,created_at,updated_at) VALUES ('w','Task','Task notes',?,'t1','t2')", (state,))
            archived = 'trashed_at' if version == 1 else 'archived_at'
            c.execute("INSERT INTO sessions (id,agent,root_agent_session_id,cwd,workspace_path_id,project_id,owner_workstream_id," + archived + ",source_kind,source_path,source_byte_offset,fact_generation,latest_message_seq) VALUES ('s','codex','native-id','/work','path','p','w','archive-time','test','/source',123,2,2)")
            c.execute("INSERT INTO sessions (id,agent,root_agent_session_id,forked_from_session_id,source_kind,source_path) VALUES ('fork','codex','fork-id','s','test','/fork')")
            for mid, seq, text in [('retired', 1, 'Old evidence'), ('current', 2, 'Current conversation')]:
                c.execute("INSERT INTO session_messages (id,session_id,sequence,source_identity_hash,role,content,source_position,raw_ref) VALUES (?,'s',?,?,'user',?,'line:1','/source#line:1')", (mid, seq, mid, text))
            c.execute("INSERT INTO session_message_projection VALUES ('s',1,'current')")
            c.execute("INSERT INTO workstream_paths VALUES ('link','w','path',0,'t1')")
            c.execute("INSERT INTO context_items (id,workstream_id,kind,current_revision_id,created_at,updated_at) VALUES ('item','w','goal','r2','t1','t2')")
            for rid in ['r1', 'r2']:
                c.execute("INSERT INTO context_item_revisions (id,item_id,title,content,metadata,source_type,source_ref,created_at) VALUES (?,'item','Goal','Manual note','{\"evidence\":[\"retired\"]}','session_context','session-context:s:1','t1')", (rid,))
            c.execute("INSERT INTO session_contexts VALUES ('s','Summary','[\"Decision\"]','[\"Question\"]','[\"Next\"]',1,2,1,'t2')")
            c.execute("INSERT INTO session_context_revisions VALUES ('s',1,'Summary','[\"Decision\"]','[\"Question\"]','[\"Next\"]',2,1,'t2')")
            c.execute("INSERT INTO workstream_context_state VALUES ('w',3,4,4)")
            c.execute("INSERT INTO workstream_session_frontiers VALUES ('w','s',1,2,1)")
            c.execute("INSERT INTO context_conflicts (id,workstream_id,left_item_id,left_revision_id,created_at,updated_at) VALUES ('conflict','w','item','r1','t1','t2')")
            c.execute("INSERT INTO context_conflict_events (id,conflict_id,previous_status,new_status,actor,created_at,snapshot_json) VALUES ('event','conflict','open','open','user','t1','{}')")
            c.execute("INSERT INTO workstream_review_state VALUES ('w','t1','[\"r1\"]','t2')")
            c.execute("INSERT INTO launch_intents (id,agent,owner_workstream_id,launched_at,matched_session_id,status,created_at,updated_at) VALUES ('intent','codex','w','t1','s','matched','t1','t2')")
            c.execute("INSERT INTO settings VALUES ('setting','preserved')")
            c.execute("INSERT INTO assistant_sessions VALUES ('assistant','Assistant','t1')")
            c.execute("INSERT INTO assistant_messages (id,session_id,role,content,created_at) VALUES ('a','assistant','user','Assistant history','t1')")
            c.execute("INSERT INTO ingest_sources VALUES ('source','codex','/agent',1,'user','t1')")
            c.execute("INSERT INTO agent_installations VALUES ('codex','/bin/codex','1','path','t1')")
            c.execute("INSERT INTO search_index VALUES ('message','retired','s','','stale result')")

    def test_preserves_all_state_and_maps_archive_for_both_versions(self):
        for version in [1, 2]:
            with self.subTest(version=version):
                self.source.unlink(missing_ok=True)
                self.target.unlink(missing_ok=True)
                self.fixture(version)
                report = migration.convert(self.source, self.target)
                self.assertEqual(report['rows']['session_messages'], 2)
                with sqlite3.connect(self.target) as c:
                    self.assertEqual(c.execute('PRAGMA user_version').fetchone()[0], 3)
                    self.assertEqual(c.execute('SELECT archived_at FROM sessions WHERE id="s"').fetchone()[0], 'archive-time')
                    self.assertEqual(c.execute('SELECT visibility FROM workstreams').fetchone()[0], 'archived')
                    self.assertEqual(c.execute('SELECT created_at,updated_at FROM launch_intents').fetchone(), ('t1', 't2'))
                    self.assertNotIn('project_id', migration.columns(c, 'sessions'))
                    self.assertNotIn('raw_ref', migration.columns(c, 'session_messages'))
                    self.assertEqual(c.execute("SELECT ref_id FROM search_index WHERE kind='message'").fetchall(), [('current',)])
                    self.assertEqual(c.execute("SELECT count(*) FROM search_index WHERE search_index MATCH 'conversation'").fetchone()[0], 1)
                    self.assertEqual(c.execute("SELECT count(*) FROM search_index WHERE search_index MATCH 'current'").fetchone()[0], 1)
                    # Independent fixture expectations: do not rely only on convert's own row checks.
                    self.assertEqual(c.execute("SELECT id,content FROM session_messages ORDER BY sequence").fetchall(), [('retired', 'Old evidence'), ('current', 'Current conversation')])
                    self.assertEqual(c.execute("SELECT owner_workstream_id,cwd,workspace_path_id,source_byte_offset,fact_generation,latest_message_seq FROM sessions WHERE id='s'").fetchone(), ('w', '/work', 'path', 123, 2, 2))
                    self.assertEqual(c.execute("SELECT forked_from_session_id FROM sessions WHERE id='fork'").fetchone(), ('s',))
                    self.assertEqual(c.execute("SELECT id,source_ref,metadata FROM context_item_revisions ORDER BY id").fetchall(), [('r1', 'session-context:s:1', '{"evidence":["retired"]}'), ('r2', 'session-context:s:1', '{"evidence":["retired"]}')])
                    self.assertEqual(c.execute("SELECT summary_current_state,decisions,open_questions,next_steps,revision,ingest_generation,processed_through_seq FROM session_contexts").fetchone(), ('Summary', '["Decision"]', '["Question"]', '["Next"]', 1, 2, 1))
                    self.assertEqual(c.execute("SELECT * FROM workstream_session_frontiers").fetchone(), ('w', 's', 1, 2, 1))
                    self.assertEqual(c.execute("SELECT value FROM settings WHERE key='setting'").fetchone(), ('preserved',))
                    self.assertEqual(c.execute("SELECT content FROM assistant_messages WHERE id='a'").fetchone(), ('Assistant history',))
                    self.assertEqual(c.execute("SELECT enabled,origin FROM ingest_sources WHERE id='source'").fetchone(), (1, 'user'))
                    self.assertEqual(c.execute("SELECT left_revision_id FROM context_conflicts WHERE id='conflict'").fetchone(), ('r1',))
                    self.assertEqual(c.execute("SELECT snapshot_json FROM context_conflict_events WHERE id='event'").fetchone(), ('{}',))
                    migration.integrity(c)
                with sqlite3.connect(self.source) as old:
                    self.assertEqual(old.execute('PRAGMA user_version').fetchone()[0], version)

    def test_refuses_unknown_columns_instead_of_dropping_user_data(self):
        self.fixture()
        with sqlite3.connect(self.source) as c:
            c.execute('ALTER TABLE workstreams ADD COLUMN important_note TEXT')
        with self.assertRaisesRegex(ValueError, 'Unrecognized columns'):
            migration.convert(self.source, self.target)

    def test_refuses_foreign_database(self):
        self.fixture()
        with sqlite3.connect(self.source) as c:
            c.execute('PRAGMA application_id=123')
        with self.assertRaisesRegex(ValueError, 'Not a NoEnding'):
            migration.convert(self.source, self.target)

    def test_refuses_nonempty_retired_project_description(self):
        self.fixture()
        with sqlite3.connect(self.source) as c:
            c.execute("UPDATE projects SET description='User note'")
        with self.assertRaisesRegex(ValueError, 'Nonempty Project descriptions'):
            migration.convert(self.source, self.target)

    def test_refuses_unknown_task_state(self):
        self.fixture()
        with sqlite3.connect(self.source) as c:
            c.execute("UPDATE workstreams SET visibility='unexpected'")
        with self.assertRaisesRegex(ValueError, 'Unknown task visibility'):
            migration.convert(self.source, self.target)

    def test_installs_with_backup_including_crash_left_wal(self):
        self.fixture()
        subprocess.run([sys.executable, '-c', "import sqlite3,sys,os; c=sqlite3.connect(sys.argv[1]); c.execute('PRAGMA journal_mode=WAL'); c.execute(\"INSERT INTO settings VALUES ('wal','latest')\"); c.commit(); os._exit(0)", str(self.source)], check=True)
        self.assertTrue(Path(str(self.source) + '-wal').exists())
        report = migration.migrate(self.source, apply=True)
        self.assertTrue(report['installed'])
        for path in [self.source, Path(report['backup'])]:
            with sqlite3.connect(path) as c:
                self.assertEqual(c.execute("SELECT value FROM settings WHERE key='wal'").fetchone()[0], 'latest')
        with sqlite3.connect(report['backup']) as c:
            self.assertEqual(c.execute('PRAGMA user_version').fetchone()[0], 1)

    def test_default_run_prepares_without_replacing_source(self):
        self.fixture()
        report = migration.migrate(self.source)
        self.assertFalse(report['installed'])
        self.assertTrue(Path(report['candidate']).exists())
        with sqlite3.connect(self.source) as c:
            self.assertEqual(c.execute('PRAGMA user_version').fetchone()[0], 1)

    def test_refuses_open_database(self):
        if sys.platform not in ('darwin', 'linux'):
            self.skipTest('lsof handle check is Unix only')
        self.fixture()
        with sqlite3.connect(self.source) as c:
            c.execute('SELECT count(*) FROM sessions')
            with self.assertRaisesRegex(ValueError, 'Close NoEnding'):
                migration.migrate(self.source, apply=True)

    def test_detects_changes_between_snapshot_and_install(self):
        self.fixture()
        convert = migration.convert
        def concurrent_write(source, target):
            result = convert(source, target)
            with sqlite3.connect(self.source) as c:
                c.execute("UPDATE settings SET value='changed'")
            return result
        with patch.object(migration, 'convert', concurrent_write):
            with self.assertRaisesRegex(ValueError, 'Source changed'):
                migration.migrate(self.source, apply=True)
        with sqlite3.connect(self.source) as c:
            self.assertEqual(c.execute('PRAGMA user_version').fetchone()[0], 1)
            self.assertEqual(c.execute('SELECT value FROM settings').fetchone()[0], 'changed')


if __name__ == '__main__':
    unittest.main()
