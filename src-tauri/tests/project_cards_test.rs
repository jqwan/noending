//! Projects Experience v0.2 — the `list_project_cards` projection (方案 §8),
//! plus the review P2-1 contract: search_paths covers EVERY workspace path
//! while representative_paths stays a display-only truncation.

use noending::domain::{Agent, Project, Session, Workstream};
use noending::storage::workspace::insert_workspace_path_conn;
use noending::storage::{new_id, now, Db};
use noending::workspace::normalize_path;

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
        project_id: None,
        title: title.into(),
        description: String::new(),
        lifecycle: "active".into(),
        visibility: "normal".into(),
        default_cwd: None,
        created_at: now(),
        updated_at: now(),
    };
    db.upsert_workstream(&w).unwrap();
    db.tx(|tx| {
        noending::storage::workstream_paths::append_workstream_path_conn(
            tx,
            id,
            workspace_path_id,
            "user",
        )
    })
    .unwrap();
}

fn session_at(db: &Db, id: &str, path_id: &str, trashed: bool) {
    let mut s = Session {
        id: new_id(),
        agent: Agent::Codex,
        agent_session_id: format!("as-{id}"),
        title: Some(id.into()),
        cwd: None,
        workspace_path_id: Some(path_id.into()),
        project_id: None,
        raw_path: format!("/raw/{id}.jsonl"),
        parent_agent_session_id: None,
        started_at: Some(now()),
        last_activity_at: Some(now()),
        trashed_at: None,
    };
    s.cwd = None;
    db.upsert_session(&s).unwrap();
    if trashed {
        // upsert_session 刻意不写生命周期列；回收站状态要显式落库（v13）。
        let sid = s.id.clone();
        db.conn()
            .execute(
                "UPDATE sessions SET trashed_at = ?1 WHERE id = ?2",
                [now(), sid],
            )
            .unwrap();
    }
}

#[test]
fn board_card_projection_counts_and_paths() {
    let (_d, db) = temp_db();
    db.upsert_project(&Project::new("p-1".into(), "NoEnding"))
        .unwrap();

    db.upsert_project(&Project::new("p-2".into(), "Other"))
        .unwrap();
    let a = add_path(&db, "p-1", "/work/alpha");
    let b = add_path(&db, "p-1", "/work/beta");
    let c = add_path(&db, "p-1", "/work/gamma");
    // 纯注册写入 exists_on_disk = 0（观察属于 reconcile）。a/b 已被观察到存在，
    // gamma 从磁盘上消失：missing 计数来自注册表观察，不是身份变化。
    for id in [&a, &b] {
        db.conn()
            .execute(
                "UPDATE workspace_paths SET exists_on_disk = 1 WHERE id = ?1",
                [id],
            )
            .unwrap();
    }
    let other = add_path(&db, "p-2", "/work/elsewhere");
    db.conn()
        .execute(
            "UPDATE workspace_paths SET exists_on_disk = 1 WHERE id = ?1",
            [&other],
        )
        .unwrap();

    // ws-primary 的 position-0 落在 p-1 → 主关联；ws-related 的 position-0 在
    // p-2、第二条路径在 p-1 → 对 p-1 是关联（方案 §1.12）。
    workstream_with_path(&db, "ws-primary", "Primary", &a);
    workstream_with_path(&db, "ws-related", "Related", &other);
    db.tx(|tx| {
        noending::storage::workstream_paths::append_workstream_path_conn(
            tx,
            "ws-related",
            &b,
            "user",
        )
    })
    .unwrap();
    session_at(&db, "s-live", &a, false);
    session_at(&db, "s-trashed", &a, true);

    let cards = noending::commands::project::project_cards(&db).unwrap();
    assert_eq!(cards.len(), 2);
    let card = cards.iter().find(|c| c.id == "p-1").unwrap();

    assert_eq!(card.path_count, 3);
    assert_eq!(card.missing_path_count, 1, "gamma is observed missing");
    assert_eq!(card.primary_workstream_count, 1);
    assert_eq!(card.related_workstream_count, 1, "related excludes primary");
    assert_eq!(card.session_count, 1, "trashed sessions never count (v13)");
    assert!(!card.has_git_identity);
    let other_card = cards.iter().find(|c| c.id == "p-2").unwrap();
    assert_eq!(other_card.primary_workstream_count, 1);
    assert_eq!(other_card.related_workstream_count, 0);

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
