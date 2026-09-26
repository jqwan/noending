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
use noending::domain::{Agent, Session, SessionMemberRelation, SessionMessageRole};
use noending::error::Result;
use noending::ingestion::{ingest_session, reconcile_all, session_title};
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
fn workspace_path_move_does_not_mutate_a_trashed_session() {
    let (_d, db) = temp_db("trash-freeze-move");
    project(&db, "p1", "one");
    project(&db, "p2", "two");
    let (session_id, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "trash-freeze-root",
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
        noending::storage::session_lifecycle::trash_session_conn(
            tx,
            &session_id,
            &noending::storage::now(),
        )
    })
    .unwrap();

    assert!(!db
        .tx(|tx| move_session_to_path_conn(tx, &session_id, &second))
        .unwrap());
    assert_eq!(
        stored(&db, &session_id).workspace_path_id.as_deref(),
        Some(first.as_str())
    );
}

#[test]
fn project_id_writers_are_confined_to_the_derived_doors() {
    // as an executable check: `sessions.project_id` is a cache, and a
    // cache with a fourth writer stops being a cache.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let offenders = rust_files(&manifest.join("src"))
        .into_iter()
        .filter(|f| {
            let rel = f
                .strip_prefix(manifest)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // The two sanctioned files: the derived doors in session_paths.rs,
            // and storage/mod.rs (upsert_logical_session's in-statement
            // COALESCE plus the batch refresh and the referential cleanup).
            let allowed = rel == "src/storage/session_paths.rs" || rel == "src/storage/mod.rs";
            !allowed && writes_sessions_project_id(&std::fs::read_to_string(f).unwrap_or_default())
        })
        .collect::<Vec<_>>();
    assert!(
        offenders.is_empty(),
        "sessions.project_id may only be written by upsert_logical_session's in-statement \
         derivation, the batch refresh and the referential cleanup: {offenders:?}"
    );
}

/// `UPDATE sessions … SET project_id`, tolerant of a statement wrapped across
/// lines, and blind to prose: the doc comments that *explain* the prohibition
/// must not be reported as violating it. A near-miss like
/// `UPDATE workspace_paths SET project_id` must not trip it either — that write
/// is legitimate Project policy, not a cache violation.
fn writes_sessions_project_id(text: &str) -> bool {
    let code = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join(" ");
    let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut from = 0usize;
    while let Some(i) = flat[from..].find("UPDATE sessions") {
        let window = &flat[from + i..flat.len().min(from + i + 160)];
        if window.contains("SET project_id") {
            return true;
        }
        from += i + "UPDATE sessions".len();
    }
    false
}

fn rust_files(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        std::fs::read_dir(path)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .flat_map(|e| rust_files(&e.path()))
                    .collect()
            })
            .unwrap_or_default()
    } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
        vec![path.to_path_buf()]
    } else {
        vec![]
    }
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

// 5. ordinary member ingestion does no Project work

#[test]
fn member_ingestion_alone_does_not_change_a_project() {
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
    // re-resolution is skipped by the cursor, and a direct member ingest has
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

// ─────────────────────────────────────────────────────────  T1–T4
// The mechanical anti-drift assertions. `project_id_writers_are_confined_to_the_
// derived_doors` above is T2; these are the rest of the same family, and they
// live beside it because a guard nobody can find is a guard nobody keeps.
// Every one of them reads *source text*, so each must be proven to fire: see the
// per-test note on what mutation breaks it. A grep guard that matches prose is
// worse than no guard — it reports confidence it has not earned.

/// Strip `//` and `///` lines and flatten the rest, so a doc comment that
/// *explains* a prohibition can never be reported as violating it.
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The argument list of `tauri::generate_handler![ … ]` as entries, comments
/// removed. Counting *entries* rather than "mentions" is the whole point: the
/// retired commands are named repeatedly in this file's own rationale comments
///, and a substring grep would read that as them being registered.
fn registered_commands() -> Vec<String> {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("lib.rs"),
    )
    .expect("lib.rs");
    let marker = "generate_handler![";
    let start = src.find(marker).expect("generate_handler! block");
    let body = &src[start + marker.len()..];
    let mut depth = 1usize;
    let mut end = 0usize;
    for (i, c) in body.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(end > 0, "the generate_handler! macro must be closed");
    body[..end]
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .flat_map(|l| l.split(','))
        .map(|e| e.trim().trim_end_matches(';').to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

/// the retired Project / Session / Workstream commands are not
/// registered. Breaks if any of them is added back to `generate_handler!`.
#[test]
fn retired_commands_are_not_registered() {
    let retired = [
        "create_project",
        "delete_project",
        "update_project",
        "assign_session_project",
        "suggest_session_project",
        "merge_workstreams",
        "add_project_resource",
        "list_project_resources",
        "remove_project_resource",
    ];
    let live = registered_commands();
    assert!(
        live.len() > 40,
        "the handler list must actually be parsed, not silently emptied: {} entries",
        live.len()
    );
    let leaked: Vec<&String> = live
        .iter()
        .filter(|entry| {
            let last = entry.rsplit("::").next().unwrap_or(entry);
            retired.iter().any(|r| *r == last)
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "these left the product API and must not be re-registered: {leaked:?}"
    );
    // The read side of the same freeze: the UI still gets exactly three Project
    // commands, and `list_sessions` is the Session list.
    for kept in ["list_projects", "get_project_detail", "rename_project"] {
        assert!(
            live.iter().any(|e| e.ends_with(&format!("::{kept}"))),
            "{kept} must stay registered"
        );
    }
}

/// The substring from `window[at] == '('` through its matching close paren.
fn balanced_paren(window: &str, from: usize) -> Option<&str> {
    let open = window[from..].find('(')? + from;
    let mut depth = 0usize;
    for (i, c) in window[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&window[open..=open + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// every product write to `workstreams` names `lifecycle`
/// explicitly (the column default is not a safety net for a field the
/// user sees), and nothing writes the retired vocabulary. Reading the old value
/// is a different statement shape, so only the write form is matched, as
/// `SET lifecycle = …`.
#[test]
fn workstream_writes_name_lifecycle_and_never_write_the_retired_vocabulary() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders: Vec<String> = Vec::new();
    for file in rust_files(&manifest.join("src")) {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let code = code_only(&text);
        let mut from = 0usize;
        while let Some(i) = code[from..].find("INSERT INTO workstreams") {
            let at = from + i;
            let window = &code[at..code.len().min(at + 400)];
            // The *column list*, matched-parenthesis. Scanning to the last `)`
            // in the window would swallow the `ON CONFLICT DO UPDATE SET …
            // lifecycle = ?5` clause, which mentions lifecycle without the
            // INSERT naming the column — and the guard would pass on a write
            // that violates it.
            let columns = match balanced_paren(window, "INSERT INTO workstreams".len()) {
                Some(c) => c,
                None => "",
            };
            if !columns.contains("lifecycle") {
                let rel = file.strip_prefix(manifest).unwrap().display().to_string();
                offenders.push(format!("{rel}: INSERT INTO workstreams without lifecycle"));
            }
            from = at + "INSERT INTO workstreams".len();
        }
        for bad in [
            "SET lifecycle = 'open'",
            "SET lifecycle = \"open\"",
            "SET lifecycle = 'abandoned'",
            "lifecycle: \"open\"",
            "lifecycle: \"abandoned\"",
        ] {
            if code.contains(bad) {
                let rel = file.strip_prefix(manifest).unwrap().display().to_string();
                offenders.push(format!("{rel}: writes {bad:?}"));
            }
        }
    }
    assert!(offenders.is_empty(), "/ {offenders:?}");
}

/// `git` is reached only through the executable resolver (M10: a
/// Finder-launched macOS app has no shell PATH, and `git` is exactly the binary
/// that lives in Homebrew), and the Git *protocol* strings live in
/// `workspace/` alone. Breaks on `Command::new("git")` anywhere, or on a second
/// `--git-common-dir` implementation outside the resolver.
#[test]
fn git_is_only_reached_through_the_resolver_and_only_from_workspace() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders: Vec<String> = Vec::new();
    for file in rust_files(&manifest.join("src")) {
        let rel = file
            .strip_prefix(manifest)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let code = code_only(&std::fs::read_to_string(&file).unwrap_or_default());
        if code.contains("Command::new(\"git\")") {
            offenders.push(format!("{rel}: Command::new(\"git\")"));
        }
        for token in ["--git-common-dir", "worktree list", "--show-toplevel"] {
            if code.contains(token) && rel != "src/workspace/resolver.rs" {
                offenders.push(format!("{rel}: {token}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "put exactly one place allowed to run git: {offenders:?}"
    );
    // Prove the scan reaches the file that is allowed to do it: if this assert
    // ever fails, the walker is broken and every assertion above is vacuous.
    let resolver = std::fs::read_to_string(manifest.join("src/workspace/resolver.rs")).unwrap();
    assert!(code_only(&resolver).contains("--git-common-dir"));
}

/// + no launcher entry point may resolve a launch without
/// being told the NoEnding Home. The Home-less spellings were the risk: with
/// `default_workspace: None`, 's third tier is simply absent, and M30's
/// "do not teach the Workstream the fallback directory" gate in `apply_match`
/// *inverts* to growing the list. Enforced here as well as by the compiler,
/// because `LaunchWorkspace::default()` is how the mistake would come back.
#[test]
fn no_launch_entry_point_can_forget_the_home() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let launcher = std::fs::read_to_string(manifest.join("src/launcher/mod.rs")).expect("launcher");
    let offenders: Vec<&str> = launcher
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .filter(|l| l.contains("LaunchWorkspace::default()"))
        .collect();
    assert!(
        offenders.is_empty(),
        "tier 3 must be injected, never defaulted: {offenders:?}"
    );
    // Every public prepare/launch entry point takes the workspace explicitly.
    for name in [
        "prepare_new_in",
        "prepare_resume_in",
        "launch_prepared_in",
        "launch_prepared_with_in",
    ] {
        let sig = launcher
            .split(&format!("pub fn {name}(")[..])
            .nth(1)
            .unwrap_or_else(|| panic!("{name} must exist"));
        let head = &sig[..sig.find("->").unwrap_or(sig.len())];
        assert!(
            head.contains("workspace: &LaunchWorkspace"),
            "{name} must require the LaunchWorkspace rather than assume one"
        );
    }
    // The Home-less wrappers themselves are gone, not merely unused.
    for gone in [
        "pub fn prepare_new(",
        "pub fn prepare_resume(",
        "pub fn launch_prepared(",
        "pub fn new_session(",
        "pub fn resume_session(",
        "pub fn resolve_new_session_cwd(",
    ] {
        assert!(
            !launcher.contains(gone),
            "{gone} is a Home-less launcher entry point and was removed (E-(f))"
        );
    }
}

///, and the row "migration occurs before DB open": the Home pointer
/// decides *which file* `Db::open` is handed, so a relocation that has not been
/// applied yet must not be skipped past. Breaks if `Db::open` is hoisted above
/// `prepare_home`, which is precisely the failure mode exists to
/// avoid (the user's history looks deleted).
#[test]
fn noending_home_is_resolved_before_the_database_opens() {
    let src = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("lib.rs");
    let home = src
        .find("prepare_home(")
        .expect("prepare_home call in setup");
    let db = src.find("Db::open(").expect("Db::open call in setup");
    assert!(
        home < db,
        "relocation runs inside prepare_home, so it must precede opening the db ({home} vs {db})"
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

///  — a Session's title comes from the first source that has one:
/// the root's native title, else the first user text, else the first agent
/// text. The order is the whole point (an Agent's own title beats a derived
/// one, and a human prompt beats a machine reply), and the rule lives in
/// exactly one function (`ingestion::session_title`).
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

    let cases: [(DiscoveredMember, Option<String>); 5] = [
        (
            member(
                Some("原生标题"),
                Some("第一句用户话"),
                Some("第一句 agent 话"),
            ),
            Some("原生标题".into()),
        ),
        (
            member(None, Some("第一句用户话"), Some("第一句 agent 话")),
            Some("第一句用户话".into()),
        ),
        (
            member(None, None, Some("第一句 agent 话")),
            Some("第一句 agent 话".into()),
        ),
        (member(None, None, None), None),
        // A machine blob is not a title, so the next source gets its turn —
        // and when no source has prose, there is no title at all.
        (member(None, None, Some("{\"outcome\":\"allow\"}")), None),
    ];
    for (d, expected) in cases {
        assert_eq!(
            session_title(&d),
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
/// facts cannot drift from the store: the member graph (root first), the
/// aggregate stats, the two frontiers, and the fresh root source verdict.
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
        .upsert_logical_session_unchecked(
            Agent::Codex,
            root_id,
            Some("detail title"),
            Some("/repo/detail"),
            None,
            None,
            None,
            None,
        )
        .unwrap();
    let root_member = db
        .upsert_session_member(
            &s,
            Agent::Codex,
            root_id,
            SessionMemberRelation::Root,
            None,
            "test_root",
            &file.to_string_lossy(),
            Some("/repo/detail"),
            None,
            None,
            &serde_json::json!({}),
        )
        .unwrap();
    let child_member = db
        .upsert_session_member(
            &s,
            Agent::Codex,
            "detail-child",
            SessionMemberRelation::Child,
            Some(root_id),
            "subagent_file",
            "/raw/child.jsonl",
            Some("/child/cwd"),
            None,
            None,
            &serde_json::json!({}),
        )
        .unwrap();

    // One root conversation batch with stats, then a child stats-only update.
    let messages = vec![
        support::parsed_message("m1", SessionMessageRole::User, "detail first"),
        support::parsed_message("m2", SessionMessageRole::Assistant, "detail reply"),
    ];
    let stored_messages = db
        .commit_member_ingest(
            &s,
            &root_member,
            &messages,
            Some(stats_delta(2, 1)),
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

    // The member graph, root first.
    let members = db.members_for_session(&s).unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].relation.as_str(), "root");
    assert_eq!(members[0].id, root_member);
    assert_eq!(members[1].id, child_member);
    // Child cwd never reached the session.
    assert_eq!(stored(&db, &s).cwd.as_deref(), Some("/repo/detail"));

    // The aggregate is query-time and covers the whole graph.
    let stats = db.aggregate_session_stats(&s).unwrap();
    assert_eq!(stats.member_count, 2);
    assert_eq!(stats.child_count, 1);
    assert_eq!(stats.side_count, 0);
    assert_eq!(stats.max_depth, 1, "the child hangs off the root");

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
    assert!(!session.is_trashed());
}

/// A stats delta so the commit above reads like the observation batch it is.
fn stats_delta(tool_calls: i64, tool_errors: i64) -> noending::domain::StatsUpdate {
    noending::domain::StatsUpdate::Delta(noending::domain::MemberStatsDelta {
        tool_call_count: Some(tool_calls),
        tool_error_count: Some(tool_errors),
        compaction_count: None,
        side_activity_count: None,
    })
}

/// The adapter resolved by `adapter_for` is the one the lifecycle verdicts
/// consult: a Codex root pointing at a real file is Present, and pointing at
/// a removed file is Missing — never something in between.
#[test]
fn the_source_verdict_follows_the_file_on_disk() {
    let dir = unique_dir("verdict-src");
    let file = dir.join("session.jsonl");
    std::fs::write(&file, "transcript\n").unwrap();
    let member = |path: &Path| noending::domain::SessionMember {
        id: "m".into(),
        session_id: "s".into(),
        agent: Agent::Codex,
        source_member_id: "root".into(),
        relation: SessionMemberRelation::Root,
        parent_source_member_id: None,
        source_kind: "test".into(),
        source_path: path.to_string_lossy().to_string(),
        cwd: None,
        started_at: None,
        last_activity_at: None,
        metadata: serde_json::json!({}),
    };

    let adapter = adapter_for(Agent::Codex);
    assert_eq!(
        adapter.inspect_member_source(&member(&file)).unwrap(),
        noending::domain::SourceAvailability::Present
    );
    std::fs::remove_file(&file).unwrap();
    assert_eq!(
        adapter.inspect_member_source(&member(&file)).unwrap(),
        noending::domain::SourceAvailability::Missing
    );

    // A directory is a non-regular file: Unavailable — any doubt ≠ missing.
    assert_eq!(
        adapter.inspect_member_source(&member(&dir)).unwrap(),
        noending::domain::SourceAvailability::Unavailable
    );
}
