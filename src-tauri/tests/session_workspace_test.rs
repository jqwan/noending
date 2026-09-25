//! Session workspace semantics (方案 §19).
//!
//! Every test here pins one edge of the fact chain
//!
//! ```text
//! Session.cwd → workspace_path_id → workspace_paths.project_id → Project
//! ```
//!
//! Two things are deliberately scripted rather than real:
//!
//! * the WorkspacePath creator is a [`Scripted`] stand-in for
//!   `workspace::project`'s implementation of [`WorkspaceAttaching`], so Session
//!   rules are testable without Git detection;
//! * every path is a plain `/repo/...` string — no temp-dir prefix is ever
//!   asserted, because `std::env::temp_dir()` is a symlink on macOS (§42.3-M8).
//!
//! Only temp databases are opened; no user Home, no real transcripts.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
mod support;

use noending::adapters::{adapter_for, DiscoveredSession};
use noending::domain::{Agent, Session};
use noending::error::Result;
use noending::ingestion::{ensure_session_row_with, ingest_session, reconcile_with_engine};
use noending::launcher::LaunchWorkspace;
use noending::storage::session_paths::{
    attach_session_workspace_path_conn, refresh_sessions_project_for_path_conn,
};
use noending::storage::workspace::{
    insert_workspace_path_conn, reassign_workspace_path_project_conn,
};
use noending::storage::Db;
use noending::sync::SyncEngine;
use noending::workspace::session::{
    attach_sessions_to_registered_paths, register_workspace_attacher, UnattachedWorkspacePaths,
};
use noending::workspace::{normalize_path, path_identity_of, WorkspaceAttaching};

// ------------------------------------------------------------- fixtures

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
/// WorkspacePath under one fixed Project, and every call is counted so "ordinary
/// ingestion does no Project work" is a measurement instead of an intention.
struct Scripted {
    project_id: String,
    calls: AtomicUsize,
}

impl Scripted {
    fn new(project_id: &str) -> Self {
        Self {
            project_id: project_id.into(),
            calls: AtomicUsize::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl WorkspaceAttaching for Scripted {
    fn ensure_path(&self, conn: &rusqlite::Connection, raw: &str) -> Result<Option<String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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

fn project(db: &Db, id: &str, name: &str) {
    db.upsert_project(&support::project(id.into(), name))
        .unwrap();
}

fn discovered(agent_session_id: &str, cwd: Option<&str>, raw_path: &Path) -> DiscoveredSession {
    DiscoveredSession {
        agent: Agent::Codex,
        agent_session_id: agent_session_id.into(),
        path: raw_path.to_path_buf(),
        cwd: cwd.map(Into::into),
        started_at: Some("2026-09-13T10:00:00Z".into()),
        last_activity_at: Some("2026-09-13T10:05:00Z".into()),
        first_user_text: Some("帮我看下这个模块".into()),
        native_title: None,
        first_agent_text: None,
        parent_agent_session_id: None,
    }
}

/// One discovery pass over a fake transcript location.
fn discover(
    db: &Db,
    attacher: &dyn WorkspaceAttaching,
    agent_session_id: &str,
    cwd: Option<&str>,
) -> (Session, bool) {
    ensure_session_row_with(
        db,
        &discovered(
            agent_session_id,
            cwd,
            &PathBuf::from(format!("/raw/{agent_session_id}.jsonl")),
        ),
        attacher,
    )
    .unwrap()
}

fn stored(db: &Db, id: &str) -> Session {
    db.get_session(id).unwrap().expect("session row")
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

// ------------------------------------------- 1. discovery → WorkspacePath

#[test]
fn discovered_session_gets_a_workspace_path() {
    let (_d, db) = temp_db("discover");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");

    let (s, is_new) = discover(&db, &attacher, "s1", Some("/repo/app"));
    assert!(is_new);
    let stored = stored(&db, &s.id);
    assert_eq!(
        stored.workspace_path_id,
        path_identity_of("/repo/app"),
        "the observed cwd became its WorkspacePath, by deterministic identity"
    );
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);

    // Re-discovery of the same transcript learns nothing new, so no second
    // attach happens — the seam is free to write a row and must not be called.
    discover(&db, &attacher, "s1", Some("/repo/app"));
    assert_eq!(attacher.calls(), 1, "a steady-state scan does no path work");
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
}

#[test]
fn a_different_spelling_of_the_same_cwd_does_not_move_the_session() {
    let (_d, db) = temp_db("spell");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");

    let (s, _) = discover(&db, &attacher, "s1", Some("/repo/app"));
    let before = stored(&db, &s.id);
    // A trailing separator is the same directory, so the same identity: no
    // second WorkspacePath and no "drift".
    let (_, is_new) = discover(&db, &attacher, "s1", Some("/repo/app/"));
    assert!(!is_new);
    let after = stored(&db, &s.id);
    assert_eq!(before.workspace_path_id, after.workspace_path_id);
    assert_eq!(before.project_id, after.project_id);
    assert_eq!(db.list_workspace_paths().unwrap().len(), 1);
}

#[test]
fn an_unwired_workspace_layer_leaves_the_session_unattached() {
    let (_d, db) = temp_db("unwired");
    let (s, _) = ensure_session_row_with(
        &db,
        &discovered("s1", Some("/repo/app"), &PathBuf::from("/raw/s1.jsonl")),
        &UnattachedWorkspacePaths,
    )
    .unwrap();
    let stored = stored(&db, &s.id);
    assert_eq!(stored.workspace_path_id, None);
    assert_eq!(stored.project_id, None);
    assert!(db.list_workspace_paths().unwrap().is_empty());
}

// ------------------------------------------- 2/3. the derived Project

#[test]
fn session_gets_a_derived_project() {
    let (_d, db) = temp_db("derived");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");

    let (s, _) = discover(&db, &attacher, "s1", Some("/repo/app"));
    let stored = stored(&db, &s.id);
    assert_eq!(stored.project_id.as_deref(), Some("p1"));
    // The cache agrees with its source, which is the whole invariant.
    let wp = db
        .get_workspace_path(stored.workspace_path_id.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(wp.project_id.as_str(), "p1");
    // And what discovery hands back is the stored row, not a caller's guess:
    // nothing in this flow ever supplied a Project at all.
    assert_eq!(s.project_id, stored.project_id);
    assert_eq!(s.workspace_path_id, stored.workspace_path_id);
}

#[test]
fn a_standalone_session_still_gets_a_project() {
    // §24 — no Workstream is involved: membership comes from the Session's own
    // path, straight down the chain.
    let (_d, db) = temp_db("standalone");
    project(&db, "p1", "repo");
    let attacher = Scripted::new("p1");

    let (s, _) = discover(&db, &attacher, "s1", Some("/repo/app"));
    assert!(
        stored(&db, &s.id).owner_workstream_id.is_none(),
        "§3.3 — a standalone Session has no Owner Workstream"
    );
    assert_eq!(stored(&db, &s.id).project_id.as_deref(), Some("p1"));
}

#[test]
fn an_explicit_attach_recomputes_the_cached_project_from_the_path() {
    // The explicit re-attach door: a Session moves and the cache follows in the
    // same statement. Passing a Project to it is not an option, by design.
    let (_d, db) = temp_db("reattach");
    project(&db, "p1", "one");
    project(&db, "p2", "two");
    let attacher = Scripted::new("p2");
    let (s, _) = discover(&db, &attacher, "s1", Some("/repo/two"));
    assert_eq!(stored(&db, &s.id).project_id.as_deref(), Some("p2"));

    let first = db
        .tx(|tx| insert_workspace_path_conn(tx, &normalize_path("/repo/one").unwrap(), "p1"))
        .unwrap();
    db.tx(|tx| attach_session_workspace_path_conn(tx, &s.id, Some(&first)))
        .unwrap();
    let after = stored(&db, &s.id);
    assert_eq!(after.workspace_path_id.as_deref(), Some(first.as_str()));
    assert_eq!(after.project_id.as_deref(), Some("p1"));
}

#[test]
fn project_id_writers_are_confined_to_the_derived_doors() {
    // §42.5-T2 as an executable check: `sessions.project_id` is a cache, and a
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
            // and storage/mod.rs (upsert_session's in-statement COALESCE plus
            // the batch refresh and the referential cleanup on Project delete).
            let allowed = rel == "src/storage/session_paths.rs" || rel == "src/storage/mod.rs";
            !allowed && writes_sessions_project_id(&std::fs::read_to_string(f).unwrap_or_default())
        })
        .collect::<Vec<_>>();
    assert!(
        offenders.is_empty(),
        "sessions.project_id may only be written by upsert_session's in-statement \
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

// ------------------------------------------------ 4. batch Project refresh

#[test]
fn changing_a_workspace_paths_project_refreshes_all_its_sessions() {
    let (_d, db) = temp_db("refresh");
    project(&db, "p1", "one");
    project(&db, "p2", "two");
    let attacher = Scripted::new("p1");

    let a = discover(&db, &attacher, "a", Some("/repo/app")).0;
    let b = discover(&db, &attacher, "b", Some("/repo/app")).0;
    let c = discover(&db, &attacher, "c", Some("/repo/other")).0;
    for s in [&a, &b, &c] {
        assert_eq!(stored(&db, &s.id).project_id.as_deref(), Some("p1"));
    }

    let path = path_identity_of("/repo/app").unwrap();
    db.tx(|tx| reassign_workspace_path_project_conn(tx, &path, "p2"))
        .unwrap();

    assert_eq!(stored(&db, &a.id).project_id.as_deref(), Some("p2"));
    assert_eq!(stored(&db, &b.id).project_id.as_deref(), Some("p2"));
    // Only the Sessions behind that path moved; the third still reads p1 through
    // its own path. A refresh is a projection, not a rename-everything.
    assert_eq!(stored(&db, &c.id).project_id.as_deref(), Some("p1"));
}

#[test]
fn the_batch_refresh_is_alone_sufficient_and_idempotent() {
    let (_d, db) = temp_db("refresh2");
    project(&db, "p1", "one");
    project(&db, "p2", "two");
    let attacher = Scripted::new("p1");
    let a = discover(&db, &attacher, "a", Some("/repo/app")).0;
    let path = path_identity_of("/repo/app").unwrap();
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
    assert_eq!(stored(&db, &a.id).project_id.as_deref(), Some("p2"));
    // Running it again cannot apply the change twice.
    let again = db
        .tx(|tx| refresh_sessions_project_for_path_conn(tx, &path))
        .unwrap();
    assert_eq!(again, 1);
    assert_eq!(stored(&db, &a.id).project_id.as_deref(), Some("p2"));
}

// ------------------------------- 5. ordinary event ingestion does no Project work

#[test]
fn event_ingestion_alone_does_not_change_a_project() {
    let (_d, db) = temp_db("ingest");
    project(&db, "p1", "repo");
    let dir = unique_dir("ingest");
    let file = dir.join("rollout-2026-09-13-t-1-2-3-4.jsonl");
    std::fs::write(
        &file,
        format!(
            "{}\n{}\n",
            codex_meta_line("ingest-1", "/repo/app"),
            codex_user_line("first message")
        ),
    )
    .unwrap();

    let attacher = Scripted::new("p1");
    let (mut s, _) = ensure_session_row_with(
        &db,
        &discovered("ingest-1", Some("/repo/app"), &file),
        &attacher,
    )
    .unwrap();
    assert_eq!(attacher.calls(), 1);
    let before = stored(&db, &s.id);
    let path_before = db
        .get_workspace_path(before.workspace_path_id.as_ref().unwrap())
        .unwrap()
        .unwrap();

    // Real event ingestion through the real Codex adapter.
    let ingested = ingest_session(&db, adapter_for(Agent::Codex), &mut s).unwrap();
    assert!(ingested > 0, "the fixture really ingested events");

    let after = stored(&db, &s.id);
    assert_eq!(after.project_id, before.project_id);
    assert_eq!(after.workspace_path_id, before.workspace_path_id);
    assert_eq!(
        attacher.calls(),
        1,
        "no Project work per event batch (§1.11)"
    );
    let path_after = db
        .get_workspace_path(after.workspace_path_id.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(path_after.last_seen_at, path_before.last_seen_at);
    assert_eq!(path_after.project_id, path_before.project_id);
}

// ------------------------------------------------ product-level discovery

#[test]
fn reconcile_discovers_and_attaches_sessions_to_workspace_paths() {
    let dir = unique_dir("reconcile");
    let root = dir.join("sessions");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("rollout-2026-09-13-x-1-2-3-4.jsonl"),
        format!(
            "{}\n{}\n",
            codex_meta_line("reconciled-1", "/repo/app"),
            codex_user_line("hello")
        ),
    )
    .unwrap();

    let db = Db::open(&dir.join("noending.db")).unwrap();
    project(&db, "p-repo", "repo");
    db.add_ingest_source(Agent::Codex, &root.to_string_lossy(), true)
        .unwrap();
    let attacher = Arc::new(Scripted::new("p-repo"));
    // The one test that needs the app-wide seam; every other test injects one.
    let _ = register_workspace_attacher(attacher.clone());

    let engine = SyncEngine::from_settings(&db);
    let seen = AtomicUsize::new(0);
    reconcile_with_engine(&db, &engine, &LaunchWorkspace::default(), &|_| {
        seen.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();

    let s = db
        .find_session_by_agent_id(Agent::Codex, "reconciled-1")
        .unwrap()
        .expect("discovered through the real adapter");
    assert_eq!(s.workspace_path_id, path_identity_of("/repo/app"));
    assert_eq!(s.project_id.as_deref(), Some("p-repo"));
    assert_eq!(s.cwd.as_deref(), Some("/repo/app"));
    assert!(seen.load(Ordering::SeqCst) > 0);
    assert!(attacher.calls() >= 1);
}

/// Discovery skips transcripts whose stored cursor still matches the file on
/// disk: a skipped file produces no DiscoveredSession at all, so nothing may
/// refresh the row — observable by a sentinel `last_activity_at` surviving a
/// second pass. A row WITHOUT a title is deliberately never in the skipset:
/// re-parsing its (unchanged) file is what heals titles written by older
/// builds that stopped scanning at session_meta.
#[test]
fn reconcile_skips_unchanged_files_but_reparses_untitled_rows() {
    let dir = unique_dir("skip-unchanged");
    let root = dir.join("sessions");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("rollout-2026-09-13-a-b-c-d.jsonl"),
        format!(
            "{}\n{}\n",
            codex_meta_line("skip-1", "/repo/app"),
            codex_user_line("first user message")
        ),
    )
    .unwrap();

    let db = Db::open(&dir.join("noending.db")).unwrap();
    // The global attacher is process state another test in this binary may
    // have registered; "p-repo" existing keeps that seam working here too.
    project(&db, "p-repo", "repo");
    db.add_ingest_source(Agent::Codex, &root.to_string_lossy(), true)
        .unwrap();
    let engine = SyncEngine::from_settings(&db);
    let reconcile = || reconcile_with_engine(&db, &engine, &LaunchWorkspace::default(), &|_| {});

    reconcile().unwrap();
    let s = db
        .find_session_by_agent_id(Agent::Codex, "skip-1")
        .unwrap()
        .expect("ingested on the first pass");
    assert!(s.title.is_some(), "the fixture yields a title");

    // Pass 2 over the unchanged file: discovery must skip the parse entirely,
    // so nothing overwrites the sentinel.
    db.write()
        .execute(
            "UPDATE sessions SET last_activity_at = 'sentinel' WHERE id = ?1",
            rusqlite::params![s.id],
        )
        .unwrap();
    reconcile().unwrap();
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
    reconcile().unwrap();
    let healed = stored(&db, &s.id);
    assert!(
        healed.title.is_some(),
        "untitled rows are re-parsed and healed"
    );
}

// ───────────────────────────────────────────────────────── 方案 §42.5 T1–T4
//
// The mechanical anti-drift assertions. `project_id_writers_are_confined_to_the_
// derived_doors` above is T2; these are the rest of the same family, and they
// live beside it because a guard nobody can find is a guard nobody keeps.
//
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
/// (§43.9-3), and a substring grep would read that as them being registered.
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

/// §42.5-T1 — the retired Project / Session / Workstream commands are not
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
        "§11: these left the product API and must not be re-registered: {leaked:?}"
    );
    // The read side of the same freeze: the UI still gets exactly three Project
    // commands, and `list_sessions` is the Session list.
    for kept in ["list_projects", "get_project_detail", "rename_project"] {
        assert!(
            live.iter().any(|e| e.ends_with(&format!("::{kept}"))),
            "{kept} must stay registered (§11)"
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

/// §42.5-T3 — every product write to `workstreams` names `lifecycle`
/// explicitly (§42.2-E2: the column default is not a safety net for a field the
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
    assert!(offenders.is_empty(), "§42.5-T3 / §5.7: {offenders:?}");
}

/// §42.5-T4 — `git` is reached only through the executable resolver (M10: a
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
        "§42.3-M9/M10/M11 put exactly one place allowed to run git: {offenders:?}"
    );
    // Prove the scan reaches the file that is allowed to do it: if this assert
    // ever fails, the walker is broken and every assertion above is vacuous.
    let resolver = std::fs::read_to_string(manifest.join("src/workspace/resolver.rs")).unwrap();
    assert!(code_only(&resolver).contains("--git-common-dir"));
}

/// §42.3-M31 + §43.9-2 — no launcher entry point may resolve a launch without
/// being told the NoEnding Home. The Home-less spellings were the risk: with
/// `default_workspace: None`, §13's third tier is simply absent, and M30's
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
        "§13 tier 3 must be injected, never defaulted: {offenders:?}"
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
            "{gone} is a Home-less launcher entry point and was removed (§43.8 E-(f))"
        );
    }
}

/// §2/§3, and the §25 row "migration occurs before DB open": the Home pointer
/// decides *which file* `Db::open` is handed, so a relocation that has not been
/// applied yet must not be skipped past. Breaks if `Db::open` is hoisted above
/// `prepare_home`, which is precisely the §41 failure mode §43.4-4 exists to
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
        "§3: relocation runs inside prepare_home, so it must precede opening the db ({home} vs {db})"
    );
}

/// §1.10 — a Session is a member of a Project because its own path says so.
/// The derived cache is a cache: a row still holding only a v11 hand-attached
/// label is not in that Project, and `list_sessions` must agree with
/// `get_project_detail` instead of disagreeing with it (§42.3-M29).
#[test]
fn a_session_with_only_the_cached_project_id_is_not_a_project_member() {
    let (_d, db) = temp_db("cache-is-not-membership");
    project(&db, "p-real", "Real");
    let attacher = Scripted::new("p-real");
    let (member, _) = discover(&db, &attacher, "s-member", Some("/cache-authority/repo"));
    let (ghost, _) = discover(&db, &attacher, "s-ghost", Some("/cache-authority/repo"));
    // Make the second row look like what it is in a real upgraded database: a
    // v11 Session whose only Project fact was the label a person attached by
    // hand, and no resolvable path. `attach_session_workspace_path_conn(None)`
    // is the door that clears both, so the label is re-written afterwards the
    // way the v11 data already had it.
    db.tx(|tx| attach_session_workspace_path_conn(tx, &ghost.id, None))
        .unwrap();
    db.write()
        .execute(
            "UPDATE sessions SET project_id = 'p-real' WHERE id = ?1",
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
            project_id: Some("p-real".into()),
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
        db.get_session(&ghost.id)
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some("p-real"),
        "the stale cache value is left alone — this is a read-side fix, not a rewrite"
    );
}

/// §37.15 — a Session's title comes from the first source that has one:
/// the transcript's own title, else the first user text, else the first agent
/// text. The order is the whole point (an Agent's own title beats a derived
/// one, and a human prompt beats a machine reply), so it is pinned here rather
/// than left to whichever adapter happens to fill which field.
#[test]
fn a_session_title_prefers_the_native_then_the_user_then_the_agent() {
    let dir = unique_dir("title-order");
    let db = Db::open(&dir.join("noending.db")).unwrap();
    project(&db, "p-repo", "repo");
    let raw = dir.join("raw.jsonl");

    let with = |native: Option<&str>, user: Option<&str>, agent: Option<&str>| DiscoveredSession {
        agent: Agent::Codex,
        agent_session_id: format!("t-{native:?}-{user:?}-{agent:?}"),
        path: raw.clone(),
        cwd: None,
        started_at: None,
        last_activity_at: None,
        native_title: native.map(Into::into),
        first_user_text: user.map(Into::into),
        first_agent_text: agent.map(Into::into),
        parent_agent_session_id: None,
    };

    let cases = [
        (
            with(
                Some("原生标题"),
                Some("第一句用户话"),
                Some("第一句 agent 话"),
            ),
            Some("原生标题"),
        ),
        (
            with(None, Some("第一句用户话"), Some("第一句 agent 话")),
            Some("第一句用户话"),
        ),
        (
            with(None, None, Some("第一句 agent 话")),
            Some("第一句 agent 话"),
        ),
        (with(None, None, None), None),
        // A machine blob is not a title, so the next source gets its turn.
        (with(None, None, Some("{\"outcome\":\"allow\"}")), None),
    ];
    for (d, expected) in cases {
        let (session, _) = ensure_session_row_with(&db, &d, &Scripted::new("p-repo")).unwrap();
        assert_eq!(
            session.title.as_deref(),
            expected,
            "agent_session_id={}",
            d.agent_session_id
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// §37.19 — discovery skips a transcript whose cursor says "unchanged", so a
/// Session ingested before its directory was registered used to keep
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

    // A Session row as `ensure_session_row_with` leaves it when the seam had
    // nothing to answer with — which is what a pre-registration ingest does.
    let orphan = |cwd: &str, id: &str| Session {
        id: id.into(),
        agent: Agent::Codex,
        agent_session_id: format!("a-{id}"),
        title: Some("t".into()),
        cwd: Some(cwd.into()),
        workspace_path_id: None,
        project_id: None,
        owner_workstream_id: None,
        raw_path: format!("/raw/{id}.jsonl"),
        parent_agent_session_id: None,
        started_at: None,
        last_activity_at: None,
        trashed_at: None,
    };
    db.upsert_session(&orphan("/repo/late", "late")).unwrap();
    db.upsert_session(&orphan("/repo/nobody-registered", "stranger"))
        .unwrap();
    assert_eq!(stored(&db, "late").workspace_path_id, None);

    assert_eq!(attach_sessions_to_registered_paths(&db).unwrap(), 1);

    let late = stored(&db, "late");
    assert_eq!(late.workspace_path_id.as_deref(), Some(path_id.as_str()));
    assert_eq!(
        late.project_id.as_deref(),
        Some("p1"),
        "the derived Project cache moves in the same statement, not after it"
    );
    assert_eq!(
        stored(&db, "stranger").workspace_path_id,
        None,
        "an unregistered directory is not registered as a side effect"
    );
    assert_eq!(
        attach_sessions_to_registered_paths(&db).unwrap(),
        0,
        "idempotent: once attached, the Session leaves the set"
    );
}
