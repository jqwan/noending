//! Workspace Assistant Core — Background Sync (heuristic v0).
//!
//! A SyncJob reads the delta after the last cursor, pre-filters, extracts
//! candidate context mutations, classifies to workstreams, and merges.
//! The merge engine is deterministic; the extractor behind
//! `ContextExtractor` is a trait so an LLM runtime can replace the
//! heuristic without touching domain code.
//!
//! Policy (docs §13/§26):
//! - merge automatically, never silently overwrite user-authority items;
//! - conflicts are kept, not resolved;
//! - every mutation leaves a full source trail.

pub mod extractor;
pub mod merge;

use serde::Serialize;

use crate::domain::{Agent, ContextItem, ContextItemRevision, Session, SessionEvent, SyncRun};
use crate::error::Result;
use crate::storage::{now, new_id, Db};

/// A proposed change to a workstream's context, produced by the Assistant.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ContextMutation {
    Add {
        workstream_id: String,
        item_kind: String,
        title: String,
        content: String,
        source_ref: String,
        authority: String,
    },
    Update {
        item_id: String,
        title: String,
        content: String,
        source_ref: String,
        authority: String,
    },
    Supersede {
        item_id: String,
        title: String,
        content: String,
        source_ref: String,
        authority: String,
    },
    Resolve {
        item_id: String,
        source_ref: String,
    },
    CreateWorkstream {
        project_id: String,
        title: String,
        reason: String,
    },
    Conflict {
        workstream_id: String,
        item_id: String,
        title: String,
        content: String,
        source_ref: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncJobInput {
    pub session_id: String,
    pub from_cursor: i64,
    pub to_cursor: i64,
    pub new_events: Vec<SessionEvent>,
    pub candidate_workstream_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncJobOutput {
    pub run_id: String,
    pub status: String,
    pub applied: usize,
    pub skipped: usize,
    pub unclassified: usize,
    pub summary: String,
}

/// The extraction seam between raw session deltas and the deterministic
/// merge engine. Implementations: HeuristicExtractor (rule-based fallback,
/// always available) and CliExtractor (runs the user's agent CLI headlessly).
pub trait ContextExtractor: Send + Sync {
    fn name(&self) -> String;

    fn extract(
        &self,
        db: &Db,
        session: &Session,
        events: &[&SessionEvent],
        candidate_workstream_ids: &[String],
    ) -> Result<Vec<ContextMutation>>;
}

pub struct SyncEngine {
    pub heuristic: extractor::HeuristicExtractor,
    pub merger: merge::MergeEngine,
    /// When configured, tried first; any failure falls back to heuristic
    /// and is reported via SyncRun.runtime = "<llm-name>->heuristic".
    pub llm: Option<extractor::CliExtractor>,
}

impl Default for SyncEngine {
    fn default() -> Self {
        Self {
            heuristic: extractor::HeuristicExtractor,
            merger: merge::MergeEngine,
            llm: None,
        }
    }
}

impl SyncEngine {
    /// Build an engine using the assistant configuration stored in settings.
    /// Falls back to heuristic-only when the configured agent CLI is missing.
    pub fn from_settings(db: &Db) -> Self {
        let cfg = extractor::CliExtractor::from_settings(db);
        match cfg {
            Some(cli) if crate::adapters::adapter_for(cli.agent).detect().is_some() => {
                Self {
                    heuristic: extractor::HeuristicExtractor,
                    merger: merge::MergeEngine,
                    llm: Some(cli),
                }
            }
            _ => Self::default(),
        }
    }

    /// Run a full sync job for one session delta.
    pub fn run_session_sync(
        &self,
        db: &Db,
        session: &Session,
        events: &[SessionEvent],
        from_cursor: i64,
        to_cursor: i64,
    ) -> Result<SyncJobOutput> {
        let run_id = new_id();

        // 1. candidate workstreams: bound ones, plus keyword-matched ones
        let mut candidates: Vec<String> = db
            .bindings_for_session(&session.id)?
            .into_iter()
            .map(|b| b.workstream_id)
            .collect();
        if candidates.is_empty() {
            candidates = self.auto_classify(db, session, events);
        }

        // 2. pre-filter: only meaningful message kinds
        let meaningful: Vec<&SessionEvent> = events
            .iter()
            .filter(|e| matches!(e.kind.as_str(), "user_message" | "assistant_message"))
            .filter(|e| e.text.as_deref().map(|t| t.len() > 30).unwrap_or(false))
            .collect();

        // 3. extract mutations — LLM first, heuristic fallback
        let (mut mutations, runtime) = if meaningful.is_empty() || candidates.is_empty() {
            (Vec::new(), "none".to_string())
        } else if let Some(cli) = &self.llm {
            match cli.extract(db, session, &meaningful, &candidates) {
                Ok(m) => (m, cli.name()),
                Err(e) => {
                    eprintln!("[sync] cli extractor failed, falling back: {}", e);
                    (
                        self.heuristic.extract(db, session, &meaningful, &candidates)?,
                        format!("{}->heuristic", cli.name()),
                    )
                }
            }
        } else {
            (
                self.heuristic.extract(db, session, &meaningful, &candidates)?,
                "heuristic".to_string(),
            )
        };

        // 4. deterministic merge
        let mut applied = 0usize;
        let mut skipped = 0usize;
        for m in &mutations {
            let r = self.merger.apply(db, m, &run_id)?;
            if r {
                applied += 1;
            } else {
                skipped += 1;
            }
        }
        let unclassified = if candidates.is_empty() && !meaningful.is_empty() {
            meaningful.len()
        } else {
            0
        };

        let summary = format!(
            "同步 {} 条新增消息：新增/更新 {} 项，跳过 {} 项{}",
            meaningful.len(),
            applied,
            skipped,
            if unclassified > 0 {
                format!("；{} 条消息暂未能归类到 Workstream", unclassified)
            } else {
                String::new()
            }
        );

        // 5. record run, then advance session cursor — only on success
        let run = SyncRun {
            id: run_id.clone(),
            session_id: session.id.clone(),
            from_sequence: from_cursor,
            to_sequence: to_cursor,
            status: "ok".into(),
            mutations: serde_json::to_value(&mutations)?,
            summary: summary.clone(),
            error: None,
            created_at: now(),
            runtime,
        };
        db.insert_sync_run(&run)?;
        db.set_cursor(&session.id, to_cursor, 0)?;

        mutations.clear();
        Ok(SyncJobOutput {
            run_id,
            status: "ok".into(),
            applied,
            skipped,
            unclassified,
            summary,
        })
    }

    /// Keyword-based workstream classification.
    /// Title / description tokens are matched against event text.
    fn auto_classify(&self, db: &Db, _session: &Session, events: &[SessionEvent]) -> Vec<String> {
        let workstreams = match db.list_workstreams(None) {
            Ok(v) => v,
            Err(_) => return vec![],
        };
        let text = events
            .iter()
            .filter_map(|e| e.text.as_deref())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if text.is_empty() {
            return vec![];
        }
        let mut scored: Vec<(String, usize)> = Vec::new();
        for w in workstreams {
            let mut score = 0usize;
            for token in tokenize(&format!("{} {}", w.title, w.description)) {
                if token.len() >= 3 && text.contains(&token) {
                    score += token.len();
                }
            }
            if score > 0 {
                scored.push((w.id, score));
            }
        }
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().take(2).map(|(id, _)| id).collect()
    }
}

fn tokenize(s: &str) -> Vec<String> {
    // split on whitespace/punct; keep CJK bigrams for Chinese titles
    let mut out = Vec::new();
    for word in s.split(|c: char| c.is_whitespace() || ",.;:!?()[]{}\"'/|".contains(c)) {
        let w = word.trim();
        if w.is_empty() {
            continue;
        }
        if w.chars().any(|c| c.is_ascii()) {
            out.push(w.to_lowercase());
        }
        let chars: Vec<char> = w.chars().collect();
        if chars.len() >= 2 && chars.iter().all(|c| !c.is_ascii()) {
            for pair in chars.windows(2) {
                out.push(pair.iter().collect());
            }
        }
    }
    out.dedup();
    out
}

/// Create a new item with its first revision (shared by merge + manual UI).
pub fn create_item(
    db: &Db,
    workstream_id: &str,
    kind: &str,
    title: &str,
    content: &str,
    authority: &str,
    source_type: &str,
    source_ref: Option<&str>,
    sync_run_id: Option<&str>,
) -> Result<ContextItem> {
    let item = ContextItem {
        id: new_id(),
        workstream_id: workstream_id.to_string(),
        kind: kind.to_string(),
        status: "active".into(),
        authority: authority.to_string(),
        current_revision_id: None,
        supersedes_item_id: None,
        created_at: now(),
        updated_at: now(),
    };
    let rev = ContextItemRevision {
        id: new_id(),
        item_id: item.id.clone(),
        title: title.to_string(),
        content: content.to_string(),
        metadata: serde_json::json!({}),
        source_type: Some(source_type.to_string()),
        source_ref: source_ref.map(|s| s.to_string()),
        sync_run_id: sync_run_id.map(|s| s.to_string()),
        created_at: now(),
    };
    db.insert_item(&item, &rev)?;
    Ok(item)
}
