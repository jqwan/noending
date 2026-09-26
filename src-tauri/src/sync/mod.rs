//! Explicit Context update — the deterministic side.
//!
//! AI is never called automatically. A user action ("更新摘要" on a Session,
//! "更新状态" on a Workstream) runs exactly one model call, then this module
//! verifies the proposed changes against the unified AuthorityPolicy before
//! anything is written.
//!
//! `ContextMutation` is the only shape a Workstream change may take (`add` /
//! `update` / `supersede` / `resolve` and nothing else). Authority is derived by
//! the backend from the cited sources — the model's word never decides whose
//! claim it is. All writes happen on the caller's connection inside ONE
//! transaction, so the whole update commits or rolls back together.

pub mod extractor;
pub mod merge;
pub mod policy;

use serde::{Deserialize, Serialize};

use crate::domain::{ContextItem, ContextItemRevision, SessionContextFields};
use crate::error::Result;
use crate::storage::{new_id, now, Db};

/// A proposed change to a workstream's context, produced by an explicit
/// Context update. `source_refs` are stable references
/// (`session-message:<id>` or `session-context:<id>:<revision>`); a mutation
/// may cite several. Empty = no resolvable source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ContextMutation {
    Add {
        workstream_id: String,
        item_kind: String,
        title: String,
        content: String,
        source_refs: Vec<String>,
        authority: String,
    },
    Update {
        item_id: String,
        title: String,
        content: String,
        source_refs: Vec<String>,
        authority: String,
    },
    Supersede {
        item_id: String,
        title: String,
        content: String,
        source_refs: Vec<String>,
        authority: String,
    },
    Resolve {
        item_id: String,
        source_refs: Vec<String>,
    },
}

/// One Session Context the model produced, tied to a target Session.
#[derive(Debug, Clone)]
pub struct SessionContextDraft {
    pub session_id: String,
    pub fields: SessionContextFields,
}

/// Strict result of ONE explicit Context update model call: the whole object
/// is rejected unless every field validates — no partial application.
#[derive(Debug, Clone, Default)]
pub struct ContextUpdateOutput {
    pub session_contexts: Vec<SessionContextDraft>,
    pub workstream_mutations: Vec<ContextMutation>,
}

/// Everything the merge engine needs to know about the update it writes for.
#[derive(Debug, Clone)]
pub struct MergeContext {
    /// Model runtime name; recorded as `created_by = sync:<runtime>`.
    pub runtime: String,
    /// The ONE Workstream this update may write. Every mutation is checked
    /// against it before anything is stored: a Session has a single Owner, and
    /// model output is untrusted input, so an `item_id` the transcript happened
    /// to contain must not become a write into another Workstream's Context.
    pub workstream_id: String,
}

/// Create a new item with its first revision (shared by merge + manual UI).
#[allow(clippy::too_many_arguments)]
pub fn create_item(
    db: &Db,
    workstream_id: &str,
    kind: &str,
    title: &str,
    content: &str,
    authority: &str,
    source_type: &str,
    source_refs: &[String],
    created_by: &str,
) -> Result<ContextItem> {
    db.tx(|tx| {
        create_item_conn(
            tx,
            workstream_id,
            kind,
            title,
            content,
            authority,
            source_type,
            source_refs,
            created_by,
        )
    })
}

/// Connection-level twin used inside transactions.
#[allow(clippy::too_many_arguments)]
pub fn create_item_conn(
    conn: &rusqlite::Connection,
    workstream_id: &str,
    kind: &str,
    title: &str,
    content: &str,
    authority: &str,
    source_type: &str,
    source_refs: &[String],
    created_by: &str,
) -> Result<ContextItem> {
    let item = ContextItem {
        id: new_id(),
        workstream_id: workstream_id.to_string(),
        kind: kind.to_string(),
        status: "active".into(),
        authority: authority.to_string(),
        created_by: created_by.to_string(),
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
        metadata: {
            let actor = if authority.starts_with("user") || created_by == "user" {
                "user"
            } else if created_by == "agent"
                || created_by.starts_with("sync:")
                || authority.starts_with("agent")
                || source_type == "session_message"
            {
                "agent"
            } else {
                "system"
            };
            let mut meta = serde_json::json!({
                "provenance": {
                    "authority": authority,
                    "actor": actor,
                    "source_type": source_type,
                    "source_ref": source_refs.first(),
                }
            });
            if source_refs.len() > 1 {
                meta["source_refs"] = serde_json::json!(source_refs);
            }
            meta
        },
        source_type: Some(source_type.to_string()),
        source_ref: source_refs.first().cloned(),
        created_at: now(),
    };
    crate::storage::insert_item_conn(conn, &item, &rev)?;
    let mut item = item;
    item.current_revision_id = Some(rev.id);
    Ok(item)
}
