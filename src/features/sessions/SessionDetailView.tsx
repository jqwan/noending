import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import AgentIcon from "../../components/AgentIcon";
import { timeAgo, useRefreshSignal } from "../../components/common";
import SessionMessage, { type SessionMessageData } from "./SessionMessage";
import ResumeSessionModal from "./ResumeSessionModal";
import { Modal } from "../../components/common";
import { AGENT_LABELS, type SessionDetail, type Workstream } from "../../types";
import type { Route } from "../../app/routes";

/**
 * Session Detail = 一次具体 Agent 执行的记录（整体设计方案 §42-§45）。
 * Header 回答：哪个 Agent、哪个 Workstream、什么时候；正文是标准化消息流。
 */
export default function SessionDetailView({ sessionId, navigate }: {
  sessionId: string;
  navigate: (r: Route) => void;
}) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [syncMsg, setSyncMsg] = useState("");
  const [bindingOpen, setBindingOpen] = useState(false);
  const [resumeOpen, setResumeOpen] = useState(false);

  const refresh = useCallback(() => {
    api.getSessionDetail(sessionId).then(setDetail).catch(console.error);
  }, [sessionId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  if (!detail) return <div className="main narrow">加载中…</div>;
  const { session, events, bindings } = detail;

  /** 刷新 = 只摄入。Context 提取只在智能开启时发生（方案 v0.1 §11.6）。 */
  const doSync = async () => {
    setSyncMsg("正在刷新…");
    try {
      const r = await api.syncSession(sessionId);
      setSyncMsg(
        r.ingested > 0
          ? `摄入了 ${r.ingested} 条新消息`
          : r.applied > 0
            ? `提取了 ${r.applied} 个 Context 变更`
            : "没有新内容",
      );
      refresh();
    } catch (e) {
      console.error(e);
      setSyncMsg("刷新失败");
    }
    setTimeout(() => setSyncMsg(""), 4000);
  };

  const messages: SessionMessageData[] = events.map((e) => ({
    sequence: e.sequence,
    kind: e.kind,
    text: e.text,
    ts: e.ts,
    who: AGENT_LABELS[session.agent],
  }));

  return (
    <div className="main narrow">
      <PageHeader
        back="Sessions"
        onBack={() => navigate({ view: "sessions" })}
        title={session.title ?? session.agent_session_id}
        actions={
          <>
            {syncMsg && <span className="badge accent" style={{ padding: "4px 10px" }}>{syncMsg}</span>}
            <button className="btn ghost" onClick={doSync}>刷新</button>
            <button className="btn primary" onClick={() => setResumeOpen(true)}>
              Resume
            </button>
          </>
        }
      >
        <div className="ws-detail-head-meta">
          <span className="row" style={{ gap: 6 }}>
            <AgentIcon agent={session.agent} />
            {AGENT_LABELS[session.agent]}
          </span>
          {bindings.length > 0 && (
            <>
              <span className="dot-sep" />
              <span>{bindings.map(([, title]) => title ?? "").filter(Boolean).join(" · ")}</span>
            </>
          )}
          <span className="dot-sep" />
          <span>Started {session.started_at ? timeAgo(session.started_at) : "—"}</span>
          <span className="dot-sep" />
          <span>Last active {timeAgo(session.last_activity_at ?? session.started_at)}</span>
        </div>
      </PageHeader>

      <div className="row between" style={{ marginTop: 26, marginBottom: 6 }}>
        <div className="section-label" style={{ margin: 0 }}>Workstreams</div>
        <button className="btn small ghost" onClick={() => setBindingOpen(true)}>Edit</button>
      </div>
      {bindings.length === 0 && (
        <div className="l1-none">
          尚未关联 Workstream。可以在这里关联，也可以直接继续这个 Session。
        </div>
      )}
      {bindings.map(([b, title]) => (
        <div className="rail-row" key={b.workstream_id}
          onClick={() => navigate({ view: "workstream", workstreamId: b.workstream_id })}>
          <div className="rail-main">
            <div className="rail-title">{title ?? b.workstream_id}</div>
          </div>
          <span className={`badge ${b.role === "primary" ? "dark" : ""}`}>{b.role}</span>
        </div>
      ))}

      <div className="section-label" style={{ marginTop: 34 }}>消息</div>
      <p className="muted small" style={{ margin: "0 0 6px" }}>标准化视图，只读。原始数据始终保留在 Agent 自己的目录中。</p>
      {messages.map((m) => <SessionMessage key={m.sequence} msg={m} />)}
      {messages.length === 0 && (
        <div className="empty">
          还没有摄入消息。
          <div className="invite"><button className="btn small" onClick={doSync}>刷新这个 Session</button></div>
        </div>
      )}

      {bindingOpen && (
        <BindingModal
          sessionId={sessionId}
          bindings={bindings.map(([b]) => b)}
          onClose={() => setBindingOpen(false)}
          onChanged={refresh}
        />
      )}
      {resumeOpen && (
        <ResumeSessionModal
          sessionId={sessionId}
          onClose={() => setResumeOpen(false)}
        />
      )}
    </div>
  );
}

/** Binding 编辑（实施方案 §36）：Primary / Related / Remove / Add，Modal 即可。 */
function BindingModal({ sessionId, bindings, onClose, onChanged }: {
  sessionId: string;
  bindings: { workstream_id: string; role: string }[];
  onClose: () => void;
  onChanged: () => void;
}) {
  const [workstreams, setWorkstreams] = useState<Workstream[]>([]);
  const [rows, setRows] = useState(bindings.map((b) => ({ ...b })));
  const [addId, setAddId] = useState("none");
  const [addRole, setAddRole] = useState("related");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    api.listWorkstreams().then(setWorkstreams).catch(console.error);
  }, []);

  const apply = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      // 后端在单个事务里做 diff：未改动的 Binding 原 row 保留
      // （provenance / created_at / cursor 不动），只有新增的才是 user_assigned。
      await api.replaceSessionBindings(
        sessionId,
        rows.map((r) => ({ workstream_id: r.workstream_id, role: r.role })),
      );
      onChanged();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const bound = new Set(rows.map((r) => r.workstream_id));

  return (
    <Modal title="Edit Workstreams" onClose={onClose}>
      {rows.length === 0 && <div className="muted small" style={{ marginBottom: 10 }}>尚未关联任何 Workstream。</div>}
      {rows.map((r) => (
        <div className="row-line" key={r.workstream_id}>
          <div className="settings-row-label">
            {workstreams.find((w) => w.id === r.workstream_id)?.title ?? r.workstream_id}
          </div>
          <div className="row">
            <select style={{ width: 110 }} value={r.role}
              onChange={(e) => setRows((rs) => rs.map((x) => x.workstream_id === r.workstream_id ? { ...x, role: e.target.value } : x))}>
              <option value="primary">Primary</option>
              <option value="related">Related</option>
            </select>
            <button className="btn small ghost"
              onClick={() => setRows((rs) => rs.filter((x) => x.workstream_id !== r.workstream_id))}>
              Remove
            </button>
          </div>
        </div>
      ))}

      <div className="row-line">
        <div className="settings-row-label">Add Workstream</div>
        <div className="row">
          <select style={{ width: 200 }} value={addId} onChange={(e) => setAddId(e.target.value)}>
            <option value="none">选择…</option>
            {workstreams.filter((w) => !bound.has(w.id) && w.visibility === "normal").map((w) => (
              <option key={w.id} value={w.id}>{w.title}</option>
            ))}
          </select>
          <select style={{ width: 110 }} value={addRole} onChange={(e) => setAddRole(e.target.value)}>
            <option value="related">Related</option>
            <option value="primary">Primary</option>
          </select>
          <button className="btn small" disabled={addId === "none"}
            onClick={() => { setRows((rs) => [...rs, { workstream_id: addId, role: addRole }]); setAddId("none"); }}>
            添加
          </button>
        </div>
      </div>

      {error && <div className="badge warn" style={{ marginTop: 8 }}>{error}</div>}
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 14 }}>
        <button className="btn" onClick={onClose}>取消</button>
        <button className="btn primary" disabled={busy} onClick={apply}>{busy ? "保存中…" : "保存"}</button>
      </div>
    </Modal>
  );
}
