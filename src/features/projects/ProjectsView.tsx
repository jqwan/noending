import React, { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import { Modal, timeAgo } from "../../components/common";
import { EVT_SYNCED, onEvent, type Route } from "../../app/routes";
import type { Project, Session, Workstream } from "../../types";

/**
 * Projects = Organize long-term topics（整体设计方案 §51/§52）：
 * 弱管理属性的单列列表；Project 是可选层，不要求 Workstream 归属。
 */
export default function ProjectsView({ navigate }: { navigate: (r: Route) => void }) {
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [workstreams, setWorkstreams] = useState<Workstream[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");

  const refresh = () => {
    api.listProjects().then(setProjects).catch(console.error);
    api.listWorkstreams().then(setWorkstreams).catch(console.error);
    api.listAllSessions().then(setSessions).catch(console.error);
  };
  useEffect(refresh, []);
  useEffect(() => onEvent(EVT_SYNCED, refresh), []);

  const stats = useMemo(() => {
    const m = new Map<string, { ws: number; sessions: number; last: string | null }>();
    for (const p of projects ?? []) {
      const ws = workstreams.filter((w) => w.project_id === p.id && w.visibility === "normal");
      const ss = sessions.filter((s) => s.project_id === p.id);
      const last = [...ws.map((w) => w.updated_at), ...ss.map((s) => s.last_activity_at ?? s.started_at ?? "")]
        .filter(Boolean)
        .sort()
        .pop() ?? null;
      m.set(p.id, { ws: ws.length, sessions: ss.length, last });
    }
    return m;
  }, [projects, workstreams, sessions]);

  const create = async () => {
    if (!name.trim()) return;
    await api.createProject(name, desc);
    setCreating(false); setName(""); setDesc("");
    refresh();
  };

  return (
    <div className="main narrow">
      <PageHeader
        title="Projects"
        sub="Project 是可选的长期整理层；Workstream 不要求归属任何 Project。"
        actions={
          <button className="btn primary" onClick={() => setCreating(true)}>+ New Project</button>
        }
      />

      {projects === null && <div className="muted">加载中…</div>}
      {projects !== null && projects.length === 0 && (
        <EmptyState
          title="No projects yet."
          hint="Projects are optional. 当你想把相关的工作归到同一个长期主题时再创建即可。"
          actions={<button className="btn small" onClick={() => setCreating(true)}>+ New Project</button>}
        />
      )}

      <div className="ws-stack">
        {projects?.map((p) => {
          const st = stats.get(p.id);
          return (
            <div key={p.id} className="ws-card compact" onClick={() => navigate({ view: "project", projectId: p.id })}>
              <header className="ws-card-head">
                <h3 className="ws-card-title">{p.name}</h3>
              </header>
              {p.description && <p className="ws-card-body">{p.description}</p>}
              <footer className="ws-card-meta">
                <span>{st?.ws ?? 0} workstreams · {st?.sessions ?? 0} sessions</span>
                <span>Last active {st?.last ? timeAgo(st.last) : "—"}</span>
              </footer>
            </div>
          );
        })}
      </div>

      {creating && (
        <Modal title="新建 Project" onClose={() => setCreating(false)}>
          <label className="field"><span>名称</span>
            <input type="text" value={name} onChange={(e) => setName(e.target.value)} autoFocus
              placeholder="例如：Agent Workspace / Japan Trip" /></label>
          <label className="field"><span>描述（可选）</span>
            <textarea value={desc} onChange={(e) => setDesc(e.target.value)} /></label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setCreating(false)}>取消</button>
            <button className="btn primary" onClick={create}>创建</button>
          </div>
        </Modal>
      )}
    </div>
  );
}
