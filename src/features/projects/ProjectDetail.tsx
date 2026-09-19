import React, { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import { Modal, timeAgo, useRefreshSignal } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import NewWorkstreamModal from "../workstreams/NewWorkstreamModal";
import { sessionDisplayTitle, UNTITLED_SESSION } from "../sessions/SessionTable";
import type { Route } from "../../app/routes";
import { IntelligenceOnly, useBaseExperience } from "../../app/experience";
import { cardSummaryLine } from "../workstreams/WorkstreamCard";
import {
  AGENT_LABELS,
  type Project,
  type ProjectResource,
  type Session,
  type Workstream,
  type WorkstreamCardData,
} from "../../types";

/**
 * Project 的引用资料类型（`project_resources.kind` 的定义域）。
 * §2 词表没有覆盖这一层，这里只翻译普通名词，不引入新的领域词。
 */
const RESOURCE_KIND_LABELS: Record<string, string> = {
  repository: "代码仓库",
  workspace: "工作区",
  file: "文件",
  document: "文档",
  url: "链接",
  artifact: "产物",
  external: "外部引用",
};

/**
 * Project Detail（整体设计方案 §53/§54）：Workstreams 是主要 section，
 * Resources 用轻量行；Project 视觉权重保持次于 Workstream。
 */
export default function ProjectDetail({ projectId, navigate }: {
  projectId: string;
  navigate: (r: Route) => void;
}) {
  const { intelligenceEnabled } = useBaseExperience();
  const [project, setProject] = useState<Project | null>(null);
  const [projectLoaded, setProjectLoaded] = useState(false);
  const [workstreams, setWorkstreams] = useState<Workstream[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [resources, setResources] = useState<ProjectResource[]>([]);
  const [creatingWs, setCreatingWs] = useState(false);
  const [addingRes, setAddingRes] = useState(false);
  const [resKind, setResKind] = useState("url");
  const [resUri, setResUri] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(() => {
    api.listProjects()
      .then((ps) => setProject(ps.find((p) => p.id === projectId) ?? null))
      .catch(console.error)
      .finally(() => setProjectLoaded(true));
    api.listWorkstreamCards().then((cards) =>
      setWorkstreams(cards.filter((w) => w.project_id === projectId && w.visibility === "normal")),
    ).catch(console.error);
    api.listSessions(projectId).then(setSessions).catch(console.error);
    api.listResources(projectId).then(setResources).catch(console.error);
  }, [projectId]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  // 读完了还是空 = 这个 Project 真的不存在（删除、链接过期）。
  // 继续显示「加载中…」会把用户困在一个假象里。
  if (!project) {
    return (
      <div className="main narrow">
        <PageHeader
          back="Projects"
          onBack={() => navigate({ view: "projects" })}
          title={projectLoaded ? "读取 Project 失败" : "加载中…"}
        >
          {projectLoaded && (
            <>
              <p className="muted small">
                这个 Project 已经不存在，或从未创建成功。它下面的 Workstream 与
                Session 不会被删除——Project 只是可选的组织层。
              </p>
              <div className="invite">
                <button className="btn small" onClick={refresh}>重试</button>
              </div>
            </>
          )}
        </PageHeader>
      </div>
    );
  }

  const addResource = async () => {
    const uri = resUri.trim();
    if (!uri || busy) return;
    setBusy(true);
    setError("");
    try {
      await api.addResource(projectId, resKind, uri);
      setAddingRes(false); setResUri(""); setError("");
      refresh();
    } catch (e) {
      console.error(e);
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const removeResource = async (id: string) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await api.removeResource(id);
      refresh();
    } catch (e) {
      console.error(e);
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="main narrow">
      <PageHeader
        back="Projects"
        onBack={() => navigate({ view: "projects" })}
        title={project.name}
        sub={project.description || undefined}
        actions={
          <>
            <IntelligenceOnly>
              <button className="btn ghost"
                onClick={() => navigate({ view: "assistant", scope: { type: "project", id: project.id } })}>
                询问 Assistant
              </button>
            </IntelligenceOnly>
            <button className="btn" onClick={() => setCreatingWs(true)}>新建 Workstream</button>
          </>
        }
      />

      <div className="section-label" style={{ marginTop: 26 }}>Workstreams</div>
      {workstreams.length === 0 && (
        <div className="l1-none">这个 Project 下还没有 Workstream。</div>
      )}
      {workstreams.map((w) => {
        // 与 Workstream 卡片同一条规则：智能关闭时这里只出现用户自己写的描述，
        // 不展示冻结期的 Agent 摘要。规则只写在 cardSummaryLine 一处。
        const summary = cardSummaryLine(
          w as unknown as WorkstreamCardData,
          intelligenceEnabled,
        );
        return (
          <div key={w.id} className="list-row" onClick={() => navigate({ view: "workstream", workstreamId: w.id })}>
            <div className="grow">
              <div className="title" title={w.title}>{w.title}</div>
              {summary && <div className="meta">{summary}</div>}
            </div>
            <div className="side">
              <span>{timeAgo((w as any).last_activity_at ?? w.updated_at)}</span>
            </div>
          </div>
        );
      })}

      <div className="section-label" style={{ marginTop: 34 }}>最近 Sessions</div>
      {sessions.length === 0 && (
        <div className="l1-none">这个 Project 下还没有 Session。</div>
      )}
      {sessions.slice(0, 8).map((s) => (
        <div key={s.id} className="list-row" onClick={() => navigate({ view: "session", sessionId: s.id })}>
          <div className="grow">
            <div className="title" title={s.title ?? `${UNTITLED_SESSION} · ${s.agent_session_id}`}>
              {sessionDisplayTitle(s.title)}
            </div>
          </div>
          <div className="side">
            <span title={AGENT_LABELS[s.agent]}><AgentIcon agent={s.agent} /></span>
            <span>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
          </div>
        </div>
      ))}

      <div className="page-head" style={{ marginTop: 34, marginBottom: 8 }}>
        <div className="section-label" style={{ margin: 0 }}>引用资料</div>
        <button className="btn small ghost" onClick={() => setAddingRes(true)}>添加引用资料</button>
      </div>
      {error && (
        <div className="badge warn" style={{ marginBottom: 8, overflowWrap: "anywhere" }}>{error}</div>
      )}
      {resources.length === 0 && (
        <div className="l1-none">还没有引用资料。Project 不依赖任何路径，这里只是可选的引用集合。</div>
      )}
      {resources.map((r) => (
        <div className="ext-row" key={r.id}>
          <span className="badge">{RESOURCE_KIND_LABELS[r.kind] ?? r.kind}</span>
          <span className="ext-title mono" title={r.uri ?? undefined}>
            {r.uri || <span className="muted">（没有记录 URI）</span>}
          </span>
          <button className="link" disabled={busy} onClick={() => void removeResource(r.id)}>移除</button>
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
        <Modal title="添加引用资料" onClose={() => setAddingRes(false)}>
          <label className="field"><span>类型</span>
            <select value={resKind} onChange={(e) => setResKind(e.target.value)}>
              {Object.keys(RESOURCE_KIND_LABELS).map((k) => (
                <option key={k} value={k}>{RESOURCE_KIND_LABELS[k]}</option>
              ))}
            </select></label>
          <label className="field"><span>URI / 路径</span>
            <input type="text" className="mono" value={resUri}
              style={{ overflowWrap: "anywhere" }}
              onChange={(e) => setResUri(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && void addResource()}
              autoFocus /></label>
          {error && (
            <div className="badge warn" style={{ marginBottom: 8, overflowWrap: "anywhere" }}>{error}</div>
          )}
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setAddingRes(false)} disabled={busy}>取消</button>
            <button className="btn primary" disabled={busy || !resUri.trim()} onClick={() => void addResource()}>
              {busy ? "添加中…" : "添加"}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
