//! Search behavior contract: FTS5 answers token queries, and a query its
//! tokenizer cannot match still finds the document through the LIKE pass.

use noending::search::search;
use noending::storage::{new_id, Db};
use std::ops::Deref;

mod support;

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
    let dir = std::env::temp_dir().join(format!("noending-search-{tag}-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("test.db")).unwrap();
    TestDb { db: Some(db), dir }
}

/// A query inside a token ("Foob" in "NoEndingFoobar") is invisible to FTS5's
/// MATCH, which compares whole tokens. The LIKE pass is what makes such a query
/// find anything at all — and it must run whenever MATCH came back empty, not
/// only when the index happens to be empty.
#[test]
fn a_query_inside_a_token_still_finds_the_document() {
    let db = open_db("fallback");
    let p = support::project("p1".into(), "NoEndingFoobar");
    db.upsert_project(&p).unwrap();
    db.index_project(&p).unwrap();

    let whole = search(&db, "NoEndingFoobar", 10).unwrap();
    assert_eq!(whole.len(), 1, "a whole token is the FTS pass");

    let substring = search(&db, "Foob", 10).unwrap();
    assert_eq!(
        substring.len(),
        1,
        "an in-token query must fall back to LIKE: {substring:?}"
    );
    assert_eq!(substring[0].ref_id, "p1");

    assert!(
        search(&db, "nothing-here", 10).unwrap().is_empty(),
        "no match anywhere is an empty result, not an error"
    );
}
