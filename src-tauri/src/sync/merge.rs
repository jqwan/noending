//! Context Merge Engine — deterministic verification of mutations.
//!
//! Every mutation passes through the unified AuthorityPolicy before it
//! touches anything. All writes happen on the caller's connection, which is
//! always a transaction owned by the SyncEngine — the whole run commits or
//! rolls back together.
//!
//! Rules: dedup (skip near-identical items), supersede (build evolution
//! chains), resolve, conflict (persisted ContextConflict rows; the user's
//! item is never silently overwritten). Every applied mutation produces a
//! new revision with full source trail.

use rusqlite::Connection;

use crate::domain::{ContextConflict, ContextItem};
use crate::error::Result;
use crate::storage::{
    apply_status_change_conn, get_item_conn, get_revision_conn, insert_conflict_conn,
    insert_item_conn, insert_revision_conn, items_for_workstream_conn, new_id, now,
    set_item_head_conn, upsert_workstream_conn,
};
use crate::sync::policy::{Actor, AuthorityPolicy, MutationDecision, Op};
use crate::sync::{create_item_conn, ContextMutation, MergeContext};

pub struct MergeEngine;

impl MergeEngine {
    /// Returns true when the mutation was applied, false when skipped.
    /// `conn` is always the transaction held by the sync run.
    pub fn apply(
        &self,
        conn: &Connection,
        m: &ContextMutation,
        ctx: &MergeContext,
    ) -> Result<bool> {
        match m {
            ContextMutation::Add {
                workstream_id,
                item_kind,
                title,
                content,
                source_refs,
                authority,
            } => self.apply_add(
                conn,
                ctx,
                workstream_id,
                item_kind,
                title,
                content,
                source_refs,
                authority,
            ),
            ContextMutation::Update {
                item_id,
                title,
                content,
                source_refs,
                ..
            } => {
                let Some(item) = get_item_conn(conn, item_id)? else {
                    return Ok(false); // deterministic skip: nothing to update
                };
                match AuthorityPolicy::decide(&item.authority, Actor::Agent, Op::Update) {
                    MutationDecision::Allow => {
                        let rev = agent_revision(conn, &item, title, content, source_refs, ctx)?;
                        set_item_head_conn(conn, &item.id, &rev.id, None)?;
                        Ok(true)
                    }
                    MutationDecision::CreateConflict => {
                        self.record_conflict(conn, ctx, &item, title, content, source_refs)?;
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
            ContextMutation::Supersede {
                item_id,
                title,
                content,
                source_refs,
                authority,
            } => {
                let Some(old) = get_item_conn(conn, item_id)? else {
                    return Ok(false);
                };
                match AuthorityPolicy::decide(&old.authority, Actor::Agent, Op::Supersede) {
                    MutationDecision::Allow => {
                        let new_item = ContextItem {
                            id: new_id(),
                            workstream_id: old.workstream_id.clone(),
                            kind: old.kind.clone(),
                            status: "active".into(),
                            authority: authority.clone(),
                            created_by: format!("sync:{}", ctx.runtime),
                            current_revision_id: None,
                            supersedes_item_id: Some(old.id.clone()),
                            created_at: now(),
                            updated_at: now(),
                        };
                        let rev = crate::domain::ContextItemRevision {
                            id: new_id(),
                            item_id: new_item.id.clone(),
                            title: title.clone(),
                            content: content.clone(),
                            metadata: serde_json::json!({}),
                            source_type: Some("session_event".into()),
                            source_ref: source_refs.first().cloned(),
                            sync_run_id: Some(ctx.run_id.clone()),
                            created_at: now(),
                        };
                        insert_item_conn(conn, &new_item, &rev)?;
                        // status change leaves its own audit revision
                        apply_status_change_conn(
                            conn,
                            &old.id,
                            "superseded",
                            &format!("sync:{}", ctx.runtime),
                            "被更新版本取代（supersede）",
                            Some(&ctx.run_id),
                            source_refs,
                        )?;
                        Ok(true)
                    }
                    MutationDecision::CreateConflict => {
                        self.record_conflict(conn, ctx, &old, title, content, source_refs)?;
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
            ContextMutation::Resolve {
                item_id,
                source_refs,
            } => {
                let Some(item) = get_item_conn(conn, item_id)? else {
                    return Ok(false);
                };
                match AuthorityPolicy::decide(&item.authority, Actor::Agent, Op::Resolve) {
                    MutationDecision::Allow => {
                        apply_status_change_conn(
                            conn,
                            &item.id,
                            "resolved",
                            &format!("sync:{}", ctx.runtime),
                            "会话证据表明该条目已完成",
                            Some(&ctx.run_id),
                            source_refs,
                        )?;
                        Ok(true)
                    }
                    MutationDecision::CreateConflict => {
                        // The user's item must not be resolved by an agent:
                        // persist the disagreement instead.
                        self.record_conflict(
                            conn,
                            ctx,
                            &item,
                            "完成状态争议",
                            "Agent 依据会话内容认为该条目已完成，但这是用户确认过的信息，已保留原状态。",
                            source_refs,
                        )?;
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
            ContextMutation::CreateWorkstream {
                project_id,
                title,
                reason,
            } => {
                // Workstream is the core continuity unit; Project is an
                // optional organization layer. Creating one without a
                // project home is explicitly allowed.
                let w = crate::domain::Workstream {
                    id: new_id(),
                    project_id: project_id.clone(),
                    title: title.clone(),
                    description: format!("由同步自动识别：{}", reason),
                    lifecycle: crate::domain::workstream_lifecycle::ACTIVE.into(),
                    visibility: "normal".into(),
                    // sync-created workstreams carry no launch directory;
                    // the launcher falls back to the latest session cwd
                    default_cwd: None,
                    created_at: now(),
                    updated_at: now(),
                };
                upsert_workstream_conn(conn, &w)?;
                Ok(true)
            }
            ContextMutation::Conflict {
                workstream_id,
                item_id,
                title,
                content,
                source_refs,
                ..
            } => {
                // The model flagged disagreement with an existing item:
                // materialize both sides as a ContextConflict.
                let existing = if item_id.is_empty() {
                    None
                } else {
                    get_item_conn(conn, item_id)?
                };
                let right = create_item_conn(
                    conn,
                    workstream_id,
                    "finding",
                    title,
                    content,
                    "agent_inferred",
                    "conflict",
                    source_refs,
                    Some(&ctx.run_id),
                    &format!("sync:{}", ctx.runtime),
                )?;
                let candidate_snapshot = serde_json::json!({
                    "title": title,
                    "content": content,
                    "authority": "agent_inferred",
                    "source_refs": source_refs,
                });
                insert_conflict_conn(
                    conn,
                    &ContextConflict {
                        id: new_id(),
                        workstream_id: workstream_id.clone(),
                        left_item_id: existing
                            .as_ref()
                            .map(|i| i.id.clone())
                            .unwrap_or_else(|| right.id.clone()),
                        right_item_id: if existing.is_some() {
                            Some(right.id.clone())
                        } else {
                            None
                        },
                        conflict_type: "content".into(),
                        status: "open".into(),
                        resolution: None,
                        created_at: now(),
                        updated_at: now(),
                        left_revision_id: existing
                            .as_ref()
                            .and_then(|i| i.current_revision_id.clone()),
                        right_revision_id: right.current_revision_id.clone(),
                        candidate_snapshot_json: Some(candidate_snapshot.to_string()),
                    },
                )?;
                Ok(true)
            }
        }
    }

    fn apply_add(
        &self,
        conn: &Connection,
        ctx: &MergeContext,
        workstream_id: &str,
        item_kind: &str,
        title: &str,
        content: &str,
        source_refs: &[String],
        authority: &str,
    ) -> Result<bool> {
        // Dedup: same workstream+kind with a very similar active title → skip.
        let existing = items_for_workstream_conn(conn, workstream_id, false)?;
        let norm = normalize_title(title);
        for (item, rev) in &existing {
            if item.kind == item_kind && normalize_title(&rev.title) == norm {
                // Same subject but materially different content.
                if rev.content.trim() == content.trim() {
                    return Ok(false);
                }
                // Authority policy decides: agents may evolve agent-owned
                // items; user-owned items get a persisted conflict.
                match AuthorityPolicy::decide(&item.authority, Actor::Agent, Op::Update) {
                    MutationDecision::Allow => {
                        let new_rev = agent_revision(conn, item, title, content, source_refs, ctx)?;
                        set_item_head_conn(conn, &item.id, &new_rev.id, None)?;
                        return Ok(true);
                    }
                    _ => {
                        self.record_conflict(conn, ctx, item, title, content, source_refs)?;
                        return Ok(true);
                    }
                }
            }
        }
        create_item_conn(
            conn,
            workstream_id,
            item_kind,
            title,
            content,
            authority,
            "session_event",
            source_refs,
            Some(&ctx.run_id),
            &format!("sync:{}", ctx.runtime),
        )?;
        Ok(true)
    }

    /// Persist a disagreement: the user's item stays untouched, the agent's
    /// version becomes its own evidence item, and a ContextConflict row
    /// links both. Conflicts are kept, never auto-resolved.
    fn record_conflict(
        &self,
        conn: &Connection,
        ctx: &MergeContext,
        user_item: &ContextItem,
        title: &str,
        content: &str,
        source_refs: &[String],
    ) -> Result<()> {
        let rev = get_revision_conn(conn, user_item.current_revision_id.as_deref().unwrap_or(""))?;
        let user_content = rev.map(|r| r.content).unwrap_or_default();
        let right = create_item_conn(
            conn,
            &user_item.workstream_id,
            "finding",
            &format!(
                "与用户上下文可能冲突：{}",
                crate::adapters::truncate_text(title, 60)
            ),
            &format!(
                "用户已确认：{}\n\nAgent 新信息：{}\n\n来源：{}",
                user_content, title, content
            ),
            "agent_inferred",
            "conflict",
            source_refs,
            Some(&ctx.run_id),
            &format!("sync:{}", ctx.runtime),
        )?;
        let candidate_snapshot = serde_json::json!({
            "title": title,
            "content": content,
            "authority": "agent_statement",
            "source_refs": source_refs,
        });
        insert_conflict_conn(
            conn,
            &ContextConflict {
                id: new_id(),
                workstream_id: user_item.workstream_id.clone(),
                left_item_id: user_item.id.clone(),
                right_item_id: Some(right.id.clone()),
                conflict_type: "authority".into(),
                status: "open".into(),
                resolution: None,
                created_at: now(),
                updated_at: now(),
                left_revision_id: user_item.current_revision_id.clone(),
                right_revision_id: right.current_revision_id.clone(),
                candidate_snapshot_json: Some(candidate_snapshot.to_string()),
            },
        )?;
        Ok(())
    }
}

/// Build AND persist the agent-side revision for an allowed update. The
/// caller may then point the item head at the returned revision — it is
/// already durable, so the head can never dangle.
fn agent_revision(
    conn: &Connection,
    item: &ContextItem,
    title: &str,
    content: &str,
    source_refs: &[String],
    ctx: &MergeContext,
) -> Result<crate::domain::ContextItemRevision> {
    let mut meta = serde_json::json!({
        "provenance": {
            "authority": "agent_statement",
            "actor": "agent",
            "source_type": "session_event",
            "source_ref": source_refs.first(),
        }
    });
    if source_refs.len() > 1 {
        meta["source_refs"] = serde_json::json!(source_refs);
    }
    let rev = crate::domain::ContextItemRevision {
        id: new_id(),
        item_id: item.id.clone(),
        title: title.to_string(),
        content: content.to_string(),
        metadata: meta,
        source_type: Some("session_event".into()),
        source_ref: source_refs.first().cloned(),
        sync_run_id: Some(ctx.run_id.clone()),
        created_at: now(),
    };
    insert_revision_conn(conn, &rev)?;
    Ok(rev)
}

fn normalize_title(t: &str) -> String {
    t.chars()
        .filter(|c| c.is_alphanumeric() || !c.is_ascii())
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}
