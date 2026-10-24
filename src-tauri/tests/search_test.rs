//! Search behavior contract.
//!
//! FTS5 answers token queries; a query its tokenizer cannot match still finds
//! the document through the LIKE pass. Only Logical Session documents and ROOT
//! conversation messages are indexed — and a trashed session's rows are hidden
//! by the read-side lifecycle guard, not only by the unindex-on-trash write.

use noending::domain::{Agent, SessionMessageRole};
use noending::lifecycle;
use noending::storage::{new_id, Db};
use noending_lib as noending;
use std::ops::Deref;
use support::search_messages as search;

mod support;

struct TestDb {
    db: Option<Db>,
    dir: std::path::PathBuf,
}

impl Deref for TestDb {
    type Target = Db;
    fn deref(&self) -> &Self::Target {
        self.db.as_ref().unwrap()
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        self.db.take();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn open_db(tag: &str) -> TestDb {
    let dir = std::env::temp_dir().join(format!("noending-search-{tag}-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).unwrap();
    TestDb { db: Some(db), dir }
}

/// Object lookup matches literal substrings, including punctuation.
#[test]
fn object_lookup_matches_substrings_and_literal_wildcards() {
    use noending::search::{
        navigation::lookup,
        objects::{matching_ids, ObjectKind},
    };
    let db = open_db("object-literals");
    for (id, name) in [
        ("a", "NoEndingFoobar"),
        ("b", "hello_world"),
        ("c", "progress 100%"),
        ("d", "ordinary title"),
    ] {
        db.upsert_project(&support::project(id.into(), name))
            .unwrap();
    }
    for (query, id) in [("Foob", "a"), ("_", "b"), ("%", "c")] {
        assert_eq!(
            matching_ids(
                &db,
                ObjectKind::Project,
                query,
                &["a", "b", "c", "d"].map(str::to_owned)
            )
            .unwrap(),
            vec![id]
        );
        let hits: Vec<_> = lookup(&db, query)
            .unwrap()
            .items
            .into_iter()
            .filter(|h| h.kind == ObjectKind::Project)
            .collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].ref_id, id);
    }
    assert!(lookup(&db, "nothing-here").unwrap().items.is_empty());
}

/// A ROOT conversation message is searchable by its content, with
/// `ref_id` = the message id and `parent_id` = the session id — and there is NO
/// length filter: a short message is as findable as a long one. Every
/// stored message IS the curated conversation.
#[test]
fn a_root_message_is_searchable_regardless_of_length() {
    let mut db = open_db("message");
    let long = "决定使用 SQLite 作为本地存储，因为它是单文件、零配置且可嵌入的。";
    let short = "先做迁移";
    let (_session, stored) = support::seed_conversation(
        &db,
        Agent::Codex,
        "root-search-1",
        &[
            support::parsed_message("m1", SessionMessageRole::User, long),
            support::parsed_message("m2", SessionMessageRole::User, short),
        ],
    );
    // Search must survive a restart using only the ingestion transaction;
    // startup performs no global document backfill.
    drop(db.db.take());
    db.db = Some(Db::open(&db.dir.join("test.db")).unwrap());

    let hits = search(&db, "SQLite", 20).unwrap();
    let message_hits: Vec<_> = hits.iter().filter(|h| h.kind == "message").collect();
    assert_eq!(message_hits.len(), 1, "{hits:?}");
    assert_eq!(message_hits[0].ref_id, stored[0].id);
    assert_eq!(
        message_hits[0].parent_id, _session.id,
        "a message's parent is its session"
    );

    // No length filter: the five-character message is indexed and found.
    let hits = search(&db, "迁移", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == stored[1].id),
        "a short message must be searchable: {hits:?}"
    );
}

/// The Logical Session's own document (`kind = 'session'`) carries the title
/// and is searchable — this is how a session without a matching message body
/// is still found.
#[test]
fn a_session_is_searchable_by_its_document() {
    let db = open_db("session-doc");
    let (session, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "root-doc-1",
            Some("quant script debug"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

    let hits = search(&db, "quant", 20).unwrap();
    let session_hits: Vec<_> = hits.iter().filter(|h| h.kind == "session").collect();
    assert_eq!(session_hits.len(), 1, "{hits:?}");
    assert_eq!(session_hits[0].ref_id, session);
    assert_eq!(session_hits[0].title, "quant script debug");

    // An untitled session is still documented, under the agent's name.
    let (untitled, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "root-doc-2",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    let hits = search(&db, "codex", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "session" && h.ref_id == untitled),
        "an untitled session falls back to the agent name as its title: {hits:?}"
    );
}

/// Lifecycle × search (doc): trashing unindexes the session's
/// messages and its own document; restoring reindexes the same durable rows.
#[test]
fn archive_keeps_session_and_messages_searchable() {
    let db = open_db("lifecycle");
    let (_session, stored) = support::seed_conversation(
        &db,
        Agent::Codex,
        "root-lifecycle-1",
        &[support::parsed_message(
            "m1",
            SessionMessageRole::User,
            "the sqlite storage decision lives here",
        )],
    );
    db.index_new_messages(&stored).unwrap();
    assert!(
        search(&db, "sqlite", 20)
            .unwrap()
            .iter()
            .any(|h| h.kind == "message"),
        "searchable before trash"
    );

    lifecycle::archive_session(&db, &_session.id).unwrap();
    let hits = search(&db, "sqlite", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == stored[0].id),
        "archive keeps messages searchable: {hits:?}"
    );
    let documents = search(&db, "codex", 20).unwrap();
    assert!(
        documents
            .iter()
            .any(|h| h.kind == "session" && h.ref_id == _session.id),
        "archive keeps the session document searchable: {documents:?}"
    );

    lifecycle::restore_session(&db, &_session.id).unwrap();
    let hits = search(&db, "sqlite", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == stored[0].id),
        "restore reindexes the durable rows: {hits:?}"
    );
    // The session DOCUMENT is rebuilt as well — an untitled session's document
    // title falls back to the agent name, which is how it is findable.
    let hits = search(&db, "codex", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "session" && h.ref_id == _session.id),
        "restore rebuilds the session document: {hits:?}"
    );
}

/// The read-side guard is the second half of the lifecycle invariant: a stale
/// index row left behind by an interrupted write must not surface a trashed or
/// deleted session in search.
#[test]
fn the_read_side_guard_hides_rows_without_a_live_session() {
    let db = open_db("stale-guard");
    let (session, stored) = support::seed_conversation(
        &db,
        Agent::Codex,
        "root-stale-1",
        &[support::parsed_message(
            "m1",
            SessionMessageRole::User,
            "unique stale marker text",
        )],
    );
    db.index_new_messages(&stored).unwrap();

    // The session goes to the trash: the write side unindexes, and even a
    // row that re-appears afterwards stays hidden while the trash holds.
    lifecycle::archive_session(&db, &session.id).unwrap();
    db.write()
        .execute(
            "INSERT INTO search_documents (kind, ref_id, parent_id, title, body)
             VALUES ('message', 'stale-msg', ?1, '', 'unique stale marker text')",
            rusqlite::params![session.id],
        )
        .unwrap();
    db.write()
        .execute(
            "INSERT INTO search_documents (kind, ref_id, parent_id, title, body)
             VALUES ('session', ?1, '', 'unique stale marker title', '')
             ON CONFLICT(kind,ref_id) DO UPDATE SET title=excluded.title, body=excluded.body",
            rusqlite::params![session.id],
        )
        .unwrap();

    let hits = search(&db, "unique stale marker", 20).unwrap();
    assert!(
        !hits.iter().any(|h| h.ref_id == "stale-msg"),
        "a stale message row of a trashed session must never surface: {hits:?}"
    );
    assert!(
        hits.iter()
            .any(|h| h.kind == "session" && h.ref_id == session.id),
        "an archived session document remains searchable: {hits:?}"
    );

    // A row whose session is GONE entirely (purged, crashed mid-write) is
    // equally invisible.
    db.write()
        .execute(
            "INSERT INTO search_documents (kind, ref_id, parent_id, title, body)
             VALUES ('message', 'stale-gone', 'sess-gone', '', 'unique stale marker text')",
            [],
        )
        .unwrap();
    let hits = search(&db, "unique stale marker", 20).unwrap();
    assert!(
        !hits.iter().any(|h| h.ref_id == "stale-gone"),
        "a row with no backing session must never surface: {hits:?}"
    );

    // Restore makes the durable rows visible again — the guard reads the
    // live lifecycle state, it does not guess from the index.
    lifecycle::restore_session(&db, &session.id).unwrap();
    let hits = search(&db, "unique stale marker", 20).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.kind == "message" && h.ref_id == stored[0].id),
        "restore reindexes and the guard lets the live rows through: {hits:?}"
    );
}

/// Routing ids remain available on hits, but only title/body are searchable.
#[test]
fn search_indexes_content_without_tokenizing_routing_metadata() {
    let db = open_db("metadata-unindexed");
    let (session, messages) = support::seed_conversation(
        &db,
        Agent::Codex,
        "routingnativeonly",
        &[support::parsed_message(
            "routingmessageonly",
            SessionMessageRole::User,
            "Searchable prose",
        )],
    );
    for metadata in [
        "message",
        messages[0].id.as_str(),
        session.id.as_str(),
        "routingmessageonly",
        "routingnativeonly",
    ] {
        let hits = search(&db, metadata, 10).unwrap();
        assert!(
            !hits.iter().any(|h| h.kind == "message"),
            "message routing metadata must not be tokenized: {hits:?}"
        );
    }
    for content in ["prose", "archable"] {
        let hits = search(&db, content, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].ref_id, messages[0].id);
        assert_eq!(hits[0].parent_id, session.id);
    }
}

#[test]
fn message_search_uses_external_prose_and_keeps_fts_consistent_after_retirement() {
    use noending::domain::SourceCursorUpdate;
    let db = open_db("external-prose");
    let prose = format!("uniqueposting {}", "正文只保留一份。".repeat(4000));
    let (session, stored) = support::seed_conversation(
        &db,
        Agent::Codex,
        "external-prose",
        &[support::parsed_message(
            "m1",
            SessionMessageRole::User,
            &prose,
        )],
    );
    let hits = search(&db, "uniqueposting", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].ref_id, stored[0].id);
    assert!(hits[0].snippet.contains("uniqueposting"));
    assert_eq!(search(&db, "只保留", 10).unwrap()[0].ref_id, stored[0].id);
    db.write()
        .execute(
            "INSERT INTO search_index(search_index,rank) VALUES('integrity-check',1)",
            [],
        )
        .unwrap();
    db.commit_ingest(
        &session.id,
        &[],
        &SourceCursorUpdate {
            file_identity: "replacement".into(),
            generation: 1,
            byte_offset: 0,
            last_seen_size: 0,
            mtime: None,
            start_byte_offset: 0,
            prefix_hash: String::new(),
        },
    )
    .unwrap();
    assert!(search(&db, "uniqueposting", 10).unwrap().is_empty());
    db.write()
        .execute(
            "INSERT INTO search_index(search_index,rank) VALUES('integrity-check',1)",
            [],
        )
        .unwrap();
    // Cascading removal also retires postings before their source prose goes.
    let (cascade, _) = support::seed_conversation(
        &db,
        Agent::Codex,
        "external-cascade",
        &[support::parsed_message(
            "cascade",
            SessionMessageRole::User,
            "cascadeposting",
        )],
    );
    db.write()
        .execute("DELETE FROM sessions WHERE id=?1", [&cascade.id])
        .unwrap();
    let remaining: i64 = db
        .read()
        .query_row(
            "SELECT COUNT(*) FROM search_documents WHERE kind='message' AND parent_id=?1",
            [&cascade.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);
    db.write()
        .execute(
            "INSERT INTO search_index(search_index,rank) VALUES('integrity-check',1)",
            [],
        )
        .unwrap();
}

#[test]
fn navigation_lookup_matches_objects_and_paths_but_excludes_content() {
    use noending::search::navigation::lookup;
    use noending::search::objects::ObjectKind;
    let db = open_db("navigation-scope");
    let project = support::project("nav-project".into(), "Needle Project");
    db.upsert_project(&project).unwrap();
    let mut task = support::workstream("nav-task".into(), "Needle Task");
    task.description = "contentonly".into();
    db.upsert_workstream(&task).unwrap();
    let (session, _) = support::seed_conversation(
        &db,
        Agent::Codex,
        "nav-session",
        &[support::parsed_message(
            "m",
            SessionMessageRole::User,
            "contentonly",
        )],
    );
    db.tx(|tx| {
        noending::storage::workspace::insert_workspace_path_conn(tx,"/work/100%_done",&project.id)?;
        tx.execute("UPDATE sessions SET title='Needle Session',cwd='/work/100%_done',archived_at='now' WHERE id=?1",[&session.id])?;
        tx.execute("UPDATE workstreams SET visibility='archived' WHERE id=?1",[&task.id])?;
        tx.execute("INSERT INTO project_context_state(project_id,summary) VALUES(?1,'{\"items\":[{\"id\":\"00000000-0000-4000-8000-000000000001\",\"title\":\"contentonly\",\"description\":\"\",\"details\":[],\"title_locked\":false}]}')",[&project.id])?;
        tx.execute("UPDATE workstream_context_state SET summary='{\"items\":[{\"id\":\"00000000-0000-4000-8000-000000000001\",\"title\":\"contentonly\",\"description\":\"\",\"details\":[],\"title_locked\":false}]}' WHERE workstream_id=?1",[&task.id])?;
        tx.execute("INSERT INTO session_contexts(session_id,summary,updated_at) VALUES(?1,'{\"items\":[{\"id\":\"00000000-0000-4000-8000-000000000001\",\"title\":\"contentonly\",\"description\":\"\",\"details\":[],\"title_locked\":false}]}','now')",[&session.id])?;


        Ok(())
    }).unwrap();
    let document_kinds: Vec<String> = {
        let conn = db.read();
        let mut stmt = conn
            .prepare("SELECT DISTINCT kind FROM search_documents ORDER BY kind")
            .unwrap();
        let rows = stmt.query_map([], |row| row.get(0)).unwrap();
        rows.collect::<std::result::Result<_, _>>().unwrap()
    };
    assert_eq!(document_kinds, vec!["message", "session"]);
    for retired_kind in ["project", "workstream"] {
        assert!(db
            .write()
            .execute(
                "INSERT INTO search_documents(kind,ref_id) VALUES(?1,'retired')",
                [retired_kind]
            )
            .is_err());
    }
    assert!(
        !search(&db, "contentonly", 20).unwrap().is_empty(),
        "content remains available to the separate search service"
    );
    assert!(
        lookup(&db, "contentonly").unwrap().items.is_empty(),
        "lookup must not match summaries, descriptions or message text"
    );
    let result = lookup(&db, "nEeDlE").unwrap();
    assert!(!result.has_more);
    let ids: std::collections::BTreeSet<_> =
        result.items.iter().map(|h| h.ref_id.as_str()).collect();
    assert_eq!(
        ids,
        std::collections::BTreeSet::from([
            project.id.as_str(),
            task.id.as_str(),
            session.id.as_str()
        ])
    );
    assert!(
        result
            .items
            .iter()
            .find(|h| h.kind == ObjectKind::Session)
            .unwrap()
            .archived
    );
    assert!(
        result
            .items
            .iter()
            .find(|h| h.kind == ObjectKind::Workstream)
            .unwrap()
            .archived
    );
    let paths = lookup(&db, "100%_").unwrap();
    assert_eq!(paths.items.len(), 2);
    assert!(paths.items.iter().all(|h| h.hint == "/work/100%_done"));
    assert!(
        lookup(&db, "100xx").unwrap().items.is_empty(),
        "percent and underscore must match literally"
    );
}

#[test]
fn navigation_lookup_includes_agents_without_detection_and_reports_truncation() {
    use noending::search::navigation::lookup;
    use noending::search::objects::ObjectKind;
    let db = open_db("navigation-agents");
    let initial = lookup(&db, "").unwrap();
    assert!(initial.items.iter().all(|h| h.kind == ObjectKind::Agent));
    let qoder = lookup(&db, "qcoder").unwrap();
    assert_eq!(qoder.items.len(), 1);
    assert_eq!(qoder.items[0].agent, Some(Agent::Qoder));
    assert_eq!(qoder.items[0].kind, ObjectKind::Agent);
    for i in 0..30 {
        let task = support::workstream(format!("nav-{i:02}"), format!("Codex task {i}"));
        db.upsert_workstream(&task).unwrap();
    }
    let result = lookup(&db, "codex").unwrap();
    assert!(result.has_more);
    assert!(result.items.len() < 31);
    assert_eq!(
        result.items[0].kind,
        ObjectKind::Agent,
        "exact Agent names remain ahead of broad prefix matches"
    );
    assert_eq!(result.items[0].agent, Some(Agent::Codex));
    let narrow = lookup(&db, "Codex task 29").unwrap();
    assert!(!narrow.has_more);
    assert_eq!(narrow.items.len(), 1);
    assert_eq!(narrow.items[0].ref_id, "nav-29");
    assert!(
        lookup(&db, "")
            .unwrap()
            .items
            .iter()
            .all(|h| h.kind == ObjectKind::Agent),
        "default suggestions must not preload task records"
    );
}

#[test]
fn board_matches_are_complete_and_exclude_non_identity_fields() {
    use noending::search::{
        navigation::lookup,
        objects::{matching_ids, ObjectKind},
    };
    let db = open_db("board-match-range");
    let mut expected = Vec::new();
    for n in 0..35 {
        let id = format!("task-{n:02}");
        let mut task = support::workstream(id.clone(), format!("Shared title {n}"));
        if n == 34 {
            task.visibility = "archived".into();
        }
        db.upsert_workstream(&task).unwrap();
        expected.push(id);
    }
    assert_eq!(
        matching_ids(&db, ObjectKind::Workstream, " Shared title ", &expected).unwrap(),
        expected
    );
    // Scope is a query constraint: matching objects outside it never return.
    let scope = vec![
        "task-34".into(),
        "task-02".into(),
        "task-34".into(),
        "missing".into(),
    ];
    assert_eq!(
        matching_ids(&db, ObjectKind::Workstream, "Shared", &scope).unwrap(),
        vec!["task-02", "task-34"]
    );
    assert_eq!(
        matching_ids(&db, ObjectKind::Workstream, "", &scope).unwrap(),
        vec!["task-02", "task-34"]
    );
    assert!(matching_ids(&db, ObjectKind::Workstream, "Shared", &[])
        .unwrap()
        .is_empty());
    assert!(
        matching_ids(&db, ObjectKind::Agent, "claude", &["codex".into()])
            .unwrap()
            .is_empty()
    );
    let preview = lookup(&db, "Shared title").unwrap();
    assert_eq!(preview.items.len(), 20);
    assert!(preview.has_more);
    let mut content = support::workstream("other".into(), "Unrelated");
    content.description = "descriptiononly".into();
    db.upsert_workstream(&content).unwrap();
    assert!(matching_ids(
        &db,
        ObjectKind::Workstream,
        "descriptiononly",
        &[content.id.clone()]
    )
    .unwrap()
    .is_empty());
    assert_eq!(
        matching_ids(
            &db,
            ObjectKind::Agent,
            "claude",
            &["claude_code".into(), "codex".into()]
        )
        .unwrap(),
        vec!["claude_code"]
    );
    let (session, _) = support::seed_conversation(&db, Agent::Codex, "native-exact-id", &[]);
    for query in [session.id.as_str(), "native-exact-id"] {
        assert_eq!(
            matching_ids(&db, ObjectKind::Session, query, &[session.id.clone()]).unwrap(),
            vec![session.id.as_str()]
        );
        assert_eq!(lookup(&db, query).unwrap().items[0].ref_id, session.id);
    }
    assert!(
        matching_ids(&db, ObjectKind::Session, "exact-id", &[session.id.clone()])
            .unwrap()
            .is_empty()
    );
    assert!(
        matching_ids(&db, ObjectKind::Session, "Codex", &[session.id.clone()])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn navigation_lookup_searches_skills_by_name_id_and_description_while_default_remains_clean() {
    use noending::search::{
        navigation::lookup,
        objects::{matching_ids, ObjectKind},
    };
    use noending::skills::{self, SkillSource};

    let db = open_db("navigation-skills");

    // Empty query returns only agents/recent sessions, never skills.
    let initial = lookup(&db, "").unwrap();
    assert!(initial.items.iter().all(|h| h.kind != ObjectKind::Skill));

    let mut registry = skills::load_skill_registry(&db).unwrap();
    registry.sources.push(SkillSource {
        id: "git-commit-helper".into(),
        name: "Git Commit".into(),
        description: "Assists with generating conventional commit messages".into(),
        path: "/mock/git-commit/SKILL.md".into(),
    });
    registry.sources.push(SkillSource {
        id: "deploy-pipeline".into(),
        name: "Deploy Cloud".into(),
        description: "CI/CD deployment utilities".into(),
        path: "/mock/deploy/SKILL.md".into(),
    });
    skills::save_skill_registry(&db, &registry).unwrap();

    // Default suggestions still never return skills.
    assert!(lookup(&db, "")
        .unwrap()
        .items
        .iter()
        .all(|h| h.kind != ObjectKind::Skill));

    // Match by name
    let by_name = lookup(&db, "Git Commit").unwrap();
    let hit = by_name
        .items
        .iter()
        .find(|h| h.kind == ObjectKind::Skill)
        .expect("found by name");
    assert_eq!(hit.ref_id, "git-commit-helper");
    assert_eq!(hit.title, "Git Commit");
    assert_eq!(
        hit.hint,
        "Assists with generating conventional commit messages"
    );
    assert_eq!(hit.agent, None);
    assert!(!hit.archived);

    // Match by id
    let by_id = lookup(&db, "deploy-pipeline").unwrap();
    let hit_id = by_id
        .items
        .iter()
        .find(|h| h.kind == ObjectKind::Skill)
        .expect("found by id");
    assert_eq!(hit_id.ref_id, "deploy-pipeline");
    assert_eq!(hit_id.title, "Deploy Cloud");

    // Match by description substring
    let by_desc = lookup(&db, "conventional commit").unwrap();
    let hit_desc = by_desc
        .items
        .iter()
        .find(|h| h.kind == ObjectKind::Skill)
        .expect("found by description");
    assert_eq!(hit_desc.ref_id, "git-commit-helper");

    // Ranking: exact name/id match is rank 0, ahead of description match
    let rank_test = lookup(&db, "Deploy").unwrap();
    let hit_rank = rank_test
        .items
        .iter()
        .find(|h| h.kind == ObjectKind::Skill)
        .expect("found by prefix");
    assert_eq!(hit_rank.ref_id, "deploy-pipeline");

    // Matching IDs helper
    let matches = matching_ids(
        &db,
        ObjectKind::Skill,
        "commit",
        &["git-commit-helper".into(), "deploy-pipeline".into()],
    )
    .unwrap();
    assert_eq!(matches, vec!["git-commit-helper"]);
}

#[test]
fn navigation_lookup_with_context_discovers_disk_skills() {
    use noending::search::navigation::lookup_with_context;
    use noending::search::objects::ObjectKind;
    use noending::skills::Roots;

    let db = open_db("navigation-disk-skills");
    let temp_root = std::env::temp_dir().join(format!(
        "noending-test-roots-{}",
        noending::storage::new_id()
    ));
    let home = temp_root.join("home");
    let skill_dir = home.join(".agents/skills/formatter");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: Code Formatter\ndescription: Formats codebase according to rules\n---\n",
    )
    .unwrap();

    let roots = Roots {
        codex: home.join(".codex"),
        claude: home.join(".claude"),
        pi: home.join(".pi/agent"),
        home: home.clone(),
    };

    let result = lookup_with_context(&db, "Formatter", Some(&roots), None).unwrap();
    let skill = result
        .items
        .iter()
        .find(|h| h.kind == ObjectKind::Skill)
        .expect("skill found on disk");
    assert_eq!(skill.title, "Code Formatter");
    assert_eq!(skill.hint, "Formats codebase according to rules");

    let _ = std::fs::remove_dir_all(&temp_root);
}
