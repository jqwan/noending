import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { Modal, timeAgo, useRefreshSignal } from "../../components/common";
import { AgentBadge } from "../../App";
import type { Route } from "../../App";
import { AUTHORITY_LABELS, KIND_LABELS, type ContextItem, type ContextItemRevision, type WorkstreamContext } from "../../types";
import LauncherModal from "../launcher/LauncherModal";

const CORE_ORDER = ["goal", "current_state", "constraint", "decision", "open_question"] as const;
const CORE_LABELS: Record<string, { en: string; zh: string }> = {
  goal: { en: "Goal", zh: "我们最终在做什么" },
  current_state: { en: "Current State", zh: "现在是什么状态" },
  constraint: { en: "Constraints", zh: "不能违反的限制" },
  decision: { en: "Decisions", zh: "仍然有效的决定" },
  open_question: { en: "Open Questions", zh: "未解决的关键问题" },
};
const EXTENDED_KINDS = ["todo", "finding", "issue", "risk", "note", "reference", "artifact", "requirement", "decision_detail", "research_note"];

export default function WorkstreamView({ workstreamId, navigate, refreshSidebar }: {
  workstreamId: string;
  navigate: (r: Route) => void;
  refreshSidebar: () => void;
}) {
  const [ctx, setCtx] = useState<WorkstreamContext | null>(null);
  const [launching, setLaunching] = useState(false);
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<ContextItem | null>(null);
  const [editRev, setEditRev] = useState<ContextItemRevision | null>(null);
  const [historyOf, setHistoryOf] = useState<[ContextItem, ContextItemRevision[]] | null>(null);
  const [kind, setKind] = useState("note");
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");

  const refresh = useCallback(() => {
    api.getWorkstreamContext(workstreamId).then(setCtx).catch(console.error);
  }, [workstreamId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  if (!ctx) return <div className="main">加载中…</div>;
  const { workstream, core, items, related_sessions } = ctx;

  const coreSections = CORE_ORDER.map((k) => ({ kind: k, entries: core.filter((s) => s.kind === k) }));
  const extended = items.filter(([i]) => !CORE_ORDER.includes(i.kind as any) && i.status === "active");
  const superseded = items.filter(([i]) => i.status !== "active" && i.status !== "deleted");

  const saveNew = async () => {
    if (!title.trim()) return;
    await api.addContextItem(workstreamId, kind, title, content);
    setAdding(false); setTitle(""); setContent("");
    refresh(); refreshSidebar();
  };
  const saveEdit = async () => {
    if (!editing || !title.trim()) return;
    await api.editContextItem(editing.id, title, content);
    setEditing(null); refresh(); refreshSidebar();
  };

  const openEdit = async (item: ContextItem) => {
    const revs = await api.getItemHistory(item.id);
    const head = revs[revs.length - 1];
    setEditing(item); setEditRev(head ?? null);
    setTitle(head?.title ?? ""); setContent(head?.content ?? "");
  };
  const openHistory = async (item: ContextItem) => {
    const revs = await api.getItemHistory(item.id);
    setHistoryOf([item, revs]);
  };

  return (
    <div className="main narrow">
      <div className="page-head">
        <div>
          <h1>{workstream.title}</h1>
          <p className="page-sub" style={{ marginBottom: 8 }}>
            {workstream.description || "一个可以跨 Session 和 Agent 继续推进的工作上下文。"}
          </p>
        </div>
        <div className="actions">
          <button className="btn" onClick={refresh}>同步</button>
          <button className="btn accent" onClick={() => setLaunching(true)}>New Session</button>
        </div>
      </div>

      {/* L1 Core Context — ledger layout: current state first, transcript never */}
      <div className="l1">
        {coreSections.map(({ kind: k, entries }) => (
          <div className="l1-row" key={k}>
            <div className="label">
              <span className="en">{CORE_LABELS[k].en}</span>
              {CORE_LABELS[k].zh}
            </div>
            <div className="l1-body">
              {entries.length === 0 && <div className="l1-none">还没有内容</div>}
              {entries.map((s, idx) => (
                <div className="l1-entry" key={idx}>
                  <div className="entry-title">{s.title}</div>
                  {s.content && s.content !== s.title && <div className="small">{s.content}</div>}
                  <div className="entry-src">
                    {AUTHORITY_LABELS[s.authority] ?? s.authority}
                    {s.source_ref ? ` ${s.source_ref}` : ""}
                  </div>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>

      <div className="page-head" style={{ marginTop: 34 }}>
        <h2 style={{ margin: 0 }}>Extended Items</h2>
        <div className="actions">
          <button className="btn small" onClick={() => { setKind("note"); setAdding(true); }}>添加条目</button>
        </div>
      </div>
      {extended.length === 0 && (
        <div className="empty">暂无扩展条目。Sync 会自动从相关 Session 提取 todo、finding 等条目，也可以手动添加。</div>
      )}
      {extended.map(([item, rev]) => (
        <div className="list-row" key={item.id} style={{ cursor: "default", alignItems: "flex-start" }}>
          <div className="grow">
            <div className="title">
              <span className="badge" style={{ marginRight: 8 }}>{KIND_LABELS[item.kind] ?? item.kind}</span>
              {rev.title}
            </div>
            {rev.content && rev.content !== rev.title && (
              <div className="small muted" style={{ marginTop: 3, whiteSpace: "pre-wrap" }}>{rev.content}</div>
            )}
            <div className="entry-src" style={{ marginTop: 5 }}>
              {AUTHORITY_LABELS[item.authority] ?? item.authority}
              {rev.source_ref ? ` ${rev.source_ref}` : " 手动添加"}
            </div>
          </div>
          <div className="side" style={{ paddingTop: 2 }}>
            <span>{timeAgo(item.updated_at)}</span>
            <button className="link" onClick={() => openEdit(item)}>编辑</button>
            {["todo", "open_question", "issue"].includes(item.kind) && (
              <button className="link" onClick={async () => { await api.setItemStatus(item.id, "resolved"); refresh(); }}>完成</button>
            )}
            <button className="link" onClick={() => openHistory(item)}>历史</button>
            <button className="link" onClick={async () => { await api.setItemStatus(item.id, "obsolete"); refresh(); }}>废弃</button>
          </div>
        </div>
      ))}

      {superseded.length > 0 && (
        <>
          <h2>History <span className="muted small">（superseded / resolved / obsolete）</span></h2>
          {superseded.map(([item, rev]) => (
            <div className="list-row" key={item.id} style={{ cursor: "default", opacity: 0.6 }}>
              <div className="grow">
                <div className="title">
                  <span className="badge" style={{ marginRight: 8 }}>{KIND_LABELS[item.kind] ?? item.kind}</span>
                  {rev.title}
                </div>
              </div>
              <div className="side">
                <span className="badge">{item.status}</span>
                <button className="link" onClick={() => openHistory(item)}>查看历史</button>
              </div>
            </div>
          ))}
        </>
      )}

      <h2>Related Sessions</h2>
      {related_sessions.length === 0 && (
        <div className="empty">还没有 Session 关联到这里。从这台机器同步后，相关 Session 会自动出现在这里。</div>
      )}
      {related_sessions.map((s) => (
        <div className="list-row" key={s.id} onClick={() => navigate({ view: "session", sessionId: s.id })}>
          <div className="grow">
            <div className="title">{s.title ?? s.agent_session_id}</div>
            <div className="meta">{s.cwd ?? "无工作目录"}</div>
          </div>
          <div className="side">
            <AgentBadge agent={s.agent} />
            <span>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
          </div>
        </div>
      ))}

      {adding && (
        <Modal title="添加 Context 条目" onClose={() => setAdding(false)}>
          <label className="field"><span>类型</span>
            <select value={kind} onChange={(e) => setKind(e.target.value)}>
              {CORE_ORDER.concat(EXTENDED_KINDS as any).map((k) => <option key={k} value={k}>{KIND_LABELS[k] ?? k}</option>)}
            </select></label>
          <label className="field"><span>标题</span>
            <input type="text" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus /></label>
          <label className="field"><span>内容</span><textarea value={content} onChange={(e) => setContent(e.target.value)} /></label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setAdding(false)}>取消</button>
            <button className="btn primary" onClick={saveNew}>保存</button>
          </div>
        </Modal>
      )}
      {editing && (
        <Modal title="编辑条目（生成新 Revision，历史保留）" onClose={() => setEditing(null)}>
          <label className="field"><span>标题</span>
            <input type="text" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus /></label>
          <label className="field"><span>内容</span><textarea value={content} onChange={(e) => setContent(e.target.value)} /></label>
          {editRev?.source_ref && <p className="muted small">当前版本来源：{editRev.source_ref}</p>}
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setEditing(null)}>取消</button>
            <button className="btn primary" onClick={saveEdit}>保存为新 Revision</button>
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

      {launching && <LauncherModal workstreamIds={[workstreamId]} mode="new" onClose={() => setLaunching(false)} />}
    </div>
  );
}
