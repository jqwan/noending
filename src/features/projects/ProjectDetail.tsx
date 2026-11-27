import { DetailLayout, DetailPanelProvider } from "../../components/DetailLayout";
import { recentFirst } from "../../components/detailOrder";
import { DetailFields, DetailList, DetailListRow, DetailSection } from "../../components/DetailSection";
import CompactPath from "../../components/CompactPath";
import DialogActions from "../../components/DialogActions";
import { removedTargets } from "../../app/removedTargets";
import Icon from "../../components/Icon";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import { Field, submitsOnEnter, timeAgo, useRefreshSignal, Modal, openPath, openRemoteUrl } from "../../components/common";
import { GitStateBadge, MissingBadge, PathError } from "../workstreams/WorkspacePaths";
import SkillScopeConfig from "../skills/SkillScopeConfig";
import AgentMailbox from "../agents/AgentMailbox";
import ProjectAgentSection, { AgentReadOnlyDocument } from "../agents/ProjectAgentSection";
import ProjectLifecycleActions from "./ProjectLifecycleActions";
import { clearProjectCardsCache } from "./projectCardsCache";
import { showToast } from "../../components/Toast";
import ProjectSummary from "./ProjectSummary";
import SessionMiniList from "../sessions/SessionMiniList";
import NewProjectSessionButton from "../sessions/NewProjectSessionButton";
import WorkstreamFormModal from "../workstreams/WorkstreamFormModal";
import { clearWorkstreamCardsCache } from "../workstreams/useWorkstreamCards";
import { notifyDataChanged } from "../../app/routes";
import { httpsRemoteUrl } from "./remoteUrl";
import { projectKindLabels, projectPurposeLabels, isDirectoryGroupProject } from "./projectKind";
import {
  type AgentFileName,
  type ProjectAgent,
  type ProjectDetailData,
} from "../../types";
import type { ProjectEntry, Route } from "../../app/routes";

/**
 * Project Detail：一次 `get_project_detail` 读到全部真相——它拥有的 WorkspacePaths、
 * 经由这些路径到达的 Workstreams，以及 cwd 落在这些路径上的
 * Sessions 通过工作目录关联到项目。
 * 项目事实由目录派生，当前摘要由用户主动更新或重新生成。
 */
import { projectDetailCache } from "./projectDetailCache";
import { useLoadingDelay } from "../../hooks/useLoadingDelay";

export default function ProjectDetail({ projectId, initialTitle, navigate, entry, fromAgents }: {
  projectId: string;
  initialTitle?: string;
  navigate: (r: Route) => void;
  entry?: ProjectEntry;
  fromAgents?: boolean;
}) {
  const [data, setData] = useState<ProjectDetailData | null>(() => projectDetailCache.get(projectId) ?? null);
  const [gone, setGone] = useState(false);
  const [failure, setFailure] = useState("");
  const showSkeleton = useLoadingDelay(data === null && failure === "" && !gone);
  const [renaming, setRenaming] = useState(false);
  const [newTaskOpen, setNewTaskOpen] = useState(false);
  const [nameInput, setNameInput] = useState("");
  const [renameError, setRenameError] = useState("");
  const [renameBusy, setRenameBusy] = useState(false);
  const [confirmCleanup, setConfirmCleanup] = useState(false);
  const [cleanupBusy, setCleanupBusy] = useState(false);
  const cleanupBusyRef = useRef(false);
  const currentProjectId = useRef(projectId);
  currentProjectId.current = projectId;
  const renameBusyRef = useRef(false);
  const seqRef = useRef(0);

  const refresh = useCallback(() => {
    const seq = ++seqRef.current;
    api.getProjectDetail(projectId)
      .then((d) => {
        if (seq !== seqRef.current) return;
        if (removedTargets.projects.has(projectId)) return;
        projectDetailCache.set(projectId, d);
        setData(d);
        setGone(false);
        setFailure("");
      })
      .catch((e) => {
        if (seq !== seqRef.current) return;
        const text = String(e);
        setFailure(text);
        // 只按「Project <id> 不存在」判定已消失，其余一律当作读取失败——
        // 猜错的代价是让用户以为自己的 Project 没了。
        const isGone = text.includes(`Project ${projectId} 不存在`);
        if (isGone) {
          projectDetailCache.delete(projectId);
        }
        setGone(isGone);
      });
  }, [projectId]);

  useEffect(refresh, [refresh]);
  useEffect(() => setConfirmCleanup(false), [projectId, entry]);
  useRefreshSignal(refresh);

  const detail = data;
  const project = detail?.project ?? null;
  const agent = detail?.agent;
  const agentView = entry === "agent" && Boolean(agent);
  const icon = (detail ? agentView : entry === "agent") ? "spark" : "project";
  const archiveBoard: Route = { view: "projects", scope: "archived", ...(fromAgents && { agentOnly: true }) };
  const onAgentSaved = (updated: ProjectAgent, file?: AgentFileName) => {
    if (removedTargets.projects.has(projectId)) return;
    ++seqRef.current;
    setData(current => {
      if (!current?.agent) return current;
      const agent = current.agent;
      const next = {
        ...current,
        agent: {
          ...agent,
          agents: !file || file === "agents" ? updated.agents : agent.agents,
          guide_is_default: !file || file === "agents" ? updated.guide_is_default : agent.guide_is_default,
          claude_md_missing: file ? agent.claude_md_missing : updated.claude_md_missing,
          soul: file === "soul" || (!file && agent.soul.state === "missing") ? updated.soul : agent.soul,
          role: file === "role" || (!file && agent.role.state === "missing") ? updated.role : agent.role,
        },
      };
      projectDetailCache.set(projectId, next);
      return next;
    });
    clearProjectCardsCache();
    notifyDataChanged(["projects"]);
  };
  useEffect(() => setNewTaskOpen(false), [projectId, entry]);

  const sortedWorkspacePaths = useMemo(() => {
    if (!detail) return [];
    return [...detail.workspace_paths].sort((a, b) => {
      const aMain = a.git_kind === "main" ? 0 : 1;
      const bMain = b.git_kind === "main" ? 0 : 1;
      if (aMain !== bMain) return aMain - bMain;
      return a.canonical_path.localeCompare(b.canonical_path);
    });
  }, [detail]);
  const displayedPaths = agentView
    ? sortedWorkspacePaths.filter(path => path.id === agent?.workspace_path_id)
    : sortedWorkspacePaths;
  const directoryLabel = agentView ? "档案目录" : "工作目录";
  const displayedSessions = agentView
    ? detail?.sessions.filter(session => session.workspace_path_id === agent?.workspace_path_id) ?? []
    : detail?.sessions ?? [];
  const unusedMissingPaths = detail?.workspace_paths.filter(path => {
    if (path.exists) return false;
    if (detail.cleanable_missing_path_ids) {
      return detail.cleanable_missing_path_ids.includes(path.id);
    }
    return !detail.sessions.some(session => session.workspace_path_id === path.id);
  }) ?? [];
  const cleanup = async () => {
    if (cleanupBusyRef.current) return;
    cleanupBusyRef.current = true; setCleanupBusy(true);
    try {
      const count = await api.cleanMissingProjectPaths(projectId);
      clearProjectCardsCache();
      if (currentProjectId.current === projectId) {
        setConfirmCleanup(false);
        refresh();
      }
      notifyDataChanged(["projects"]);
      showToast(`已清理 ${count} 个失效目录登记`);
    } catch (error) {
      showToast(`清理失败：${String(error)}`);
    } finally {
      cleanupBusyRef.current = false; setCleanupBusy(false);
    }
  };

  const commitRename = async () => {
    const next = nameInput.trim();
    if (project === null || renameBusyRef.current) return;
    if (next === "") {
      setRenameError("名字不能为空");
      return;
    }
    if (next === project.name) {
      setRenaming(false);
      return;
    }
    renameBusyRef.current = true;
    setRenameBusy(true);
    setRenameError("");
    try {
      const renamed = await api.renameProject(project.id, next);
      setData((cur) => {
        const nextData = cur ? { ...cur, project: renamed } : cur;
        if (nextData && !removedTargets.projects.has(projectId)) projectDetailCache.set(projectId, nextData);
        return nextData;
      });
      setRenaming(false);
      clearProjectCardsCache();
      notifyDataChanged(["projects"]);
    } catch (e) {
      // 失败留在弹窗里说原因：静默关闭会让用户以为已经改好了。
      console.error(e);
      setRenameError(`重命名失败：${String(e)}`);
    } finally {
      renameBusyRef.current = false;
      setRenameBusy(false);
    }
  };

  if (detail === null || project === null) {
    if (failure === "" && !gone) {
      return (
        <div className="main project-detail" role="status">
          <PageHeader
            title={initialTitle ?? ""} icon={<Icon name={icon} />}
          />
          {showSkeleton && (
            <div style={{ display: "flex", flexDirection: "column", gap: 16, marginTop: 16 }}>
              <div className="skeleton" style={{ height: 100, borderRadius: "var(--radius-lg)" }} />
              <div className="skeleton" style={{ height: 160, borderRadius: "var(--radius-lg)" }} />
            </div>
          )}
        </div>
      );
    }
    return (
      <div className="main project-detail">
        <PageHeader
          title={initialTitle ?? ""} icon={<Icon name={icon} />}
        >
          {failure !== "" && <PathError text={failure} />}
          {gone && (
            <p className="muted small">
              项目已被永久删除或合并到其他项目。关联任务和会话不会因此被删除。
            </p>
          )}
          {!gone && failure !== "" && (
            <p className="muted small">
              这通常只是读取失败，不代表项目不见了。下面的按钮会重新读一次真实状态。
            </p>
          )}
          <div className="invite">
            <button className="btn small" onClick={refresh}>重试</button>
          </div>
        </PageHeader>
      </div>
    );
  }


  const openEntry = (next?: ProjectEntry) => {
    if (next === entry) return;
    navigate({
      view: "project", projectId, initialTitle: project.name,
      ...(next && { entry: next }),
      ...(fromAgents && { fromAgents }),
    });
  };

  return (
    <DetailPanelProvider kind="project"><div className="main project-detail detail-page">
      <PageHeader
        title={project.name} icon={<Icon name={icon} />}
        navigation={agent && <div className="settings-seg icon-seg" role="group" aria-label="项目子页">
          <button type="button" className={agentView ? "" : "on"} aria-pressed={!agentView}
            aria-label="项目详情" title="项目详情" onClick={() => openEntry()}>
            <Icon name="project" />
          </button>
          <button type="button" className={agentView ? "on" : ""} aria-pressed={agentView}
            aria-label="智能体详情" title="智能体详情" onClick={() => openEntry("agent")}>
            <Icon name="spark" />
          </button>
        </div>}
        actions={
          <>
          <button className="btn ghost icon-button" title="重命名" aria-label="重命名"
            onClick={() => { setNameInput(project.name); setRenameError(""); setRenaming(true); }}>
            <Icon name="edit" />
          </button>
          <ProjectLifecycleActions project={{ ...project, purpose: detail.purpose, is_agent: Boolean(agent), session_count: detail.sessions.length }}
            onChanged={() => { clearProjectCardsCache(); refresh(); }}
            replacement={fromAgents ? archiveBoard : undefined}
            onDeleted={() => navigate(archiveBoard)} />
          </>
        }
      />

      <DetailLayout panel={<>
        {!agentView && (
          <DetailSection sectionId="properties" label="属性" title={<>
              <Icon name="info" />
              <span>属性</span>
            </>}>

            <DetailFields>
            <Field label="类型与状态">
            <div className="row">
              <span className={`badge ${detail.kind === "git" ? "accent" : ""}`}>
                {projectKindLabels[detail.kind]}
              </span>
              {detail.purpose && <span className="badge accent">{projectPurposeLabels[detail.purpose]}</span>}
              {agent && <span className="badge accent">智能体</span>}
              <span className="badge">{project.visibility === "archived" ? "已归档" : "未归档"}</span>
              {project.name_customized && <span className="small muted">自定义名称</span>}
            </div>
            </Field>
            {detail.remote_url && (
              <Field label="远程仓库">
              <a
                className="path-link small muted mono"
                href={httpsRemoteUrl(detail.remote_url)}
                style={{ display: "block", overflowWrap: "anywhere" }}
                title="在浏览器打开远程仓库"
                onClick={(e) => {
                  e.preventDefault();
                  void openRemoteUrl(httpsRemoteUrl(detail.remote_url!));
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    void openRemoteUrl(httpsRemoteUrl(detail.remote_url!));
                  }
                }}
              >
                {httpsRemoteUrl(detail.remote_url)}
              </a>
              </Field>
            )}
            <Field label="项目统计">
              {detail.workspace_paths.length} 个工作目录 ·{" "}
              {detail.workstreams.length} 个任务 · {detail.sessions.length} 个会话
            </Field>
            <Field label="项目 ID" copyValue={project.id}><code>{project.id}</code></Field>
            <Field label="创建时间">{timeAgo(project.created_at)}</Field>
            </DetailFields>
          </DetailSection>
        )}
          <DetailSection sectionId="workspace-paths" label={directoryLabel} className="project-workspace-paths" title={<>
              <Icon name="folder" />
              <span>{agentView ? directoryLabel : `${directoryLabel}（${displayedPaths.length}）`}</span>
            </>} actions={!agentView && unusedMissingPaths.length > 0 && <button type="button" className="btn ghost icon-button small"
              title="清理失效目录" aria-label="清理失效目录" disabled={cleanupBusy} onClick={() => setConfirmCleanup(true)}>
              <Icon name="trash" />
            </button>}>

            {displayedPaths.length === 0 && (
              <div className="detail-empty">
                {agentView ? "尚未登记档案目录。" : "这个项目目前没有登记目录。"}
              </div>
            )}
            <DetailList label={`${directoryLabel}列表`}>
            {displayedPaths.map((p) => (
              <DetailListRow key={p.id} icon={<Icon name="folder" />}
                title={<CompactPath path={p.canonical_path} />}
                titleHint={p.canonical_path} openLabel={agentView ? "打开档案目录" : undefined} onOpen={() => void openPath(p.canonical_path)}
                meta={<>
                  {!p.exists && <MissingBadge />}
                  <GitStateBadge state={p.git_state} kind={p.git_kind} />
                </>}
              />
            ))}
            </DetailList>
          </DetailSection>
          <SkillScopeConfig projectId={projectId} navigate={navigate}
            workspacePaths={detail.workspace_paths.filter(path => path.exists).map(path => path.canonical_path)} />
      </>}>
        {agent && <div hidden={!agentView}>
          <AgentReadOnlyDocument file="intro" document={agent.intro} onRefresh={refresh} />
          <AgentMailbox key={agent.id} agentId={agent.id} name={project.name}
            active={agentView} archived={project.visibility === "archived"} sessions={displayedSessions} />
          <ProjectAgentSection projectId={projectId} agent={agent} active={agentView}
            archived={project.visibility === "archived"} onSaved={onAgentSaved} onRefresh={refresh} />
        </div>}
        {!agentView && <>
          <ProjectSummary key={projectId} projectId={projectId} sessions={detail.sessions} />
          <DetailSection sectionId="tasks" label="关联任务" title={<>
                <Icon name="tasks" />
                <span>关联任务（{detail.workstreams.length}）</span>
              </>} actions={<button type="button" className="btn ghost icon-button small"
                title="新建任务"
                aria-label="新建任务" onClick={() => setNewTaskOpen(true)}>
                <Icon name="plus" />
              </button>}>
            {detail.workstreams.length === 0 ? (
              <div className="detail-empty">未关联任何任务</div>
            ) : (
              <DetailList label="关联任务列表">
                {recentFirst(detail.workstreams, row => row.workstream.updated_at).map(({ workstream: w }) => (
                  <DetailListRow key={w.id} icon={<Icon name="tasks" />}
                    title={w.title} titleHint={w.title}
                    onOpen={() => navigate({ view: "workstream", workstreamId: w.id, initialTitle: w.title })}
                    meta={w.visibility === "archived" && <span className="badge warn">已归档</span>}
                  />
                ))}
              </DetailList>
            )}
          </DetailSection>
        </>}
          <DetailSection sectionId="sessions" label={agentView ? "对话记录" : "会话"} className="project-sessions-section" title={<>
                <Icon name="chat" />
                <span>{agentView ? "对话记录" : "会话"}（{displayedSessions.length}）</span>
              </>} actions={project.visibility !== "archived" && !isDirectoryGroupProject(detail) && (
                <NewProjectSessionButton projectId={project.id} navigate={navigate}
                  archivePathId={agentView ? agent?.workspace_path_id : undefined} />
              )}>
            <SessionMiniList sessions={displayedSessions}
              emptyText={agentView ? "还没有和这位智能体的对话。" : "这个项目下还没有会话。"} navigate={navigate} />
          </DetailSection>
      </DetailLayout>

      {!agentView && newTaskOpen && (
        <WorkstreamFormModal
          key={projectId}
          initialProjectId={projectId}
          onClose={() => setNewTaskOpen(false)}
          onCreated={() => {
            clearProjectCardsCache();
            clearWorkstreamCardsCache();
            notifyDataChanged([]);
          }}
        />
      )}

      {renaming && (
        <Modal title="重命名项目" onClose={() => setRenaming(false)}>
          <label className="field"><span>名称</span>
            <input type="text" value={nameInput} autoFocus
              onChange={(e) => { setNameInput(e.target.value); setRenameError(""); }}
              onKeyDown={(e) => submitsOnEnter(e) && commitRename()} /></label>
          {renameError && (
            <div className="badge warn" style={{ marginBottom: 10, overflowWrap: "anywhere" }}>{renameError}</div>
          )}
          <DialogActions>
            <button className="btn" onClick={() => setRenaming(false)} disabled={renameBusy}>取消</button>
            <button className="btn primary" onClick={commitRename}
              disabled={renameBusy || nameInput.trim() === ""}>
              {renameBusy ? "保存中…" : "保存"}
            </button>
          </DialogActions>
        </Modal>
      )}
      {!agentView && confirmCleanup && <Modal title="清理失效目录？" onClose={() => { if (!cleanupBusyRef.current) setConfirmCleanup(false); }}>
        <p>清理 {unusedMissingPaths.length} 个缺失且没有会话引用的目录登记？</p>
        <p className="small muted">仅移除 NoEnding 中的登记，不删除磁盘内容，项目本身保留。执行时会重新检查目录及会话引用。</p>
        <DialogActions>
          <button type="button" className="btn" disabled={cleanupBusy} onClick={() => setConfirmCleanup(false)}>取消</button>
          <button type="button" className="btn danger" disabled={cleanupBusy} onClick={() => void cleanup()}>{cleanupBusy ? "清理中…" : "清理"}</button>
        </DialogActions>
      </Modal>}
    </div></DetailPanelProvider>
  );
}
