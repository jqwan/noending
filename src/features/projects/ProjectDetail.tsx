import Icon from "../../components/Icon";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import { submitsOnEnter, timeAgo, useRefreshSignal, Modal, openRemoteUrl } from "../../components/common";
import { GitStateBadge, MissingBadge, PathError, PathText } from "../workstreams/WorkspacePaths";
import SessionMiniList from "../sessions/SessionMiniList";
import { httpsRemoteUrl } from "./remoteUrl";
import { projectKindLabels } from "./projectKind";
import { cardSummaryLine } from "../workstreams/WorkstreamCard";
import {
  type ProjectDetailData,
  type WorkstreamCardData,
} from "../../types";
import type { Route } from "../../app/routes";

/**
 * Project Detail：一次 `get_project_detail` 读到全部真相——它拥有的 WorkspacePaths、
 * 经由这些路径到达的 Workstreams，以及 cwd 落在这些路径上的
 * Sessions（走权威链，不走缓存列，M29）。
 * 用户在这页唯一能做的编辑是改名：其余事实都是派生的，手工新建 / 删除 / 移动都已退出产品 API。
 */
export const projectDetailCache = new Map<string, ProjectDetailData>();

export function clearProjectDetailCache() {
  projectDetailCache.clear();
}

export default function ProjectDetail({ projectId, navigate }: {
  projectId: string;
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
            title={<span className="skeleton" style={{ display: "inline-block", width: 160, height: 24, borderRadius: "var(--radius-sm)" }} />}
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
          title={gone ? "这个项目已经不存在" : "读取项目失败"}
        >
          {failure !== "" && <PathError text={failure} />}
          {gone && (
            <p className="muted small">
              项目没有归档、也没有「删除」这个操作：当它拥有的最后一个工作目录
              不再被任务或会话引用并被清理，或被别的项目认领时，它就自动消失了。
              它下面的任务与会话不会被删除 —— 项目从来不是它们的生命周期所有者。
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
        title={<span style={{ overflowWrap: "anywhere" }}>{project.name}</span>}
        actions={
          <button className="btn ghost icon-button" title="重命名" aria-label="重命名"
            onClick={() => { setNameInput(project.name); setRenameError(""); setRenaming(true); }}>
            <Icon name="edit" />
          </button>
        }
      />

      {/* 概览数字与类型 / 命名 / 创建时间都收在右栏「属性」块里：标题栏只放标题与图标动作。 */}
      <div className="project-detail-layout">
      <div>
      <section className="rail-section">
        <div className="section-label">任务</div>

        {detail.workstreams.length === 0 && (
          <div className="l1-none">还没有任务经由这些目录关联进来。</div>
        )}
        {/* 展示所有通过工作路径关联的任务。 */}
        {detail.workstreams.map(({ workstream: w }) => {
          // 与 Workstream 卡片同一条规则：摘要取 Agent 的 current_state，退回用户描述。
          const summary = cardSummaryLine(w as unknown as WorkstreamCardData);
          return (
            <div key={w.id} className="list-row"
              onClick={() => navigate({ view: "workstream", workstreamId: w.id })}>
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
          <button
            className="btn ghost icon-button"
            title="新建会话"
            aria-label="新建会话"
            onClick={() => navigate({ view: "new-session", projectId: project.id })}
          >
            <Icon name="plus" />
          </button>
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
            这个项目已经不再拥有任何目录 —— 它会在下一次整理时自动消失。
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
