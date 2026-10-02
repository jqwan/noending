//! Read-only audit of local source sessions. Run with `cargo run --example usage_source_audit`.
//! Writes only a disposable scratch DB; never opens the user's NoEnding DB.
//! Prints token totals and identities, never conversation text.
use noending::{
    adapters::adapter_for,
    domain::{Agent, SessionMember, SessionMemberCursor},
    platform::paths::agent_ingest_roots,
};
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

#[derive(Default, Serialize)]
struct Report {
    agent: String,
    members: usize,
    events: usize,
    queried: [i64; 4],
    ledger: [i64; 4],
    deduplicated_ledger: [i64; 4],
    duplicate_keys: usize,
    mismatches: Vec<serde_json::Value>,
    errors: Vec<String>,
    repeat_mismatches: usize,
    unchanged_read_events: usize,
}
fn add(a: &mut [i64; 4], b: [i64; 4]) {
    for i in 0..4 {
        a[i] += b[i];
    }
}
fn totals(events: &[noending::adapters::UsageEvent]) -> ([i64; 4], [i64; 4], usize) {
    let mut raw = [0; 4];
    let mut unique = [0; 4];
    let mut keys = BTreeSet::new();
    let mut duplicates = 0;
    for e in events {
        let v = [
            e.input_tokens as i64,
            e.output_tokens as i64,
            e.cached_tokens as i64,
            e.reasoning_tokens as i64,
        ];
        add(&mut raw, v);
        if e.key.as_ref().is_none_or(|k| keys.insert(k.clone())) {
            add(&mut unique, v)
        } else {
            duplicates += 1
        }
    }
    (raw, unique, duplicates)
}
fn main() {
    // Each real source member gets an isolated scratch session, so the reader's
    // root/child behavior is retained while persistence can be checked directly.
    let scratch = std::env::temp_dir().join(format!(
        "noending-usage-audit-{}",
        noending::storage::new_id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let db = noending::storage::Db::open(&scratch.join("audit.db")).unwrap();
    let mut expected_workspace = [0; 4];
    for agent in Agent::all() {
        let adapter = adapter_for(*agent);
        let mut r = Report {
            agent: agent.as_str().into(),
            ..Default::default()
        };
        let mut discovered =
            match adapter.discover_members_in(&agent_ingest_roots(*agent), &|_| false) {
                Ok(d) => d,
                Err(e) => {
                    r.errors.push(format!("discovery: {e}"));
                    println!("{}", serde_json::to_string(&r).unwrap());
                    continue;
                }
            };
        discovered.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then(a.source_path.cmp(&b.source_path))
        });
        let claims: RefCell<BTreeMap<String, String>> = RefCell::new(BTreeMap::new());
        for d in discovered {
            let member = SessionMember {
                id: format!(
                    "{}:{}:{}",
                    agent.as_str(),
                    d.source_path.display(),
                    d.source_member_id
                ),
                session_id: "audit".into(),
                agent: *agent,
                source_member_id: d.source_member_id,
                relation: d.kind.relation(),
                parent_source_member_id: d.parent_source_member_id,
                source_kind: d.source_kind,
                source_path: d.source_path.to_string_lossy().into_owned(),
                cwd: d.cwd,
                started_at: d.started_at,
                last_activity_at: d.last_activity_at,
                metadata: d.metadata,
            };
            let claim = |key: &str| {
                let mut c = claims.borrow_mut();
                c.entry(key.into()).or_insert_with(|| member.id.clone()) == &member.id
            };
            let read = match adapter.read_member_delta_claimed(
                &member,
                &SessionMemberCursor::default(),
                &claim,
            ) {
                Ok(v) => v,
                Err(e) => {
                    r.errors.push(format!("{}: {e}", member.source_path));
                    continue;
                }
            };
            r.members += 1;
            r.events += read.usage_events.len();
            let (raw, unique, duplicates) = totals(&read.usage_events);
            add(&mut r.ledger, raw);
            add(&mut r.deduplicated_ledger, unique);
            r.duplicate_keys += duplicates;
            let (session_id, _) = db
                .upsert_logical_root(
                    *agent,
                    &member.id,
                    None,
                    None,
                    None,
                    None,
                    None,
                    member.started_at.as_deref(),
                    "audit",
                    &member.source_path,
                    None,
                    &member.metadata,
                )
                .unwrap();
            let stored_member = db.members_for_session(&session_id).unwrap().remove(0);
            db.commit_member_ingest_with_provenance_state(
                &session_id,
                &stored_member.id,
                &[],
                read.stats,
                read.source.as_ref().unwrap(),
                read.complete_snapshot,
                read.next_active_provider.clone(),
                read.next_active_model.clone(),
                &read.usage_events,
            )
            .unwrap();
            let q = db
                .get_member_stats(&stored_member.id)
                .unwrap()
                .map(|s| {
                    [
                        s.input_tokens.unwrap_or(0),
                        s.output_tokens.unwrap_or(0),
                        s.cached_tokens.unwrap_or(0),
                        s.reasoning_tokens.unwrap_or(0),
                    ]
                })
                .unwrap_or([0; 4]);
            add(&mut r.queried, q);
            if q != unique {
                r.mismatches.push(serde_json::json!({"source":member.source_path,"relation":member.relation.as_str(),"queried":q,"deduplicated":unique}));
            }
            if let Ok(repeat) =
                adapter.read_member_delta_claimed(&member, &SessionMemberCursor::default(), &claim)
            {
                if totals(&repeat.usage_events).1 != unique {
                    r.repeat_mismatches += 1;
                }
            } else {
                r.errors
                    .push(format!("repeat read failed: {}", member.source_path));
            }
            if let Some(source) = read.source.as_ref() {
                let mut cursor = SessionMemberCursor::from_update(&member.id, source);
                cursor.active_model = read.next_active_model;
                cursor.active_provider = read.next_active_provider;
                match adapter.read_member_delta_claimed(&member, &cursor, &claim) {
                    Ok(next) => {
                        if !next.complete_snapshot {
                            r.unchanged_read_events += next.usage_events.len();
                        }
                    }
                    Err(e) => r
                        .errors
                        .push(format!("cursor read {}: {e}", member.source_path)),
                }
            }
        }
        add(&mut expected_workspace, r.deduplicated_ledger);
        println!("{}", serde_json::to_string(&r).unwrap());
    }
    let overview = db.usage_overview().unwrap();
    let queried = [
        overview.input_tokens.unwrap_or(0),
        overview.output_tokens.unwrap_or(0),
        overview.cached_tokens.unwrap_or(0),
        overview.reasoning_tokens.unwrap_or(0),
    ];
    assert_eq!(
        queried, expected_workspace,
        "workspace totals must equal all deduplicated events"
    );
    println!(
        "{}",
        serde_json::json!({"workspace_queried": queried, "workspace_events":overview.ledger_events,"workspace_requests":overview.requests})
    );
    drop(db);
    std::fs::remove_dir_all(scratch).unwrap();
}
