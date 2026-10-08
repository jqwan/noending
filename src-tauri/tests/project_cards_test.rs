//! Projects Experience v0.2 — the `list_project_cards` projection,
//! plus the review P2-1 contract: search_paths covers EVERY workspace path
//! while representative_paths stays a display-only truncation.

use noending::domain::{Agent, ProjectKind, Workstream};
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, now, Db};
use noending::workspace::normalize_path;
use noending::workspace::project::UnrestrictedWorkspace;
mod support;

fn temp_db() -> (std::path::PathBuf, Db) {
    let dir = std::env::temp_dir().join(format!("noending-cards-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).unwrap();
    (dir, db)
}

fn canon(raw: &str) -> String {
    normalize_path(raw).expect("fixture path normalizes")
}

fn add_path(db: &Db, project_id: &str, raw: &str) -> String {
    db.tx(|tx| insert_workspace_path_conn(tx, &canon(raw), project_id))
        .unwrap()
}

fn workstream_with_path(db: &Db, id: &str, title: &str, workspace_path_id: &str) {
    let w = Workstream {
        id: id.into(),
        title: title.into(),
        description: String::new(),
        visibility: "normal".into(),
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    db.tx(|tx| {
        noending::storage::workstream_paths::append_workstream_path_conn(tx, id, workspace_path_id)
    })
    .unwrap();
}

fn session_at(db: &Db, id: &str, path_id: &str, trashed: bool) {
    // The Logical Session is keyed by (agent, root identity); project_id is
    // derived from workspace_paths inside upsert_logical_session.
    let (sid, _) = db
        .upsert_logical_session_unchecked(
            Agent::Codex,
            &format!("root-{id}"),
            Some(id),
            None,
            Some(path_id),
            None,
            Some(&now()),
            Some(&now()),
        )
        .unwrap();
    if trashed {
        // 发现路径不写生命周期列；已归档状态要显式落库。
        db.write()
            .execute(
                "UPDATE sessions SET archived_at = ?1 WHERE id = ?2",
                [now(), sid],
            )
            .unwrap();
    }
}

#[test]
fn board_card_projection_counts_and_paths() {
    let (_d, db) = temp_db();
    db.upsert_project(&support::project("p-1".into(), "NoEnding"))
        .unwrap();

    db.upsert_project(&support::project("p-2".into(), "Other"))
        .unwrap();
    let a = add_path(&db, "p-1", "/work/alpha");
    let b = add_path(&db, "p-1", "/work/beta");
    let _c = add_path(&db, "p-1", "/work/gamma");
    // 纯注册写入 exists_on_disk = 0（观察属于 reconcile）。a/b 已被观察到存在，
    // gamma 从磁盘上消失：missing 计数来自注册表观察，不是身份变化。
    for id in [&a, &b] {
        db.write()
            .execute(
                "UPDATE workspace_paths SET exists_on_disk = 1 WHERE id = ?1",
                [id],
            )
            .unwrap();
    }
    let other = add_path(&db, "p-2", "/work/elsewhere");
    db.write()
        .execute(
            "UPDATE workspace_paths SET exists_on_disk = 1 WHERE id = ?1",
            [&other],
        )
        .unwrap();

    // Both tasks reach p-1 through their paths; path order makes no difference.
    workstream_with_path(&db, "ws-primary", "Primary", &a);
    workstream_with_path(&db, "ws-related", "Related", &other);
    db.tx(|tx| {
        noending::storage::workstream_paths::append_workstream_path_conn(tx, "ws-related", &b)?;
        noending::storage::workstream_paths::append_workstream_path_conn(tx, "ws-related", &a)
    })
    .unwrap();
    session_at(&db, "s-live", &a, false);
    session_at(&db, "s-trashed", &a, true);

    let cards = noending::commands::project::project_cards(&db, &UnrestrictedWorkspace).unwrap();
    assert_eq!(cards.len(), 2);
    let card = cards.iter().find(|c| c.id == "p-1").unwrap();

    assert_eq!(card.path_count, 3);
    assert_eq!(card.missing_path_count, 1, "gamma is observed missing");
    assert_eq!(card.workstream_count, 2);
    assert_eq!(card.session_count, 2, "archived sessions still count");
    assert_eq!(card.kind, ProjectKind::Directory);
    let other_card = cards.iter().find(|c| c.id == "p-2").unwrap();
    assert_eq!(other_card.workstream_count, 1);

    // Review P2-1 — representative stays a two-path display truncation while
    // search_paths carries the whole registry slice, canonical order.
    assert_eq!(
        card.representative_paths,
        vec![canon("/work/alpha"), canon("/work/beta")]
    );
    assert_eq!(
        card.search_paths,
        vec![
            canon("/work/alpha"),
            canon("/work/beta"),
            canon("/work/gamma")
        ]
    );
    assert!(
        card.last_activity_at.is_some(),
        "session activity feeds the signal"
    );
}
