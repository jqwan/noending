import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { Modal, timeAgo, useRefreshSignal } from "../../components/common";
import type { Route } from "../../App";
import { AgentBadge } from "../../App";
import type { Project, ProjectResource, Session, Workstream } from "../../types";
import LauncherModal from "../launcher/LauncherModal";

export default function ProjectDetail({ projectId, navigate, refreshSidebar }: {
  projectId: string;
  navigate: (r: Route) => void;
  refreshSidebar: () => void;
}) {
  const [project, setProject] = useState<Project | null>(null);
  const [workstreams, setWorkstreams] = useState<Workstream[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [resources, setResources] = useState<ProjectResource[]>([]);
  const [creatingWs, setCreatingWs] = useState(false);
  const [title, setTitle] = useState("");
  const [desc, setDesc] = useState("");
  const [addingRes, setAddingRes] = useState(false);
  const [resKind, setResKind] = useState("url");
  const [resUri, setResUri] = useState("");
  const [launchWs, setLaunchWs] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.listProjects().then((ps) => setProject(ps.find((p) => p.id === projectId) ?? null)).catch(console.error);
    api.listWorkstreams(projectId).then((ws) => setWorkstreams(ws.filter((w) => w.visibility === "normal"))).catch(console.error);
    api.listSessions(projectId).then(setSessions).catch(console.error);
    api.listResources(projectId).then(setResources).catch(console.error);
  }, [projectId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  if (!project) return <div className="main">加载中…</div>;

  const createWs = async () => {
    if (!title.trim()) return;
    const w = await api.createWorkstream(projectId, title, desc);
    setCreatingWs(false); setTitle(""); setDesc("");
    refreshSidebar(); refresh();
    navigate({ view: "workstream", workstreamId: w.id });
  };

  return (
    <div className="main">
      <div className="page-head">
        <div>
          <h1>{project.name}</h1>
          <p className="page-sub">{project.description || "Project 提供长期主题边界。"}</p>
        </div>
        <div className="actions">
          <button className="btn primary" onClick={() => setCreatingWs(true)}>New Workstream</button>
        </div>
      </div>

      <h2>Workstreams</h2>
      {workstreams.length === 0 && (
        <div className="empty">
          这个 Project 下还没有 Workstream。
          <div className="invite"><button className="btn small" onClick={() => setCreatingWs(true)}>New Workstream</button></div>
        </div>
      )}
      <div>
        {workstreams.map((w) => (
          <div key={w.id} className="list-row" onClick={() => navigate({ view: "workstream", workstreamId: w.id })}>
            <div className="grow">
              <div className="title">{w.title}</div>
              {w.description && <div className="meta">{w.description}</div>}
            </div>
            <div className="side" onClick={(e) => e.stopPropagation()}>
              {w.lifecycle !== "open" && <span className="badge">{w.lifecycle}</span>}
              <span>{timeAgo(w.updated_at)}</span>
              <button className="btn small" onClick={() => setLaunchWs(w.id)}>New Session</button>
            </div>
          </div>
        ))}
      </div>

      <h2>Recent Sessions</h2>
      {sessions.length === 0 && (
        <div className="empty">
          这个 Project 下还没有 Session。先在侧栏同步 Agent Sessions，再在 Session 详情里关联到本 Project。
        </div>
      )}
      <div>
        {sessions.slice(0, 8).map((s) => (
          <div key={s.id} className="list-row" onClick={() => navigate({ view: "session", sessionId: s.id })}>
            <div className="grow">
              <div className="title">{s.title ?? s.agent_session_id}</div>
              <div className="meta mono">{s.cwd ?? "无工作目录"}</div>
            </div>
            <div className="side">
              <AgentBadge agent={s.agent} />
              <span>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
            </div>
          </div>
        ))}
      </div>

      <h2>Resources <span className="muted small">（可选；Project 不依赖任何路径）</span></h2>
      {resources.map((r) => (
        <div className="list-row" key={r.id} style={{ cursor: "default" }}>
          <div className="grow">
            <div className="title"><span className="badge" style={{ marginRight: 8 }}>{r.kind}</span><span className="mono small">{r.uri}</span></div>
          </div>
          <div className="side">
            <button className="link" onClick={async () => { await api.removeResource(r.id); refresh(); }}>移除</button>
          </div>
        </div>
      ))}
      <div style={{ marginTop: 8 }}>
        <button className="btn small ghost" onClick={() => setAddingRes(true)}>添加 Resource</button>
      </div>

      {creatingWs && (
        <Modal title="New Workstream" onClose={() => setCreatingWs(false)}>
          <label className="field"><span>标题</span>
            <input type="text" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus
              placeholder="例如：Context Sync / 行程设计 / 预算" /></label>
          <label className="field"><span>描述（可选）</span><textarea value={desc} onChange={(e) => setDesc(e.target.value)} /></label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setCreatingWs(false)}>取消</button>
            <button className="btn primary" onClick={createWs}>创建并打开</button>
          </div>
        </Modal>
      )}

      {addingRes && (
        <Modal title="添加 Resource" onClose={() => setAddingRes(false)}>
          <label className="field"><span>类型</span>
            <select value={resKind} onChange={(e) => setResKind(e.target.value)}>
              {["repository", "workspace", "file", "document", "url", "artifact", "external"].map((k) => (
                <option key={k} value={k}>{k}</option>
              ))}
            </select></label>
          <label className="field"><span>URI / 路径</span>
            <input type="text" value={resUri} onChange={(e) => setResUri(e.target.value)} autoFocus /></label>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setAddingRes(false)}>取消</button>
            <button className="btn primary" onClick={async () => {
              await api.addResource(projectId, resKind, resUri);
              setAddingRes(false); setResUri(""); refresh();
            }}>添加</button>
          </div>
        </Modal>
      )}

      {launchWs && <LauncherModal workstreamIds={[launchWs]} mode="new" onClose={() => setLaunchWs(null)} />}
    </div>
  );
}
