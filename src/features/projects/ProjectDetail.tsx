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
import ProjectLifecycleActions from "./ProjectLifecycleActions";
import { clearProjectCardsCache } from "./projectCardsCache";
import ProjectSummary from "./ProjectSummary";
import SessionMiniList from "../sessions/SessionMiniList";
import WorkstreamFormModal from "../workstreams/WorkstreamFormModal";
import { clearWorkstreamCardsCache } from "../workstreams/useWorkstreamCards";
import { EVT_SYNCED } from "../../app/routes";
import { httpsRemoteUrl } from "./remoteUrl";
import { projectKindLabels, projectPurposeLabels, isDirectoryGroupProject, isProjectAllPathsMissing } from "./projectKind";
import {
  type ProjectDetailData,
} from "../../types";
import type { Route } from "../../app/routes";

/**
 * Project Detail：一次 `get_project_detail` 读到全部真相——它拥有的 WorkspacePaths、
 * 经由这些路径到达的 Workstreams，以及 cwd 落在这些路径上的
 * Sessions（走权威链，不走缓存列，M29）。
 * 项目事实由目录派生，当前摘要由用户主动更新或重新生成。
 */
import { projectDetailCache, clearProjectDetailCache } from "./projectDetailCache";
import { useLoadingDelay } from "../../hooks/useLoadingDelay";
export { projectDetailCache, clearProjectDetailCache };


export default function ProjectDetail({ projectId, initialTitle, navigate }: {
  projectId: string;
  initialTitle?: string;
  navigate: (r: Route) => void;
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
  useRefreshSignal(refresh);

  const detail = data;
  const project = detail?.project ?? null;
  const allPathsMissing = !detail || isProjectAllPathsMissing({
    path_count: detail.workspace_paths.length,
    missing_path_count: detail.workspace_paths.filter(path => !path.exists).length,
  });
  useEffect(() => setNewTaskOpen(false), [projectId]);

  const sortedWorkspacePaths = useMemo(() => {
    if (!detail) return [];
    return [...detail.workspace_paths].sort((a, b) => {
      const aMain = a.git_kind === "main" ? 0 : 1;
      const bMain = b.git_kind === "main" ? 0 : 1;
      if (aMain !== bMain) return aMain - bMain;
      return a.canonical_path.localeCompare(b.canonical_path);
    });
  }, [detail]);

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
      refresh();
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
            title={initialTitle ?? ""}
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
          title={initialTitle ?? ""}
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


  return (
    <DetailPanelProvider kind="project"><div className="main project-detail detail-page">
      <PageHeader
        title={
          <span style={{ display: "inline-flex", alignItems: "center", gap: 8, overflowWrap: "anywhere" }}>
            <Icon name="project" />
            <span>{project.name}</span>
          </span>
        }
        actions={
          <>
          <ProjectLifecycleActions project={{ ...project, purpose: detail.purpose, session_count: detail.sessions.length }}
            onChanged={() => { clearProjectCardsCache(); refresh(); }}
            onDeleted={() => { clearProjectDetailCache(); navigate({ view: "projects", scope: "archived" }); }} />
          <button className="btn ghost icon-button" title="重命名" aria-label="重命名"
            onClick={() => { setNameInput(project.name); setRenameError(""); setRenaming(true); }}>
            <Icon name="edit" />
          </button>
          </>
        }
      />

      {/* 概览数字与类型 / 命名 / 创建时间都收在右栏「属性」块里：标题栏只放标题与图标动作。 */}
      <DetailLayout panel={<><DetailSection sectionId="properties" label="属性" title={<>
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
            <Field label="创建时间">{timeAgo(project.created_at)}</Field>
            </DetailFields>
          </DetailSection>
<DetailSection sectionId="workspace-paths" label="工作目录" className="project-workspace-paths" title={<>
              <Icon name="folder" />
              <span>工作目录（{sortedWorkspacePaths.length}）</span>
            </>}>

            {sortedWorkspacePaths.length === 0 && (
              <div className="detail-empty">
                这个项目目前没有登记目录。
              </div>
            )}
            <DetailList label="工作目录列表">
            {sortedWorkspacePaths.map((p) => (
              <DetailListRow key={p.id} icon={<Icon name="folder" />}
                title={<CompactPath path={p.canonical_path} />}
                titleHint={p.canonical_path} onOpen={() => void openPath(p.canonical_path)}
                meta={<>
                  {!p.exists && <MissingBadge />}
                  <GitStateBadge state={p.git_state} kind={p.git_kind} />
                </>}
              />
            ))}
            </DetailList>
          </DetailSection></>}><ProjectSummary key={projectId} projectId={projectId} sessions={detail.sessions} />
<DetailSection sectionId="tasks" label="关联任务" title={<>
                <Icon name="tasks" />
                <span>关联任务（{detail.workstreams.length}）</span>
              </>} actions={<><button type="button" className="btn ghost icon-button small"
                title="新建任务"
                aria-label="新建任务" onClick={() => setNewTaskOpen(true)}>
                <Icon name="plus" />
              </button></>}>


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
<DetailSection sectionId="sessions" label="会话" className="project-sessions-section" title={<>
                <Icon name="chat" />
                <span>会话（{detail.sessions.length}）</span>
              </>} actions={<>{(detail.purpose === "assistant" || !isDirectoryGroupProject({ ...detail.project, kind: detail.kind })) && (
                <button
                  className="btn ghost icon-button small"
                  title="新建会话"
                  aria-label="新建会话"
                  onClick={() => navigate(allPathsMissing ? { view: "new-session" } : { view: "new-session", projectId: project.id })}
                >
                  <Icon name="plus" />
                </button>
              )}</>}>


            <SessionMiniList sessions={detail.sessions} emptyText="这个项目下还没有会话。" navigate={navigate} />
          </DetailSection>
<SkillScopeConfig projectId={projectId} navigate={navigate} workspacePaths={detail.workspace_paths.filter(path => path.exists).map(path => path.canonical_path)} /></DetailLayout>

      {newTaskOpen && (
        <WorkstreamFormModal
          key={projectId}
          initialProjectId={projectId}
          onClose={() => setNewTaskOpen(false)}
          onCreated={() => {
            clearProjectCardsCache();
            clearWorkstreamCardsCache();
            window.dispatchEvent(new Event(EVT_SYNCED));
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
    </div></DetailPanelProvider>
  );
}
