import Icon from "../../components/Icon";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import { submitsOnEnter, timeAgo, useRefreshSignal, Modal, openRemoteUrl } from "../../components/common";
import { GitStateBadge, MissingBadge, PathError, PathText } from "../workstreams/WorkspacePaths";
import PreferenceList from "../preferences/PreferenceList";
import ProjectLifecycleActions from "./ProjectLifecycleActions";
import { clearProjectCardsCache } from "./ProjectsView";
import ProjectSummary, { clearProjectSummaryCache } from "./ProjectSummary";
import SessionMiniList from "../sessions/SessionMiniList";
import { httpsRemoteUrl } from "./remoteUrl";
import { projectKindLabels, projectPurposeLabels, isDirectoryGroupProject } from "./projectKind";
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
export const projectDetailCache = new Map<string, ProjectDetailData>();

export function clearProjectDetailCache() {
  projectDetailCache.clear();
  clearProjectSummaryCache();
}

export default function ProjectDetail({ projectId, initialTitle, navigate }: {
  projectId: string;
  initialTitle?: string;
  navigate: (r: Route) => void;
}) {
  const [data, setData] = useState<ProjectDetailData | null>(() => projectDetailCache.get(projectId) ?? null);
  const [gone, setGone] = useState(false);
  const [failure, setFailure] = useState("");
  const [renaming, setRenaming] = useState(false);
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

  const sortedSessions = useMemo(() => {
    if (!detail) return [];
    return [...detail.sessions].sort((a, b) =>
      (b.last_activity_at ?? b.started_at ?? "").localeCompare(
        a.last_activity_at ?? a.started_at ?? "",
      ),
    );
  }, [detail]);

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
        if (nextData) projectDetailCache.set(projectId, nextData);
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
          <div style={{ display: "flex", flexDirection: "column", gap: 16, marginTop: 16 }}>
            <div className="skeleton" style={{ height: 100, borderRadius: "var(--radius-lg)" }} />
            <div className="skeleton" style={{ height: 160, borderRadius: "var(--radius-lg)" }} />
          </div>
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
    <div className="main project-detail">
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
      <div className="project-detail-layout">
      <div>
      <ProjectSummary key={projectId} projectId={projectId} sessions={detail.sessions} />
      <PreferenceList key={`preferences-${projectId}`} projectId={projectId} />
      <section className="rail-section">
        <div className="section-label">任务</div>

        {detail.workstreams.length === 0 && (
          <div className="l1-none">还没有任务经由这些目录关联进来。</div>
        )}
        {/* 展示所有通过工作路径关联的任务。 */}
        {detail.workstreams.map(({ workstream: w }) => {
          const summary = w.description;
          return (
            <div key={w.id} className="list-row"
              onClick={() => navigate({ view: "workstream", workstreamId: w.id, initialTitle: w.title })}>
              <div className="grow">
                <div className="title mini-title" title={w.title}>
                  <span><Icon name="tasks" /></span>
                  <span className="truncate">{w.title}</span>
                </div>
                {summary && <div className="meta">{summary}</div>}
              </div>
              <div className="side">
                {w.visibility === "archived" && <span className="badge warn" title="已归档；项目与它只是投影关系">已归档</span>}
                <span>{timeAgo(w.updated_at)}</span>
              </div>
            </div>
          );
        })}
      </section>

      <section className="rail-section">
        <div className="rail-head">
          <div className="section-label" style={{ margin: 0 }}>会话</div>
          {(detail.purpose === "assistant" || !isDirectoryGroupProject({ ...detail.project, kind: detail.kind })) && (
            <button
              className="btn ghost icon-button"
              title="新建会话"
              aria-label="新建会话"
              onClick={() => navigate(detail.purpose === "assistant" ? { view: "assistant" } : { view: "new-session", projectId: project.id })}
            >
              <Icon name="plus" />
            </button>
          )}
        </div>

        <SessionMiniList sessions={sortedSessions} emptyText="这个项目下还没有会话。" navigate={navigate} />
      </section>



      </div>
      <aside>
      <section className="rail-section" style={{ marginTop: 26 }}>
        <div className="section-label">属性</div>
        <div className="row" style={{ gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          <span className={`badge ${detail.kind === "git" ? "accent" : ""}`}>
            {projectKindLabels[detail.kind]}
          </span>
          {detail.purpose && <span className="badge accent">{projectPurposeLabels[detail.purpose]}</span>}
          <span className="badge">{project.visibility === "archived" ? "已归档" : "未归档"}</span>
          {project.name_customized && <span className="small muted">自定义名称</span>}
        </div>
        {detail.remote_url && (
          <a
            className="path-link small muted mono"
            href={httpsRemoteUrl(detail.remote_url)}
            style={{ marginTop: 6, display: "block", overflowWrap: "anywhere" }}
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
        )}
        <div className="small muted" style={{ marginTop: 6 }}>
          {detail.workspace_paths.length} 个工作目录 ·{" "}
          {detail.workstreams.length} 个任务 · {detail.sessions.length} 个会话
        </div>
        <div className="small muted" style={{ marginTop: 6 }}>
          创建于 {timeAgo(project.created_at)}
        </div>
      </section>
      <section className="rail-section">
        <div className="section-label">工作目录</div>
        {sortedWorkspacePaths.length === 0 && (
          <div className="l1-none">
            这个项目目前没有登记目录。
          </div>
        )}
        {sortedWorkspacePaths.map((p) => (
          <div className="list-row" key={p.id} style={{ cursor: "default" }}>
            <div className="grow">
              {/* 不用 .title：它的 nowrap + 尾部省略会把路径末段吃掉，
                  而末段正是用户用来认目录的那一段。 */}
              <div style={{ minWidth: 0, overflowWrap: "anywhere" }}>
                <PathText path={p.canonical_path} max={72} />
              </div>
            </div>
            <div className="side">
              {!p.exists && <MissingBadge />}
              <GitStateBadge state={p.git_state} kind={p.git_kind} />
            </div>
          </div>
        ))}

      </section>

      </aside>
      </div>

      {renaming && (
        <Modal title="重命名项目" onClose={() => setRenaming(false)}>
          <label className="field"><span>名称</span>
            <input type="text" value={nameInput} autoFocus
              onChange={(e) => { setNameInput(e.target.value); setRenameError(""); }}
              onKeyDown={(e) => submitsOnEnter(e) && commitRename()} /></label>
          {renameError && (
            <div className="badge warn" style={{ marginBottom: 10, overflowWrap: "anywhere" }}>{renameError}</div>
          )}
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setRenaming(false)} disabled={renameBusy}>取消</button>
            <button className="btn primary" onClick={commitRename}
              disabled={renameBusy || nameInput.trim() === ""}>
              {renameBusy ? "保存中…" : "保存"}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
