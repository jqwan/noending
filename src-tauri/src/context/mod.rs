//! Context Builder — produces the SessionContextBundle handed to agents.
//!
//! New session: minimal sufficient context (Goal, Current State,
//! Constraints, Decisions, Open Questions, high-relevance items).
//! Resume: a *delta* against the last successfully delivered revision set
//! (recorded in `context_deliveries`), never a re-dump of full context.
//!
//! Multi-workstream bundles are aggregated FIRST (dedup / rank / conflicts)
//! and only then rendered — never stitched together while looping.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::domain::{ContextConflict, Session, CORE_ITEM_TYPES};
use crate::error::Result;
use crate::storage::{new_id, Db};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextDeliveryLevel {
    Off,
    Compact,
    Balanced,
    Detailed,
}

impl Default for ContextDeliveryLevel {
    fn default() -> Self {
        Self::Balanced
    }
}

impl ContextDeliveryLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Compact => "compact",
            Self::Balanced => "balanced",
            Self::Detailed => "detailed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "off" => Some(Self::Off),
            "compact" => Some(Self::Compact),
            "balanced" => Some(Self::Balanced),
            "detailed" => Some(Self::Detailed),
            _ => None,
        }
    }

    pub fn policy(&self) -> ContextDeliveryPolicy {
        match self {
            Self::Off => ContextDeliveryPolicy {
                enabled: false,
                new_token_budget: 0,
                resume_token_budget: 0,
                new_extended_limit: 0,
                resume_first_delivery_extended_limit: 0,
                conflict_limit: 0,
            },
            Self::Compact => ContextDeliveryPolicy {
                enabled: true,
                new_token_budget: 2000,
                resume_token_budget: 1500,
                new_extended_limit: 3,
                resume_first_delivery_extended_limit: 2,
                conflict_limit: 2,
            },
            Self::Balanced => ContextDeliveryPolicy {
                enabled: true,
                new_token_budget: 4000,
                resume_token_budget: 3000,
                new_extended_limit: 10,
                resume_first_delivery_extended_limit: 5,
                conflict_limit: 5,
            },
            Self::Detailed => ContextDeliveryPolicy {
                enabled: true,
                new_token_budget: 8000,
                resume_token_budget: 6000,
                new_extended_limit: 20,
                resume_first_delivery_extended_limit: 10,
                conflict_limit: 10,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextDeliveryPolicy {
    pub enabled: bool,
    pub new_token_budget: usize,
    pub resume_token_budget: usize,
    pub new_extended_limit: usize,
    pub resume_first_delivery_extended_limit: usize,
    pub conflict_limit: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextSection {
    pub kind: String, // goal | current_state | constraint | decision | open_question | item | delta | gone | conflict | reminder
    pub title: String,
    pub content: String,
    pub authority: String,
    pub source_ref: Option<String>,
    /// Which workstream produced this section (for delivery snapshots).
    pub workstream_id: Option<String>,
    /// The revision this section reflects (for delivery snapshots).
    pub revision_id: Option<String>,
    /// The conflict this section reflects (for delivery snapshots).
    pub conflict_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionContextBundle {
    pub bundle_id: String,
    pub mode: String, // new | resume
    pub delivery_level: String,
    pub workstream_ids: Vec<String>,
    pub sections: Vec<ContextSection>,
    pub markdown: String,
    pub approx_tokens: usize,
}

/// One workstream's view inside an aggregated bundle.
#[derive(Debug, Clone, Serialize)]
pub struct WorkstreamContextView {
    pub workstream: crate::domain::Workstream,
    pub is_primary: bool,
}

/// Aggregated context: workstreams are deduplicated / ranked / conflict-
/// detected as data BEFORE any markdown is rendered.
#[derive(Debug, Clone, Serialize)]
pub struct AggregatedContext {
    pub primary_workstream: Option<WorkstreamContextView>,
    pub related_workstreams: Vec<WorkstreamContextView>,
    /// Core sections deduped across workstreams (primary wins duplicates).
    pub core: Vec<ContextSection>,
    /// Open conflicts across all participating workstreams.
    pub conflicts: Vec<ContextConflict>,
}

/// L1 projection: active items of core types, in canonical order.
pub fn resolve_core_context(db: &Db, workstream_id: &str) -> Result<Vec<ContextSection>> {
    let items = db.items_for_workstream(workstream_id, false)?;
    let mut out = Vec::new();
    for want in CORE_ITEM_TYPES {
        for (item, rev) in &items {
            if &item.kind == want {
                out.push(ContextSection {
                    kind: want.to_string(),
                    title: rev.title.clone(),
                    content: rev.content.clone(),
                    authority: item.authority.clone(),
                    source_ref: rev.source_ref.clone(),
                    workstream_id: Some(workstream_id.to_string()),
                    revision_id: Some(rev.id.clone()),
                    conflict_id: None,
                });
            }
        }
    }
    Ok(out)
}

/// Aggregate several workstreams into one coherent context: primary /
/// related separation, cross-workstream dedup, open conflicts.
pub fn aggregate_context(db: &Db, workstream_ids: &[String]) -> Result<AggregatedContext> {
    let mut primary = None;
    let mut related = Vec::new();
    let mut core: Vec<ContextSection> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut conflicts = Vec::new();

    for (i, ws_id) in workstream_ids.iter().enumerate() {
        let Some(ws) = db.get_workstream(ws_id)? else {
            continue;
        };
        let view = WorkstreamContextView {
            workstream: ws,
            is_primary: i == 0,
        };
        if i == 0 {
            primary = Some(view);
        } else {
            related.push(view);
        }

        for section in resolve_core_context(db, ws_id)? {
            let key = format!("{}|{}", section.kind, normalize_title(&section.title));
            if seen.insert(key) {
                core.push(section);
            }
        }

        conflicts.extend(db.conflicts_for_workstream(ws_id, false)?);
    }

    Ok(AggregatedContext {
        primary_workstream: primary,
        related_workstreams: related,
        core,
        conflicts,
    })
}

/// Build the markdown bundle for a set of workstreams.
///
/// The token budget applies at SECTION SELECTION time, before rendering:
/// `bundle.sections` contains exactly the sections the rendered markdown
/// delivers — never more. Delivery snapshots are derived from `sections`,
/// so filtering after rendering (the old behavior) would record context as
/// "delivered" that the agent never received, and the next resume would
/// skip it as a false delta.
pub fn build_bundle(
    db: &Db,
    mode: &str,
    session: Option<&Session>,
    workstream_ids: &[String],
    delivery_level: ContextDeliveryLevel,
) -> Result<SessionContextBundle> {
    build_bundle_with_policy(
        db,
        mode,
        session,
        workstream_ids,
        delivery_level.policy(),
        delivery_level.as_str(),
    )
}

pub fn build_bundle_with_policy(
    db: &Db,
    mode: &str,
    session: Option<&Session>,
    workstream_ids: &[String],
    policy: ContextDeliveryPolicy,
    delivery_level_label: &str,
) -> Result<SessionContextBundle> {
    if mode == "resume" && session.is_none() {
        return Err(crate::error::other("Resume 模式必须提供 Session"));
    }

    if !policy.enabled {
        return Ok(SessionContextBundle {
            bundle_id: new_id(),
            mode: mode.to_string(),
            delivery_level: delivery_level_label.to_string(),
            workstream_ids: workstream_ids.to_vec(),
            sections: vec![],
            markdown: String::new(),
            approx_tokens: 0,
        });
    }

    let mut sections: Vec<ContextSection> = Vec::new();
    let agg = aggregate_context(db, workstream_ids)?;

    // Workstream headers — primary first, then related, clearly labeled.
    let all_views: Vec<&WorkstreamContextView> = agg
        .primary_workstream
        .iter()
        .chain(agg.related_workstreams.iter())
        .collect();
    for view in &all_views {
        let ws = &view.workstream;
        sections.push(ContextSection {
            kind: if view.is_primary {
                "workstream_header".into()
            } else {
                "related_workstream_header".into()
            },
            title: ws.title.clone(),
            content: if ws.description.is_empty() {
                String::new()
            } else {
                ws.description.clone()
            },
            authority: "system_observed".into(),
            source_ref: None,
            workstream_id: Some(ws.id.clone()),
            revision_id: None,
            conflict_id: None,
        });
    }

    if mode == "resume" {
        let session = session.ok_or_else(|| crate::error::other("Resume 模式必须提供 Session"))?;
        sections.extend(build_resume_sections(
            db,
            session,
            workstream_ids,
            &agg,
            &policy,
        )?);
    } else {
        // aggregated core (deduped across workstreams, primary wins)
        sections.extend(agg.core.clone());

        // extended items — most recent first, trimmed hard for budget;
        // extended titles are deduped across workstreams as well
        let mut seen_ext: HashSet<String> = HashSet::new();
        for ws_id in workstream_ids {
            let items = db.items_for_workstream(ws_id, false)?;
            for (item, rev) in items
                .iter()
                .filter(|(item, _)| !CORE_ITEM_TYPES.contains(&item.kind.as_str()))
                .take(policy.new_extended_limit)
            {
                let key = format!("{}|{}", item.kind, normalize_title(&rev.title));
                if !seen_ext.insert(key) {
                    continue;
                }
                sections.push(ContextSection {
                    kind: item.kind.clone(),
                    title: rev.title.clone(),
                    content: crate::adapters::truncate_text(&rev.content, 400),
                    authority: item.authority.clone(),
                    source_ref: rev.source_ref.clone(),
                    workstream_id: Some(ws_id.clone()),
                    revision_id: Some(rev.id.clone()),
                    conflict_id: None,
                });
            }
        }

        if !agg.conflicts.is_empty() {
            for c in agg.conflicts.iter().take(policy.conflict_limit) {
                sections.push(conflict_section(db, c)?);
            }
        }
    }

    let budget = if mode == "resume" {
        policy.resume_token_budget
    } else {
        policy.new_token_budget
    };

    // Sections fit the budget or they are not delivered — and what is not
    // delivered must not appear in `sections` (see doc above).
    let (markdown, delivered_sections) = render_markdown_within_budget(mode, &sections, budget * 3);
    Ok(SessionContextBundle {
        bundle_id: new_id(),
        mode: mode.to_string(),
        delivery_level: delivery_level_label.to_string(),
        workstream_ids: workstream_ids.to_vec(),
        sections: delivered_sections,
        approx_tokens: markdown.len() / 3, // rough CJK/EN mix heuristic
        markdown,
    })
}

/// Resume = Current − LastDelivered. Uses the recorded revision snapshot,
/// not timestamps: same-second edits, replayed revisions and clock skew
/// cannot fool it. "已经消失" itself counts as a change.
fn build_resume_sections(
    db: &Db,
    session: &Session,
    workstream_ids: &[String],
    agg: &AggregatedContext,
    policy: &ContextDeliveryPolicy,
) -> Result<Vec<ContextSection>> {
    let deliveries = db.latest_deliveries(&session.id)?;
    let delivered_for = |ws_id: &str| -> Option<&crate::domain::ContextDelivery> {
        deliveries.iter().find(|d| d.workstream_id == ws_id)
    };

    let mut sections = Vec::new();

    // Minimal core reminder: goal / current state of the primary workstream.
    for s in &agg.core {
        if s.kind == "goal" || s.kind == "current_state" {
            let mut s = s.clone();
            s.kind = "reminder".into();
            sections.push(s);
        }
    }

    let mut any_change = false;

    for ws_id in workstream_ids {
        let Some(delivery) = delivered_for(ws_id) else {
            // This workstream was never delivered to this session: inject
            // its full core + top extended items (first delivery).
            any_change = true;
            for s in agg
                .core
                .iter()
                .filter(|s| s.workstream_id.as_deref() == Some(ws_id))
            {
                sections.push(s.clone());
            }
            let items = db.items_for_workstream(ws_id, false)?;
            for (item, rev) in items
                .iter()
                .filter(|(item, _)| !CORE_ITEM_TYPES.contains(&item.kind.as_str()))
                .take(policy.resume_first_delivery_extended_limit)
            {
                sections.push(ContextSection {
                    kind: item.kind.clone(),
                    title: rev.title.clone(),
                    content: crate::adapters::truncate_text(&rev.content, 400),
                    authority: item.authority.clone(),
                    source_ref: rev.source_ref.clone(),
                    workstream_id: Some(ws_id.clone()),
                    revision_id: Some(rev.id.clone()),
                    conflict_id: None,
                });
            }
            for c in db
                .conflicts_for_workstream(ws_id, false)?
                .iter()
                .take(policy.conflict_limit)
            {
                sections.push(conflict_section(db, c)?);
            }
            continue;
        };

        let delivered: HashSet<&str> = delivery
            .delivered_revisions
            .iter()
            .map(|s| s.as_str())
            .collect();
        let delivered_conflicts: HashSet<&str> = delivery
            .delivered_conflicts
            .iter()
            .map(|s| s.as_str())
            .collect();

        // 1. new or updated: active items whose current revision was never
        //    delivered.
        let items = db.items_for_workstream(ws_id, false)?;
        for (item, rev) in &items {
            if !delivered.contains(rev.id.as_str()) {
                any_change = true;
                sections.push(ContextSection {
                    kind: "delta".into(),
                    title: rev.title.clone(),
                    content: rev.content.clone(),
                    authority: item.authority.clone(),
                    source_ref: rev.source_ref.clone(),
                    workstream_id: Some(ws_id.clone()),
                    revision_id: Some(rev.id.clone()),
                    conflict_id: None,
                });
            }
        }

        // 2. disappeared: previously delivered revisions whose items are no
        //    longer active (resolved / superseded / deleted).
        for rev_id in &delivery.delivered_revisions {
            if let Some(rev) = db.get_revision(rev_id)? {
                if let Some(item) = db.get_item(&rev.item_id)? {
                    if item.status != "active" {
                        any_change = true;
                        sections.push(ContextSection {
                            kind: "gone".into(),
                            title: rev.title.clone(),
                            content: format!(
                                "该条目已{}，不再属于当前有效上下文。",
                                match item.status.as_str() {
                                    "resolved" => "被标记完成",
                                    "superseded" => "被新版本取代",
                                    "deleted" => "被删除",
                                    other => other,
                                }
                            ),
                            authority: item.authority.clone(),
                            source_ref: rev.source_ref.clone(),
                            workstream_id: Some(ws_id.clone()),
                            revision_id: Some(rev.id.clone()),
                            conflict_id: None,
                        });
                    }
                }
            }
        }

        // 3. new or undelivered conflicts.
        let mut conf_count = 0;
        for c in db.conflicts_for_workstream(ws_id, false)? {
            if !delivered_conflicts.contains(c.id.as_str()) {
                if conf_count >= policy.conflict_limit {
                    break;
                }
                conf_count += 1;
                any_change = true;
                sections.push(conflict_section(db, &c)?);
            }
        }

        // 4. resolved or closed conflicts that were previously delivered to the agent
        for cid in &delivery.delivered_conflicts {
            match db.get_conflict(cid)? {
                Some(c) if c.status != "open" => {
                    any_change = true;
                    let left = db.get_item(&c.left_item_id)?;
                    let left_desc = match &left {
                        Some(i) => format!("{}（{}）", i.kind, i.authority),
                        None => "(已删除)".into(),
                    };
                    let resolution_text =
                        c.resolution.as_deref().unwrap_or(match c.status.as_str() {
                            "resolved" => "已解决",
                            "ignored" => "已忽略",
                            "closed" => "已关闭",
                            other => other,
                        });
                    sections.push(ContextSection {
                        kind: "conflict_resolved".into(),
                        title: format!("既有冲突已解决（{}）", left_desc),
                        content: format!("解决状态：{}", resolution_text),
                        authority: "user_explicit".into(),
                        source_ref: None,
                        workstream_id: Some(ws_id.clone()),
                        revision_id: None,
                        conflict_id: Some(c.id.clone()),
                    });
                }
                None => {
                    any_change = true;
                    sections.push(ContextSection {
                        kind: "conflict_resolved".into(),
                        title: "既有冲突已移除".into(),
                        content: "该冲突已被删除，不再有效。".into(),
                        authority: "user_explicit".into(),
                        source_ref: None,
                        workstream_id: Some(ws_id.clone()),
                        revision_id: None,
                        conflict_id: Some(cid.clone()),
                    });
                }
                _ => {}
            }
        }
    }

    // Nothing changed: only the minimal reminder, never the full history.
    if !any_change {
        sections.retain(|s| s.kind == "reminder");
    }
    Ok(sections)
}

fn conflict_section(db: &Db, c: &ContextConflict) -> Result<ContextSection> {
    let left = db.get_item(&c.left_item_id)?;
    let right = match &c.right_item_id {
        Some(id) => db.get_item(id)?,
        None => None,
    };
    let left_desc = match &left {
        Some(i) => format!("{}（{}）", i.kind, i.authority),
        None => "(已删除)".into(),
    };
    let right_desc = match &right {
        Some(i) => match i
            .current_revision_id
            .as_deref()
            .and_then(|rid| db.get_revision(rid).ok().flatten())
        {
            Some(r) => format!(
                "{}：{}",
                r.title,
                crate::adapters::truncate_text(&r.content, 200)
            ),
            None => "(无内容)".into(),
        },
        None => "(agent 新信息，未成条目)".into(),
    };
    Ok(ContextSection {
        kind: "conflict".into(),
        title: format!("与既有上下文冲突（{}）", left_desc),
        content: right_desc,
        authority: "agent_inferred".into(),
        source_ref: None,
        workstream_id: Some(c.workstream_id.clone()),
        revision_id: None,
        conflict_id: Some(c.id.clone()),
    })
}

fn render_markdown_within_budget(
    mode: &str,
    sections: &[ContextSection],
    max_chars: usize,
) -> (String, Vec<ContextSection>) {
    let mut md = String::new();
    md.push_str("# NoEnding Context Bundle\n\n");
    md.push_str(match mode {
        "resume" => "> Resume：以下是自你上次会话以来的上下文变化，请先消化增量，再继续工作。\n\n",
        _ => "> 这是当前 Workstream 的有效语义状态（不是聊天记录）。请基于以下上下文继续推进。\n\n",
    });

    let mut delivered = Vec::new();
    for s in sections {
        let block = render_section(s);
        if md.len() + block.len() > max_chars {
            md.push_str("\n\n… (上下文因预算被截断)\n");
            break;
        }
        md.push_str(&block);
        delivered.push(s.clone());
    }
    (md, delivered)
}

fn render_section(s: &ContextSection) -> String {
    let labels = [
        ("workstream_header", "Workstream"),
        ("related_workstream_header", "Related Workstream"),
        ("reminder", "Current Task Reminder / 当前任务提醒"),
        ("goal", "Goal / 目标"),
        ("current_state", "Current State / 当前状态"),
        ("constraint", "Constraints / 约束"),
        ("decision", "Decisions / 决定"),
        ("open_question", "Open Questions / 未决问题"),
        ("delta", "Changed Since Your Last Activity"),
        ("gone", "Resolved / Superseded / Deleted"),
        ("conflict", "New Conflicts"),
        ("conflict_resolved", "Resolved Conflicts / 已解决冲突"),
    ];

    let label = labels
        .iter()
        .find(|(k, _)| *k == s.kind)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| format!("Item ({})", s.kind));
    let mut md = String::new();
    match s.kind.as_str() {
        "workstream_header" | "related_workstream_header" => {
            if s.kind == "related_workstream_header" {
                md.push_str(&format!("## {} — Related Workstream\n\n", s.title));
            } else {
                md.push_str(&format!("## {}\n\n", s.title));
            }
            if !s.content.is_empty() {
                md.push_str(&s.content);
                md.push_str("\n\n");
            }
        }
        _ => {
            md.push_str(&format!("### {}\n", label));
            md.push_str(&format!("**{}**\n", s.title));
            if !s.content.is_empty() && s.content != s.title {
                md.push_str(&format!("{}\n", s.content));
            }
            if let Some(src) = &s.source_ref {
                md.push_str(&format!("> 来源: {}\n", src));
            }
            md.push('\n');
        }
    }
    md
}

fn normalize_title(t: &str) -> String {
    t.chars()
        .filter(|c| c.is_alphanumeric() || !c.is_ascii())
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}
