//! The Binding Modal save must be a backend atomic diff, not an orchestrated
//! unbind-all → rebind-all: unchanged rows keep provenance / created_at /
//! sync cursors verbatim, a role edit changes only the role — and on an
//! AUTOMATIC binding upgrades the provenance to user_assigned so the user's
//! choice survives the next auto-classification (which replaces AUTO rows
//! wholesale with role="related"). ANY user removal — regardless of the
//! removed binding's provenance — leaves a durable removal tombstone so
//! sync cannot silently re-add the rejected workstream.

use std::path::PathBuf;

use noending::adapters::DiscoveredSession;
use noending::domain::{binding_source, Agent};
use noending::ingestion::ensure_session_row;
use noending::launcher::{record_binding, replace_session_bindings};
use noending::storage::{new_id, now, Db};
use noending::sync::persist_auto_classification;

fn db(tag: &str) -> Db {
    let dir = std::env::temp_dir().join(format!("noending-replace-{}-{}", tag, new_id()));
    Db::open(&dir.join("test.db")).unwrap()
}

fn session(database: &Db, agent_session_id: &str) -> String {
    ensure_session_row(
        database,
        &DiscoveredSession {
            agent: Agent::Codex,
            agent_session_id: agent_session_id.into(),
            path: PathBuf::from(format!("/tmp/fake/{}.jsonl", agent_session_id)),
            cwd: None,
            started_at: Some("2026-09-01T08:00:00Z".into()),
            last_activity_at: None,
            first_user_text: None,
            parent_agent_session_id: None,
        },
    )
    .unwrap()
    .0
    .id
}

fn workstream(database: &Db, title: &str) -> String {
    let w = noending::domain::Workstream {
        id: new_id(),
        project_id: None,
        title: title.into(),
        description: String::new(),
        lifecycle: "open".into(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    database.upsert_workstream(&w).unwrap();
    w.id
}

fn bind(
    database: &Db,
    session_id: &str,
    workstream_id: &str,
    role: &str,
    source: &str,
    confidence: f64,
) {
    record_binding(
        database,
        session_id,
        workstream_id,
        role,
        source,
        confidence,
    )
    .unwrap();
}

fn rows(database: &Db, session_id: &str) -> Vec<noending::domain::SessionWorkstreamBinding> {
    let mut rs = database.bindings_for_session(session_id).unwrap();
    rs.sort_by(|a, b| a.workstream_id.cmp(&b.workstream_id));
    rs
}

fn desired(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(w, r)| (w.to_string(), r.to_string()))
        .collect()
}

#[test]
fn unchanged_rows_are_kept_verbatim() {
    let database = db("keep");
    let sid = session(&database, "s-keep");
    let wa = workstream(&database, "Workstream A");
    bind(&database, &sid, &wa, "related", binding_source::AUTO, 0.8);
    database
        .update_binding_sync(&sid, &wa, 42, Some("rev-7"))
        .unwrap();
    let before = rows(&database, &sid);

    replace_session_bindings(&database, &sid, &desired(&[(wa.as_str(), "related")])).unwrap();

    let after = rows(&database, &sid);
    assert_eq!(after.len(), 1);
    // provenance, cursors and timestamps must all survive a no-op edit
    assert_eq!(after[0].source, before[0].source);
    assert_eq!(after[0].confidence, before[0].confidence);
    assert_eq!(after[0].created_at, before[0].created_at);
    assert_eq!(after[0].last_used_at, before[0].last_used_at);
    assert_eq!(after[0].last_seen_revision.as_deref(), Some("rev-7"));
    assert_eq!(after[0].last_sync_cursor, 42);
}

#[test]
fn role_edit_on_auto_binding_upgrades_to_user_assigned_durably() {
    let database = db("role");
    let sid = session(&database, "s-role");
    let wa = workstream(&database, "Workstream A");
    bind(&database, &sid, &wa, "related", binding_source::AUTO, 0.8);
    database
        .update_binding_sync(&sid, &wa, 42, Some("rev-7"))
        .unwrap();
    let before = rows(&database, &sid);

    replace_session_bindings(&database, &sid, &desired(&[(wa.as_str(), "primary")])).unwrap();

    let after = rows(&database, &sid);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].role, "primary");
    // sync replaces AUTO rows wholesale (role reset to "related"); the
    // upgrade to a strong provenance is what makes the user's choice durable
    assert_eq!(after[0].source, binding_source::USER_ASSIGNED);
    assert_eq!(after[0].confidence, 1.0);
    // a metadata edit is not a "use"; history is preserved
    assert_eq!(after[0].created_at, before[0].created_at);
    assert_eq!(after[0].last_used_at, before[0].last_used_at);
    assert_eq!(after[0].last_seen_revision.as_deref(), Some("rev-7"));
    assert_eq!(after[0].last_sync_cursor, 42);

    // simulate the next classification run: it wipes AUTO rows and must not
    // touch the upgraded binding
    database
        .tx(|tx| persist_auto_classification(tx, &sid, &[]))
        .unwrap();
    let after_sync = rows(&database, &sid);
    assert_eq!(after_sync.len(), 1);
    assert_eq!(after_sync[0].role, "primary");
    assert_eq!(after_sync[0].source, binding_source::USER_ASSIGNED);
}

#[test]
fn explicit_provenance_survives_a_role_edit() {
    let database = db("explicit");
    let sid = session(&database, "s-explicit");
    let wa = workstream(&database, "Workstream A");
    bind(
        &database,
        &sid,
        &wa,
        "primary",
        binding_source::EXPLICIT_LAUNCH,
        1.0,
    );
    let before = rows(&database, &sid);

    replace_session_bindings(&database, &sid, &desired(&[(wa.as_str(), "related")])).unwrap();

    let after = rows(&database, &sid);
    assert_eq!(after[0].role, "related");
    assert_eq!(after[0].source, binding_source::EXPLICIT_LAUNCH);
    assert_eq!(after[0].last_used_at, before[0].last_used_at);
}

#[test]
fn removed_rows_are_deleted_and_new_rows_become_user_assigned() {
    let database = db("mixed");
    let sid = session(&database, "s-mixed");
    let wa = workstream(&database, "Workstream A");
    let wb = workstream(&database, "Workstream B");
    bind(
        &database,
        &sid,
        &wa,
        "primary",
        binding_source::USER_ASSIGNED,
        1.0,
    );
    database
        .update_binding_sync(&sid, &wa, 9, Some("rev-2"))
        .unwrap();

    replace_session_bindings(
        &database,
        &sid,
        &desired(&[(wa.as_str(), "related"), (wb.as_str(), "related")]),
    )
    .unwrap();

    let after = rows(&database, &sid);
    assert_eq!(after.len(), 2);
    let a = after.iter().find(|b| b.workstream_id == wa).unwrap();
    let b = after.iter().find(|b| b.workstream_id == wb).unwrap();
    // kept row: role updated, everything else preserved
    assert_eq!(a.role, "related");
    assert_eq!(a.source, binding_source::USER_ASSIGNED);
    assert_eq!(a.last_sync_cursor, 9);
    assert_eq!(a.last_seen_revision.as_deref(), Some("rev-2"));
    // added row: user_assigned at full confidence
    assert_eq!(b.role, "related");
    assert_eq!(b.source, binding_source::USER_ASSIGNED);
    assert_eq!(b.confidence, 1.0);
}

#[test]
fn removed_auto_binding_is_tombstoned_and_never_reclassified() {
    let database = db("tombstone");
    let sid = session(&database, "s-tomb");
    let wa = workstream(&database, "Workstream A");
    let wb = workstream(&database, "Workstream B");
    bind(&database, &sid, &wa, "related", binding_source::AUTO, 0.8);

    // the user removes the auto guess entirely
    replace_session_bindings(&database, &sid, &desired(&[])).unwrap();
    assert!(database.bindings_for_session(&sid).unwrap().is_empty());
    assert!(
        database.binding_removal_exists(&sid, &wa).unwrap(),
        "removing an AUTO binding must leave a durable negative override"
    );

    // the next classification proposes A and B again: A stays removed
    database
        .tx(|tx| persist_auto_classification(tx, &sid, &[wa.clone(), wb.clone()]))
        .unwrap();
    let after_sync = rows(&database, &sid);
    assert!(
        after_sync.iter().all(|b| b.workstream_id != wa),
        "a rejected auto guess must not come back"
    );
    assert_eq!(after_sync.len(), 1);
    assert_eq!(after_sync[0].workstream_id, wb);
    assert_eq!(after_sync[0].source, binding_source::AUTO);

    // the user explicitly re-adds A; the edit replaces the whole set, so B —
    // still an auto guess — is dropped and tombstoned as part of the same edit
    replace_session_bindings(&database, &sid, &desired(&[(wa.as_str(), "primary")])).unwrap();
    assert!(!database.binding_removal_exists(&sid, &wa).unwrap());
    let after_readd = rows(&database, &sid);
    assert_eq!(after_readd.len(), 1);
    assert_eq!(after_readd[0].workstream_id, wa);
    assert_eq!(after_readd[0].source, binding_source::USER_ASSIGNED);
    assert!(
        database.binding_removal_exists(&sid, &wb).unwrap(),
        "dropping B in the same edit tombstones its auto guess too"
    );
}

#[test]
fn removing_a_strong_binding_is_durable_too() {
    let database = db("strong-rm");
    let sid = session(&database, "s-strong-rm");
    let wa = workstream(&database, "Workstream A");
    bind(
        &database,
        &sid,
        &wa,
        "primary",
        binding_source::USER_ASSIGNED,
        1.0,
    );

    replace_session_bindings(&database, &sid, &desired(&[])).unwrap();

    assert!(database.bindings_for_session(&sid).unwrap().is_empty());
    assert!(
        database.binding_removal_exists(&sid, &wa).unwrap(),
        "a user rejection is durable regardless of provenance: with the last \
         strong binding gone the session is auto-classifiable again"
    );

    // the next classification proposing A again must be suppressed
    database
        .tx(|tx| persist_auto_classification(tx, &sid, &[wa.clone()]))
        .unwrap();
    assert!(
        database.bindings_for_session(&sid).unwrap().is_empty(),
        "the rejected workstream must not come back as an auto binding"
    );
}

#[test]
fn invalid_role_aborts_without_partial_state() {
    let database = db("atomic");
    let sid = session(&database, "s-atomic");
    let wa = workstream(&database, "Workstream A");
    let wb = workstream(&database, "Workstream B");
    bind(
        &database,
        &sid,
        &wa,
        "primary",
        binding_source::USER_ASSIGNED,
        1.0,
    );

    let err = replace_session_bindings(
        &database,
        &sid,
        &desired(&[(wb.as_str(), "related"), (wa.as_str(), "boss")]),
    )
    .unwrap_err();
    assert!(!err.to_string().is_empty());

    // the transaction rolled back: A untouched, B never inserted
    let after = rows(&database, &sid);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].workstream_id, wa);
    assert_eq!(after[0].role, "primary");
}

#[test]
fn duplicate_workstream_rows_dedupe_to_last_entry() {
    let database = db("dedupe");
    let sid = session(&database, "s-dedupe");
    let wa = workstream(&database, "Workstream A");

    replace_session_bindings(
        &database,
        &sid,
        &desired(&[(wa.as_str(), "related"), (wa.as_str(), "primary")]),
    )
    .unwrap();

    let after = rows(&database, &sid);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].role, "primary");
}
