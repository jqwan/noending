import { useViewState, useViewScroll } from "../../hooks/useViewState";
import { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import Icon from "../../components/Icon";
import { Modal, timeAgo } from "../../components/common";
import { showToast } from "../../components/Toast";
import WorkstreamCard, { cardSearchFields, cardSummaryLine, searchFieldHint } from "./WorkstreamCard";
import WorkstreamFormModal from "./WorkstreamFormModal";
import { useWorkstreamCards } from "./useWorkstreamCards";
import { AGENT_LABELS, type WorkstreamCardData } from "../../types";
import type { Route, ViewAction, WorkstreamScope } from "../../app/routes";

type SortKey = "recent" | "created" | "name";

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
  const [query, setQuery] = useViewState("workstreams.query", "");
  const [sort, setSort] = useViewState<SortKey>("workstreams.sort", "recent");
  const [projectId, setProjectId] = useViewState("workstreams.projectId", "all");
  const [viewMode, setViewMode] = useViewState<"cards" | "list">("workstreams.viewMode", "cards");
  const [creatingWs, setCreatingWs] = useState(false);
  const archivedMode = scope === "archived";
  const [bulkPurgeOpen, setBulkPurgeOpen] = useState(false);
  const [bulkPurgeBusy, setBulkPurgeBusy] = useState(false);
  const [taskActionId, setTaskActionId] = useState<string | null>(null);
  const [purgeTarget, setPurgeTarget] = useState<WorkstreamCardData | null>(null);

  // 页面动作随 Route 到达（palette → New Workstream）：actionSeq 让「已在 Workstreams 页」
  // 的重复命令同样触发。
  useEffect(() => {
    if (action === "new") setCreatingWs(true);
  }, [action, actionSeq]);

  const projectOptions = useMemo(() => {
    const projects = new Map<string, string>();
    for (const card of cards ?? []) {
      for (const project of card.projects) projects.set(project.id, project.name);
    }
    return [...projects.entries()].sort((a, b) => a[1].localeCompare(b[1], "zh-Hans"));
  }, [cards]);

  const filtersActive =
    query.trim() !== "" || projectId !== "all";

  const list = useMemo(() => {
    if (!cards) return null;
    const q = query.trim().toLowerCase();
    return cards
      .filter((c) => archivedMode ? c.visibility === "archived" : c.visibility === "normal")
      .filter((c) => projectId === "all" || (projectId === "none" ? c.projects.length === 0 : c.projects.some((p) => p.id === projectId)))
      .filter((c) =>
        q === ""
          ? true
          : cardSearchFields(c)
              .filter(Boolean)
              .some((s) => s!.toLowerCase().includes(q)),
      )
      .sort(SORTERS[sort]);
  }, [cards, query, sort, projectId, archivedMode]);

  const archivedTasks = cards?.filter((c) => c.visibility === "archived") ?? [];

  const archiveTask = async (task: WorkstreamCardData) => {
    if (taskActionId) return;
    setTaskActionId(task.id);
    try {
      await api.archiveWorkstream(task.id);
      showToast(`已归档「${task.title}」`);
      refresh();
    } catch (e) { showToast(`归档失败：${String(e)}`); }
    finally { setTaskActionId(null); }
  };

  const bulkPurge = async () => {
    if (!archivedMode || archivedTasks.length === 0 || bulkPurgeBusy) return;
    const targets = [...archivedTasks];
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
        : `已永久删除 ${deleted} 个任务，${failed} 个仍保留在已归档`,
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
    setProjectId("all");
  };

  const scrollRef = useViewScroll("workstreams.scroll", cards !== null);

  return (
    <div className="main board-page" ref={scrollRef}>
      <PageHeader
        title="任务"
        actions={
          <>
            <div className="settings-seg archive-scope" role="group" aria-label="任务列表范围">
              <button
                className={archivedMode ? "" : "on"}
                aria-pressed={!archivedMode}
                aria-label="未归档"
                title="未归档"
                onClick={() => navigate({ view: "workstreams", scope: "unarchived" })}
              >
                <Icon name="tasks" /> 未归档
              </button>
              <button
                className={archivedMode ? "on" : ""}
                aria-pressed={archivedMode}
                aria-label="已归档"
                title="已归档"
                onClick={() => navigate({ view: "workstreams", scope: "archived" })}
              >
                <Icon name="archive" /> 已归档
              </button>
            </div>
            {archivedMode ? <button className="btn ghost danger" aria-label="删除全部" title="删除全部已归档任务" disabled={archivedTasks.length === 0 || bulkPurgeBusy || Boolean(taskActionId)} onClick={() => setBulkPurgeOpen(true)}><Icon name="trash" /> 删除全部</button> : <button className="btn ghost icon-button" aria-label="新建任务" title="新建任务" onClick={() => setCreatingWs(true)}>
              <Icon name="plus" />
            </button>}
          </>
        }
      />

      <>
          <div className="board-toolbar">
          <input
            type="text"
            className="ws-search"
            aria-label="搜索任务"
            placeholder={searchFieldHint()}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />

          <div className="toolbar ws-controls">
            <label className="ws-control">
              <span className="muted small">项目</span>
              <select value={projectId} onChange={(e) => setProjectId(e.target.value)}>
                <option value="all">全部项目</option>
                {projectOptions.map(([id, name]) => <option key={id} value={id}>{name}</option>)}
                <option value="none">无项目</option>
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
                {filtersActive ? `显示 ${list.length} / 共 ${cards?.filter((c) => c.visibility === (archivedMode ? "archived" : "normal")).length ?? 0} 个` : `共 ${list.length} 个任务`}
              </span>
            )}
            <div className="settings-seg icon-seg" role="group" aria-label="展示方式">
              <button
                className={viewMode === "cards" ? "on" : ""}
                aria-pressed={viewMode === "cards"}
                aria-label="卡片视图"
                title="卡片视图"
                onClick={() => setViewMode("cards")}
              >
                <Icon name="grid" />
              </button>
              <button
                className={viewMode === "list" ? "on" : ""}
                aria-pressed={viewMode === "list"}
                aria-label="列表视图"
                title="列表视图"
                onClick={() => setViewMode("list")}
              >
                <Icon name="list" />
              </button>
            </div>
          </div>

          </div>

          {loadError && <div role="alert">读取任务失败 <button className="btn small" onClick={refresh}>重试</button></div>}
          {list === null && !loadError && (
            viewMode === "cards" ? (
              <div className="ws-grid board-grid" role="status">
                <div className="skeleton card" style={{ minHeight: 140 }} />
                <div className="skeleton card" style={{ minHeight: 140 }} />
                <div className="skeleton card" style={{ minHeight: 140 }} />
              </div>
            ) : (
              <div className="task-list" aria-busy="true">
                {Array.from({ length: 4 }).map((_, i) => (
                  <div key={i} className="skeleton" style={{ height: 64, borderRadius: "var(--radius-md)", marginBottom: 8 }} />
                ))}
              </div>
            )
          )}
          {list !== null && list.length === 0 && (
            <EmptyState
              title={
                query
                  ? "没有匹配的任务。"
                  : !filtersActive
                    ? (archivedMode ? "还没有已归档的任务。" : "还没有未归档的任务。")
                    : "没有符合当前筛选条件的任务。"
              }
              hint={
                filtersActive
                  ? "调整或清除筛选条件即可看到全部任务。"
                  : (archivedMode ? "归档的任务会显示在这里，可以取消归档或永久删除。" : "创建任务，开始工作。")
              }
              actions={
                filtersActive ? (
                  <button className="btn small" onClick={clearFilters}>清除筛选</button>
                ) : !archivedMode ? (
                  <button className="btn small" onClick={() => setCreatingWs(true)}>+ 新建任务</button>
                ) : undefined
              }
            />
          )}

          {viewMode === "cards" ? (
            <div className="ws-grid board-grid">
              {list?.map((c) => (
                <WorkstreamCard key={c.id} card={c} mode="full" navigate={navigate} defaultAgent={defaultAgent} onArchive={archiveTask} onRestore={restoreTask} onDelete={setPurgeTarget} busy={Boolean(taskActionId) || bulkPurgeBusy} />
              ))}
            </div>
          ) : (
            <div className="task-list">
              {list?.map((c) => {
                const summary = cardSummaryLine(c);
                return (
                  <article className="task-list-row" key={`list-${c.id}`}>
                    <button
                      type="button"
                      className="task-open"
                      onClick={() => navigate({ view: "workstream", workstreamId: c.id })}
                    >
                      <div className="task-list-title" title={c.title}>
                        <Icon name="tasks" />
                        <span>{c.title}</span>
                        {c.visibility === "archived" && <span className="badge warn">已归档</span>}
                      </div>
                      <div className="task-list-meta">
                        {c.projects.map((p) => (
                          <span key={p.id} title={p.name} className="task-list-project">
                            <Icon name="folder" />
                            <span>{p.name}</span>
                          </span>
                        ))}
                        <span>{c.session_count === 0 ? "暂无会话" : `${c.session_count} 个会话`}</span>
                        <span title={c.last_activity_at ?? c.updated_at}>
                          {timeAgo(c.last_activity_at ?? c.updated_at)}
                        </span>
                      </div>
                      {summary && (
                        <div className="task-list-summary" title={summary}>
                          {summary}
                        </div>
                      )}
                    </button>
                    {(
                      <div className="task-list-actions" onClick={(e) => e.stopPropagation()}>
                        {c.visibility !== "archived" && <button
                          className="btn small ws-btn ghost icon-button"
                          aria-label="新建会话"
                          title={defaultAgent ? `用 ${AGENT_LABELS[defaultAgent]} 新建会话` : "新建会话"}
                          onClick={() => navigate({ view: "new-session", workstreamId: c.id })}
                        >
                          <Icon name="plus" />
                        </button>}
                        {c.visibility === "archived" ? <>
                          <button className="btn small ghost icon-button" aria-label={`取消归档${c.title}`} title="取消归档" disabled={Boolean(taskActionId) || bulkPurgeBusy} onClick={() => void restoreTask(c)}><Icon name="unarchive" /></button>
                          <button className="btn small ghost icon-button danger" aria-label={`永久删除${c.title}`} title="永久删除" disabled={Boolean(taskActionId) || bulkPurgeBusy} onClick={() => setPurgeTarget(c)}><Icon name="trash" /></button>
                        </> : <button className="btn small ghost icon-button" aria-label={`归档${c.title}`} title="归档" disabled={Boolean(taskActionId) || bulkPurgeBusy} onClick={() => void archiveTask(c)}><Icon name="archive" /></button>}
                      </div>
                    )}
                  </article>
                );
              })}
            </div>
          )}
      </>

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

      {archivedMode && bulkPurgeOpen && archivedTasks.length > 0 && (
        <Modal
          title="删除全部"
          onClose={() => { if (!bulkPurgeBusy) setBulkPurgeOpen(false); }}
        >
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            确定要永久删除已归档的 <b>{archivedTasks.length} 个任务</b> 吗？
          </p>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
            此操作不可撤销，任务的 Context、审阅状态和关联数据将被清理。
          </div>
          <p className="small" style={{ margin: "0 0 14px", maxWidth: "72ch" }}>
            工作路径、项目、会话及会话事件历史会保留。某个任务删除失败时，其他任务仍会继续处理，失败的任务会留在已归档。
          </p>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setBulkPurgeOpen(false)} disabled={bulkPurgeBusy}>取消</button>
            <button className="btn danger" onClick={bulkPurge} disabled={bulkPurgeBusy}>
              {bulkPurgeBusy ? "删除中…" : "删除全部"}
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
