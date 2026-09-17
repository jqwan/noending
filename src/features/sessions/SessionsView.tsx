import React, { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import { useRefreshSignal } from "../../components/common";
import SessionTable from "./SessionTable";
import NewSessionModal from "./NewSessionModal";
import ResumeSessionModal from "./ResumeSessionModal";
import { AGENT_LABELS, type Agent, type Project, type Session, type SessionBindingRow } from "../../types";
import type { Route, ViewAction } from "../../app/routes";

type AssignedFilter = "all" | "assigned" | "unassigned";

/**
 * Sessions = Execute 记录页（整体设计方案 §38-§41）：快速找到具体 Session，
 * 不承担 Workstream 浏览。搜索与筛选在前端做，规模大了再转后端。
 */
export default function SessionsView({ navigate, action, actionSeq }: {
  navigate: (r: Route) => void;
  action?: ViewAction;
  actionSeq: number;
}) {
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [bindings, setBindings] = useState<Map<string, SessionBindingRow[]>>(new Map());
  const [query, setQuery] = useState("");
  const [agent, setAgent] = useState<"all" | Agent>("all");
  const [projectId, setProjectId] = useState("all");
  const [wsFilter, setWsFilter] = useState("all");
  const [assigned, setAssigned] = useState<AssignedFilter>("all");
  const [creating, setCreating] = useState(false);
  const [resumeModalSessionId, setResumeModalSessionId] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.listAllSessions().then(setSessions).catch(console.error);
    api.listProjects().then(setProjects).catch(console.error);
    api.listSessionBindings().then((rows) => {
      const m = new Map<string, SessionBindingRow[]>();
      for (const r of rows) {
        const list = m.get(r.session_id) ?? [];
        list.push(r);
        m.set(r.session_id, list);
      }
      setBindings(m);
    }).catch(console.error);
  }, []);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);
  // 页面动作随 Route 到达（palette → New Session）：
  // actionSeq 让「已在 Sessions 页」的重复命令同样触发。
  useEffect(() => {
    if (action === "new") setCreating(true);
  }, [action, actionSeq]);

  const wsOptions = useMemo(() => {
    const seen = new Map<string, string>();
    for (const rows of bindings.values()) {
      for (const r of rows) seen.set(r.workstream_id, r.workstream_title);
    }
    return [...seen.entries()].sort((a, b) => a[1].localeCompare(b[1], "zh-Hans"));
  }, [bindings]);

  const shown = useMemo(() => {
    if (!sessions) return null;
    const q = query.trim().toLowerCase();
    return sessions
      .filter((s) => (agent === "all" ? true : s.agent === agent))
      .filter((s) => (projectId === "all" ? true : (s.project_id ?? "none") === projectId))
      .filter((s) => {
        if (wsFilter === "all") return true;
        const rows = bindings.get(s.id) ?? [];
        return wsFilter === "unassigned"
          ? rows.length === 0
          : rows.some((r) => r.workstream_id === wsFilter);
      })
      .filter((s) => {
        if (assigned === "all") return true;
        const has = (bindings.get(s.id) ?? []).length > 0;
        return assigned === "assigned" ? has : !has;
      })
      .filter((s) =>
        q === ""
          ? true
          : [s.title, s.cwd, (bindings.get(s.id) ?? []).map((b) => b.workstream_title).join(" ")]
              .filter(Boolean)
              .some((t) => (t as string).toLowerCase().includes(q)),
      )
      .sort((a, b) =>
        (b.last_activity_at ?? b.started_at ?? "").localeCompare(
          a.last_activity_at ?? a.started_at ?? "",
        ),
      );
  }, [sessions, bindings, query, agent, projectId, wsFilter, assigned]);

  const resume = (sessionId: string) => {
    setResumeModalSessionId(sessionId);
  };

  const clearFilters = () => {
    setQuery("");
    setAgent("all");
    setProjectId("all");
    setWsFilter("all");
    setAssigned("all");
  };

  return (
    <div className="main">
      <PageHeader
        title="Sessions"
        sub="来自 Codex、Claude Code 和 Pi 的本地执行记录。长期主题由 Workstream 承载。"
        actions={
          <button className="btn primary" onClick={() => setCreating(true)}>+ New Session</button>
        }
      />

      <input
        type="text"
        className="ws-search"
        placeholder="Search sessions...（标题、目录、Workstream）"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />

      <div className="toolbar ws-controls">
        <label className="ws-control">
          <span className="muted small">Agent</span>
          <select value={agent} onChange={(e) => setAgent(e.target.value as any)}>
            <option value="all">All Agents</option>
            {Object.entries(AGENT_LABELS).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
          </select>
        </label>
        <label className="ws-control">
          <span className="muted small">Workstream</span>
          <select value={wsFilter} onChange={(e) => setWsFilter(e.target.value)}>
            <option value="all">All Workstreams</option>
            {wsOptions.map(([id, title]) => <option key={id} value={id}>{title}</option>)}
            <option value="unassigned">— Unassigned</option>
          </select>
        </label>
        <label className="ws-control">
          <span className="muted small">Project</span>
          <select value={projectId} onChange={(e) => setProjectId(e.target.value)}>
            <option value="all">All Projects</option>
            {projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
            <option value="none">— 无归属</option>
          </select>
        </label>
        <label className="ws-control">
          <span className="muted small">Assigned</span>
          <select value={assigned} onChange={(e) => setAssigned(e.target.value as AssignedFilter)}>
            <option value="all">All</option>
            <option value="assigned">Assigned</option>
            <option value="unassigned">Unassigned</option>
          </select>
        </label>
        {shown && <span className="muted small">{shown.length} 个</span>}
      </div>

      {shown === null && <div className="muted">加载中…</div>}
      {shown !== null && shown.length === 0 && (sessions?.length ?? 0) === 0 && (
        <EmptyState
          title="No sessions have been imported yet."
          hint="启用会话数据源后，本地 Agent 会话会自动出现在这里。"
          actions={
            <button className="btn small" onClick={() => navigate({ view: "settings", section: "sources" })}>
              Configure Session Sources
            </button>
          }
        />
      )}
      {shown !== null && shown.length === 0 && (sessions?.length ?? 0) > 0 && (
        <EmptyState
          title="没有匹配这些筛选条件的 Session。"
          actions={<button className="btn small" onClick={clearFilters}>Clear filters</button>}
        />
      )}
      {shown !== null && shown.length > 0 && (
        <SessionTable
          sessions={shown}
          bindings={bindings}
          onOpen={(id) => navigate({ view: "session", sessionId: id })}
          onResume={resume}
        />
      )}

      {creating && <NewSessionModal onClose={() => setCreating(false)} />}
      {resumeModalSessionId && (
        <ResumeSessionModal
          sessionId={resumeModalSessionId}
          onClose={() => setResumeModalSessionId(null)}
        />
      )}
    </div>
  );
}
