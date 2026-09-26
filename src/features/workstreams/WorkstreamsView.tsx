import { useViewState, useViewScroll } from "../../hooks/useViewState";
import { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import Icon from "../../components/Icon";
import { Modal, timeAgo } from "../../components/common";
import { showToast } from "../../components/Toast";
import WorkstreamCard, { cardSearchFields, searchFieldHint } from "./WorkstreamCard";
import WorkstreamFormModal from "./WorkstreamFormModal";
import { useWorkstreamCards } from "./useWorkstreamCards";
import { useBaseExperience } from "../../app/experience";
import type { WorkstreamCardData } from "../../types";
import type { Route, ViewAction, WorkstreamScope } from "../../app/routes";

type SortKey = "recent" | "created" | "name";
/**
 * 筛选按**两个正交维度**表达：lifecycle（进行中 / 已完成）与
 * visibility（normal / archived）。回收站就是 archived —— 它不是第三种状态，
 * 也不改变 lifecycle，所以「回收站」只能按 visibility 判，
 * 不能像旧代码那样把「非进行中」统统算成已归档。
 */
type LifecycleFilter = "all" | "active" | "completed";
type PresenceFilter = "all" | "assigned" | "unassigned";

const SORTERS: Record<SortKey, (a: WorkstreamCardData, b: WorkstreamCardData) => number> = {
  recent: (a, b) =>
    (b.last_activity_at ?? b.updated_at).localeCompare(a.last_activity_at ?? a.updated_at),
  created: (a, b) => b.created_at.localeCompare(a.created_at),
  name: (a, b) => a.title.localeCompare(b.title, "zh-Hans"),
};

/** Workstreams = Organize：浏览、搜索、排序、整理（整体设计）。 */
export default function WorkstreamsView({ navigate, action, scope, actionSeq }: {
  navigate: (r: Route) => void;
  action?: ViewAction;
  scope?: WorkstreamScope;
  actionSeq: number;
}) {
  const { cards, defaultAgent, refresh, loadError } = useWorkstreamCards();
  const { intelligenceEnabled } = useBaseExperience();
  const [query, setQuery] = useViewState("workstreams.query", "");
  const [sort, setSort] = useViewState<SortKey>("workstreams.sort", "recent");
  const [lifecycle, setLifecycle] = useViewState<LifecycleFilter>("workstreams.lifecycle", "active");
  const [projectId, setProjectId] = useViewState("workstreams.projectId", "all");
  const [sessionFilter, setSessionFilter] = useViewState<PresenceFilter>("workstreams.sessionFilter", "all");
  const [pathFilter, setPathFilter] = useViewState<PresenceFilter>("workstreams.pathFilter", "all");
  const [creatingWs, setCreatingWs] = useState(false);
  const trashMode = scope === "trash";
  const [bulkPurgeOpen, setBulkPurgeOpen] = useState(false);
  const [bulkPurgeBusy, setBulkPurgeBusy] = useState(false);
  const [taskActionId, setTaskActionId] = useState<string | null>(null);
  const [purgeTarget, setPurgeTarget] = useState<WorkstreamCardData | null>(null);

  // 页面动作随 Route 到达（palette → New Workstream）：
  // actionSeq 让「已在 Workstreams 页」的重复命令同样触发。
  useEffect(() => {
    if (action === "new") setCreatingWs(true);
  }, [action, actionSeq]);

  const projectOptions = useMemo(() => {
    const projects = new Map<string, string>();
    for (const card of cards ?? []) {
      if (card.project_id) projects.set(card.project_id, card.project_name ?? "未命名项目");
    }
    return [...projects.entries()].sort((a, b) => a[1].localeCompare(b[1], "zh-Hans"));
  }, [cards]);

  const filtersActive =
    query.trim() !== "" || lifecycle !== "active" || projectId !== "all"
    || sessionFilter !== "all" || pathFilter !== "all";

  const list = useMemo(() => {
    if (!cards) return null;
    const q = trashMode ? "" : query.trim().toLowerCase();
    return cards
      .filter((c) => trashMode ? c.visibility === "archived" : c.visibility === "normal")
      .filter((c) => trashMode || lifecycle === "all" || c.lifecycle === lifecycle)
      .filter((c) => trashMode || projectId === "all" || (projectId === "none" ? c.project_id === null : c.project_id === projectId))
      .filter((c) => trashMode || sessionFilter === "all" || (sessionFilter === "assigned" ? c.session_count > 0 : c.session_count === 0))
      .filter((c) => trashMode || pathFilter === "all" || (pathFilter === "assigned" ? c.path_count > 0 : c.path_count === 0))
      .filter((c) =>
        q === ""
          ? true
          : cardSearchFields(c, intelligenceEnabled)
              .filter(Boolean)
              .some((s) => s!.toLowerCase().includes(q)),
      )
      .sort(SORTERS[sort]);
  }, [cards, query, sort, lifecycle, projectId, sessionFilter, pathFilter, trashMode, intelligenceEnabled]);

  const bulkPurge = async () => {
    if (!trashMode || !list || list.length === 0 || bulkPurgeBusy) return;
    const targets = [...list];
    let deleted = 0;
    let failed = 0;
    setBulkPurgeBusy(true);
    for (const task of targets) {
      try {
        await api.deleteWorkstreamPermanently(task.id);
        deleted += 1;
      } catch (e) {
        failed += 1;
        console.error(`永久删除任务失败：${task.id}`, e);
      }
    }
    setBulkPurgeBusy(false);
    setBulkPurgeOpen(false);
    refresh();
    showToast(
      failed === 0
        ? `已永久删除 ${deleted} 个任务`
        : `已永久删除 ${deleted} 个任务，${failed} 个仍保留在回收站`,
    );
  };

  const restoreTask = async (task: WorkstreamCardData) => {
    if (taskActionId) return;
    setTaskActionId(task.id);
    try {
      await api.restoreWorkstream(task.id);
      showToast(`已恢复「${task.title}」`);
      refresh();
    } catch (e) {
      showToast(`恢复任务失败：${String(e)}`);
    } finally {
      setTaskActionId(null);
    }
  };

  const purgeTask = async () => {
    if (!purgeTarget || taskActionId) return;
    const task = purgeTarget;
    setTaskActionId(task.id);
    try {
      await api.deleteWorkstreamPermanently(task.id);
      setPurgeTarget(null);
      showToast(`已永久删除「${task.title}」`);
      refresh();
    } catch (e) {
      showToast(`永久删除任务失败：${String(e)}`);
    } finally {
      setTaskActionId(null);
    }
  };

  const clearFilters = () => {
    setQuery("");
    setLifecycle("active");
    setProjectId("all");
    setSessionFilter("all");
    setPathFilter("all");
  };

  const scrollRef = useViewScroll("workstreams.scroll", cards !== null);

  return (
    <div className="main board-page" ref={scrollRef}>
      <PageHeader
        title="任务"
        actions={
          <>
            <div className="settings-seg" role="group" aria-label="任务列表范围">
              <button
                className={trashMode ? "" : "on"}
                aria-pressed={!trashMode}
                aria-label="任务列表"
                title="任务列表"
                onClick={() => navigate({ view: "workstreams", scope: "active" })}
              >
                <Icon name="tasks" />
              </button>
              <button
                className={trashMode ? "on" : ""}
                aria-pressed={trashMode}
                aria-label="回收站"
                title="回收站"
                onClick={() => navigate({ view: "workstreams", scope: "trash" })}
              >
                <Icon name="archive" />
              </button>
            </div>
            <button className="btn ghost icon-button" aria-label="新建任务" title="新建任务" onClick={() => setCreatingWs(true)}>
              <Icon name="plus" />
            </button>
          </>
        }
      />

      {!trashMode && (
        <>
          <div className="board-toolbar">
          <input
            type="text"
            className="ws-search"
            aria-label="搜索任务"
            placeholder={searchFieldHint(intelligenceEnabled)}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />

          <div className="toolbar ws-controls">
            <label className="ws-control">
              <span className="muted small">状态</span>
              <select value={lifecycle} onChange={(e) => setLifecycle(e.target.value as LifecycleFilter)}>
                <option value="all">全部状态</option>
                <option value="active">进行中</option>
                <option value="completed">已完成</option>
              </select>
            </label>
            <label className="ws-control">
              <span className="muted small">项目</span>
              <select value={projectId} onChange={(e) => setProjectId(e.target.value)}>
                <option value="all">全部项目</option>
                {projectOptions.map(([id, name]) => <option key={id} value={id}>{name}</option>)}
                <option value="none">无项目</option>
              </select>
            </label>
            <label className="ws-control">
              <span className="muted small">会话关联</span>
              <select value={sessionFilter} onChange={(e) => setSessionFilter(e.target.value as PresenceFilter)}>
                <option value="all">全部</option>
                <option value="assigned">有会话</option>
                <option value="unassigned">无会话</option>
              </select>
            </label>
            <label className="ws-control">
              <span className="muted small">工作路径</span>
              <select value={pathFilter} onChange={(e) => setPathFilter(e.target.value as PresenceFilter)}>
                <option value="all">全部</option>
                <option value="assigned">有工作路径</option>
                <option value="unassigned">无工作路径</option>
              </select>
            </label>
            <label className="ws-control">
              <span className="muted small">排序</span>
              <select value={sort} onChange={(e) => setSort(e.target.value as SortKey)}>
                <option value="recent">最近活动 ↓</option>
                <option value="created">创建时间</option>
                <option value="name">名称</option>
              </select>
            </label>
            {filtersActive && <button className="btn small ghost" onClick={clearFilters}>清除筛选</button>}
            {list && (
              <span className="muted small">
                {filtersActive ? `显示 ${list.length} / 共 ${cards?.filter((c) => c.visibility === "normal").length ?? 0} 个` : `共 ${list.length} 个任务`}
              </span>
            )}
          </div>

          </div>

          {loadError && <div role="alert">读取任务失败 <button className="btn small" onClick={refresh}>重试</button></div>}
          {list === null && !loadError && <div className="muted" role="status">加载中…</div>}
          {list !== null && list.length === 0 && (
            <EmptyState
              title={
                query
                  ? "没有匹配的任务。"
                  : !filtersActive
                    ? "还没有进行中的任务。"
                    : "没有符合当前筛选条件的任务。"
              }
              hint={
                filtersActive
                  ? "调整或清除筛选条件即可看到全部任务。"
                  : "创建任务，开始工作。"
              }
              actions={
                filtersActive ? (
                  <button className="btn small" onClick={clearFilters}>清除筛选</button>
                ) : (
                  <button className="btn small" onClick={() => setCreatingWs(true)}>+ 新建任务</button>
                )
              }
            />
          )}

          <div className="ws-grid board-grid">
            {list?.map((c) => (
              <WorkstreamCard key={c.id} card={c} mode="full" navigate={navigate} defaultAgent={defaultAgent} />
            ))}
          </div>
        </>
      )}

      {trashMode && (
        <>
          <div className="muted small" style={{ margin: "4px 0 14px", maxWidth: "72ch" }}>
            回收站里的任务不出现在任务列表、首页以及新建和继续入口中。工作路径、会话
            归属与 Context 都原样保留；打开任务详情后可以恢复或永久删除。
          </div>
          {loadError && <div role="alert">读取任务失败 <button className="btn small" onClick={refresh}>重试</button></div>}
          {list === null && !loadError && <div className="muted" role="status">加载中…</div>}
          {list !== null && list.length === 0 && (
            <EmptyState
              title="回收站是空的。"
              hint="移入回收站的任务会留在这里，可以随时恢复，也可以在任务详情中永久删除。"
            />
          )}
          {list !== null && list.length > 0 && (
            <>
              <div className="session-trash-actions">
                <span className="muted small">共 {list.length} 个任务</span>
                <button className="btn small danger" onClick={() => setBulkPurgeOpen(true)}>
                  全部永久删除
                </button>
              </div>
              <TrashWorkstreamTable
                tasks={list}
                onOpen={(id) => navigate({ view: "workstream", workstreamId: id })}
                onRestore={restoreTask}
                onPurge={setPurgeTarget}
                busyId={taskActionId}
              />
            </>
          )}
        </>
      )}

      {purgeTarget && (
        <Modal title="永久删除这项任务？" onClose={() => { if (!taskActionId) setPurgeTarget(null); }}>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>不可撤销</div>
          <p style={{ margin: "0 0 12px", maxWidth: "72ch" }}>
            确定要永久删除 <b>{purgeTarget.title}</b> 吗？任务本身、工作路径列表、Context 和审阅数据会被删除，关联会话及其事件历史会保留。
          </p>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setPurgeTarget(null)} disabled={Boolean(taskActionId)}>取消</button>
            <button className="btn danger" onClick={purgeTask} disabled={Boolean(taskActionId)}>
              {taskActionId ? "删除中…" : "永久删除"}
            </button>
          </div>
        </Modal>
      )}

      {trashMode && bulkPurgeOpen && list && list.length > 0 && (
        <Modal
          title="全部永久删除"
          onClose={() => { if (!bulkPurgeBusy) setBulkPurgeOpen(false); }}
        >
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            确定要永久删除回收站中的 <b>{list.length} 个任务</b> 吗？
          </p>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
            此操作不可撤销，任务的 Context、审阅状态和关联数据将被清理。
          </div>
          <p className="small" style={{ margin: "0 0 14px", maxWidth: "72ch" }}>
            工作路径、项目、会话及会话事件历史会保留。某个任务删除失败时，其他任务仍会继续处理，失败的任务会留在回收站。
          </p>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setBulkPurgeOpen(false)} disabled={bulkPurgeBusy}>取消</button>
            <button className="btn danger" onClick={bulkPurge} disabled={bulkPurgeBusy}>
              {bulkPurgeBusy ? "删除中…" : "全部永久删除"}
            </button>
          </div>
        </Modal>
      )}

      {creatingWs && (
        <WorkstreamFormModal
          onClose={() => setCreatingWs(false)}
          onCreated={(w) => navigate({ view: "workstream", workstreamId: w.id })}
        />
      )}
    </div>
  );
}

function TrashWorkstreamTable({ tasks, onOpen, onRestore, onPurge, busyId }: {
  tasks: WorkstreamCardData[];
  onOpen: (workstreamId: string) => void;
  onRestore: (task: WorkstreamCardData) => void;
  onPurge: (task: WorkstreamCardData) => void;
  busyId: string | null;
}) {
  const rows = [...tasks].sort((a, b) => b.updated_at.localeCompare(a.updated_at));

  return (
    <table className="session-table workstream-trash-table">
      <thead>
        <tr>
          <th>任务</th>
          <th style={{ width: 170 }}>项目</th>
          <th style={{ width: 80 }}>会话</th>
          <th>主工作路径</th>
          <th style={{ width: 160 }}>最近更新</th>
          <th style={{ width: 170 }}>操作</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((task) => (
          <tr key={task.id} onClick={() => onOpen(task.id)}>
            <td className="cell-title" title={task.title}>
              <div style={{ whiteSpace: "normal" }}>{task.title}</div>
              {task.description && (
                <div className="muted small" style={{ whiteSpace: "normal" }}>{task.description}</div>
              )}
            </td>
            <td title={task.project_name ?? undefined}>{task.project_name ?? "未归属项目"}</td>
            <td>{task.session_count}</td>
            <td className={task.primary_path ? "mono" : "muted"} title={task.primary_path ?? undefined}>
              {task.primary_path ?? "未设置"}
            </td>
            <td>{timeAgo(task.updated_at)}</td>
            <td onClick={(e) => e.stopPropagation()}>
              <div className="row" style={{ gap: 6 }}>
                <button className="btn small" onClick={() => onRestore(task)} disabled={Boolean(busyId)}>
                  恢复
                </button>
                <button className="btn small danger" onClick={() => onPurge(task)} disabled={Boolean(busyId)}>
                  永久删除
                </button>
              </div>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
