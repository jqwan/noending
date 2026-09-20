import { useState, useEffect } from "react";
import { api } from "../../api";
import { Modal, timeAgo } from "../../components/common";
import {
  AUTHORITY_LABELS, KIND_LABELS,
  type ContextItem, type ContextItemRevision, type WorkstreamContext,
} from "../../types";
import SourceDetailModal from "./SourceDetailModal";

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

interface Props {
  ctx: WorkstreamContext;
  focusedItemId?: string | null;
  onChanged: () => void;
  onNavigateSession?: (sessionId: string) => void;
}

/**
 * Workstream Context Workbench（当前事实、追溯来源、演进关系、状态纠偏控制面）：
 * - Core Context 严格以后端 ctx.core (resolve_core_context 权威投影) 为准展示当前事实
 * - 提供 Provenance 追溯 (SourceDetailModal)、演进历史 (History) 与关系标签 (替代/被替代)
 * - 支持标题 + 详细内容双字段内联编辑，保存生成 user_edit 新 Revision
 */
export default function WorkstreamContext({ ctx, focusedItemId, onChanged, onNavigateSession }: Props) {
  const [editing, setEditing] = useState<string | null>(null); // item id
  const [editTitle, setEditTitle] = useState("");
  const [editContent, setEditContent] = useState("");
  const [historyOf, setHistoryOf] = useState<[ContextItem, ContextItemRevision[]] | null>(null);
  const [sourceRevisionId, setSourceRevisionId] = useState<string | null>(null);

  const [adding, setAdding] = useState(false);
  const [newKind, setNewKind] = useState("todo");
  const [newTitle, setNewTitle] = useState("");
  const [newContent, setNewContent] = useState("");
  const [showAllExt, setShowAllExt] = useState(false);

  // Maps for fast lookup
  const itemByRevId = new Map(ctx.items.map(([item, rev]) => [rev.id, [item, rev] as const]));
  const relationsMap = new Map((ctx.relations ?? []).map((r) => [r.item_id, r]));

  const active = ctx.items.filter(([i]) => i.status === "active");
  const extended = active
    .filter(([i]) => !(CORE_ORDER as readonly string[]).includes(i.kind))
    .sort((a, b) => b[0].updated_at.localeCompare(a[0].updated_at));
  const closed = ctx.items.filter(([i]) => i.status !== "active" && i.status !== "deleted");
  const extShown = showAllExt ? extended : extended.slice(0, 8);

  const startEdit = (item: ContextItem, rev: { title: string; content: string }) => {
    setEditing(item.id);
    setEditTitle(rev.title);
    setEditContent(rev.content ?? "");
  };

  const saveEdit = async (itemId: string) => {
    if (!editTitle.trim()) return;
    await api.editContextItem(itemId, editTitle.trim(), editContent);
    setEditing(null);
    onChanged();
  };

  const openHistory = async (item: ContextItem) => {
    const revs = await api.getItemHistory(item.id);
    setHistoryOf([item, revs]);
  };

  useEffect(() => {
    if (!focusedItemId) return;
    const el = document.querySelector(`[data-context-item-id="${focusedItemId}"]`);
    const hiddenByDetails = el?.closest("details:not([open])");
    const found = ctx.items.find(([i]) => i.id === focusedItemId);

    if (el && !hiddenByDetails) {
      el.scrollIntoView({ behavior: "smooth", block: "center" });
      el.classList.add("item-highlight-pulse");
      const timer = setTimeout(() => {
        el.classList.remove("item-highlight-pulse");
      }, 2000);
      return () => clearTimeout(timer);
    } else if (found) {
      openHistory(found[0]);
    }
  }, [focusedItemId, ctx.items]);

  const renderRelations = (itemId: string) => {
    const rel = relationsMap.get(itemId);
    if (!rel) return null;
    const hasSupersedes = Boolean(rel.supersedes);
    const hasSupersededBy = Boolean(rel.superseded_by && rel.superseded_by.length > 0);
    if (!hasSupersedes && !hasSupersededBy) return null;

    return (
      <div className="row" style={{ gap: 6, marginTop: 4, flexWrap: "wrap" }}>
        {rel.supersedes && (
          <span className="badge" title={`已替代旧条目: ${rel.supersedes.title}`}>
            替代: {rel.supersedes.title}
          </span>
        )}
        {hasSupersededBy && rel.superseded_by.map((s) => (
          <span key={s.id} className="badge warn" title={`已被新条目替代: ${s.title}`}>
            被替代: {s.title}
          </span>
        ))}
      </div>
    );
  };

  return (
    <div>
      <div className="section-label">Current Context</div>
      <div className="l1">
        {CORE_ORDER.map((kind) => {
          // Strictly adhere to L1 projection ctx.core
          const sections = (ctx.core ?? []).filter((s) => s.kind === kind);
          return (
            <div className="l1-row" key={kind}>
              <div className="label">
                <span className="en">{CORE_LABELS[kind].en}</span>
                {CORE_LABELS[kind].zh}
              </div>
              <div className="l1-body">
                {sections.length === 0 && (
                  <div className="row between" style={{ alignItems: "center" }}>
                    <div className="l1-none">暂无当前 {CORE_LABELS[kind].zh}</div>
                    <button
                      className="btn small ghost"
                      onClick={() => {
                        setNewKind(kind);
                        setAdding(true);
                      }}
                    >
                      + 设定
                    </button>
                  </div>
                )}
                {sections.map((sec, idx) => {
                  const pair = sec.revision_id ? itemByRevId.get(sec.revision_id) : null;
                  const item = pair ? pair[0] : null;
                  const rev = pair ? pair[1] : null;
                  const itemId = item ? item.id : `sec-${idx}`;

                  if (item && editing === item.id) {
                    return (
                      <div className="ctx-edit" key={itemId} style={{ padding: "8px 0" }}>
                        <label className="field" style={{ marginBottom: 6 }}>
                          <span>标题</span>
                          <input
                            type="text"
                            value={editTitle}
                            autoFocus
                            onChange={(e) => setEditTitle(e.target.value)}
                          />
                        </label>
                        <label className="field" style={{ marginBottom: 8 }}>
                          <span>内容</span>
                          <textarea
                            value={editContent}
                            rows={3}
                            onChange={(e) => setEditContent(e.target.value)}
                          />
                        </label>
                        <div className="row" style={{ justifyContent: "flex-end", gap: 8 }}>
                          <button className="btn small" onClick={() => setEditing(null)}>取消</button>
                          <button className="btn small primary" onClick={() => saveEdit(item.id)}>保存为新 Revision</button>
                        </div>
                      </div>
                    );
                  }

                  return (
                    <div
                      className="ctx-entry"
                      key={sec.revision_id ?? itemId}
                      data-context-item-id={item?.id ?? itemId}
                    >
                      <div className="row between" style={{ alignItems: "flex-start", gap: 12 }}>
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <div className="entry-title">{sec.title}</div>
                          {sec.content && sec.content !== sec.title && (
                            <div className="small" style={{ whiteSpace: "pre-wrap", marginTop: 3, color: "var(--text-secondary)" }}>
                              {sec.content}
                            </div>
                          )}
                          {item && renderRelations(item.id)}
                          <div className="entry-src" style={{ display: "flex", alignItems: "center", gap: 6, marginTop: 4, flexWrap: "wrap" }}>
                            <span className="badge" style={{ fontSize: 10, padding: "0 6px" }}>
                              {AUTHORITY_LABELS[sec.authority] ?? sec.authority}
                            </span>
                            {item && <span>{timeAgo(item.updated_at)}</span>}
                            {rev?.source_type && (
                              <>
                                <span>·</span>
                                <span>{rev.source_type}</span>
                              </>
                            )}
                          </div>
                        </div>

                        <div className="ctx-hover" style={{ position: "static", display: "flex", gap: 4, alignItems: "center", flexShrink: 0 }}>
                          {sec.revision_id && (
                            <button
                              className="btn small ghost"
                              title="查看来源凭据与权威"
                              onClick={() => setSourceRevisionId(sec.revision_id!)}
                            >
                              来源
                            </button>
                          )}
                          {item && (
                            <>
                              <button
                                className="btn small ghost"
                                title="查看演进历史"
                                onClick={() => openHistory(item)}
                              >
                                历史
                              </button>
                              <button
                                className="btn small ghost"
                                title="编辑标题与内容"
                                onClick={() => startEdit(item, rev ?? { title: sec.title, content: sec.content })}
                              >
                                编辑
                              </button>
                              {RESOLVABLE.includes(sec.kind) && (
                                <button
                                  className="btn small ghost"
                                  title="标记为已解决"
                                  onClick={async () => {
                                    await api.setItemStatus(item.id, "resolved");
                                    onChanged();
                                  }}
                                >
                                  完成
                                </button>
                              )}
                              <button
                                className="btn small ghost"
                                title="标记为废弃"
                                onClick={async () => {
                                  await api.setItemStatus(item.id, "obsolete");
                                  onChanged();
                                }}
                              >
                                废弃
                              </button>
                            </>
                          )}
                        </div>
                      </div>
                    </div>
                  );
                })}
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
        <div className="ext-row ctx-entry" key={item.id} data-context-item-id={item.id}>
          {editing === item.id ? (
            <div className="ctx-edit" style={{ flex: 1, padding: "4px 0" }}>
              <label className="field" style={{ marginBottom: 6 }}>
                <span>标题</span>
                <input
                  type="text"
                  value={editTitle}
                  autoFocus
                  onChange={(e) => setEditTitle(e.target.value)}
                />
              </label>
              <label className="field" style={{ marginBottom: 8 }}>
                <span>内容</span>
                <textarea
                  value={editContent}
                  rows={3}
                  onChange={(e) => setEditContent(e.target.value)}
                />
              </label>
              <div className="row" style={{ justifyContent: "flex-end", gap: 8 }}>
                <button className="btn small" onClick={() => setEditing(null)}>取消</button>
                <button className="btn small primary" onClick={() => saveEdit(item.id)}>保存为新 Revision</button>
              </div>
            </div>
          ) : (
            <div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", width: "100%", gap: 12 }}>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div className="row" style={{ gap: 8, alignItems: "baseline", flexWrap: "wrap" }}>
                  <span className="badge">{KIND_LABELS[item.kind] ?? item.kind}</span>
                  <span className="ext-title" style={{ fontWeight: 550 }}>{rev.title}</span>
                  <span className="muted small" style={{ flex: "none" }}>{timeAgo(item.updated_at)}</span>
                </div>
                {rev.content && rev.content !== rev.title && (
                  <div className="small" style={{ marginTop: 3, color: "var(--text-secondary)", whiteSpace: "pre-wrap" }}>
                    {rev.content}
                  </div>
                )}
                {renderRelations(item.id)}
              </div>
              <div className="ctx-hover" style={{ position: "static", display: "flex", gap: 4, flexShrink: 0 }}>
                <button className="btn small ghost" title="查看来源凭据" onClick={() => setSourceRevisionId(rev.id)}>来源</button>
                <button className="btn small ghost" title="查看演进历史" onClick={() => openHistory(item)}>历史</button>
                <button className="btn small ghost" title="编辑" onClick={() => startEdit(item, rev)}>编辑</button>
                {RESOLVABLE.includes(item.kind) && (
                  <button className="btn small ghost" title="标记为完成" onClick={async () => { await api.setItemStatus(item.id, "resolved"); onChanged(); }}>完成</button>
                )}
                <button className="btn small ghost" title="标记为废弃" onClick={async () => { await api.setItemStatus(item.id, "obsolete"); onChanged(); }}>废弃</button>
                <button className="btn small ghost" title="删除" onClick={async () => {
                  if (window.confirm(`确定删除「${rev.title}」吗？`)) {
                    await api.deleteContextItem(item.id);
                    onChanged();
                  }
                }}>删除</button>
              </div>
            </div>
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
              <div className="feed-row" key={item.id} data-context-item-id={item.id}>
                <span className="badge">{item.status}</span>
                <span style={{ flex: 1 }}>{rev.title}</span>
                <button className="link" onClick={() => openHistory(item)}>历史</button>
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
              await api.addContextItem(ctx.workstream.id, newKind, newTitle.trim(), newContent);
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
                <div className="row" style={{ gap: 8 }}>
                  <span className="entry-src">{rev.source_type ?? "manual"}{rev.source_ref ? ` ${rev.source_ref}` : ""}</span>
                  <button className="link small" onClick={() => setSourceRevisionId(rev.id)}>来源凭据</button>
                </div>
              </div>
              <div className="small" style={{ marginTop: 4, whiteSpace: "pre-wrap" }}>{rev.content || rev.title}</div>
            </div>
          ))}
          {historyOf[1].length === 0 && <div className="muted small">无历史记录。</div>}
        </Modal>
      )}

      {sourceRevisionId && (
        <SourceDetailModal
          revisionId={sourceRevisionId}
          onClose={() => setSourceRevisionId(null)}
          onNavigateSession={onNavigateSession}
        />
      )}
    </div>
  );
}
