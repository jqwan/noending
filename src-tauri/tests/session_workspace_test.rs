//! Session workspace semantics and the storage ingredients of the Session
//! detail view. Every test pins one edge of the fact chain:
//! `Session.cwd` (the ROOT's) → `workspace_path_id` →
//! `workspace_paths.project_id` → `Project`.
//!
//! The WorkspacePath creator is a `ScriptedAttacher` stand-in for
//! `workspace::project`'s `WorkspaceAttaching`, so Session rules are testable
//! without Git detection. Paths are plain `/repo/...` strings — no temp-dir
//! prefix is asserted, because `std::env::temp_dir` is a symlink on macOS.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
mod support;

use noending::adapters::{adapter_for, DiscoveredMember, DiscoveredMemberKind};
use noending::domain::{Agent, Session, SessionMessageRole};
use noending::error::Result;
use noending::ingestion::{ingest_session, reconcile_all, session_title_sources};
use noending::launcher::LaunchWorkspace;
use noending::lifecycle;
use noending::storage::session_paths::{
    attach_session_workspace_path_conn, refresh_sessions_project_for_path_conn,
};
use noending::storage::workspace::{
    insert_workspace_path_conn, reassign_workspace_path_project_conn,
};
use noending::storage::Db;
use noending::workspace::session::{
    attach_sessions_to_registered_paths, move_session_to_path_conn, register_workspace_attacher,
    UnattachedWorkspacePaths,
};
use noending::workspace::{normalize_path, path_identity_of, WorkspaceAttaching};

// fixtures

fn unique_dir(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "noending-session-ws-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn temp_db(tag: &str) -> (PathBuf, Db) {
    let dir = unique_dir(tag);
    let db = Db::open(&dir.join("noending.db")).expect("temp db");
    (dir, db)
}

/// The seam, scripted: every resolvable spelling becomes its deterministic
/// WorkspacePath under one fixed Project. The instance handed to the app-wide
/// registry is created exactly once per process — later registrations are
/// refused, so every reconcile-based test shares it and its project id.
struct Scripted {
    project_id: String,
}

impl Scripted {
    fn new(project_id: &str) -> Self {
        Self {
            project_id: project_id.into(),
        }
    }
}

impl WorkspaceAttaching for Scripted {
    fn ensure_path(&self, conn: &rusqlite::Connection, raw: &str) -> Result<Option<String>> {
        let Some(canonical) = normalize_path(raw) else {
            return Ok(None);
        };
        Ok(Some(insert_workspace_path_conn(
            conn,
            &canonical,
            &self.project_id,
        )?))
    }
}

/// The one shared seam of this test binary, registered on first use.
fn shared_attacher() -> &'static Arc<Scripted> {
    static SHARED: OnceLock<Arc<Scripted>> = OnceLock::new();
    SHARED.get_or_init(|| {
        let attacher = Arc::new(Scripted::new("p-repo"));
        // This is the only registration point in the binary; the Result is
        // ignored so a re-entrant init can never panic.
        let _ = register_workspace_attacher(attacher.clone());
        attacher
    })
}

fn project(db: &Db, id: &str, name: &str) {
    db.upsert_project(&support::project(id.into(), name))
        .unwrap();
}

/// Codex rollout envelope: discovery fingerprints on `{ordinal, payload, type}`
/// per line, so a fixture without `ordinal` is simply not a Codex file.
fn codex_user_line(text: &str) -> String {
    format!(
        r#"{{"ordinal":1,"type":"response_item","timestamp":"2026-09-13T10:01:00Z","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{text}"}}]}}}}"#
    )
}

fn codex_meta_line(session_id: &str, cwd: &str) -> String {
    format!(
        r#"{{"ordinal":0,"type":"session_meta","timestamp":"2026-09-13T10:00:00Z","payload":{{"id":"{session_id}","cwd":"{cwd}"}}}}"#
    )
}

/// Write a discoverable Codex transcript under `root` and run one reconcile
/// pass over the enabled ingest sources. Returns the stored session.
fn discover_via_reconcile(
    db: &Db,
    root: &Path,
    file_name: &str,
    agent_session_id: &str,
    cwd: &str,
    first_user_text: &str,
) -> Session {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join(file_name),
        format!(
            "{}\n{}\n",
            codex_meta_line(agent_session_id, cwd),
            codex_user_line(first_user_text)
        ),
    )
    .unwrap();
    let root_str = root.to_string_lossy().to_string();
    if !db.enabled_roots(Agent::Codex).unwrap().contains(&root_str) {
        db.add_ingest_source(Agent::Codex, &root_str, true).unwrap();
    }
    let _ = shared_attacher();
    reconcile_all(db, &LaunchWorkspace::default(), &|_| {}).unwrap();
    db.find_session_by_root_agent_id(Agent::Codex, agent_session_id)
        .unwrap()
        .expect("discovered through the real adapter")
}

fn stored(db: &Db, id: &str) -> Session {
    db.get_session(id).unwrap().expect("session row")
}

// 1. discovery → WorkspacePath

#[test]
fn discovered_session_gets_a_workspace_path() {
    let (_d, db) = temp_db("discover");
    project(&db, "p-repo", "repo");

    let s = discover_via_reconcile(
        &db,
        &unique_dir("discover-raw"),
        "rollout-2026-09-13-s-1-2-3-4.jsonl",
        "discover-1",
        "/repo/app",
        "帮我看下这个模块",
    );
    let after = stored(&db, &s.id);
    assert_eq!(
        after.workspace_path_id,
        path_identity_of("/repo/app"),
        "the observed cwd became its WorkspacePath, by deterministic identity"
    );
    assert_eq!(
        db.list_workspace_paths().unwrap().len(),
        1,
        "exactly one path exists"
    );

    // Re-discovery of the same transcript skips the unchanged source, so no
    // second attach happens and no second path row can appear.
    reconcile_all(&db, &LaunchWorkspace::default(), &|_| {}).unwrap();
    assert_eq!(
        db.list_workspace_paths().unwrap().len(),
        1,
        "a steady-state scan creates no new path"
    );
    assert_eq!(
        stored(&db, &s.id).workspace_path_id,
        after.workspace_path_id
    );
}

/// A trailing separator is the same directory, so the same path identity: an
/// attach through another spelling cannot move the Session or duplicate the
/// WorkspacePath (one directory, one identity).
#[test]
fn a_different_spelling_of_the_same_cwd_does_not_move_the_session() {
    let (_d, db) = temp_db("spell");
    project(&db, "p1", "repo");
    let (s, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "spell-root",
            Some("t"),
            Some("/repo/app"),
            None,
            None,
            None,
            None,
        )
        .unwrap();
    {
        let conn = db.write();
        insert_workspace_path_conn(&conn, "/repo/app", "p1").unwrap();
    }

    // The explicit attach door with a differently-spelled cwd.
    let moved = {
        let conn = db.write();
        noending::workspace::session::attach_session_conn(
            &conn,
            &Scripted::new("p1"),
            &s,
            Some("/repo/app/"),
        )
        .unwrap()
    };
    assert!(moved, "the unattached row moved onto the path");
    let after = stored(&db, &s);
    assert_eq!(after.workspace_path_id, path_identity_of("/repo/app"));
    assert_eq!(after.project_id.as_deref(), Some("p1"));
    assert_eq!(
        db.list_workspace_paths().unwrap().len(),
        1,
        "the second spelling resolved to the same identity, not a new row"
    );
}

/// The unwired seam answers "no path" to everything — an absent WorkspacePath
/// is a fact we do not know, a fabricated one is a fact that is wrong.
#[test]
fn an_unwired_workspace_layer_resolves_no_path() {
    let (_d, db) = temp_db("unwired");
    let resolved = {
        let conn = db.write();
        noending::workspace::session::resolve_session_path(
            &conn,
            &UnattachedWorkspacePaths,
            Some("/repo/app"),
        )
        .unwrap()
    };
    assert_eq!(resolved, None, "an unwired seam never invents a path");

    // And a blank cwd is answered before the seam is consulted at all.
    let scripted = {
        let conn = db.write();
        noending::workspace::session::resolve_session_path(&conn, &Scripted::new("p1"), Some("   "))
            .unwrap()
    };
    assert_eq!(scripted, None);
}

// 2/3. the derived Project

#[test]
fn session_gets_a_derived_project() {
    let (_d, db) = temp_db("derived");
    project(&db, "p-repo", "repo");

    let s = discover_via_reconcile(
        &db,
        &unique_dir("derived-raw"),
        "rollout-2026-09-13-d-1-2-3-4.jsonl",
        "derived-1",
        "/repo/app",
        "hello",
    );
    let after = stored(&db, &s.id);
    assert_eq!(after.project_id.as_deref(), Some("p-repo"));
    // The cache agrees with its source, which is the whole invariant.
    let wp = db
        .get_workspace_path(after.workspace_path_id.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(wp.project_id.as_str(), "p-repo");
    assert!(
        after.owner_workstream_id.is_none(),
        "a standalone Session has no Owner Workstream"
    );
}

#[test]
fn an_explicit_attach_recomputes_the_cached_project_from_the_path() {
    // The explicit re-attach door: a Session moves and the cache follows in the
    // same statement. Passing a Project to it is not an option, by design.
    let (_d, db) = temp_db("reattach");
    project(&db, "p1", "one");
    project(&db, "p2", "two");
    let (s, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "reattach-root",
            Some("t"),
            Some("/repo/two"),
            None,
            None,
            None,
            None,
        )
        .unwrap();
    {
        let conn = db.write();
        insert_workspace_path_conn(&conn, "/repo/two", "p2").unwrap();
        attach_session_workspace_path_conn(&conn, &s, None).unwrap();
    }
    // The row above was never attached; simulate the pre-attach state exactly.
    let first = db
        .tx(|tx| insert_workspace_path_conn(tx, &normalize_path("/repo/one").unwrap(), "p1"))
        .unwrap();
    db.tx(|tx| attach_session_workspace_path_conn(tx, &s, Some(&first)))
        .unwrap();
    let after = stored(&db, &s);
    assert_eq!(after.workspace_path_id.as_deref(), Some(first.as_str()));
    assert_eq!(after.project_id.as_deref(), Some("p1"));
}

#[test]
fn workspace_path_move_follows_a_trashed_session() {
    let (_d, db) = temp_db("trash-path-move");
    project(&db, "p1", "one");
    project(&db, "p2", "two");
    let (session_id, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "trash-path-move-root",
            Some("t"),
            Some("/repo/one"),
            None,
            None,
            None,
            None,
        )
        .unwrap();
    let path_one = normalize_path("/repo/one").unwrap();
    let path_two = normalize_path("/repo/two").unwrap();
    let first = db
        .tx(|tx| insert_workspace_path_conn(tx, &path_one, "p1"))
        .unwrap();
    let second = db
        .tx(|tx| insert_workspace_path_conn(tx, &path_two, "p2"))
        .unwrap();
    assert!(db
        .tx(|tx| move_session_to_path_conn(tx, &session_id, &first))
        .unwrap());
    db.tx(|tx| {
        noending::storage::session_lifecycle::archive_session_conn(
            tx,
            &session_id,
            &noending::storage::now(),
        )
    })
    .unwrap();

    // The recycle bin filters lists; the Session's path still follows its
    // source.
    assert!(db
        .tx(|tx| move_session_to_path_conn(tx, &session_id, &second))
        .unwrap());
    assert_eq!(
        stored(&db, &session_id).workspace_path_id.as_deref(),
        Some(second.as_str())
    );
}

// 4. batch Project refresh

#[test]
fn changing_a_workspace_paths_project_refreshes_all_its_sessions() {
    let (_d, db) = temp_db("refresh");
    project(&db, "p1", "one");
    project(&db, "p2", "two");

    let mk = |id: &str, cwd: &str| {
        let path = {
            let conn = db.write();
            insert_workspace_path_conn(&conn, cwd, "p1").unwrap()
        };
        let (sid, _) = db
            .upsert_logical_session_unchecked(
                Agent::Codex,
                &format!("root-{id}"),
                Some("t"),
                Some(cwd),
                Some(&path),
                None,
                None,
                None,
            )
            .unwrap();
        sid
    };
    let a = mk("a", "/repo/app");
    let b = mk("b", "/repo/app");
    let c = mk("c", "/repo/other");
    for s in [&a, &b, &c] {
        assert_eq!(stored(&db, s).project_id.as_deref(), Some("p1"));
    }

    let path = path_identity_of("/repo/app").unwrap();
    db.tx(|tx| reassign_workspace_path_project_conn(tx, &path, "p2"))
        .unwrap();

    assert_eq!(stored(&db, &a).project_id.as_deref(), Some("p2"));
    assert_eq!(stored(&db, &b).project_id.as_deref(), Some("p2"));
    // Only the Sessions behind that path moved; the third still reads p1 through
    // its own path. A refresh is a projection, not a rename-everything.
    assert_eq!(stored(&db, &c).project_id.as_deref(), Some("p1"));
}

#[test]
fn the_batch_refresh_is_alone_sufficient_and_idempotent() {
    let (_d, db) = temp_db("refresh2");
    project(&db, "p1", "one");
    project(&db, "p2", "two");
    let path = {
        let conn = db.write();
        insert_workspace_path_conn(&conn, "/repo/app", "p1").unwrap()
    };
    let (a, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "refresh-root",
            Some("t"),
            Some("/repo/app"),
            Some(&path),
            None,
            None,
            None,
        )
        .unwrap();
    db.tx(|tx| {
        tx.execute(
            "UPDATE workspace_paths SET project_id = 'p2' WHERE id = ?1",
            rusqlite::params![path],
        )?;
        Ok(())
    })
    .unwrap();

    // The helper alone is enough: it re-reads the Project through the path
    // instead of being told what the new value is.
    let refreshed = db
        .tx(|tx| refresh_sessions_project_for_path_conn(tx, &path))
        .unwrap();
    assert_eq!(refreshed, 1);
    assert_eq!(stored(&db, &a).project_id.as_deref(), Some("p2"));
    // Running it again cannot apply the change twice.
    let again = db
        .tx(|tx| refresh_sessions_project_for_path_conn(tx, &path))
        .unwrap();
    assert_eq!(again, 1);
    assert_eq!(stored(&db, &a).project_id.as_deref(), Some("p2"));
}

// 5. ordinary session ingestion does no Project work

#[test]
fn session_ingestion_alone_does_not_change_a_project() {
    let (_d, db) = temp_db("ingest");
    project(&db, "p-repo", "repo");

    let s = discover_via_reconcile(
        &db,
        &unique_dir("ingest-raw"),
        "rollout-2026-09-13-i-1-2-3-4.jsonl",
        "ingest-1",
        "/repo/app",
        "first message",
    );
    let before = stored(&db, &s.id);
    let path_before = db
        .get_workspace_path(before.workspace_path_id.as_ref().unwrap())
        .unwrap()
        .unwrap();

    // Another ingest pass over the same (unchanged) source: the reconcile
    // re-resolution is skipped by the cursor, and a direct session ingest has
    // no workspace code path at all — the session's Project facts stay put.
    let stored_count = ingest_session(&db, &before).unwrap();
    assert_eq!(stored_count, 0, "an unchanged source stores nothing");

    let after = stored(&db, &s.id);
    assert_eq!(after.project_id, before.project_id);
    assert_eq!(after.workspace_path_id, before.workspace_path_id);
    let path_after = db
        .get_workspace_path(after.workspace_path_id.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(path_after.project_id, path_before.project_id);
    assert_eq!(path_after.last_seen_at, path_before.last_seen_at);
}

// product-level discovery

#[test]
fn reconcile_discovers_and_attaches_sessions_to_workspace_paths() {
    let dir = unique_dir("reconcile");
    let root = dir.join("sessions");
    let db = Db::open(&dir.join("noending.db")).unwrap();
    project(&db, "p-repo", "repo");

    let s = discover_via_reconcile(
        &db,
        &root,
        "rollout-2026-09-13-r-1-2-3-4.jsonl",
        "reconciled-1",
        "/repo/app",
        "hello",
    );

    let after = stored(&db, &s.id);
    assert_eq!(after.workspace_path_id, path_identity_of("/repo/app"));
    assert_eq!(after.project_id.as_deref(), Some("p-repo"));
    assert_eq!(after.cwd.as_deref(), Some("/repo/app"));
    assert!(after.title.is_some(), "the fixture yields a title");
}

/// Discovery skips transcripts whose stored cursor still matches the file on
/// disk: a skipped file produces no DiscoveredMember at all, so nothing may
/// refresh the row — observable by a sentinel `last_activity_at` surviving a
/// second pass. A row WITHOUT a title is deliberately never in the skipset:
/// re-parsing its (unchanged) file is what heals titles written before the
/// title source was seen.
#[test]
fn reconcile_skips_unchanged_files_but_reparses_untitled_rows() {
    let dir = unique_dir("skip-unchanged");
    let root = dir.join("sessions");
    let db = Db::open(&dir.join("noending.db")).unwrap();
    // The global attacher is process state another test in this binary may
    // have registered; "p-repo" existing keeps that seam working here too.
    project(&db, "p-repo", "repo");
    let s = discover_via_reconcile(
        &db,
        &root,
        "rollout-2026-09-13-k-1-2-3-4.jsonl",
        "skip-1",
        "/repo/app",
        "first user message",
    );
    assert!(
        stored(&db, &s.id).title.is_some(),
        "the fixture yields a title"
    );

    // Pass 2 over the unchanged file: discovery must skip the parse entirely,
    // so nothing overwrites the sentinel.
    db.write()
        .execute(
            "UPDATE sessions SET last_activity_at = 'sentinel' WHERE id = ?1",
            rusqlite::params![s.id],
        )
        .unwrap();
    reconcile_all(&db, &LaunchWorkspace::default(), &|_| {}).unwrap();
    let after = stored(&db, &s.id);
    assert_eq!(
        after.last_activity_at.as_deref(),
        Some("sentinel"),
        "an unchanged file is skipped: no discovery refresh may touch the row"
    );

    // Untitled rows are outside the skipset — the pass re-parses and heals
    // the title even though the file did not change.
    db.write()
        .execute(
            "UPDATE sessions SET title = NULL WHERE id = ?1",
            rusqlite::params![s.id],
        )
        .unwrap();
    reconcile_all(&db, &LaunchWorkspace::default(), &|_| {}).unwrap();
    let healed = stored(&db, &s.id);
    assert!(
        healed.title.is_some(),
        "untitled rows are re-parsed and healed"
    );
}

/// a Session is a member of a Project because its own path says so.
/// The derived cache is a cache: a row still holding only a hand-attached
/// label is not in that Project, and `list_sessions` must agree with
/// `get_project_detail` instead of disagreeing with it.
#[test]
fn a_session_with_only_the_cached_project_id_is_not_a_project_member() {
    let (_d, db) = temp_db("cache-is-not-membership");
    project(&db, "p-repo", "Real");
    let raw = unique_dir("cache-authority-raw");
    let member = discover_via_reconcile(
        &db,
        &raw,
        "rollout-2026-09-13-c-1-2-3-4.jsonl",
        "s-member",
        "/cache-authority/repo",
        "member prompt",
    );
    let ghost = discover_via_reconcile(
        &db,
        &raw,
        "rollout-2026-09-13-c-5-6-7-8.jsonl",
        "s-ghost",
        "/cache-authority/repo",
        "ghost prompt",
    );
    // Make the second row look like what a hand-cached label produces: a row
    // whose only Project fact is the label, with no resolvable path.
    // `attach_session_workspace_path_conn(None)` is the door that clears both,
    // so the label is re-written afterwards the way stale caches had it.
    db.tx(|tx| attach_session_workspace_path_conn(tx, &ghost.id, None))
        .unwrap();
    db.write()
        .execute(
            "UPDATE sessions SET project_id = 'p-repo' WHERE id = ?1",
            rusqlite::params![ghost.id],
        )
        .unwrap();
    assert_eq!(
        stored(&db, &ghost.id).workspace_path_id,
        None,
        "sanity: the ghost has no path, only the cached label"
    );

    let in_project = db
        .list_sessions(noending::storage::SessionFilter {
            scope: Default::default(),
            project_id: Some("p-repo".into()),
            agent: None,
        })
        .unwrap();
    let ids: Vec<&str> = in_project.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![member.id.as_str()],
        "the cached row must not be listed as a member the chain denies"
    );
    assert_eq!(
        stored(&db, &ghost.id).project_id.as_deref(),
        Some("p-repo"),
        "the stale cache value is left alone — this is a read-side fix, not a rewrite"
    );
}

///  — a Session's title splits into two tiers: the NATIVE tier (the Agent
/// app's own name) and the FALLBACK tier (first user text, else first agent
/// text). Within a tier the order is the whole point (a human prompt beats a
/// machine reply), and the split lives in exactly one function
/// (`ingestion::session_title_sources`); what each tier may overwrite is the
/// storage layer's COALESCE policy.
#[test]
fn a_session_title_prefers_the_native_then_the_user_then_the_agent() {
    let member = |native: Option<&str>, user: Option<&str>, agent: Option<&str>| DiscoveredMember {
        agent: Agent::Codex,
        source_member_id: "title-root".into(),
        kind: DiscoveredMemberKind::Root,
        parent_source_member_id: None,
        root_hint: None,
        source_kind: "codex_rollout".into(),
        source_path: PathBuf::from("/raw/title.jsonl"),
        cwd: None,
        started_at: None,
        last_activity_at: None,
        native_title: native.map(Into::into),
        first_user_text: user.map(Into::into),
        first_agent_text: agent.map(Into::into),
        metadata: serde_json::json!({}),
    };

    let cases: [(DiscoveredMember, (Option<String>, Option<String>)); 5] = [
        (
            member(
                Some("原生标题"),
                Some("第一句用户话"),
                Some("第一句 agent 话"),
            ),
            (Some("原生标题".into()), Some("第一句用户话".into())),
        ),
        (
            member(None, Some("第一句用户话"), Some("第一句 agent 话")),
            (None, Some("第一句用户话".into())),
        ),
        (
            member(None, None, Some("第一句 agent 话")),
            (None, Some("第一句 agent 话".into())),
        ),
        (member(None, None, None), (None, None)),
        // A machine blob is not a title, so the next source gets its turn —
        // and when no source has prose, there is no title at all.
        (
            member(None, None, Some("{\"outcome\":\"allow\"}")),
            (None, None),
        ),
    ];
    for (d, expected) in cases {
        assert_eq!(
            session_title_sources(&d),
            expected,
            "source_member_id={}",
            d.source_member_id
        );
    }
}

/// discovery skips a transcript whose cursor says "unchanged", so a
/// Session ingested before its directory was registered keeps
/// `workspace_path_id = NULL` forever: the branch that exists for exactly that
/// case sits behind the gate. One cheap pass over the registered paths repairs
/// it, and touching an unregistered directory stays a decision rather than a
/// repair.
#[test]
fn a_pass_attaches_sessions_whose_directory_was_registered_later() {
    let (_d, db) = temp_db("late-attach");
    project(&db, "p1", "P1");
    let path_id = {
        let conn = db.write();
        insert_workspace_path_conn(&conn, "/repo/late", "p1").unwrap()
    };

    // A Session row as discovery leaves it when the seam had nothing to answer
    // with — cwd observed, no WorkspacePath attached.
    let orphan = |root: &str, cwd: &str| {
        let (id, _) = db
            .upsert_logical_session_unchecked(
                Agent::Codex,
                root,
                Some("t"),
                Some(cwd),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        id
    };
    let late = orphan("late-root", "/repo/late");
    let _stranger = orphan("stranger-root", "/repo/nobody-registered");
    assert_eq!(stored(&db, &late).workspace_path_id, None);

    assert_eq!(attach_sessions_to_registered_paths(&db).unwrap(), 1);

    let attached = stored(&db, &late);
    assert_eq!(
        attached.workspace_path_id.as_deref(),
        Some(path_id.as_str())
    );
    assert_eq!(
        attached.project_id.as_deref(),
        Some("p1"),
        "the derived Project cache moves in the same statement, not after it"
    );
    assert_eq!(
        stored(&db, &_stranger).workspace_path_id,
        None,
        "an unregistered directory is not registered as a side effect"
    );
    assert_eq!(
        attach_sessions_to_registered_paths(&db).unwrap(),
        0,
        "idempotent: once attached, the Session leaves the set"
    );
}

// 6. the detail ingredients

/// `get_session_detail` is a thin Tauri command over storage/lifecycle queries.
/// This pins exactly the rows those queries hand it, so the detail page's
/// facts cannot drift from the store: the conversation, the aggregate stats,
/// the two frontiers, and the fresh root source verdict.
#[test]
fn the_detail_ingredients_come_from_storage_queries() {
    let dir = unique_dir("detail-src");
    let file = dir.join("rollout-detail.jsonl");
    std::fs::write(&file, "agent-owned transcript\n").unwrap();
    let db = {
        let (d, db) = {
            let d = unique_dir("detail");
            let db = Db::open(&d.join("noending.db")).unwrap();
            (d, db)
        };
        let _ = d;
        db
    };
    let root_id = "detail-root";
    let (s, _) = db
        .upsert_logical_session(
            Agent::Codex,
            root_id,
            None,
            Some("detail title"),
            Some("/repo/detail"),
            None,
            None,
            None,
            None,
            "test_root",
            &file.to_string_lossy(),
            &serde_json::json!({}),
        )
        .unwrap();

    // One root conversation batch.
    let messages = vec![
        support::parsed_message("m1", SessionMessageRole::User, "detail first"),
        support::parsed_message("m2", SessionMessageRole::Assistant, "detail reply"),
    ];
    let stored_messages = db
        .commit_ingest(
            &s,
            &messages,
            &noending::domain::SourceCursorUpdate {
                file_identity: "identity".into(),
                generation: 1,
                byte_offset: 100,
                last_seen_size: 100,
                mtime: None,
                start_byte_offset: 0,
                prefix_hash: String::new(),
            },
        )
        .unwrap();
    db.index_new_messages(&stored_messages).unwrap();

    // The conversation belongs to this session alone.
    assert!(stored_messages.iter().all(|m| m.session_id == s));
    assert_eq!(stored(&db, &s).cwd.as_deref(), Some("/repo/detail"));

    // The two frontiers the detail page shows: messages ingested, and how far
    // the explicit Session Context has consumed them (no Context row → 0).
    assert_eq!(db.ingested_message_sequence(&s).unwrap(), 2);
    assert_eq!(
        db.get_session_context(&s)
            .unwrap()
            .map(|c| c.processed_through_seq)
            .unwrap_or(0),
        0,
        "no Session Context has been generated yet"
    );

    // The fresh source verdict and the lifecycle flags it feeds.
    let session = stored(&db, &s);
    let status = lifecycle::root_source_status(&db, &session).unwrap();
    assert_eq!(status, Some(noending::domain::SourceAvailability::Present));
    assert!(!session.is_archived());
}

/// A stats delta so the commit above reads like the observation batch it is.
/// The adapter resolved by `adapter_for` is the one the lifecycle verdicts
/// consult: a Codex root pointing at a real file is Present, and pointing at
/// a removed file is Missing — never something in between.
#[test]
fn the_source_verdict_follows_the_file_on_disk() {
    let dir = unique_dir("verdict-src");
    let file = dir.join("session.jsonl");
    std::fs::write(&file, "transcript\n").unwrap();
    let session_at = |path: &Path| Session {
        id: "s".into(),
        agent: Agent::Codex,
        root_agent_session_id: "root".into(),
        title: None,
        owner_workstream_id: None,
        cwd: None,
        workspace_path_id: None,
        project_id: None,
        forked_from_session_id: None,
        started_at: None,
        last_activity_at: None,
        last_conversation_at: None,
        archived_at: None,
        source_kind: "test".into(),
        source_path: path.to_string_lossy().to_string(),
        metadata: serde_json::json!({}),
        source_file_identity: String::new(),
        source_generation: 0,
        source_byte_offset: 0,
        source_last_seen_size: 0,
        source_mtime: None,
        source_prefix_hash: String::new(),
        source_tail_hash: String::new(),
        fact_generation: 0,
        latest_message_seq: 0,
    };

    let adapter = adapter_for(Agent::Codex);
    assert_eq!(
        adapter.inspect_session_source(&session_at(&file)).unwrap(),
        noending::domain::SourceAvailability::Present
    );
    std::fs::remove_file(&file).unwrap();
    assert_eq!(
        adapter.inspect_session_source(&session_at(&file)).unwrap(),
        noending::domain::SourceAvailability::Missing
    );

    // A directory is a non-regular file: Unavailable — any doubt ≠ missing.
    assert_eq!(
        adapter.inspect_session_source(&session_at(&dir)).unwrap(),
        noending::domain::SourceAvailability::Unavailable
    );
}

#[test]
fn session_message_stats_reports_counts_by_role() {
    let (_, db) = temp_db("msg-stats");
    project(&db, "p-repo", "repo");
    let stats_empty = db.session_message_stats("non-existent").unwrap();
    assert_eq!(stats_empty.user_messages, 0);
    assert_eq!(stats_empty.assistant_messages, 0);

    let s = discover_via_reconcile(
        &db,
        &unique_dir("ingest-stats"),
        "rollout-2026-09-13-stats.jsonl",
        "stats-1",
        "/repo/app",
        "hello",
    );
    let stats = db.session_message_stats(&s.id).unwrap();
    assert_eq!(stats.user_messages, 1);
    assert_eq!(stats.assistant_messages, 0);
}
