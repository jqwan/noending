import Icon from "../../components/Icon";
import { useViewState, useViewScroll } from "../../hooks/useViewState";
import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import { showToast } from "../../components/Toast";
import { timeAgo, useRefreshSignal } from "../../components/common";
import { PathText } from "../workstreams/WorkspacePaths";
import type { Route } from "../../app/routes";
import type { ProjectCardData, ProjectKind } from "../../types";
import { projectKindLabels } from "./projectKind";

/**
 * Projects Board：Projects 是一等浏览页面——物理工作空间，由工作目录自动派生。
 * 数据来自**一次** `list_project_cards` 调用，不再是 1 + N 的 detail 读取。
 * 卡片强调「地方」（路径、可用性、任务 / 会话数、最近活动）；诊断信息（uuid、
 * git_id、内部 identity）属于 Detail。这里没有创建入口：Project 是派生的。
 */
type FilterKey = "all" | "ok" | "missing";
type ProjectKindFilter = "all" | ProjectKind;
type SortKey = "recent" | "name" | "paths";

const FILTERS: Record<FilterKey, (c: ProjectCardData) => boolean> = {
  all: () => true,
  // 正常 = 所有 WorkspacePath exists=true
  ok: (c) => c.missing_path_count === 0,
  // 有目录缺失 = 至少一个 WorkspacePath exists=false
  missing: (c) => c.missing_path_count > 0,
};

const SORTERS: Record<SortKey, (a: ProjectCardData, b: ProjectCardData) => number> = {
  recent: (a, b) =>
    (b.last_activity_at ?? b.updated_at).localeCompare(a.last_activity_at ?? a.updated_at),
  name: (a, b) => a.name.localeCompare(b.name, "zh-Hans"),
  paths: (a, b) => b.path_count - a.path_count || a.name.localeCompare(b.name, "zh-Hans"),
};

/** 搜索面：Project name + WorkspacePath canonical path（搜 search_paths 全部路径，
 *  不限于展示用的前两条）。 */
function cardMatches(c: ProjectCardData, q: string): boolean {
  const needle = q.trim().toLowerCase();
  if (needle === "") return true;
  return (
    c.name.toLowerCase().includes(needle) ||
    c.search_paths.some((p) => p.toLowerCase().includes(needle))
  );
}

let cachedProjectCards: ProjectCardData[] | null = null;

export function clearProjectCardsCache() {
  cachedProjectCards = null;
}

export default function ProjectsView({ navigate }: { navigate: (r: Route) => void }) {
  const [cards, setCards] = useState<ProjectCardData[] | null>(cachedProjectCards);
  const [listError, setListError] = useState("");
  const [query, setQuery] = useViewState("projects.query", "");
  const [filter, setFilter] = useViewState<FilterKey>("projects.filter", "all");
  const [kindFilter, setKindFilter] = useViewState<ProjectKindFilter>("projects.kindFilter", "all");
  const [sort, setSort] = useViewState<SortKey>("projects.sort", "recent");
  const [viewMode, setViewMode] = useViewState<"cards" | "list">("projects.viewMode", "cards");
  const [workspaceRefreshing, setWorkspaceRefreshing] = useState(false);

  const refresh = useCallback(() => {
    api
      .listProjectCards()
      .then((cs) => {
        cachedProjectCards = cs;
        setCards(cs);
        setListError("");
      })
      .catch((e) => {
        setCards(null);
        setListError(`读取项目失败：${String(e)}`);
      });
  }, []);
  useEffect(refresh, [refresh]);
  // 单次卡片查询足够便宜，refresh 信号不再需要防抖。
  useRefreshSignal(refresh);

  // 刷新在后台执行、事件回报：卡片整个期间保持可见，不清空、不进 Loading。
  // 这里只负责恢复按钮与提示——数据读取走 AppShell 的 EVT_SYNCED 失效信号，
  // 避免同一次刷新触发两次 listProjectCards。
  useEffect(() => {
    const unCompleted = listen("workspace-reconcile-completed", (e) => {
      setWorkspaceRefreshing(false);
      const p = e.payload as {
        missing?: number;
        deleted_paths?: number;
        deleted_projects?: number;
        discovered?: number;
      };
      const notes: string[] = [];
      if ((p.missing ?? 0) > 0) notes.push(`${p.missing} 个目录变为不可用`);
      if ((p.deleted_paths ?? 0) > 0) notes.push(`${p.deleted_paths} 个目录离开注册表`);
      if ((p.deleted_projects ?? 0) > 0) notes.push(`${p.deleted_projects} 个项目自动整理`);
      if ((p.discovered ?? 0) > 0) notes.push(`发现 ${p.discovered} 个新目录`);
      showToast(
        notes.length > 0 ? `工作区状态已刷新：${notes.join("，")}` : "工作区状态已刷新",
      );
    });
    const unFailed = listen("workspace-reconcile-failed", (e) => {
      setWorkspaceRefreshing(false);
      showToast(`工作区刷新失败：${String((e.payload as { error?: string }).error ?? "")}`);
    });
    return () => {
      unCompleted.then((f) => f());
      unFailed.then((f) => f());
    };
  }, [refresh]);

  const refreshWorkspace = useCallback(() => {
    setWorkspaceRefreshing(true);
    api
      .refreshWorkspaceProjects()
      .catch((e) => {
        setWorkspaceRefreshing(false);
        showToast(`工作区刷新失败：${String(e)}`);
      });
  }, []);

  const [addingProject, setAddingProject] = useState(false);

  const handleAddProject = async () => {
    if (addingProject) return;
    setAddingProject(true);
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "选择项目目录",
      });
      if (!picked) return;
      const pickedPath = Array.isArray(picked) ? picked[0] : picked;
      if (!pickedPath) return;

      const res = await api.addProjectPath(pickedPath);
      clearProjectCardsCache();
      refresh();
      showToast(`已添加到项目「${res.project_name}」`);
    } catch (e) {
      console.error(e);
      showToast(`添加项目失败：${String(e)}`);
    } finally {
      setAddingProject(false);
    }
  };

  const list = useMemo(() => {
    if (cards === null) return null;
    return cards
      .filter(FILTERS[filter])
      .filter((c) => kindFilter === "all" || c.kind === kindFilter)
      .filter((c) => cardMatches(c, query))
      .sort(SORTERS[sort]);
  }, [cards, query, filter, kindFilter, sort]);

  const scrollRef = useViewScroll("projects.scroll", cards !== null);

  return (
    <div className="main board-page" ref={scrollRef}>
      <PageHeader
        title="项目"
        actions={
          <>
            <button
              className="btn ghost icon-button"
              disabled={workspaceRefreshing}
              aria-label={workspaceRefreshing ? "正在刷新工作区状态" : "刷新工作区状态"}
              title="重新检查工作目录的存在性与 Git 状态，并执行已有的整理规则。"
              onClick={refreshWorkspace}
            >
              <Icon name="refresh" />
            </button>
            <button
              className="btn ghost icon-button"
              disabled={addingProject || workspaceRefreshing}
              aria-label="新增项目"
              title="新增项目"
              onClick={handleAddProject}
            >
              <Icon name="plus" />
            </button>
          </>
        }
      />

      <div className="board-toolbar">
      <input
        type="text"
        className="ws-search"
        aria-label="搜索项目"
        placeholder="搜索项目…（名称或工作目录）"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />

      <div className="toolbar ws-controls">
        <label className="ws-control">
          <span className="muted small">筛选</span>
          <select value={filter} onChange={(e) => setFilter(e.target.value as FilterKey)}>
            <option value="all">全部</option>
            <option value="ok">正常</option>
            <option value="missing">有目录缺失</option>
          </select>
        </label>
        <label className="ws-control">
          <span className="muted small">项目类型</span>
          <select
            aria-label="项目类型"
            value={kindFilter}
            onChange={(e) => setKindFilter(e.target.value as ProjectKindFilter)}
          >
            <option value="all">全部类型</option>
            {Object.entries(projectKindLabels).map(([kind, label]) => (
              <option key={kind} value={kind}>{label}</option>
            ))}
          </select>
        </label>
        <label className="ws-control">
          <span className="muted small">排序</span>
          <select value={sort} onChange={(e) => setSort(e.target.value as SortKey)}>
            <option value="recent">最近活动 ↓</option>
            <option value="name">名称</option>
            <option value="paths">目录数</option>
          </select>
        </label>
        {(query || filter !== "all" || kindFilter !== "all") && <button className="btn small ghost" onClick={() => { setQuery(""); setFilter("all"); setKindFilter("all"); }}>清除筛选</button>}
        {list && <span className="muted small">{list.length} 个项目</span>}
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

      {list === null && listError === "" && (
        viewMode === "cards" ? (
          <div className="board-grid" role="status">
            <div className="skeleton card" style={{ minHeight: 140 }} />
            <div className="skeleton card" style={{ minHeight: 140 }} />
            <div className="skeleton card" style={{ minHeight: 140 }} />
          </div>
        ) : (
          <div className="project-list" aria-busy="true">
            {Array.from({ length: 4 }).map((_, i) => (
              <div key={i} className="skeleton" style={{ height: 64, borderRadius: "var(--radius-md)", marginBottom: 8 }} />
            ))}
          </div>
        )
      )}
      {listError !== "" && (
        <div className="card hairline" style={{ padding: 14 }}>
          <div className="small" style={{ color: "var(--danger)" }}>{listError}</div>
          <button className="btn small" style={{ marginTop: 8 }} onClick={refresh}>重试</button>
        </div>
      )}
      {list !== null && list.length === 0 && cards !== null && cards.length > 0 && (
        <EmptyState
          title="没有匹配的项目"
          hint="换个关键词，或把筛选切回「全部」。"
        />
      )}
      {cards !== null && cards.length === 0 && (
        <EmptyState
          title="还没有项目"
          hint="打开会话或给任务添加工作路径后，项目会自动出现在这里。"
        />
      )}

      {viewMode === "cards" ? (
        <div className="ws-grid board-grid">
          {list?.map((c) => <ProjectCard key={c.id} card={c} navigate={navigate} />)}
        </div>
      ) : (
        <div className="project-list">
          {list?.map((c) => (
            <article className="project-list-row" key={`list-${c.id}`}>
              <button
                type="button"
                className="project-open"
                onClick={() => navigate({ view: "project", projectId: c.id })}
              >
                <div className="project-list-title" title={c.name}>
                  <Icon name="folder" />
                  <span>{c.name}</span>
                  <span className="badge">{projectKindLabels[c.kind]}</span>
                  {c.missing_path_count > 0 && (
                    <span className="badge warn">{c.missing_path_count} 个目录缺失</span>
                  )}
                </div>
                <div className="project-list-meta">
                  <span>{c.path_count} 目录</span>
                  <span>{c.workstream_count} 任务</span>
                  <span>{c.session_count} 会话</span>
                  <span title={c.last_activity_at ?? undefined}>{timeAgo(c.last_activity_at)}</span>
                </div>
                {c.representative_paths.length > 0 && (
                  <div className="project-list-path mono" title={c.representative_paths[0]}>
                    {c.representative_paths[0]}
                    {c.path_count > 1 && (
                      <span className="muted" style={{ marginLeft: 6 }}>
                        另有 {c.path_count - 1} 个目录
                      </span>
                    )}
                  </div>
                )}
              </button>
            </article>
          ))}
        </div>
      )}
    </div>
  );
}

/** 信息密度受控的卡片：名字、可用性、两条代表路径、计数、最近活动。 */
function ProjectCard({ card, navigate }: {
  card: ProjectCardData;
  navigate: (r: Route) => void;
}) {
  const restPaths = Math.max(0, card.path_count - 1);
  return (
    <button type="button"
      className="ws-card full project-card"
      onClick={() => navigate({ view: "project", projectId: card.id })}
    >
      <div className="ws-card-head">
        <div className="ws-card-title" title={card.name}><Icon name="folder" />{card.name}</div>
        <div className="ws-card-side">
          <span className="badge">{projectKindLabels[card.kind]}</span>
          {card.missing_path_count > 0 && (
            <span className="badge warn">{card.missing_path_count} 个目录缺失</span>
          )}
        </div>
      </div>

      <div className="project-card-path">
        {card.representative_paths.slice(0, 1).map((p) => (
          <div key={p} className="small" style={{ marginBottom: 2 }}>
            <PathText path={p} interactive={false} />
          </div>
        ))}
        {restPaths > 0 && <div className="muted small">另有 {restPaths} 个目录</div>}
      </div>

      <div className="board-stats">
        <span title="工作目录"><Icon name="folder" />{card.path_count}<span>目录</span></span>
        <span title="与本项目有工作路径关联的任务"><Icon name="tasks" />{card.workstream_count}<span>任务</span></span>
        <span title="会话"><Icon name="chat" />{card.session_count}<span>会话</span></span>
      </div>
      <div className="ws-card-meta muted small">{timeAgo(card.last_activity_at)}</div>
    </button>
  );
}
