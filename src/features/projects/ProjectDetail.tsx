import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import { Modal, timeAgo, useRefreshSignal } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import NewWorkstreamModal from "../workstreams/NewWorkstreamModal";
import type { Route } from "../../app/routes";
import { AGENT_LABELS, type Project, type ProjectResource, type Session, type Workstream } from "../../types";

/**
 * Project Detail（整体设计方案 §53/§54）：Workstreams 是主要 section，
 * Resources 用轻量行；Project 视觉权重保持次于 Workstream。
 */
export default function ProjectDetail({ projectId, navigate }: {
  projectId: string;
  navigate: (r: Route) => void;
}) {
  const [project, setProject] = useState<Project | null>(null);
  const [workstreams, setWorkstreams] = useState<Workstream[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [resources, setResources] = useState<ProjectResource[]>([]);
  const [creatingWs, setCreatingWs] = useState(false);
  const [addingRes, setAddingRes] = useState(false);
  const [resKind, setResKind] = useState("url");
  const [resUri, setResUri] = useState("");

  const refresh = useCallback(() => {
    api.listProjects().then((ps) => setProject(ps.find((p) => p.id === projectId) ?? null)).catch(console.error);
    api.listWorkstreamCards().then((cards) =>
      setWorkstreams(cards.filter((w) => w.project_id === projectId && w.visibility === "normal")),
    ).catch(console.error);
    api.listSessions(projectId).then(setSessions).catch(console.error);
    api.listResources(projectId).then(setResources).catch(console.error);
  }, [projectId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  if (!project) return <div className="main narrow">加载中…</div>;

  return (
    <div className="main narrow">
      <PageHeader
        back="Projects"
        onBack={() => navigate({ view: "projects" })}
        title={project.name}
        sub={project.description || undefined}
        actions={
          <>
            <button className="btn ghost"
              onClick={() => navigate({ view: "assistant", scope: { type: "project", id: project.id } })}>
              Ask Assistant
            </button>
            <button className="btn" onClick={() => setCreatingWs(true)}>New Workstream</button>
          </>
        }
      />

      <div className="section-label" style={{ marginTop: 26 }}>Workstreams</div>
      {workstreams.length === 0 && (
        <div className="l1-none">这个 Project 下还没有 Workstream。</div>
      )}
      {workstreams.map((w) => (
        <div key={w.id} className="list-row" onClick={() => navigate({ view: "workstream", workstreamId: w.id })}>
          <div className="grow">
            <div className="title">{w.title}</div>
            {(w as any).current_state && <div className="meta">{(w as any).current_state}</div>}
          </div>
          <div className="side">
            <span>{timeAgo((w as any).last_activity_at ?? w.updated_at)}</span>
          </div>
        </div>
      ))}

      <div className="section-label" style={{ marginTop: 34 }}>Recent Sessions</div>
      {sessions.length === 0 && (
        <div className="l1-none">这个 Project 下还没有 Session。</div>
      )}
      {sessions.slice(0, 8).map((s) => (
        <div key={s.id} className="list-row" onClick={() => navigate({ view: "session", sessionId: s.id })}>
          <div className="grow">
            <div className="title">{s.title ?? s.agent_session_id}</div>
          </div>
          <div className="side">
            <span title={AGENT_LABELS[s.agent]}><AgentIcon agent={s.agent} /></span>
            <span>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
          </div>
        </div>
      ))}

      <div className="page-head" style={{ marginTop: 34, marginBottom: 8 }}>
        <div className="section-label" style={{ margin: 0 }}>Resources</div>
        <button className="btn small ghost" onClick={() => setAddingRes(true)}>添加 Resource</button>
      </div>
      {resources.length === 0 && (
        <div className="l1-none">暂无 Resource。Project 不依赖任何路径，这里只是可选的引用集合。</div>
      )}
      {resources.map((r) => (
        <div className="ext-row" key={r.id}>
          <span className="badge">{r.kind}</span>
          <span className="ext-title mono">{r.uri}</span>
          <button className="link" onClick={async () => { await api.removeResource(r.id); refresh(); }}>移除</button>
        </div>
      ))}

      {creatingWs && (
        <NewWorkstreamModal
          initialProjectId={projectId}
          onClose={() => setCreatingWs(false)}
          onCreated={() => refresh()}
        />
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
    </div>
  );
}
