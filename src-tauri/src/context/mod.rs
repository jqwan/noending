//! Context Builder — produces the SessionContextBundle handed to agents.
//!
//! New session: minimal sufficient context (Goal, Current State,
//! Constraints, Decisions, Open Questions, high-relevance items).
//! Resume: delta since the session last saw (plus a compact core reminder).

use std::collections::HashSet;

use serde::Serialize;

use crate::domain::{CORE_ITEM_TYPES, Session};
use crate::error::Result;
use crate::storage::Db;

#[derive(Debug, Clone, Serialize)]
pub struct ContextSection {
    pub kind: String, // goal | current_state | constraint | decision | open_question | item
    pub title: String,
    pub content: String,
    pub authority: String,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionContextBundle {
    pub mode: String, // new | resume
    pub workstream_ids: Vec<String>,
    pub sections: Vec<ContextSection>,
    pub markdown: String,
    pub approx_tokens: usize,
}

/// L1 projection: active items of core types, in canonical order.
pub fn resolve_core_context(
    db: &Db,
    workstream_id: &str,
) -> Result<Vec<ContextSection>> {
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
                });
            }
        }
    }
    Ok(out)
}

/// Build the markdown bundle for a set of workstreams.
pub fn build_bundle(
    db: &Db,
    mode: &str,
    session: Option<&Session>,
    workstream_ids: &[String],
    token_budget: usize,
) -> Result<SessionContextBundle> {
    let mut sections: Vec<ContextSection> = Vec::new();
    let mut seen_workstreams: HashSet<String> = HashSet::new();

    for ws_id in workstream_ids {
        let ws = match db.get_workstream(ws_id)? {
            Some(w) => w,
            None => continue,
        };
        seen_workstreams.insert(ws.id.clone());

        sections.push(ContextSection {
            kind: "workstream_header".into(),
            title: ws.title.clone(),
            content: if ws.description.is_empty() {
                String::new()
            } else {
                ws.description.clone()
            },
            authority: "system_observed".into(),
            source_ref: None,
        });

        sections.extend(resolve_core_context(db, ws_id)?);

        // extended items — most recent first, trimmed hard for budget
        let items = db.items_for_workstream(ws_id, false)?;
        for (item, rev) in items.iter().take(10) {
            if CORE_ITEM_TYPES.contains(&item.kind.as_str()) {
                continue;
            }
            sections.push(ContextSection {
                kind: item.kind.clone(),
                title: rev.title.clone(),
                content: crate::adapters::truncate_text(&rev.content, 400),
                authority: item.authority.clone(),
                source_ref: rev.source_ref.clone(),
            });
        }

        // resume delta: what this session hasn't seen yet
        if mode == "resume" {
            if let Some(s) = session {
                let bindings = db.bindings_for_session(&s.id)?;
                if let Some(b) = bindings.iter().find(|b| &b.workstream_id == ws_id) {
                    let changed = db.active_items_since(ws_id, &b.last_used_at)?;
                    for (item, rev) in changed.iter().take(8) {
                        sections.push(ContextSection {
                            kind: "delta".into(),
                            title: format!("更新：{}", rev.title),
                            content: rev.content.clone(),
                            authority: item.authority.clone(),
                            source_ref: rev.source_ref.clone(),
                        });
                    }
                }
            }
        }
    }

    let markdown = render_markdown(mode, &sections);
    let approx_tokens = markdown.len() / 3; // rough CJK/EN mix heuristic
    let mut bundle = SessionContextBundle {
        mode: mode.to_string(),
        workstream_ids: workstream_ids.to_vec(),
        sections,
        markdown: markdown.clone(),
        approx_tokens,
    };
    if approx_tokens > token_budget {
        bundle.markdown = truncate_markdown(&markdown, token_budget * 3);
        bundle.approx_tokens = token_budget;
    }
    Ok(bundle)
}

fn render_markdown(mode: &str, sections: &[ContextSection]) -> String {
    let mut md = String::new();
    md.push_str("# NoEnding Context Bundle\n\n");
    md.push_str(match mode {
        "resume" => "> Resume：以下是自你上次会话以来发生变化的上下文，请先消化这些增量，再继续工作。\n\n",
        _ => "> 这是当前 Workstream 的有效语义状态（不是聊天记录）。请基于以下上下文继续推进。\n\n",
    });

    let labels = [
        ("workstream_header", "Workstream"),
        ("goal", "Goal / 目标"),
        ("current_state", "Current State / 当前状态"),
        ("constraint", "Constraints / 约束"),
        ("decision", "Decisions / 决定"),
        ("open_question", "Open Questions / 未决问题"),
        ("delta", "Changed since last session"),
    ];

    for s in sections {
        let label = labels
            .iter()
            .find(|(k, _)| *k == s.kind)
            .map(|(_, l)| l.to_string())
            .unwrap_or_else(|| format!("Item ({})", s.kind));
        match s.kind.as_str() {
            "workstream_header" => {
                md.push_str(&format!("## {}\n\n", s.title));
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
    }
    md
}

fn truncate_markdown(md: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for block in md.split("\n\n") {
        if out.len() + block.len() > max_chars {
            out.push_str("\n\n… (上下文因预算被截断)");
            break;
        }
        out.push_str(block);
        out.push_str("\n\n");
    }
    out
}
