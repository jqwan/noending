import React, { useState } from "react";
import { api } from "../../api";
import { Modal, timeAgo } from "../../components/common";
import {
  AUTHORITY_LABELS, KIND_LABELS,
  type ContextItem, type ContextItemRevision, type WorkstreamContext,
} from "../../types";

const CORE_ORDER = ["current_state", "goal", "open_question", "decision", "constraint"] as const;
const CORE_LABELS: Record<string, { en: string; zh: string }> = {
  current_state: { en: "Current State", zh: "现在做到哪了" },
  goal: { en: "Goal", zh: "最终在做什么" },
  open_question: { en: "Open Questions", zh: "未解决的关键问题" },
  decision: { en: "Decisions", zh: "仍然有效的决定" },
  constraint: { en: "Constraints", zh: "不能违反的限制" },
};
const EXTENDED_KINDS = ["todo", "finding", "issue", "risk", "note", "reference", "artifact", "requirement", "decision_detail", "research_note"];
const RESOLVABLE = ["todo", "open_question", "issue"];

/**
 * Workstream Context 主栏（整体设计方案 §31-§34）：
 * Core Context（Current State 第一位）→ Extended Context（compact rows + View all）。
 * 编辑直接内联完成，保存生成 user_edit 新 Revision（§32）；历史走 Modal（§33）。
 */
export default function WorkstreamContext({ ctx, onChanged }: {
  ctx: WorkstreamContext;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState<string | null>(null); // item id
  const [draft, setDraft] = useState("");
  const [historyOf, setHistoryOf] = useState<[ContextItem, ContextItemRevision[]] | null>(null);
  const [adding, setAdding] = useState(false);
  const [newKind, setNewKind] = useState("todo");
  const [newTitle, setNewTitle] = useState("");
  const [newContent, setNewContent] = useState("");
  const [showAllExt, setShowAllExt] = useState(false);

  const active = ctx.items.filter(([i]) => i.status === "active");
  const coreOf = (kind: string) =>
    active
      .filter(([i]) => i.kind === kind)
      .sort((a, b) => b[0].updated_at.localeCompare(a[0].updated_at));
  const extended = active
    .filter(([i]) => !(CORE_ORDER as readonly string[]).includes(i.kind))
    .sort((a, b) => b[0].updated_at.localeCompare(a[0].updated_at));
  const closed = ctx.items.filter(([i]) => i.status !== "active" && i.status !== "deleted");
  const extShown = showAllExt ? extended : extended.slice(0, 8);

  const startEdit = async (item: ContextItem) => {
    const revs = await api.getItemHistory(item.id);
    const head = revs[revs.length - 1];
    setEditing(item.id);
    setDraft(head?.content ?? "");
  };

  const saveEdit = async (item: ContextItem) => {
    const revs = await api.getItemHistory(item.id);
    const head = revs[revs.length - 1];
    await api.editContextItem(item.id, head?.title ?? "", draft);
    setEditing(null);
    onChanged();
  };

  const openHistory = async (item: ContextItem) => {
    const revs = await api.getItemHistory(item.id);
    setHistoryOf([item, revs]);
  };

  return (
    <div>
      <div className="section-label">Context</div>
      <div className="l1">
        {CORE_ORDER.map((kind) => {
          const entries = coreOf(kind);
          return (
            <div className="l1-row" key={kind}>
              <div className="label">
                <span className="en">{CORE_LABELS[kind].en}</span>
                {CORE_LABELS[kind].zh}
              </div>
              <div className="l1-body">
                {entries.length === 0 && <div className="l1-none">还没有内容</div>}
                {entries.map(([item, rev]) => (
                  <div className="ctx-entry" key={item.id}>
                    {editing === item.id ? (
                      <div className="ctx-edit">
                        <textarea value={draft} autoFocus onChange={(e) => setDraft(e.target.value)} />
                        <div className="row" style={{ justifyContent: "flex-end" }}>
                          <button className="btn small" onClick={() => setEditing(null)}>取消</button>
                          <button className="btn small primary" onClick={() => saveEdit(item)}>保存为新 Revision</button>
                        </div>
                      </div>
                    ) : (
                      <>
                        <div className="entry-title">{rev.title}</div>
                        {rev.content && rev.content !== rev.title && (
                          <div className="small" style={{ whiteSpace: "pre-wrap", marginTop: 2 }}>{rev.content}</div>
                        )}
                        <div className="entry-src">
                          {AUTHORITY_LABELS[item.authority] ?? item.authority}
                          {rev.source_ref ? ` ${rev.source_ref}` : ""}
                        </div>
                        <div className="ctx-hover">
                          <button className="link" onClick={() => startEdit(item)}>Edit</button>
                          <button className="link" onClick={() => openHistory(item)}>History</button>
                        </div>
                      </>
                    )}
                  </div>
                ))}
              </div>
            </div>
          );
        })}
      </div>

      <div className="page-head" style={{ marginTop: 34, marginBottom: 8 }}>
        <div className="section-label" style={{ margin: 0 }}>More Context</div>
        <button className="btn small ghost" onClick={() => { setNewKind("todo"); setAdding(true); }}>添加条目</button>
      </div>
      {extended.length === 0 && (
        <div className="l1-none">暂无扩展条目。Sync 会自动从相关 Session 提取 todo、finding 等条目。</div>
      )}
      {extShown.map(([item, rev]) => (
        <div className="ext-row ctx-entry" key={item.id}>
          {editing === item.id ? (
            <div className="ctx-edit" style={{ flex: 1 }}>
              <textarea value={draft} autoFocus onChange={(e) => setDraft(e.target.value)} />
              <div className="row" style={{ justifyContent: "flex-end" }}>
                <button className="btn small" onClick={() => setEditing(null)}>取消</button>
                <button className="btn small primary" onClick={() => saveEdit(item)}>保存为新 Revision</button>
              </div>
            </div>
          ) : (
            <>
              <span className="badge">{KIND_LABELS[item.kind] ?? item.kind}</span>
              <span className="ext-title">{rev.title}</span>
              {rev.content && rev.content !== rev.title && <span className="ext-content">{rev.content}</span>}
              <span className="muted small" style={{ flex: "none" }}>{timeAgo(item.updated_at)}</span>
              <div className="ctx-hover" style={{ position: "static" }}>
                {RESOLVABLE.includes(item.kind) && (
                  <button className="link" onClick={async () => { await api.setItemStatus(item.id, "resolved"); onChanged(); }}>完成</button>
                )}
                <button className="link" onClick={() => startEdit(item)}>Edit</button>
                <button className="link" onClick={() => openHistory(item)}>History</button>
                <button className="link" onClick={async () => { await api.setItemStatus(item.id, "obsolete"); onChanged(); }}>废弃</button>
              </div>
            </>
          )}
        </div>
      ))}
      {extended.length > 8 && (
        <button className="link" style={{ marginTop: 8 }} onClick={() => setShowAllExt((v) => !v)}>
          {showAllExt ? "收起" : `View all (${extended.length})`}
        </button>
      )}

      {closed.length > 0 && (
        <details className="details-feed" style={{ marginTop: 26 }}>
          <summary>已解决 / 已废弃 <span className="muted">（{closed.length}）</span></summary>
          <div>
            {closed.map(([item, rev]) => (
              <div className="feed-row" key={item.id}>
                <span className="badge">{item.status}</span>
                <span style={{ flex: 1 }}>{rev.title}</span>
                <button className="link" onClick={() => openHistory(item)}>History</button>
              </div>
            ))}
          </div>
        </details>
      )}

      {adding && (
        <Modal title="添加 Context 条目" onClose={() => setAdding(false)}>
          <label className="field"><span>类型</span>
            <select value={newKind} onChange={(e) => setNewKind(e.target.value)}>
              {(CORE_ORDER as readonly string[]).concat(EXTENDED_KINDS).map((k) => (
                <option key={k} value={k}>{KIND_LABELS[k] ?? k}</option>
              ))}
            </select></label>
          <label className="field"><span>标题</span>
            <input type="text" value={newTitle} onChange={(e) => setNewTitle(e.target.value)} autoFocus /></label>
          <label className="field"><span>内容</span><textarea value={newContent} onChange={(e) => setNewContent(e.target.value)} /></label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setAdding(false)}>取消</button>
            <button className="btn primary" onClick={async () => {
              if (!newTitle.trim()) return;
              await api.addContextItem(ctx.workstream.id, newKind, newTitle, newContent);
              setAdding(false); setNewTitle(""); setNewContent("");
              onChanged();
            }}>保存</button>
          </div>
        </Modal>
      )}

      {historyOf && (
        <Modal title={`演进历史 · ${historyOf[1][historyOf[1].length - 1]?.title ?? ""}`} onClose={() => setHistoryOf(null)}>
          {historyOf[1].map((rev, i) => (
            <div className="card hairline" key={rev.id} style={{ marginBottom: 8 }}>
              <div className="row between">
                <strong className="small">#{i + 1} {timeAgo(rev.created_at)}</strong>
                <span className="entry-src">{rev.source_type ?? "manual"}{rev.source_ref ? ` ${rev.source_ref}` : ""}</span>
              </div>
              <div className="small" style={{ marginTop: 4, whiteSpace: "pre-wrap" }}>{rev.content || rev.title}</div>
            </div>
          ))}
          {historyOf[1].length === 0 && <div className="muted small">无历史记录。</div>}
        </Modal>
      )}
    </div>
  );
}
