import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { timeAgo, useRefreshSignal } from "../../components/common";
import { AgentBadge } from "../../App";
import type { Route } from "../../App";
import type { SessionDetail } from "../../types";
import LauncherModal from "../launcher/LauncherModal";

export default function SessionDetailView({ sessionId, navigate }: {
  sessionId: string;
  navigate: (r: Route) => void;
}) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [resuming, setResuming] = useState(false);
  const [syncMsg, setSyncMsg] = useState("");

  const refresh = useCallback(() => {
    api.getSessionDetail(sessionId).then(setDetail).catch(console.error);
  }, [sessionId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  if (!detail) return <div className="main">加载中…</div>;
  const { session, events, bindings, cursor } = detail;

  const doSync = async () => {
    setSyncMsg("同步中…");
    const r = await api.syncSession(sessionId);
    setSyncMsg(r.applied > 0 ? `提取了 ${r.applied} 个 Context 变更` : "没有新的有效上下文");
    refresh();
    setTimeout(() => setSyncMsg(""), 4000);
  };

  return (
    <div className="main narrow">
      <div className="page-head">
        <div>
          <div className="row" style={{ gap: 10, marginBottom: 2 }}>
            <AgentBadge agent={session.agent} />
            <span className="muted small mono">cursor {cursor}</span>
          </div>
          <h1>{session.title ?? session.agent_session_id}</h1>
          <p className="page-sub mono">{session.cwd ?? "无工作目录"}</p>
        </div>
        <div className="actions">
          {syncMsg && <span className="badge accent" style={{ padding: "4px 10px" }}>{syncMsg}</span>}
          <button className="btn" onClick={doSync}>同步</button>
          <button className="btn accent" onClick={() => setResuming(true)}>Resume</button>
        </div>
      </div>

      <h2>Workstream</h2>
      {bindings.length === 0 && (
        <div className="empty">
          尚未关联 Workstream。同步之后，系统会尝试把这段会话自动归类；也可以不带上下文直接 Resume。
        </div>
      )}
      {bindings.map(([b, title]) => (
        <div className="list-row" key={b.workstream_id} style={{ cursor: "default" }}>
          <div className="grow">
            <div className="title">{title ?? b.workstream_id}</div>
          </div>
          <div className="side">
            {b.role === "primary" && <span className="badge dark">primary</span>}
            <span>最近使用 {timeAgo(b.last_used_at)}</span>
          </div>
        </div>
      ))}

      <h2>消息</h2>
      <p className="muted small">标准化视图，只读。原始数据始终保留在 Agent 自己的目录中。</p>
      {events.map((e) => {
        const isMsg = e.kind === "user_message" || e.kind === "assistant_message";
        const text = e.text ?? "";
        const long = text.length > 420;
        const open = expanded.has(e.sequence);
        return (
          <div className={`event ${!isMsg ? "tool" : ""}`} key={e.sequence}>
            <div className="head">
              <span className="who">
                {e.kind === "user_message" ? "User" : e.kind === "assistant_message" ? "Agent" : e.kind}
              </span>
              <span className="when mono">#{e.sequence}{e.ts ? `  ${new Date(e.ts).toLocaleString()}` : ""}</span>
            </div>
            <div className="body">{long && !open ? text.slice(0, 420) + "…" : text}</div>
            {long && (
              <button className="link" onClick={() => setExpanded((s) => { const n = new Set(s); n.has(e.sequence) ? n.delete(e.sequence) : n.add(e.sequence); return n; })}>
                {open ? "收起" : "展开全文"}
              </button>
            )}
          </div>
        );
      })}
      {events.length === 0 && (
        <div className="empty">还没有摄入消息。
          <div className="invite"><button className="btn small" onClick={doSync}>同步这个 Session</button></div>
        </div>
      )}

      {resuming && <LauncherModal sessionId={sessionId} mode="resume" onClose={() => setResuming(false)} />}
    </div>
  );
}
