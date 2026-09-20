import React, { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import { showToast } from "../../components/Toast";
import { timeAgo, useRefreshSignal } from "../../components/common";
import { PathText } from "../workstreams/WorkspacePaths";
import type { Route } from "../../app/routes";
import type { ProjectCardData } from "../../types";

/**
 * Projects Board（Projects Experience v0.2 §3-§7、§29）：Projects 是一等浏览
 * 页面——物理工作空间，由工作目录自动派生。数据来自**一次** `list_project_cards`
 * 调用（§8），不再是 1 + N 的 detail 读取。
 *
 * 卡片强调「地方」：物理路径、可用性、Workstream / Session 数量、最近活动
 * （§20）。诊断信息（uuid、git_id、内部 identity）属于 Detail，不上卡片。
 * 这里没有创建入口：Project 是派生的，Refresh 也不是编辑（§25）。
 */
type FilterKey = "all" | "ok" | "missing";
type SortKey = "recent" | "name" | "paths";

const FILTERS: Record<FilterKey, (c: ProjectCardData) => boolean> = {
  all: () => true,
  // §6: 正常 = 所有 WorkspacePath exists=true
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

/** §5 — 搜索面：Project name + WorkspacePath canonical path。
 *  Review P2-1：搜的是 search_paths（全部路径），不限于展示用的前两条。 */
function cardMatches(c: ProjectCardData, q: string): boolean {
  const needle = q.trim().toLowerCase();
  if (needle === "") return true;
  return (
    c.name.toLowerCase().includes(needle) ||
    c.search_paths.some((p) => p.toLowerCase().includes(needle))
  );
}

export default function ProjectsView({ navigate }: { navigate: (r: Route) => void }) {
  const [cards, setCards] = useState<ProjectCardData[] | null>(null);
  const [listError, setListError] = useState("");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<FilterKey>("all");
  const [sort, setSort] = useState<SortKey>("recent");
  const [workspaceRefreshing, setWorkspaceRefreshing] = useState(false);

  const refresh = useCallback(() => {
    api
      .listProjectCards()
      .then((cs) => {
        setCards(cs);
        setListError("");
      })
      .catch((e) => {
        setCards(null);
        setListError(`读取 Projects 失败：${String(e)}`);
      });
  }, []);
  useEffect(refresh, [refresh]);
  // 单次卡片查询足够便宜，refresh 信号不再需要防抖。
  useRefreshSignal(refresh);

  // §12/§13 — 刷新在后台执行，事件回报。卡片在整个期间保持可见，
  // 不清空页面、不进 Loading。Review P2-2：这里只负责恢复按钮与提示——
  // 数据读取统一走 AppShell 的 EVT_SYNCED 失效信号，避免同一次刷新触发
  // 两次 listProjectCards。
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
      if ((p.deleted_projects ?? 0) > 0) notes.push(`${p.deleted_projects} 个 Project 自动整理`);
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

  const list = useMemo(() => {
    if (cards === null) return null;
    return cards
      .filter(FILTERS[filter])
      .filter((c) => cardMatches(c, query))
      .sort(SORTERS[sort]);
  }, [cards, query, filter, sort]);

  return (
    <div className="main">
      <PageHeader
        title="Projects"
        sub="NoEnding 根据 Session 和 Workstream 使用的工作目录自动整理这些工作空间。"
        actions={
          <button
            className={`btn ${workspaceRefreshing ? "" : "primary"}`}
            disabled={workspaceRefreshing}
            title="重新检查工作目录的存在性与 Git 状态，并执行已有的整理规则。"
            onClick={refreshWorkspace}
          >
            {workspaceRefreshing ? "正在刷新工作区状态…" : "刷新工作区状态"}
          </button>
        }
      />

      <input
        type="text"
        className="ws-search"
        placeholder="搜索 Projects…（名称或工作目录）"
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
          <span className="muted small">排序</span>
          <select value={sort} onChange={(e) => setSort(e.target.value as SortKey)}>
            <option value="recent">最近活动 ↓</option>
            <option value="name">名称</option>
            <option value="paths">目录数</option>
          </select>
        </label>
        {list && <span className="muted small">{list.length} 个 Project</span>}
      </div>

      {list === null && listError === "" && <div className="muted">加载中…</div>}
      {listError !== "" && (
        <div className="card hairline" style={{ padding: 14 }}>
          <div className="small" style={{ color: "var(--danger)" }}>{listError}</div>
          <button className="btn small" style={{ marginTop: 8 }} onClick={refresh}>重试</button>
        </div>
      )}
      {list !== null && list.length === 0 && cards !== null && cards.length > 0 && (
        <EmptyState
          title="没有匹配的 Project"
          hint="换个关键词，或把筛选切回「全部」。"
        />
      )}
      {cards !== null && cards.length === 0 && (
        <EmptyState
          title="还没有 Project"
          hint="打开 Session 或给 Workstream 添加工作路径后，Project 会自动出现在这里。"
        />
      )}

      <div className="ws-grid">
        {list?.map((c) => <ProjectCard key={c.id} card={c} navigate={navigate} />)}
      </div>
    </div>
  );
}

/** §4 — 信息密度受控的卡片：名字、可用性、两条代表路径、计数、最近活动。 */
function ProjectCard({ card, navigate }: {
  card: ProjectCardData;
  navigate: (r: Route) => void;
}) {
  const restPaths = card.path_count - card.representative_paths.length;
  return (
    <article
      className="ws-card full"
      onClick={() => navigate({ view: "project", projectId: card.id })}
    >
      <div className="ws-card-head">
        <div className="ws-card-title">{card.name}</div>
        <div className="ws-card-side">
          {card.has_git_identity && <span className="badge">Git 家族</span>}
          {card.missing_path_count > 0 && (
            <span className="badge warn">{card.missing_path_count} 个目录不在</span>
          )}
        </div>
      </div>

      <div className="ws-card-body">
        {card.representative_paths.map((p) => (
          <div key={p} className="small" style={{ marginBottom: 2 }}>
            <PathText path={p} />
          </div>
        ))}
        {restPaths > 0 && <div className="muted small">另有 {restPaths} 个目录</div>}
      </div>

      <div className="ws-card-meta">
        {card.path_count} 个目录 · {card.primary_workstream_count + card.related_workstream_count} 个
        Workstream · {card.session_count} 个 Session
      </div>
      <div className="ws-card-meta muted small">最近活动 {timeAgo(card.last_activity_at)}</div>
    </article>
  );
}
