//! UI list projection: complete lightweight rows, bounded previews and counts.
use noending::domain::{Agent, SessionListScope};
use noending::storage::{Db, SessionFilter};
use noending_lib as noending;

#[test]
fn session_lists_keep_old_rows_without_loading_source_metadata_and_counts_are_complete() {
    let dir = std::env::temp_dir().join(format!(
        "noending-session-list-{}",
        noending::storage::new_id()
    ));
    {
        let db = Db::open(&dir.join("test.db")).unwrap();
        db.tx(|tx| {
            for i in 0..629 {
                let agent = if i < 600 { "codex" } else { "pi" };
                let archived = (600..625).contains(&i).then_some("2026-10-01");
                tx.execute(
                    "INSERT INTO sessions(id,agent,root_agent_session_id,title,source_kind,source_path,
                        archived_at,last_activity_at,metadata,source_prefix_hash)
                     VALUES(?1,?2,?1,?1,'fixture','/source',?3,?4,?5,?6)",
                    rusqlite::params![format!("s{i:04}"),agent,archived,format!("{i:04}"),
                        serde_json::json!({"private_source_facts":"metadata-marker".repeat(1000)}).to_string(),
                        "hash-marker".repeat(8)],
                )?;
            }
            Ok(())
        }).unwrap();
        db.write().execute(
            "INSERT INTO session_contexts(session_id,summary,updated_at) VALUES('s0628','仅详情显示的摘要','now')",
            [],
        ).unwrap();
        let rows = db
            .list_session_items(SessionFilter::default(), None)
            .unwrap();
        assert_eq!(rows.len(), 604);
        assert_eq!(rows.first().unwrap().id, "s0628");
        assert_eq!(
            rows.last().unwrap().id,
            "s0000",
            "old/pinned identities stay available to the sidebar"
        );
        let json = serde_json::to_value(&rows[0]).unwrap();
        assert!(!json.to_string().contains("仅详情显示的摘要"));
        assert_eq!(
            db.get_session_context("s0628")
                .unwrap()
                .unwrap()
                .fields
                .summary,
            "仅详情显示的摘要"
        );
        assert_eq!(json["root_agent_session_id"], "s0628");
        assert_eq!(json["latest_message_seq"], 0);
        for field in [
            "metadata",
            "summary",
            "context",
            "source_path",
            "source_file_identity",
            "source_prefix_hash",
            "source_tail_hash",
            "source_generation",
            "fact_generation",
        ] {
            assert!(
                json.get(field).is_none(),
                "list payload must exclude detail field {field}"
            );
        }
        let full = db.get_session("s0628").unwrap().unwrap();
        assert_eq!(full.source_path, "/source");
        assert!(full.metadata["private_source_facts"]
            .as_str()
            .unwrap()
            .contains("metadata-marker"));
        assert_eq!(
            db.session_counts_by_agent().unwrap(),
            std::collections::HashMap::from([("codex".to_string(), 600), ("pi".to_string(), 4)])
        );
        let activities = db.agent_latest_activities().unwrap();
        assert!(activities.contains_key("codex"));
        assert!(activities.contains_key("pi"));
        let preview = db
            .list_session_items(
                SessionFilter {
                    agent: Some(Agent::Codex),
                    ..Default::default()
                },
                Some(25),
            )
            .unwrap();
        assert_eq!(preview.len(), 25);
        assert_eq!(preview[0].id, "s0599");
        assert_eq!(preview[24].id, "s0575");
        let archived = db
            .list_session_items(
                SessionFilter {
                    scope: SessionListScope::Archived,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(archived.len(), 25);
        assert!(archived.iter().all(|s| s.archived_at.is_some()));
        db.tx(|tx| {
            tx.execute("INSERT INTO workstreams(id,title,created_at,updated_at) VALUES('w','tracked','now','now')",[])?;
            tx.execute("INSERT INTO session_workstreams(session_id,workstream_id) VALUES('s0000','w'),('s0600','w')",[])?;
            Ok(())
        }).unwrap();
        let tracked = db.workstream_session_list_items("w").unwrap();
        assert_eq!(
            tracked.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["s0600", "s0000"]
        );
        assert!(tracked[0].archived_at.is_some());
        assert_eq!(tracked[0].tracked_workstream_ids, vec!["w"]);
        assert!(serde_json::to_value(&tracked[0])
            .unwrap()
            .get("metadata")
            .is_none());
        assert_eq!(
            db.list_session_items(
                SessionFilter {
                    scope: SessionListScope::All,
                    ..Default::default()
                },
                None
            )
            .unwrap()
            .len(),
            629
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
