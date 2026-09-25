//! Session single-Owner invariant suite (方案 §43).
//!
//! Every test here pins one rule of the converged model:
//!
//! ```text
//! Session ── 0..1 ──> Owner Workstream
//! ```
//!
//! The point of the file is not the happy path alone — it is that the two
//! directions stay INDEPENDENT: setting an Owner never mutates the Workstream's
//! path list, and mutating the path list never changes an Owner (§5).
//!
//! Logical-Session shape (重构方案): a Session is keyed by its ROOT member's
//! Resume identity (`root_agent_session_id`); conversation is seeded through
//! the production commit path (`commit_member_ingest`) and read back through
//! `get_messages` — there are no session events any more.

use noending::domain::{
    Agent, ParsedSessionMessage, Session, SessionMessage, SessionMessageRole, Workstream,
};
use noending::launcher::{LaunchWorkspace, SessionLauncher};
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, now, Db};
use noending::sync::{ContextExtractor, ContextMutation, ExtractOutput, SyncEngine};
use noending::workspace::workstream::{
    add_workstream_path, archive_workstream, create_workstream, delete_workstream_permanently,
    remove_workstream_path,
};
use noending::workspace::WorkspaceAttaching;
use rusqlite::Connection;
use std::ops::Deref;
mod support;

// ---------------------------------------------------------------- harness

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
    let dir = std::env::temp_dir().join(format!("noending-owner-{}-{}", tag, new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).unwrap();
    TestDb { db: Some(db), dir }
}

/// Real directories inside the test's own temp dir, attached to one Project
/// through the production WorkspacePath door. `ensure_path` refuses a path that
/// is not an existing directory, exactly like the real attacher's usability
/// gate — a cwd tier that does not exist must not be launched into.
struct TempPaths {
    root: std::path::PathBuf,
    project_id: String,
}

impl TempPaths {
    /// `project_id` must already exist: `workspace_paths.project_id` is a real
    /// foreign key, and a path attached to a phantom Project is exactly the
    /// kind of fabricated fact the workspace layer refuses to store.
    fn new(root: &std::path::Path, project_id: &str) -> Self {
        Self {
            root: root.to_path_buf(),
            project_id: project_id.to_string(),
        }
    }

    fn dir(&self, name: &str) -> String {
        let p = self.root.join(name);
        std::fs::create_dir_all(&p).unwrap();
        p.to_string_lossy().to_string()
    }
}

impl WorkspaceAttaching for TempPaths {
    fn ensure_path(&self, conn: &Connection, raw: &str) -> noending::error::Result<Option<String>> {
        if !std::path::Path::new(raw).is_dir() {
            return Ok(None);
        }
        Ok(Some(insert_workspace_path_conn(
            conn,
            raw,
            &self.project_id,
        )?))
    }
}

fn seed_project(db: &Db, id: &str) {
    db.upsert_project(&support::project(id.to_string(), "Owner Model Test"))
        .unwrap();
}

fn workstream(db: &Db, title: &str) -> Workstream {
    let w = Workstream {
        id: new_id(),
        title: title.into(),
        description: String::new(),
        lifecycle: "active".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    w
}

/// A Logical Session + its ROOT member. The root source is a REAL file inside
/// the test's temp dir, because resume preparation refuses a Session whose
/// ROOT member source is not present on disk (§17.2) — a resumable fixture
/// must be resumable.
fn session(db: &TestDb, cwd: Option<&str>) -> Session {
    session_of_agent(db, Agent::Codex, cwd)
}

fn session_of_agent(db: &TestDb, agent: Agent, cwd: Option<&str>) -> Session {
    let root_id = format!("root-{}", new_id());
    let raw = db.dir.join(format!("raw-{}.jsonl", new_id()));
    std::fs::write(&raw, "").unwrap();
    let ts = now();
    let (id, _) = db
        .upsert_logical_session(
            agent,
            &root_id,
            Some("Owner Model"),
            cwd,
            None,
            None,
            Some(&ts),
            Some(&ts),
        )
        .unwrap();
    support::ensure_root_member(db, &id, agent, &root_id, &raw.to_string_lossy());
    db.get_session(&id).unwrap().unwrap()
}

fn owner_of(db: &Db, session_id: &str) -> Option<String> {
    db.get_session(session_id)
        .unwrap()
        .unwrap()
        .owner_workstream_id
}

// ------------------------------------------------- §43 Session Owner basics

#[test]
fn newly_ingested_session_has_no_owner() {
    let db = open_db("owner-default");
    let s = session(&db, None);
    assert!(owner_of(&db, &s.id).is_none());
}

#[test]
fn setting_and_replacing_the_owner_is_single_valued() {
    let db = open_db("owner-replace");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");
    let s = session(&db, None);

    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(a.id.as_str()));

    // Replacing is a REPLACEMENT, not an addition: A + B is unrepresentable.
    db.set_session_owner(&s.id, Some(&b.id)).unwrap();
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(b.id.as_str()));
    assert!(db.sessions_for_workstream(&a.id).unwrap().is_empty());
    assert_eq!(db.sessions_for_workstream(&b.id).unwrap().len(), 1);
}

#[test]
fn clearing_the_owner_returns_to_null() {
    let db = open_db("owner-clear");
    let a = workstream(&db, "A");
    let s = session(&db, None);

    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    db.set_session_owner(&s.id, None).unwrap();
    assert!(owner_of(&db, &s.id).is_none());
}

#[test]
fn setting_the_same_owner_twice_is_idempotent() {
    let db = open_db("owner-idempotent");
    let a = workstream(&db, "A");
    let s = session(&db, None);

    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(a.id.as_str()));
    assert_eq!(db.sessions_for_workstream(&a.id).unwrap().len(), 1);
}

#[test]
fn unknown_workstream_and_unknown_session_are_refused() {
    let db = open_db("owner-refuse");
    let s = session(&db, None);
    assert!(db.set_session_owner(&s.id, Some("no-such-ws")).is_err());
    assert!(db.set_session_owner("no-such-session", None).is_err());
}

// --------------------------------------------------------- §43 lifecycle

#[test]
fn deleting_a_workstream_clears_the_owner_but_keeps_the_session() {
    let db = open_db("owner-ws-delete");
    let a = workstream(&db, "A");
    let s = session(&db, None);
    db.set_session_owner(&s.id, Some(&a.id)).unwrap();

    archive_workstream(&db, &a.id).unwrap();
    delete_workstream_permanently(&db, &a.id).unwrap();

    let after = db.get_session(&s.id).unwrap().expect("Session survives");
    assert!(after.owner_workstream_id.is_none());
    assert!(db.get_workstream(&a.id).unwrap().is_none());
}

#[test]
fn deleting_a_session_leaves_its_workstream_untouched() {
    let db = open_db("owner-session-delete");
    let a = workstream(&db, "A");
    let s = session(&db, None);
    db.set_session_owner(&s.id, Some(&a.id)).unwrap();

    db.tx(|tx| {
        tx.execute(
            "DELETE FROM sessions WHERE id = ?1",
            rusqlite::params![s.id],
        )?;
        Ok(())
    })
    .unwrap();

    assert!(db.get_workstream(&a.id).unwrap().is_some());
}

#[test]
fn trash_and_restore_preserve_the_owner() {
    let db = open_db("owner-trash");
    let a = workstream(&db, "A");
    let s = session(&db, None);
    db.set_session_owner(&s.id, Some(&a.id)).unwrap();

    noending::lifecycle::trash_session(&db, &s.id).unwrap();
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(a.id.as_str()));
    // Trashed Sessions are inactive: they leave the Workstream's list (§11).
    assert!(db.sessions_for_workstream(&a.id).unwrap().is_empty());

    noending::lifecycle::restore_session(&db, &s.id).unwrap();
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(a.id.as_str()));
    assert_eq!(db.sessions_for_workstream(&a.id).unwrap().len(), 1);
}

// ------------------------------------------- §43 workspace independence (§5)

#[test]
fn removing_a_workstream_path_does_not_change_the_owner() {
    let db = open_db("owner-path-remove");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir = paths.dir("repo-a");
    let w = create_workstream(&db, &paths, "A", "", &[dir.clone()])
        .unwrap()
        .workstream;
    let s = session(&db, Some(&dir));
    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    let row = db.list_workstream_paths(&w.id).unwrap().remove(0);
    remove_workstream_path(&db, &w.id, &row.id).unwrap();

    assert!(db.list_workstream_paths(&w.id).unwrap().is_empty());
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(w.id.as_str()));
}

#[test]
fn adding_a_workstream_path_does_not_change_the_owner() {
    let db = open_db("owner-path-add");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let first = paths.dir("repo-a");
    let second = paths.dir("repo-b");
    let w = create_workstream(&db, &paths, "A", "", &[first.clone()])
        .unwrap()
        .workstream;
    let s = session(&db, Some(first.as_str()));
    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    add_workstream_path(&db, &paths, &w.id, &second).unwrap();

    assert_eq!(db.list_workstream_paths(&w.id).unwrap().len(), 2);
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(w.id.as_str()));
}

#[test]
fn changing_the_session_cwd_or_project_does_not_change_the_owner() {
    let db = open_db("owner-cwd-drift");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let a_dir = paths.dir("repo-a");
    let b_dir = paths.dir("repo-b");
    let w = workstream(&db, "A");
    let s = session(&db, Some(&a_dir));
    noending::workspace::session::attach_session_conn(&db.read(), &paths, &s.id, Some(&a_dir))
        .unwrap();
    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    // The Session's physical facts move to another directory (and therefore
    // another WorkspacePath / Project). Ownership is a separate statement.
    let b_path = noending::workspace::path_identity_of(&b_dir).unwrap();
    db.tx(|tx| noending::workspace::session::move_session_to_path_conn(tx, &s.id, &b_path))
        .unwrap();

    let after = db.get_session(&s.id).unwrap().unwrap();
    assert_eq!(after.workspace_path_id.as_deref(), Some(b_path.as_str()));
    assert_eq!(after.owner_workstream_id.as_deref(), Some(w.id.as_str()));
}

// ---------------------------------------------- §43 launcher → matched intent

/// §43-14 — a New Session launched for Workstream A records A on its
/// LaunchIntent, and when discovery matches that intent the discovered Session
/// inherits exactly that one owner.
#[test]
fn matched_launch_intent_gives_the_discovered_session_that_owner() {
    use noending::domain::{launch_status, LaunchIntent};

    let db = open_db("owner-intent-match");
    let a = workstream(&db, "A");
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(a.id.clone()),
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    let s = session(&db, None);
    let matched =
        noending::launcher::try_match_launch_intents_in(&db, &s, &LaunchWorkspace::default())
            .unwrap();
    assert!(matched, "the only pending intent wins outright");
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(a.id.as_str()));
    assert_eq!(db.sessions_for_workstream(&a.id).unwrap().len(), 1);

    let stored = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert_eq!(stored.status, launch_status::MATCHED);
    assert_eq!(stored.matched_session_id.as_deref(), Some(s.id.as_str()));
}

/// §43-15 — a standalone launch (no owner chosen) matches too, and the matched
/// Session stays unowned. Matching must not invent an owner.
#[test]
fn matched_ownerless_intent_leaves_the_session_unowned() {
    use noending::domain::{launch_status, LaunchIntent};

    let db = open_db("owner-intent-standalone");
    let a = workstream(&db, "A");
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: None,
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    let s = session(&db, None);
    assert!(
        noending::launcher::try_match_launch_intents_in(&db, &s, &LaunchWorkspace::default(),)
            .unwrap()
    );
    assert!(owner_of(&db, &s.id).is_none());
    assert!(db.sessions_for_workstream(&a.id).unwrap().is_empty());
}

// ------------------------------------------------------------ §43 launcher

#[test]
fn new_session_launch_intent_carries_the_chosen_owner() {
    let db = open_db("owner-launch-new");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir = paths.dir("repo-a");
    let w = create_workstream(&db, &paths, "A", "", &[dir.clone()])
        .unwrap()
        .workstream;

    let workspace = LaunchWorkspace {
        default_workspace: None,
    };
    let prepared = SessionLauncher {
        runtime_dir: db.dir.join("runtime"),
    }
    .prepare_new_in(&db, Agent::Codex, Some(&w.id), None, &workspace)
    .unwrap();

    assert_eq!(prepared.owner_workstream_id.as_deref(), Some(w.id.as_str()));
    // §13 — with no explicit cwd the Owner's first usable path is the tier.
    assert_eq!(prepared.cwd.as_deref(), Some(dir.as_str()));
}

#[test]
fn standalone_new_session_has_no_owner_and_falls_back_to_the_default_workspace() {
    let db = open_db("owner-launch-standalone");
    let default = db.dir.join("default-ws");
    let workspace = LaunchWorkspace {
        default_workspace: Some(default.to_string_lossy().to_string()),
    };
    let prepared = SessionLauncher {
        runtime_dir: db.dir.join("runtime"),
    }
    .prepare_new_in(&db, Agent::Codex, None, None, &workspace)
    .unwrap();

    assert!(prepared.owner_workstream_id.is_none());
    assert_eq!(
        prepared.cwd.as_deref(),
        Some(default.to_string_lossy().as_ref())
    );
}

#[test]
fn resume_uses_the_sessions_current_owner_and_needs_no_extra_argument() {
    let db = open_db("owner-launch-resume");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir = paths.dir("repo-a");
    let w = create_workstream(&db, &paths, "A", "", &[dir.clone()])
        .unwrap()
        .workstream;
    let s = session(&db, Some(&dir));
    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    let workspace = LaunchWorkspace {
        default_workspace: None,
    };
    let prepared = SessionLauncher {
        runtime_dir: db.dir.join("runtime"),
    }
    .prepare_resume_in(&db, &s.id, &workspace)
    .unwrap();

    assert_eq!(prepared.owner_workstream_id.as_deref(), Some(w.id.as_str()));
    assert_eq!(prepared.cwd.as_deref(), Some(dir.as_str()));
}

#[test]
fn resume_falls_back_to_the_owners_path_when_the_session_cwd_is_gone() {
    let db = open_db("owner-resume-fallback");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir = paths.dir("repo-a");
    let w = create_workstream(&db, &paths, "A", "", &[dir.clone()])
        .unwrap()
        .workstream;
    // A recorded cwd that no longer exists on disk. The ROOT SOURCE is still
    // present (the fixture writes a real file) — only the cwd is gone.
    let s = session(&db, Some(&db.dir.join("vanished").to_string_lossy()));
    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    let workspace = LaunchWorkspace {
        default_workspace: None,
    };
    let prepared = SessionLauncher {
        runtime_dir: db.dir.join("runtime"),
    }
    .prepare_resume_in(&db, &s.id, &workspace)
    .unwrap();

    assert_eq!(prepared.cwd.as_deref(), Some(dir.as_str()));
    assert!(prepared.cwd_resolution.fallback);
}

#[test]
fn ownerless_session_with_no_usable_cwd_resolves_to_unresolved() {
    let db = open_db("owner-unresolved");
    let s = session(&db, Some(&db.dir.join("vanished").to_string_lossy()));
    let workspace = LaunchWorkspace {
        default_workspace: None,
    };
    let resolution = noending::launcher::resolve_resume_cwd(&db, &s.id, None, &workspace).unwrap();
    assert_eq!(resolution.source, noending::launcher::CwdSource::Unresolved);
}

// ---------------------------------------------------------------- §43 sync

/// A stub extractor: it never looks at the transcript, it just records the
/// workstream it was routed to and proposes one `add` there. Enough to pin the
/// routing contract without depending on heuristic quality.
struct RoutingProbe {
    seen: std::sync::Mutex<Vec<String>>,
}

impl ContextExtractor for RoutingProbe {
    fn name(&self) -> String {
        "routing-probe".into()
    }

    fn extract(
        &self,
        _session: &Session,
        _messages: &[&SessionMessage],
        workstream_id: &str,
        _inputs: &noending::sync::extractor::PromptInputs,
    ) -> noending::error::Result<ExtractOutput> {
        self.seen.lock().unwrap().push(workstream_id.to_string());
        Ok(ExtractOutput::mutations(vec![ContextMutation::Add {
            workstream_id: workstream_id.to_string(),
            item_kind: "decision".into(),
            title: "routed".into(),
            content: "routed".into(),
            source_refs: vec![],
            authority: "agent_statement".into(),
        }]))
    }
}

/// Write `texts` through the production ingestion door (`commit_member_ingest`
/// on the Session's ROOT member), so the stored messages carry app-assigned
/// identity and sequence exactly like a real ingest.
fn seed_messages(db: &Db, s: &Session, texts: &[&str]) -> Vec<SessionMessage> {
    let root = db
        .root_member_for_session(&s.id)
        .unwrap()
        .expect("fixture session has a ROOT member");
    let parsed: Vec<ParsedSessionMessage> = texts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            support::parsed_message(format!("src-{}", i + 1), SessionMessageRole::User, *text)
        })
        .collect();
    db.commit_member_ingest(&s.id, &root.id, &parsed, None, &support::seed_source(0))
        .unwrap()
}

/// The whole stored conversation, in sequence order.
fn conversation(db: &Db, s: &Session) -> Vec<SessionMessage> {
    db.get_messages(&s.id, None, 10_000).unwrap()
}

/// Drive one full prepare → extract → commit cycle with an injected extractor,
/// so the routing assertions do not depend on the heuristic's content rules.
fn run_with_probe(
    db: &Db,
    engine: &SyncEngine,
    probe: &RoutingProbe,
    s: &Session,
) -> noending::error::Result<Option<noending::sync::SyncJobOutput>> {
    let messages = conversation(db, s);
    let to = messages.last().map(|m| m.sequence).unwrap_or(0);
    let Some(pre) = engine.prepare(db, s, &messages, 0, to)? else {
        return Ok(None);
    };
    let refs: Vec<&SessionMessage> = pre.messages.iter().collect();
    let ws = pre.owner_workstream_id.clone().unwrap_or_default();
    let out = probe.extract(s, &refs, &ws, &pre.inputs)?;
    let job = engine.commit(db, s, &pre, out.mutations, "probe", out.diagnostics)?;
    Ok(Some(job))
}

#[test]
fn sync_routes_to_the_owner_only_and_never_defaults_it() {
    let db = open_db("sync-owner-route");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");
    let s = session(&db, None);
    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    seed_messages(
        &db,
        &s,
        &["我们决定采用单 Owner 模型，这个方案就这么定了，请照着实现。"],
    );

    let engine = SyncEngine::default();
    let probe = RoutingProbe {
        seen: std::sync::Mutex::new(Vec::new()),
    };
    let job = run_with_probe(&db, &engine, &probe, &s)
        .unwrap()
        .expect("run happens");
    assert_eq!(job.status, "ok");

    assert_eq!(probe.seen.lock().unwrap().as_slice(), &[a.id.clone()]);
    assert_eq!(db.items_for_workstream(&a.id, false).unwrap().len(), 1);
    assert!(db.items_for_workstream(&b.id, false).unwrap().is_empty());
}

#[test]
fn ownerless_session_does_not_run_context_processing_or_advance_the_frontier() {
    let db = open_db("sync-ownerless");
    let s = session(&db, None);
    let messages = seed_messages(
        &db,
        &s,
        &["我们决定采用单 Owner 模型，这个方案就这么定了，请照着实现。"],
    );
    let engine = SyncEngine::default();

    // The messages ARE stored: an ownerless session keeps ingesting (§21).
    assert_eq!(messages.len(), 1);
    assert_eq!(db.ingested_message_sequence(&s.id).unwrap(), 1);

    // prepare() itself refuses to produce a plan without an Owner.
    let prepared = engine.prepare(&db, &s, &messages, 0, 1).unwrap();
    assert!(prepared.is_none(), "no owner ⇒ no extraction plan");

    let out = engine.run_session_sync(&db, &s, &messages, 0, 1).unwrap();
    assert_eq!(out.applied, 0);
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        0,
        "the Context frontier stays frozen while the Session is ownerless"
    );

    // Assigning an Owner replays the backlog from the frozen frontier.
    let a = workstream(&db, "A");
    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    let replay = engine.run_session_sync(&db, &s, &messages, 0, 1).unwrap();
    assert_eq!(replay.status, "ok");
    assert!(
        replay.applied >= 1,
        "the frozen backlog is processed once an Owner exists"
    );
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        1
    );
}

#[test]
fn owner_change_during_extraction_makes_the_run_stale() {
    let db = open_db("sync-owner-cas");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");
    let s = session(&db, None);
    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    let messages = seed_messages(
        &db,
        &s,
        &["我们决定采用单 Owner 模型，这个方案就这么定了，请照着实现。"],
    );
    let engine = SyncEngine::default();

    let pre = engine
        .prepare(&db, &s, &messages, 0, 1)
        .unwrap()
        .expect("plan");
    assert_eq!(pre.owner_workstream_id.as_deref(), Some(a.id.as_str()));

    // The user re-homes the Session while extraction runs without the lock.
    db.set_session_owner(&s.id, Some(&b.id)).unwrap();

    let job = engine
        .commit(
            &db,
            &s,
            &pre,
            vec![ContextMutation::Add {
                workstream_id: a.id.clone(),
                item_kind: "decision".into(),
                title: "t".into(),
                content: "c".into(),
                source_refs: vec![],
                authority: "agent_statement".into(),
            }],
            "probe",
            vec![],
        )
        .unwrap();
    assert_eq!(job.status, "stale");
    assert_eq!(job.applied, 0);
    // §20 — a stale run must not write Context and must not move the frontier.
    assert!(db.items_for_workstream(&a.id, false).unwrap().is_empty());
    assert!(db.items_for_workstream(&b.id, false).unwrap().is_empty());
    assert_eq!(
        db.get_context_state(&s.id)
            .unwrap()
            .processed_message_sequence,
        0
    );
}

#[test]
fn sync_never_invents_an_owner() {
    let db = open_db("sync-no-auto");
    let a = workstream(&db, "单 Owner 模型重构");
    let s = session(&db, None);
    let messages = seed_messages(
        &db,
        &s,
        &["单 Owner 模型重构：我们决定采用单 Owner 模型，这个方案就这么定了。"],
    );
    let engine = SyncEngine::default();

    // Even though the transcript mentions the Workstream title verbatim, the
    // old keyword auto-classification is gone: a run cannot mint an Owner.
    assert!(engine.prepare(&db, &s, &messages, 0, 1).unwrap().is_none());
    assert!(owner_of(&db, &s.id).is_none());
    assert!(db.sessions_for_workstream(&a.id).unwrap().is_empty());
}

// ----------------------------------------------------------- §43 workstream

#[test]
fn a_session_never_appears_in_two_workstream_lists() {
    let db = open_db("ws-mutex");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");
    let s = session(&db, None);

    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    db.set_session_owner(&s.id, Some(&b.id)).unwrap();

    let in_a = db.sessions_for_workstream(&a.id).unwrap();
    let in_b = db.sessions_for_workstream(&b.id).unwrap();
    assert!(in_a.is_empty());
    assert_eq!(in_b.len(), 1);
    assert_eq!(in_b[0].id, s.id);
}

// ------------------------------------------- §5.1 the owner side-effect rule

/// §5.1 — `set_session_owner` writes `sessions.owner_workstream_id` and
/// NOTHING else. The Workstream's path list is untouched (no path is appended
/// on the Session's behalf), and the Session's cwd, `workspace_path_id` and
/// derived `project_id` stay exactly as they were.
#[test]
fn setting_the_owner_never_adds_a_workstream_path_or_moves_the_session() {
    let db = open_db("owner-no-side-effects");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir = paths.dir("repo-a");
    let w = workstream(&db, "A");
    let s = session(&db, Some(&dir));
    noending::workspace::session::attach_session_conn(&db.read(), &paths, &s.id, Some(&dir))
        .unwrap();

    let before = db.get_session(&s.id).unwrap().unwrap();
    assert!(before.workspace_path_id.is_some(), "attached for the test");
    assert_eq!(before.project_id.as_deref(), Some("p1"));

    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(w.id.as_str()));
    assert!(
        db.list_workstream_paths(&w.id).unwrap().is_empty(),
        "the user never chose a directory, so the list must not grow"
    );
    let after = db.get_session(&s.id).unwrap().unwrap();
    assert_eq!(after.cwd, before.cwd);
    assert_eq!(after.workspace_path_id, before.workspace_path_id);
    assert_eq!(after.project_id, before.project_id);
}

/// The clearing direction is equally inert: dropping the Owner leaves the
/// Session's physical facts and the Workstream's path list alone.
#[test]
fn clearing_the_owner_moves_nothing() {
    let db = open_db("owner-clear-inert");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir = paths.dir("repo-a");
    let w = workstream(&db, "A");
    let s = session(&db, Some(&dir));
    noending::workspace::session::attach_session_conn(&db.read(), &paths, &s.id, Some(&dir))
        .unwrap();
    db.set_session_owner(&s.id, Some(&w.id)).unwrap();

    let before = db.get_session(&s.id).unwrap().unwrap();
    db.set_session_owner(&s.id, None).unwrap();

    assert!(owner_of(&db, &s.id).is_none());
    assert!(db.list_workstream_paths(&w.id).unwrap().is_empty());
    let after = db.get_session(&s.id).unwrap().unwrap();
    assert_eq!(after.cwd, before.cwd);
    assert_eq!(after.workspace_path_id, before.workspace_path_id);
}

// ------------------------------------------ §43-23 Workstream statistics

/// §43-23 — a Workstream's stats count exactly the Sessions that OWN it. An
/// unowned Session is counted by no Workstream, and re-homing a Session moves
/// its count instead of duplicating it (§41).
#[test]
fn workstream_session_stats_count_only_owned_sessions() {
    let db = open_db("ws-stats-owner");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");
    let owned_a = session(&db, None);
    let owned_a2 = session(&db, None);
    let stray = session(&db, None);
    db.set_session_owner(&owned_a.id, Some(&a.id)).unwrap();
    db.set_session_owner(&owned_a2.id, Some(&a.id)).unwrap();

    let (count, latest, _) = db.workstream_session_stats(&a.id).unwrap();
    assert_eq!(count, 2);
    assert!(latest.is_some());
    assert_eq!(db.workstream_session_stats(&b.id).unwrap().0, 0);
    assert!(
        db.sessions_for_workstream(&a.id)
            .unwrap()
            .iter()
            .all(|s| s.id != stray.id),
        "an unowned Session belongs to no Workstream"
    );

    // Re-home one Session: the count MOVES, it is never duplicated.
    db.set_session_owner(&owned_a2.id, Some(&b.id)).unwrap();
    assert_eq!(db.workstream_session_stats(&a.id).unwrap().0, 1);
    assert_eq!(db.workstream_session_stats(&b.id).unwrap().0, 1);
}

// --------------------------------- §43 the Owner is the only write target

/// §3.3/§20 — a run writes its Owner Workstream and nothing else.
///
/// `update` / `supersede` / `resolve` name their target by `item_id`, and that
/// id comes from model output: untrusted text that may echo an identifier
/// found anywhere in the transcript. The boundary therefore lives in the merge
/// engine, on the deterministic side — an id from another Workstream is a
/// skip, not a way in.
#[test]
fn mutations_outside_the_owner_are_skipped_not_written() {
    use noending::sync::merge::MergeEngine;
    use noending::sync::{create_item, MergeContext};

    let db = open_db("merge-owner-boundary");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");

    // B owns an item; the run below is owned by A and never names A's items.
    let foreign = create_item(
        &db,
        &b.id,
        "decision",
        "属于 B 的决定",
        "B 的内容",
        "agent_inferred",
        "session_message",
        &[],
        None,
        "sync:other",
    )
    .unwrap();
    let before = db.get_item(&foreign.id).unwrap().unwrap();

    let ctx = MergeContext {
        run_id: new_id(),
        runtime: "probe".into(),
        workstream_id: a.id.clone(),
    };
    let mutations = vec![
        ContextMutation::Add {
            workstream_id: b.id.clone(),
            item_kind: "decision".into(),
            title: "偷渡进 B".into(),
            content: "x".into(),
            source_refs: vec![],
            authority: "agent_statement".into(),
        },
        ContextMutation::Update {
            item_id: foreign.id.clone(),
            title: "被改写".into(),
            content: "y".into(),
            source_refs: vec![],
            authority: "agent_statement".into(),
        },
        ContextMutation::Supersede {
            item_id: foreign.id.clone(),
            title: "被取代".into(),
            content: "z".into(),
            source_refs: vec![],
            authority: "agent_statement".into(),
        },
        ContextMutation::Resolve {
            item_id: foreign.id.clone(),
            source_refs: vec![],
        },
        // A conflict that links B's item into A's workstream: refused whole,
        // because the linked item is not this run's to drag across.
        ContextMutation::Conflict {
            workstream_id: a.id.clone(),
            item_id: foreign.id.clone(),
            title: "跨库冲突".into(),
            content: "c".into(),
            source_refs: vec![],
            reason: "模型判定与用户约束可能冲突".into(),
        },
        // The control: the same run CAN write its own Owner.
        ContextMutation::Add {
            workstream_id: a.id.clone(),
            item_kind: "decision".into(),
            title: "合法写入".into(),
            content: "own".into(),
            source_refs: vec![],
            authority: "agent_statement".into(),
        },
    ];

    let applied = db
        .tx(|tx| {
            let mut n = 0;
            for m in &mutations {
                if MergeEngine.apply(tx, m, &ctx)? {
                    n += 1;
                }
            }
            Ok(n)
        })
        .unwrap();

    assert_eq!(applied, 1, "only the Owner's own mutation is applied");

    // B is untouched: same head, same status, no new item, no conflict.
    let after = db.get_item(&foreign.id).unwrap().unwrap();
    assert_eq!(after.current_revision_id, before.current_revision_id);
    assert_eq!(after.status, before.status);
    assert_eq!(
        db.items_for_workstream(&b.id, true).unwrap().len(),
        1,
        "nothing was added to B"
    );
    assert!(db.conflicts_for_workstream(&b.id, true).unwrap().is_empty());
    assert!(
        db.conflicts_for_workstream(&a.id, true).unwrap().is_empty(),
        "the refused conflict must not land on A either"
    );
    assert_eq!(db.items_for_workstream(&a.id, true).unwrap().len(), 1);
}

// ------------------------- §43 resume reads the Owner after the sync seam

/// §21-3 — Resume preparation reads ownership AFTER its sync, never from the
/// snapshot taken before it.
///
/// Sync runs without the DB lock, so the user can move the Session to another
/// Workstream while it is in flight (sync's own owner CAS tolerates exactly
/// that). Preparing against the pre-sync Owner would deliver Workstream A's
/// directory and bundle to a Session that now belongs to B.
#[test]
fn resume_preparation_follows_the_current_owner_after_the_sync_seam() {
    let db = open_db("owner-resume-fresh");
    seed_project(&db, "p1");
    let paths = TempPaths::new(&db.dir, "p1");
    let dir_a = paths.dir("repo-a");
    let dir_b = paths.dir("repo-b");
    let a = create_workstream(&db, &paths, "A", "", &[dir_a.clone()])
        .unwrap()
        .workstream;
    let b = create_workstream(&db, &paths, "B", "", &[dir_b.clone()])
        .unwrap()
        .workstream;

    // The Session's own cwd is gone, so the OWNER decides the launch
    // directory — the only way the result can be evidence of which Owner was
    // used (§13 tier 2). The ROOT SOURCE is present (real fixture file), so
    // the resume gate passes.
    let s = session(&db, Some("/gone/workspace"));
    db.set_session_owner(&s.id, Some(&a.id)).unwrap();

    let launcher = SessionLauncher {
        runtime_dir: db.dir.join("runtime"),
    };
    let workspace = LaunchWorkspace {
        default_workspace: None,
    };

    let first = launcher
        .prepare_resume_from_current(&db, &s.id, &workspace)
        .unwrap();
    assert_eq!(first.owner_workstream_id.as_deref(), Some(a.id.as_str()));
    assert_eq!(first.cwd.as_deref(), Some(dir_a.as_str()));
    assert_eq!(first.bundle.workstream_id.as_deref(), Some(a.id.as_str()));

    // Ownership moves during the window in which sync would have been running.
    db.set_session_owner(&s.id, Some(&b.id)).unwrap();

    let second = launcher
        .prepare_resume_from_current(&db, &s.id, &workspace)
        .unwrap();
    assert_eq!(second.owner_workstream_id.as_deref(), Some(b.id.as_str()));
    assert_eq!(
        second.cwd.as_deref(),
        Some(dir_b.as_str()),
        "the cwd tier follows the CURRENT Owner"
    );
    assert_eq!(second.bundle.workstream_id.as_deref(), Some(b.id.as_str()));

    // And the end-to-end entry point (which syncs first) agrees.
    let full = launcher.prepare_resume_in(&db, &s.id, &workspace).unwrap();
    assert_eq!(full.owner_workstream_id.as_deref(), Some(b.id.as_str()));
    assert_eq!(full.cwd.as_deref(), Some(dir_b.as_str()));
}

// ------------------- §43 first discovery, whichever door found the session

/// A Codex rollout the real adapter discovers, so the source-scoped
/// reconcile below runs the production discovery → row → ingest path.
fn codex_rollout(dir: &std::path::Path, session_id: &str, cwd: &str) -> std::path::PathBuf {
    let file = dir.join(format!("rollout-2026-09-22T21-17-07-{session_id}.jsonl"));
    std::fs::write(
        &file,
        format!(
            "{}\n{}\n",
            format_args!(
                r#"{{"ordinal":0,"type":"session_meta","payload":{{"id":"{session_id}","cwd":"{cwd}","timestamp":"2026-09-22T21:17:07Z"}}}}"#
            ),
            r#"{"ordinal":1,"type":"response_item","timestamp":"2026-09-22T21:18:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"从 NoEnding 启动的新会话"}]}}"#
        ),
    )
    .unwrap();
    file
}

/// §15.1/§21 — a Session's Owner is decided ONCE, at first discovery: by a
/// user action, or by the LaunchIntent that started it. A source-scoped sync
/// discovers sessions too, so it must offer the same chance — otherwise the
/// intent stays pending forever (the later full reconcile sees a known
/// Session) and the Session is left permanently ownerless.
#[test]
fn source_scoped_reconcile_lets_a_first_discovery_claim_its_intent() {
    use noending::domain::{ingest_origin, launch_status, IngestSource, LaunchIntent};
    use noending::ingestion::reconcile_source;
    use noending::sync::SyncEngine;

    let db = open_db("owner-intent-source-first");
    let a = workstream(&db, "A");

    let dir = db.dir.join("codex-source");
    std::fs::create_dir_all(&dir).unwrap();
    codex_rollout(&dir, "rollout-thread-1", "/repo/app");

    // The user launched a New Session for A: the intent exists BEFORE the
    // agent session is discoverable (方案 §15.1).
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(a.id.clone()),
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: "2026-09-22T21:17:07Z".into(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    let source = IngestSource {
        id: new_id(),
        agent: Agent::Codex,
        path: dir.to_string_lossy().to_string(),
        enabled: true,
        origin: ingest_origin::USER.into(),
        created_at: now(),
    };

    let engine = SyncEngine::default();
    let (discovered, _) =
        reconcile_source(&db, &engine, &source, &LaunchWorkspace::default(), &|_| {}).unwrap();
    assert_eq!(discovered, 1, "the rollout fixture is discoverable");

    let stored = db
        .find_session_by_root_agent_id(Agent::Codex, "rollout-thread-1")
        .unwrap()
        .expect("discovery created the Session row");
    assert_eq!(
        stored.owner_workstream_id.as_deref(),
        Some(a.id.as_str()),
        "first discovery must claim the pending LaunchIntent"
    );
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        launch_status::MATCHED
    );
    // The discovered root's conversation arrived through the member commit.
    assert_eq!(
        db.ingested_message_sequence(&stored.id).unwrap(),
        1,
        "the rollout's user turn is ingested"
    );
    let root = db.root_member_for_session(&stored.id).unwrap().unwrap();
    assert_eq!(root.source_member_id, "rollout-thread-1");
}

// ---------------------- §43 a match is one atomic ownership handover

/// A match writes the Owner, the delivery snapshot and the intent status as
/// ONE unit: an intent that says MATCHED always has a Session that exists.
#[test]
fn a_failed_match_leaves_the_intent_pending() {
    use noending::domain::{launch_status, LaunchIntent};
    use noending::launcher::{apply_match, LaunchWorkspace};

    let db = open_db("owner-match-atomic");
    let a = workstream(&db, "A");
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(a.id.clone()),
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    // A Session row that is not there: the owner write cannot land, and
    // nothing else may land either.
    let ghost = Session {
        id: "no-such-session".into(),
        agent: Agent::Codex,
        root_agent_session_id: "ghost".into(),
        title: None,
        cwd: None,
        workspace_path_id: None,
        project_id: None,
        owner_workstream_id: None,
        forked_from_session_id: None,
        started_at: None,
        last_activity_at: None,
        last_conversation_at: None,
        trashed_at: None,
    };
    assert!(apply_match(&db, &intent.id, &ghost, &LaunchWorkspace::default()).is_err());

    let stored = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        launch_status::PENDING,
        "a failed match must not consume the intent"
    );
    assert!(stored.matched_session_id.is_none());
}

/// Deleting the Workstream a pending intent pointed at clears the intent's
/// Owner (`ON DELETE SET NULL`) instead of leaving a dangling id. The match
/// then still lands — the Session simply inherits nothing, which is the
/// honest answer when the chosen Workstream is gone.
#[test]
fn a_deleted_workstream_cannot_leave_a_dangling_intent_owner() {
    use noending::domain::{launch_status, LaunchIntent};
    use noending::launcher::{apply_match, LaunchWorkspace};

    let db = open_db("owner-intent-ws-deleted");
    let a = workstream(&db, "A");
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(a.id.clone()),
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: Some(
            serde_json::json!({
                "bundle_id": "bundle-gone",
                "workstream_id": a.id.clone(),
                "revisions": [],
                "conflicts": [],
            })
            .to_string(),
        ),
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    archive_workstream(&db, &a.id).unwrap();
    delete_workstream_permanently(&db, &a.id).unwrap();

    let stored = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert!(
        stored.owner_workstream_id.is_none(),
        "the FK must null the intent's Owner with the Workstream"
    );

    let s = session(&db, None);
    apply_match(&db, &stored.id, &s, &LaunchWorkspace::default()).unwrap();
    assert!(
        owner_of(&db, &s.id).is_none(),
        "there is nothing to inherit"
    );
    assert!(
        db.latest_deliveries(&s.id).unwrap().is_empty(),
        "no delivery for a bundle whose Workstream is gone"
    );
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        launch_status::MATCHED
    );
}

// -------------------- §20/§21 prepare-time consistency (Preview = Launch)

/// The consistency predicate a Resume preparation ends with: it may only hand
/// back a plan while the Session still names the Owner it was built for.
///
/// Read against the live row, because that is what the predicate does: the
/// fingerprint cannot stand in for it — it hashes the Owner the bundle was
/// built for AND the Owner currently on the row, so an ownership move while the
/// bundle renders produces the very same hash on both sides.
#[test]
fn preparation_matches_current_owner_tracks_the_row() {
    use noending::launcher::preparation_matches_current_owner;

    let db = open_db("prepare-owner-cas");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");
    let s = session(&db, None);

    // Unowned: only "no Owner" matches.
    assert!(preparation_matches_current_owner(&db, &s.id, None).unwrap());
    assert!(!preparation_matches_current_owner(&db, &s.id, Some(&a.id)).unwrap());

    db.set_session_owner(&s.id, Some(&a.id)).unwrap();
    assert!(preparation_matches_current_owner(&db, &s.id, Some(&a.id)).unwrap());
    assert!(
        !preparation_matches_current_owner(&db, &s.id, Some(&b.id)).unwrap(),
        "a plan built for A must not be certified against B"
    );

    // Ownership moves mid-flight (what a concurrent edit does).
    db.set_session_owner(&s.id, Some(&b.id)).unwrap();
    assert!(!preparation_matches_current_owner(&db, &s.id, Some(&a.id)).unwrap());
    assert!(preparation_matches_current_owner(&db, &s.id, Some(&b.id)).unwrap());

    // A trashed Session matches nothing, and neither does a missing one.
    noending::lifecycle::trash_session(&db, &s.id).unwrap();
    assert!(!preparation_matches_current_owner(&db, &s.id, Some(&b.id)).unwrap());
    assert!(!preparation_matches_current_owner(&db, "no-such-session", None).unwrap());
}

/// A New Session prepared against a Workstream that is deleted before the
/// preview is built must fail loudly instead of launching against nothing.
#[test]
fn new_preparation_refuses_a_workstream_that_no_longer_exists() {
    let db = open_db("prepare-new-gone-ws");
    let a = workstream(&db, "A");
    archive_workstream(&db, &a.id).unwrap();
    delete_workstream_permanently(&db, &a.id).unwrap();

    let workspace = LaunchWorkspace {
        default_workspace: None,
    };
    let err = SessionLauncher {
        runtime_dir: db.dir.join("runtime"),
    }
    .prepare_new_in(&db, Agent::Codex, Some(&a.id), None, &workspace)
    .unwrap_err();
    assert!(
        err.to_string().contains("Workstream"),
        "the error must name the missing Workstream, got {err}"
    );
}

// --------------------- §15.1 a LaunchIntent is spent exactly once

/// An intent is a one-shot capability: the second consumer must not be able to
/// re-point it at its own Session, and the first Session keeps what it got.
#[test]
fn a_launch_intent_is_consumed_exactly_once() {
    use noending::domain::{launch_status, LaunchIntent};
    use noending::launcher::{apply_match, LaunchWorkspace};

    let db = open_db("intent-one-shot");
    let a = workstream(&db, "A");
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(a.id.clone()),
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    let first = session(&db, None);
    let second = session(&db, None);
    apply_match(&db, &intent.id, &first, &LaunchWorkspace::default()).unwrap();

    let err = apply_match(&db, &intent.id, &second, &LaunchWorkspace::default())
        .expect_err("a consumed intent must not be handed out again");
    assert!(err.to_string().contains("LaunchIntent"), "got {err}");

    // The first match stands, and the loser has nothing.
    assert_eq!(owner_of(&db, &first.id).as_deref(), Some(a.id.as_str()));
    assert!(owner_of(&db, &second.id).is_none());
    let stored = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert_eq!(stored.status, launch_status::MATCHED);
    assert_eq!(
        stored.matched_session_id.as_deref(),
        Some(first.id.as_str())
    );
}

/// The storage CAS behind the match: a second claim affected zero rows, which
/// is what makes the losing transaction roll back instead of overwriting the
/// winner's `matched_session_id`.
#[test]
fn the_match_claim_is_a_cas_that_only_fires_once() {
    use noending::domain::{launch_status, LaunchIntent};
    use noending::storage::mark_launch_intent_matched_conn;

    let db = open_db("intent-cas");
    let a = workstream(&db, "A");
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(a.id.clone()),
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    let first = db
        .tx(|tx| mark_launch_intent_matched_conn(tx, &intent.id, "session-a", "first"))
        .unwrap();
    assert!(first, "the waiting intent is claimable");
    let second = db
        .tx(|tx| mark_launch_intent_matched_conn(tx, &intent.id, "session-b", "second"))
        .unwrap();
    assert!(!second, "a claimed intent is no longer claimable");

    let stored = db.get_launch_intent(&intent.id).unwrap().unwrap();
    assert_eq!(
        stored.matched_session_id.as_deref(),
        Some("session-a"),
        "the winner's match is never overwritten"
    );
}

/// Matching refuses a Session that is not the intent's to give an Owner to:
/// another Agent's Session, or one that has been trashed.
#[test]
fn matching_refuses_a_foreign_agent_or_a_trashed_session() {
    use noending::domain::{launch_status, LaunchIntent};
    use noending::launcher::{apply_match, LaunchWorkspace};

    let db = open_db("intent-preconditions");
    let a = workstream(&db, "A");
    let mk_intent = || {
        let intent = LaunchIntent {
            id: new_id(),
            launch_type: "new".into(),
            agent: Agent::Codex,
            owner_workstream_id: Some(a.id.clone()),
            cwd: None,
            context_bundle_markdown: None,
            context_bundle_revisions: None,
            process_id: None,
            launched_at: now(),
            matched_session_id: None,
            status: launch_status::PENDING.into(),
            note: String::new(),
            created_at: now(),
            updated_at: now(),
        };
        db.insert_launch_intent(&intent).unwrap();
        intent
    };

    // A different Agent's Session.
    let foreign = mk_intent();
    let claude = session_of_agent(&db, Agent::ClaudeCode, None);
    assert!(apply_match(&db, &foreign.id, &claude, &LaunchWorkspace::default()).is_err());
    assert_eq!(
        db.get_launch_intent(&foreign.id).unwrap().unwrap().status,
        launch_status::PENDING,
        "a refused match leaves the intent waiting"
    );
    assert!(owner_of(&db, &claude.id).is_none());

    // A trashed Session.
    let trashed = mk_intent();
    let s = session(&db, None);
    noending::lifecycle::trash_session(&db, &s.id).unwrap();
    assert!(apply_match(&db, &trashed.id, &s, &LaunchWorkspace::default()).is_err());
    assert_eq!(
        db.get_launch_intent(&trashed.id).unwrap().unwrap().status,
        launch_status::PENDING
    );
    assert!(owner_of(&db, &s.id).is_none());
}

// ---------------- §15.1 the discovery retry window for an unclaimed intent

/// An ownerless Session gets more than one chance to claim a pending intent.
///
/// The Session row is persisted before matching runs, so `is_new` is true
/// exactly once: if the only attempt were the first one, a transient failure
/// (or a Workstream deleted between the read and the write) would leave the
/// intent pending and the Session ownerless forever — `is_new` never comes back.
#[test]
fn an_ownerless_session_can_still_claim_its_intent_later() {
    use noending::domain::{launch_status, LaunchIntent};
    use noending::ingestion::finalize_newly_discovered_root;

    let db = open_db("intent-retry");
    let a = workstream(&db, "A");
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(a.id.clone()),
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    let s = session(&db, None);
    // NOT a first discovery (`is_new = false`): this is the retry the seam has
    // to allow, because the row already exists and the Owner is still unset.
    finalize_newly_discovered_root(&db, &s, false, &LaunchWorkspace::default()).unwrap();
    assert_eq!(owner_of(&db, &s.id).as_deref(), Some(a.id.as_str()));
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        launch_status::MATCHED
    );
}

/// The retry is bounded by ownership and by waiting intents: a Session that
/// already has an Owner is left alone, and so is everyone while no intent is
/// pending.
#[test]
fn the_intent_retry_is_scoped_to_ownerless_sessions_with_waiting_intents() {
    use noending::domain::{launch_status, LaunchIntent};
    use noending::ingestion::finalize_newly_discovered_root;

    let db = open_db("intent-retry-scope");
    let a = workstream(&db, "A");
    let b = workstream(&db, "B");
    let intent = LaunchIntent {
        id: new_id(),
        launch_type: "new".into(),
        agent: Agent::Codex,
        owner_workstream_id: Some(a.id.clone()),
        cwd: None,
        context_bundle_markdown: None,
        context_bundle_revisions: None,
        process_id: None,
        launched_at: now(),
        matched_session_id: None,
        status: launch_status::PENDING.into(),
        note: String::new(),
        created_at: now(),
        updated_at: now(),
    };
    db.insert_launch_intent(&intent).unwrap();

    // Already owned: the retry must not re-route a decided Session.
    let owned = session(&db, None);
    db.set_session_owner(&owned.id, Some(&b.id)).unwrap();
    finalize_newly_discovered_root(&db, &owned, false, &LaunchWorkspace::default()).unwrap();
    assert_eq!(owner_of(&db, &owned.id).as_deref(), Some(b.id.as_str()));
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        launch_status::PENDING,
        "the waiting intent is untouched while nothing is claimable"
    );

    // A trashed Session is never a candidate.
    let trashed = session(&db, None);
    noending::lifecycle::trash_session(&db, &trashed.id).unwrap();
    finalize_newly_discovered_root(&db, &trashed, true, &LaunchWorkspace::default()).unwrap();
    assert!(owner_of(&db, &trashed.id).is_none());
    assert_eq!(
        db.get_launch_intent(&intent.id).unwrap().unwrap().status,
        launch_status::PENDING
    );

    // With no waiting intent, an ownerless Session is left as it is.
    let stray = session(&db, None);
    db.tx(|tx| {
        noending::storage::update_waiting_launch_intent_conn(
            tx,
            &intent.id,
            "expired",
            "test: no longer waiting",
        )
    })
    .unwrap();
    finalize_newly_discovered_root(&db, &stray, false, &LaunchWorkspace::default()).unwrap();
    assert!(owner_of(&db, &stray.id).is_none());
}

// ------------------ §39 search projections follow the facts they copy

/// A Session's search document embeds its Owner's title and its Project's name
/// (§39). Every write that changes either fact must rebuild those documents —
/// otherwise a rename leaves the old name searchable, and a deletion keeps a
/// Workstream findable through the Sessions that used to own it.
#[test]
fn session_search_documents_follow_workstream_and_project_renames() {
    use noending::search::search;

    let db = open_db("search-projection");
    db.upsert_project(&support::project("p1".to_string(), "ProjX".to_string()))
        .unwrap();
    let a = workstream(&db, "Alpha");
    // A real directory + WorkspacePath row, so the Project membership comes
    // from the authoritative chain (`workspace_path_id → project_id`, §1.10).
    let dir = db.dir.join("repo-search");
    std::fs::create_dir_all(&dir).unwrap();
    let dir_str = dir.to_string_lossy().to_string();
    let path_id = db
        .tx(|tx| insert_workspace_path_conn(tx, &dir_str, "p1"))
        .unwrap();
    let root_id = format!("root-{}", new_id());
    let (s_id, _) = db
        .upsert_logical_session(
            Agent::Codex,
            &root_id,
            None,
            Some(&dir_str),
            Some(&path_id),
            None,
            None,
            None,
        )
        .unwrap();
    db.set_session_owner(&s_id, Some(&a.id)).unwrap();

    let hits_session = |query: &str| {
        search(&db, query, 40)
            .unwrap()
            .into_iter()
            .any(|h| h.kind == "session" && h.ref_id == s_id)
    };
    assert!(
        hits_session("Alpha"),
        "the session carries its Owner's title"
    );
    assert!(hits_session("ProjX"), "and its Project's name");

    // Rename the Workstream: the Session's document must follow.
    let mut renamed = db.get_workstream(&a.id).unwrap().unwrap();
    renamed.title = "Beta".into();
    renamed.updated_at = now();
    db.upsert_workstream(&renamed).unwrap();
    assert!(!hits_session("Alpha"), "the old title is gone");
    assert!(hits_session("Beta"), "the new title is there");

    // Rename the Project: same rule, the other input of the body.
    db.tx(|tx| noending::storage::workspace::rename_project_conn(tx, "p1", "ProjY"))
        .unwrap();
    assert!(!hits_session("ProjX"), "the old Project name is gone");
    assert!(hits_session("ProjY"));

    // Deleting the Workstream clears the Owner (FK) and must clear its title
    // from the Session's document with it.
    archive_workstream(&db, &a.id).unwrap();
    delete_workstream_permanently(&db, &a.id).unwrap();
    assert!(
        !hits_session("Beta"),
        "a deleted Workstream's title must not stay findable through its Sessions"
    );
    assert!(hits_session("ProjY"), "the rest of the document survives");
}
