import React, { useEffect, useState } from "react";
import { api } from "../../api";
import { timeAgo } from "../../components/common";
import { AgentBadge } from "../../App";
import type { Route } from "../../App";
import { AGENT_LABELS, type Agent, type Session } from "../../types";
import LauncherModal from "../launcher/LauncherModal";

export default function SessionsView({ navigate }: { navigate: (r: Route) => void }) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [filter, setFilter] = useState<"all" | Agent>("all");
  const [query, setQuery] = useState("");
  const [resumeTarget, setResumeTarget] = useState<string | null>(null);

  useEffect(() => {
    api.listSessions(undefined, filter === "all" ? undefined : filter).then(setSessions).catch(console.error);
  }, [filter]);

  const shown = sessions.filter((s) =>
    !query || (s.title ?? "").toLowerCase().includes(query.toLowerCase())
      || (s.cwd ?? "").toLowerCase().includes(query.toLowerCase())
  );

  return (
    <div className="main">
      <h1>All Sessions</h1>
      <p className="page-sub">来自 Codex、Claude Code 和 Pi 的本地会话。Session 是执行容器，长期主题由 Workstream 承载。</p>

      <div className="toolbar">
        <select style={{ width: 170 }} value={filter} onChange={(e) => setFilter(e.target.value as any)}>
          <option value="all">全部 Agent</option>
          {Object.entries(AGENT_LABELS).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
        </select>
        <input type="text" style={{ width: 300 }} placeholder="过滤标题或工作目录…" value={query} onChange={(e) => setQuery(e.target.value)} />
      </div>

      {shown.length === 0 && (
        <div className="empty">
          没有匹配的 Session。
          <div className="invite">
            <button className="btn small" onClick={() => api.syncAll().then(() => api.listSessions(undefined, filter === "all" ? undefined : filter).then(setSessions))}>
              同步 Agent Sessions
            </button>
          </div>
        </div>
      )}
      <div className="fullbleed" style={{ maxWidth: "none" }}>
        {shown.map((s) => (
          <div className="list-row" key={s.id} onClick={() => navigate({ view: "session", sessionId: s.id })}>
            <div className="grow">
              <div className="title">{s.title ?? s.agent_session_id}</div>
              <div className="meta mono">{s.cwd ?? "无工作目录"}</div>
            </div>
            <div className="side" onClick={(e) => e.stopPropagation()}>
              <span>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
              <AgentBadge agent={s.agent} />
              <button className="btn small" onClick={() => setResumeTarget(s.id)}>Resume</button>
            </div>
          </div>
        ))}
      </div>

      {resumeTarget && <LauncherModal sessionId={resumeTarget} mode="resume" onClose={() => setResumeTarget(null)} />}
    </div>
  );
}
