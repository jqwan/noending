//! Repro: the user's real claude transcripts must each produce ledger events.
//!
//! Regression shape: session ead8405f（这是个什么工程）was forked from the big
//! multi-agent-context-workspace transcript and shares both of its message
//! ids. Under globally-scoped claim keys the fork's rows were all suppressed
//! and the session showed zero tokens; claim keys are scoped per logical
//! session now, so BOTH records bill.
use noending::domain::Agent;
use noending::storage::new_id;

mod support;

#[test]
fn real_claude_fork_and_original_each_bill_their_own_ledger() {
    let base = std::path::Path::new("/Users/jqk/.claude/projects/-Users-jqk-projects-noending");
    let fork = base.join("91ade297-66ae-46e3-a2d3-3402ac96aa9a.jsonl");
    let original = base.join("f6dcd71f-863a-48b8-9976-2a32174e4b14.jsonl");
    if !fork.exists() || !original.exists() {
        return; // machine-specific repro; skip elsewhere
    }
    let dir = std::env::temp_dir().join(format!("claude-repro-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();

    let db = noending::storage::Db::open(&dir.join("test.db")).unwrap();
    let mut sessions = Vec::new();
    for (name, src) in [("fork", &fork), ("original", &original)] {
        let copy = dir.join(format!("{name}.jsonl"));
        std::fs::copy(src, &copy).unwrap();
        let root_agent_session_id = format!("as-{name}-{}", new_id());
        let id =
            support::ensure_session(&db, new_id(), Agent::ClaudeCode, &root_agent_session_id).id;
        support::ensure_root_member(
            &db,
            &id,
            Agent::ClaudeCode,
            &root_agent_session_id,
            &copy.to_string_lossy(),
        );
        sessions.push(db.get_session(&id).unwrap().unwrap());
    }

    for s in &sessions {
        noending::ingestion::ingest_session(&db, s).unwrap();
    }

    for s in &sessions {
        let agg = db.aggregate_session_stats(&s.id).unwrap();
        assert!(
            agg.input_tokens.unwrap_or(0) > 0 && agg.output_tokens.unwrap_or(0) > 0,
            "session {} must bill its own record (in={:?}, out={:?})",
            s.id,
            agg.input_tokens,
            agg.output_tokens
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
