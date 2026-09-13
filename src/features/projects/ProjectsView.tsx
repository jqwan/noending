import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { timeAgo, useRefreshSignal } from "../../components/common";
import SidebarLogo from "../../components/SidebarLogo";
import type { Route } from "../../App";
import type { Project } from "../../types";
import LauncherModal from "../launcher/LauncherModal";

export default function ProjectsView({ navigate, refreshSidebar }: {
  navigate: (r: Route) => void;
  refreshSidebar: () => void;
}) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [stats, setStats] = useState<Record<string, number>>({});
  const [agents, setAgents] = useState<Record<string, { name: string; detected: boolean; version: string | null }>>({});
  const [creating, setCreating] = useState(false);
  const [creatingWs, setCreatingWs] = useState(false);
  const [launching, setLaunching] = useState(false);
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [wsTitle, setWsTitle] = useState("");
  const [wsDesc, setWsDesc] = useState("");
  const [wsProject, setWsProject] = useState<string>("none");

  const refresh = useCallback(() => {
    api.listProjects().then(setProjects).catch(console.error);
    api.getStats().then(setStats).catch(console.error);
    api.getAgentStatus().then(setAgents).catch(console.error);
  }, []);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  const create = async () => {
    if (!name.trim()) return;
    await api.createProject(name, desc);
    setCreating(false); setName(""); setDesc("");
    refresh(); refreshSidebar();
  };

  const createWs = async () => {
    if (!wsTitle.trim()) return;
    const w = await api.createWorkstream(wsProject === "none" ? null : wsProject, wsTitle, wsDesc);
    setCreatingWs(false); setWsTitle(""); setWsDesc(""); setWsProject("none");
    refreshSidebar();
    navigate({ view: "workstream", workstreamId: w.id });
  };

  return (
    <div className="main narrow">
      {/* the one orchestrated moment: the mark draws itself (600–900ms, brand §15) */}
      <div className="hero">
        <SidebarLogo size={64} animated />
        <div className="tagline">对话会结束，上下文不会。</div>
      </div>

      {/* Primary actions: Workstream 与 Session 可以被创建；Session 可以被恢复 */}
      <div className="actions-row" style={{ marginBottom: 8 }}>
        <button className="btn primary" onClick={() => setCreatingWs(true)}>New Workstream</button>
        <button className="btn accent" onClick={() => setLaunching(true)}>New Session</button>
      </div>
      <p className="page-sub" style={{ textAlign: "center" }}>
        Workstream 承载持续演进的上下文；Session 是 Agent 的执行容器，之后由 Sync 自动关联。
      </p>

      <div className="page-head" style={{ marginTop: 26 }}>
        <h1 style={{ margin: 0 }}>Projects</h1>
        <div className="actions">
          <button className="btn small ghost" onClick={() => setCreating(true)}>新建 Project</button>
        </div>
      </div>

      {projects.length === 0 ? (
        <div className="empty">
          还没有 Project。Workstream 可以独立存在，Project 用来聚合长期主题。
          <div className="invite">
            <button className="btn small" onClick={() => setCreatingWs(true)}>先建一个 Workstream</button>
            <button className="btn small ghost" onClick={() => setCreating(true)}>新建 Project</button>
          </div>
        </div>
      ) : (
        <div>
          {projects.map((p) => (
            <div key={p.id} className="list-row" onClick={() => navigate({ view: "project", projectId: p.id })}>
              <div className="grow">
                <div className="title">{p.name}</div>
                {p.description && <div className="meta">{p.description}</div>}
              </div>
              <div className="side">{timeAgo(p.updated_at)}</div>
            </div>
          ))}
        </div>
      )}

      <h2>Agent</h2>
      <div>
        {Object.entries(agents).map(([k, a]) => (
          <div key={k} className="list-row" style={{ cursor: "default" }}>
            <div className="grow">
              <div className="title">{a.name}</div>
              {a.version && <div className="meta mono">{a.version}</div>}
            </div>
            <div className="side">
              <span className={`badge ${a.detected ? "accent" : ""}`}>{a.detected ? "已检测" : "未安装"}</span>
            </div>
          </div>
        ))}
      </div>

      <h2>数据</h2>
      <div className="grid2">
        <div>
          <div className="muted small">Sessions</div>
          <div style={{ fontSize: 24, fontWeight: 600 }}>{stats.sessions ?? "—"}</div>
        </div>
        <div>
          <div className="muted small">Context Items</div>
          <div style={{ fontSize: 24, fontWeight: 600 }}>{stats.context_items ?? "—"}</div>
        </div>
      </div>

      {creating && (
        <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && setCreating(false)}>
          <div className="modal">
            <h2>新建 Project</h2>
            <label className="field"><span>名称</span>
              <input type="text" value={name} onChange={(e) => setName(e.target.value)} autoFocus
                placeholder="例如：Agent Workspace / Japan Trip" /></label>
            <label className="field"><span>描述（可选）</span>
              <textarea value={desc} onChange={(e) => setDesc(e.target.value)} /></label>
            <div className="row" style={{ justifyContent: "flex-end" }}>
              <button className="btn" onClick={() => setCreating(false)}>取消</button>
              <button className="btn primary" onClick={create}>创建</button>
            </div>
          </div>
        </div>
      )}

      {creatingWs && (
        <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && setCreatingWs(false)}>
          <div className="modal">
            <h2>New Workstream</h2>
            <p className="muted small">开始一件需要长期持续的事情。</p>
            <label className="field"><span>标题</span>
              <input type="text" value={wsTitle} onChange={(e) => setWsTitle(e.target.value)} autoFocus
                placeholder="例如：Context Sync / 行程设计 / 预算" /></label>
            <label className="field"><span>描述（可选）</span><textarea value={wsDesc} onChange={(e) => setWsDesc(e.target.value)} /></label>
            <label className="field"><span>归属 Project（可选）</span>
              <select value={wsProject} onChange={(e) => setWsProject(e.target.value)}>
                <option value="none">独立 Workstream（暂不归属）</option>
                {projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
              </select></label>
            <div className="row" style={{ justifyContent: "flex-end" }}>
              <button className="btn" onClick={() => setCreatingWs(false)}>取消</button>
              <button className="btn primary" onClick={createWs}>创建并打开</button>
            </div>
          </div>
        </div>
      )}

      {launching && <LauncherModal mode="new" onClose={() => setLaunching(false)} />}
    </div>
  );
}
