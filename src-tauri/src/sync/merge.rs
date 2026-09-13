//! Context Merge Engine — deterministic verification of mutations.
//!
//! Rules: dedup (skip near-identical items), supersede (build evolution
//! chains), resolve, conflict (keep both sides when user authority would be
//! silently overridden). Every applied mutation produces a new revision with
//! full source trail; nothing is written without history.

use crate::error::Result;
use crate::sync::{create_item, ContextMutation};
use crate::storage::Db;

pub struct MergeEngine;

impl MergeEngine {
    /// Returns true when the mutation was applied, false when skipped.
    pub fn apply(&self, db: &Db, m: &ContextMutation, run_id: &str) -> Result<bool> {
        match m {
            ContextMutation::Add { workstream_id, item_kind, title, content, source_ref, authority } => {
                // Dedup: same workstream+kind with a very similar active title → skip.
                let existing = db.items_for_workstream(workstream_id, false)?;
                let norm = normalize_title(title);
                for (item, rev) in &existing {
                    if &item.kind == item_kind && normalize_title(&rev.title) == norm {
                        // Same subject but materially different content → update revision.
                        if rev.content.trim() == content.trim() {
                            return Ok(false);
                        }
                        // Never silently overwrite user authority.
                        if item.authority == "user_explicit" || item.authority == "user_edit" {
                            return self.conflict_or_update(db, item, title, content, source_ref, run_id);
                        }
                        let new_rev = crate::domain::ContextItemRevision {
                            id: crate::storage::new_id(),
                            item_id: item.id.clone(),
                            title: title.clone(),
                            content: content.clone(),
                            metadata: serde_json::json!({}),
                            source_type: Some("session_event".into()),
                            source_ref: Some(source_ref.clone()),
                            sync_run_id: Some(run_id.into()),
                            created_at: crate::storage::now(),
                        };
                        db.insert_revision(&new_rev)?;
                        db.set_item_head(&item.id, &new_rev.id, None)?;
                        db.index_item(item, &new_rev)?;
                        return Ok(true);
                    }
                }
                create_item(
                    db,
                    workstream_id,
                    item_kind,
                    title,
                    content,
                    authority,
                    "session_event",
                    Some(source_ref),
                    Some(run_id),
                )?;
                Ok(true)
            }
            ContextMutation::Supersede { item_id, title, content, source_ref, .. } => {
                let old = db.get_item(item_id)?.ok_or_else(|| crate::error::other("item missing"))?;
                let item = crate::domain::ContextItem {
                    id: crate::storage::new_id(),
                    workstream_id: old.workstream_id.clone(),
                    kind: old.kind.clone(),
                    status: "active".into(),
                    authority: "agent_inferred".into(),
                    current_revision_id: None,
                    supersedes_item_id: Some(old.id.clone()),
                    created_at: crate::storage::now(),
                    updated_at: crate::storage::now(),
                };
                let rev = crate::domain::ContextItemRevision {
                    id: crate::storage::new_id(),
                    item_id: item.id.clone(),
                    title: title.clone(),
                    content: content.clone(),
                    metadata: serde_json::json!({}),
                    source_type: Some("session_event".into()),
                    source_ref: Some(source_ref.clone()),
                    sync_run_id: Some(run_id.into()),
                    created_at: crate::storage::now(),
                };
                db.insert_item(&item, &rev)?;
                db.set_item_status(item_id, "superseded")?;
                Ok(true)
            }
            ContextMutation::Resolve { item_id, .. } => {
                db.set_item_status(item_id, "resolved")?;
                Ok(true)
            }
            ContextMutation::CreateWorkstream { .. } => {
                // Auto workstream creation needs a confirmed project home;
                // heuristic v0 leaves it to the user / assistant UI.
                Ok(false)
            }
            ContextMutation::Conflict { workstream_id, title, content, source_ref, .. } => {
                create_item(
                    db,
                    workstream_id,
                    "risk",
                    title,
                    content,
                    "agent_inferred",
                    "conflict",
                    Some(source_ref),
                    Some(run_id),
                )?;
                Ok(true)
            }
            ContextMutation::Update { .. } => Ok(false), // used by LLM runtime later
        }
    }

    /// User authority present: add an evidence item instead of touching the
    /// user's content. Conflict is preserved, never auto-resolved.
    fn conflict_or_update(
        &self,
        db: &Db,
        user_item: &crate::domain::ContextItem,
        title: &str,
        content: &str,
        source_ref: &str,
        run_id: &str,
    ) -> Result<bool> {
        let rev = db
            .get_revision(user_item.current_revision_id.as_deref().unwrap_or(""))?
            .ok_or_else(|| crate::error::other("revision missing"))?;
        create_item(
            db,
            &user_item.workstream_id,
            "finding",
            &format!("与用户约束可能冲突：{}", crate::adapters::truncate_text(&rev.title, 60)),
            &format!(
                "用户已确认：{}\n\nAgent 新信息：{} — {}\n\n来源：{}",
                rev.content, title, content, source_ref
            ),
            "agent_inferred",
            "conflict",
            Some(source_ref),
            Some(run_id),
        )?;
        Ok(true)
    }
}

fn normalize_title(t: &str) -> String {
    t.chars()
        .filter(|c| c.is_alphanumeric() || !c.is_ascii())
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}
