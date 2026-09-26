import Icon from "../../components/Icon";
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import { showToast } from "../../components/Toast";
import { timeAgo, useRefreshSignal, Modal } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { GitStateBadge, MissingBadge, PathError, PathText } from "../workstreams/WorkspacePaths";
import { sessionDisplayTitle, UNTITLED_SESSION, cwdDisplayLabel } from "../sessions/SessionTable";
import { cardSummaryLine } from "../workstreams/WorkstreamCard";
import {
  AGENT_LABELS,
  type ProjectDetailData,
  type WorkstreamCardData,
} from "../../types";
import type { Route } from "../../app/routes";

/**
 * Project Detail：一次 `get_project_detail` 读到全部真相——它拥有的 WorkspacePaths、
 * 经由这些路径到达的 Workstreams（含主关联 / 关联），以及 cwd 落在这些路径上的
 * Sessions（走权威链，不走缓存列，M29）。
 * 用户在这页唯一能做的编辑是改名：其余事实都是派生的，手工新建 / 删除 / 移动都已退出产品 API。
 */
export default function ProjectDetail({ projectId, navigate }: {
  projectId: string;
  navigate: (r: Route) => void;
}) {
  const [showAllSessions, setShowAllSessions] = useState(false);
  const [data, setData] = useState<ProjectDetailData | null>(null);
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
        setGone(text.includes(`Project ${projectId} 不存在`));
      });
  }, [projectId]);

  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  // 定点刷新在后台执行，完成/失败事件把按钮恢复；detail 重读走 useRefreshSignal。
  // 刷新可能让 Project 自己消失（最后一条路径被 GC），此时页面切到 gone 视图。
  const [refreshingWorkspace, setRefreshingWorkspace] = useState(false);
  useEffect(() => {
    const unCompleted = listen("workspace-reconcile-completed", () => {
      setRefreshingWorkspace(false);
    });
    const unFailed = listen("workspace-reconcile-failed", (e) => {
      setRefreshingWorkspace(false);
      showToast(`工作区刷新失败：${String((e.payload as { error?: string }).error ?? "")}`);
    });
    return () => {
      unCompleted.then((f) => f());
      unFailed.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- listeners are static
  }, []);

  const refreshWorkspace = useCallback(() => {
    setRefreshingWorkspace(true);
    api
      .refreshProjectWorkspace(projectId)
      .catch((e) => {
        setRefreshingWorkspace(false);
        showToast(`工作区刷新失败：${String(e)}`);
      });
  }, [projectId]);

  const detail = data;
  const project = detail?.project ?? null;

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
      setData((cur) => (cur ? { ...cur, project: renamed } : cur));
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
    return (
      <div className="main project-detail">
        <PageHeader
          title={gone ? "这个项目已经不存在" : (failure === "" ? "加载中…" : "读取项目失败")}
        >
          {failure !== "" && <PathError text={failure} />}
          {gone && (
            <p className="muted small">
              项目没有归档、也没有「删除」这个操作：当它拥有的最后一个工作目录
              被移除、被别的项目认领，或者在本机上再也找不到时，它就自动消失了。
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

  const sessions = detail.sessions;

  return (
    <div className="main project-detail">
      <PageHeader
        title={<span style={{ overflowWrap: "anywhere" }}>{project.name}</span>}
        actions={
          <>
            <button className="btn ghost icon-button" aria-label="询问 Assistant" title="询问 Assistant"
              onClick={() => navigate({ view: "assistant", scope: { type: "project", id: project.id } })}>
              <Icon name="chat" />
            </button>
            <button className="btn ghost icon-button" aria-label="刷新目录状态"
              title="刷新目录状态"
              disabled={refreshingWorkspace}
              onClick={refreshWorkspace}>
              <Icon name="refresh" />
            </button>
            <button className="btn ghost icon-button" title="重命名" aria-label="重命名"
              onClick={() => { setNameInput(project.name); setRenameError(""); setRenaming(true); }}>
              <Icon name="edit" />
            </button>
          </>
        }
      />

      {refreshingWorkspace && (
        <div className="muted small" style={{ marginTop: 18 }}>
          正在重新观察这个项目的工作目录与 Git 状态…
        </div>
      )}

      {/* 概览数字与类型 / 命名 / 创建时间都收在右栏「属性」块里：标题栏只放标题与图标动作。 */}
      <div className="project-detail-layout">
      <div>
      <section className="rail-section">
        <div className="section-label">任务</div>

        {detail.workstreams.length === 0 && (
          <div className="l1-none">还没有任务经由这些目录关联进来。</div>
        )}
        {/* 不再分「主关联 / 关联」两组：关联方式不改变归属，列表只按后端给的顺序平铺。 */}
        {detail.workstreams.map(({ workstream: w }) => {
          // 与 Workstream 卡片同一条规则：摘要取 Agent 的 current_state，退回用户描述。
          const summary = cardSummaryLine(w as unknown as WorkstreamCardData);
          return (
            <div key={w.id} className="list-row"
              onClick={() => navigate({ view: "workstream", workstreamId: w.id })}>
              <div className="grow">
                <div className="title" title={w.title}>{w.title}</div>
                {summary && <div className="meta">{summary}</div>}
              </div>
              <div className="side">
                {w.visibility === "archived" && <span className="badge warn" title="已移入回收站；项目与它只是投影关系">回收站</span>}
                {w.lifecycle === "completed" && <span className="badge">已完成</span>}
                <span>{timeAgo(w.updated_at)}</span>
              </div>
            </div>
          );
        })}
      </section>

      <section className="rail-section">
        <div className="section-label">会话</div>

        {sessions.length === 0 && (
          <div className="l1-none">这个项目下还没有会话。</div>
        )}
        {sessions.slice(0, showAllSessions ? undefined : 12).map((s) => (
          <div key={s.id} className="list-row" onClick={() => navigate({ view: "session", sessionId: s.id })}>
            <div className="grow">
              <div className="title" title={s.title ?? `${UNTITLED_SESSION} · ${s.root_agent_session_id}`}>
                {sessionDisplayTitle(s.title)}
              </div>
              <div className="meta mono" title={s.cwd ?? undefined}>
                {s.cwd ? cwdDisplayLabel(s.cwd, 56) : "没有记录到 cwd"}
              </div>
            </div>
            <div className="side">
              <span title={AGENT_LABELS[s.agent]}><AgentIcon agent={s.agent} /></span>
              <span>{timeAgo(s.last_activity_at ?? s.started_at)}</span>
            </div>
          </div>
        ))}
        {sessions.length > 12 && (
          <div className="small muted" style={{ marginTop: 6 }}>
            <button className="btn small ghost" onClick={() => setShowAllSessions(value => !value)}>{showAllSessions ? "收起" : `查看全部 ${sessions.length} 个会话`}</button>
          </div>
        )}
      </section>



      </div>
      <aside>
      <section className="rail-section" style={{ marginTop: 26 }}>
        <div className="section-label">属性</div>
        <div className="row" style={{ gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          <span className={`badge ${project.git_id ? "accent" : ""}`}>
            {project.git_id ? "Git 项目" : "目录项目"}
          </span>
          <span className="small muted">{project.name_customized ? "自定义名称" : "自动命名"}</span>
        </div>
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
        {detail.workspace_paths.length === 0 && (
          <div className="l1-none">
            这个项目已经不再拥有任何目录 —— 它会在下一次整理时自动消失。
          </div>
        )}
        {detail.workspace_paths.map((p) => (
          <div className="list-row" key={p.id} style={{ cursor: "default" }}>
            <div className="grow">
              {/* 不用 .title：它的 nowrap + 尾部省略会把路径末段吃掉，
                  而末段正是用户用来认目录的那一段。 */}
              <div style={{ minWidth: 0, overflowWrap: "anywhere" }}>
                <PathText path={p.canonical_path} max={72} />
              </div>
              <div className="meta" style={{ whiteSpace: "normal" }}>
                首次见到 {timeAgo(p.first_seen_at)} · 最近确认 {timeAgo(p.last_seen_at)}
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
              onKeyDown={(e) => e.key === "Enter" && !e.nativeEvent.isComposing && e.nativeEvent.keyCode !== 229 && commitRename()} /></label>
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
