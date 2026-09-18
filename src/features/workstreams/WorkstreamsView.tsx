import React, { useEffect, useMemo, useState } from "react";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import WorkstreamCard from "./WorkstreamCard";
import NewWorkstreamModal from "./NewWorkstreamModal";
import { useWorkstreamCards } from "./useWorkstreamCards";
import type { WorkstreamCardData } from "../../types";
import type { Route, ViewAction } from "../../app/routes";

type SortKey = "recent" | "created" | "name";
type FilterKey = "all" | "active" | "archived";

const SORTERS: Record<SortKey, (a: WorkstreamCardData, b: WorkstreamCardData) => number> = {
  recent: (a, b) =>
    (b.last_activity_at ?? b.updated_at).localeCompare(a.last_activity_at ?? a.updated_at),
  created: (a, b) => b.created_at.localeCompare(a.created_at),
  name: (a, b) => a.title.localeCompare(b.title, "zh-Hans"),
};

/** Workstreams = Organize：浏览、搜索、排序、整理（整体设计方案 §22-§27）。 */
export default function WorkstreamsView({ navigate, action, actionSeq }: {
  navigate: (r: Route) => void;
  action?: ViewAction;
  actionSeq: number;
}) {
  const { cards, defaultAgent, refresh } = useWorkstreamCards();
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortKey>("recent");
  const [filter, setFilter] = useState<FilterKey>("active");
  const [creatingWs, setCreatingWs] = useState(false);

  // 页面动作随 Route 到达（palette → New Workstream）：
  // actionSeq 让「已在 Workstreams 页」的重复命令同样触发。
  useEffect(() => {
    if (action === "new") setCreatingWs(true);
  }, [action, actionSeq]);

  const list = useMemo(() => {
    if (!cards) return null;
    const isOpen = (c: WorkstreamCardData) =>
      c.lifecycle === "open" && c.visibility === "normal";
    const q = query.trim().toLowerCase();
    return cards
      .filter((c) =>
        filter === "all" ? true : filter === "active" ? isOpen(c) : !isOpen(c),
      )
      .filter((c) =>
        q === ""
          ? true
          : [c.title, c.description, c.current_state, c.project_name]
              .filter(Boolean)
              .some((s) => (s as string).toLowerCase().includes(q)),
      )
      .sort(SORTERS[sort]);
  }, [cards, query, sort, filter]);

  return (
    <div className="main">
      <PageHeader
        title="Workstreams"
        sub="我现在有哪些持续进行中的事情？"
        actions={
          <button className="btn primary" onClick={() => setCreatingWs(true)}>+ 新建 Workstream</button>
        }
      />

      <input
        type="text"
        className="ws-search"
        placeholder="搜索 Workstream…（标题、描述、Project）"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />

      <div className="toolbar ws-controls">
        <label className="ws-control">
          <span className="muted small">筛选</span>
          <select value={filter} onChange={(e) => setFilter(e.target.value as FilterKey)}>
            <option value="all">全部</option>
            <option value="active">进行中</option>
            <option value="archived">已归档</option>
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
        {list && <span className="muted small">{list.length} 个 Workstream</span>}
      </div>

      {list === null && <div className="muted">加载中…</div>}
      {list !== null && list.length === 0 && (
        <EmptyState
          title={query ? "没有匹配的 Workstream。" : filter === "active" ? "还没有 Workstream。" : "没有已归档 / 已完成的 Workstream。"}
          hint={
            query
              ? undefined
              : filter === "active"
                ? "为一件想跨 Session 继续的事情创建一个 Workstream。"
                : filter === "archived"
                  ? "归档后的 Workstream 会留在这里，随时可以取消归档。"
                  : undefined
          }
          actions={
            !query && filter === "active" ? (
              <button className="btn small" onClick={() => setCreatingWs(true)}>+ 新建 Workstream</button>
            ) : undefined
          }
        />
      )}

      <div className="ws-grid">
        {list?.map((c) => (
          <WorkstreamCard key={c.id} card={c} mode="full" navigate={navigate} defaultAgent={defaultAgent} />
        ))}
      </div>

      {creatingWs && (
        <NewWorkstreamModal
          onClose={() => setCreatingWs(false)}
          onCreated={(w) => navigate({ view: "workstream", workstreamId: w.id })}
        />
      )}
    </div>
  );
}
