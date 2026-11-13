use super::*;
use crate::context::{
    session_context_view,
    tests::{submit_result, ServiceFixture},
};
use serde_json::json;

#[test]
fn scoped_cli_reads_unicode_and_only_explicit_submission_saves_before_exit() {
    let fixture = ServiceFixture::new();
    let id = fixture.add_session(1, 4000);
    let db = fixture.db();
    db.write()
        .execute(
            "UPDATE session_messages SET content=?1 WHERE session_id=?2",
            rusqlite::params![format!("{}{}", "甲".repeat(2000), "乙".repeat(2000)), id],
        )
        .unwrap();
    let home = fixture.home();
    let root = super::super::diagnostics::prepare_runtime_dir(&home).unwrap();
    let mut scope = Scope::new(
        db,
        &home,
        "session",
        &id,
        super::super::interactive::Mode::Update,
    )
    .unwrap();
    let mut directory = None;
    let mut activity = crate::operations::Operation::start(
        db,
        crate::operations::OperationStatus::summary("session", &id),
    )
    .unwrap();
    run(db, &mut scope, &home, &root, &mut activity, |_, dir| {
        directory = Some(dir.to_path_buf());
        let name = format!("session-{id}.jsonl");
        assert_eq!(
            request(dir, json!({"action":"inputs"})).unwrap()["inputs"][0]["name"],
            name
        );
        assert!(request(
            dir,
            json!({"action":"read","name":"outside","cursor":0,"limit":1})
        )
        .is_err());
        assert!(request(dir, json!({"action":"write","sql":"DELETE FROM sessions"})).is_err());
        let search = request(
            dir,
            json!({"action":"search","name":name,"query":"甲乙","cursor":0,"limit":8}),
        )
        .unwrap();
        assert_eq!(search["records"][0]["record"]["content"], "甲".repeat(2000));
        assert_eq!(search["records"][1]["record"]["content"], "乙".repeat(2000));
        let body = json!({"summary":{"items":[{"title":"结果","description":"","details":[]}]}});
        assert!(submit_result(dir, "session", &id, body.clone(), false).is_err());
        assert!(session_context_view(db, &id).unwrap().fields.is_none());
        read_input(&dir.join(name));
        let saved = submit_result(dir, "session", &id, body.clone(), false).unwrap();
        assert_eq!(saved["revision"], 1);
        assert_eq!(
            submit_result(dir, "session", &id, body, false).unwrap(),
            saved
        );
        assert_eq!(
            session_context_view(db, &id)
                .unwrap()
                .fields
                .unwrap()
                .summary
                .items[0]
                .title,
            "结果"
        );
        Ok(())
    })
    .unwrap();
    assert!(request(&directory.unwrap(), json!({"action":"inputs"})).is_err());
}
