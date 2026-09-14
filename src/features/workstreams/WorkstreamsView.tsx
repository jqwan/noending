import React, { useMemo, useState } from "react";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import WorkstreamCard from "./WorkstreamCard";
import NewWorkstreamModal from "./NewWorkstreamModal";
import { useWorkstreamCards } from "./useWorkstreamCards";
import type { WorkstreamCardData } from "../../types";
import type { Route } from "../../app/routes";
import { consumeCommand, onEvent, EVT_NEW_WORKSTREAM } from "../../app/routes";

type SortKey = "recent" | "created" | "name";
type FilterKey = "all" | "active" | "archived";

const SORTERS: Record<SortKey, (a: WorkstreamCardData, b: WorkstreamCardData) => number> = {
  recent: (a, b) =>
    (b.last_activity_at ?? b.updated_at).localeCompare(a.last_activity_at ?? a.updated_at),
  created: (a, b) => b.created_at.localeCompare(a.created_at),
  name: (a, b) => a.title.localeCompare(b.title, "zh-Hans"),
};

/** Workstreams = Organize：浏览、搜索、排序、整理（整体设计方案 §22-§27）。 */
export default function WorkstreamsView({ navigate }: { navigate: (r: Route) => void }) {
  const { cards, defaultAgent, refresh } = useWorkstreamCards();
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortKey>("recent");
  const [filter, setFilter] = useState<FilterKey>("active");
  const [creatingWs, setCreatingWs] = useState(false);

  React.useEffect(() => {
    if (consumeCommand(EVT_NEW_WORKSTREAM)) setCreatingWs(true);
    return onEvent(EVT_NEW_WORKSTREAM, () => setCreatingWs(true));
  }, []);

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
          <button className="btn primary" onClick={() => setCreatingWs(true)}>+ New Workstream</button>
        }
      />

      <input
        type="text"
        className="ws-search"
        placeholder="Search workstreams...（标题、描述、Current State、Project）"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />

      <div className="toolbar ws-controls">
        <label className="ws-control">
          <span className="muted small">Filter</span>
          <select value={filter} onChange={(e) => setFilter(e.target.value as FilterKey)}>
            <option value="all">All</option>
            <option value="active">Active</option>
            <option value="archived">Archived</option>
          </select>
        </label>
        <label className="ws-control">
          <span className="muted small">Sort</span>
          <select value={sort} onChange={(e) => setSort(e.target.value as SortKey)}>
            <option value="recent">Last Activity ↓</option>
            <option value="created">Created</option>
            <option value="name">Name</option>
          </select>
        </label>
        {list && <span className="muted small">{list.length} 个</span>}
      </div>

      {list === null && <div className="muted">加载中…</div>}
      {list !== null && list.length === 0 && (
        <EmptyState
          title={query ? "没有匹配的 Workstream。" : filter === "active" ? "No workstreams yet." : "没有已归档 / 已完成的 Workstream。"}
          hint={
            query
              ? undefined
              : filter === "active"
                ? "为一件想跨 Session 继续的事情创建一个 Workstream。"
                : undefined
          }
          actions={
            !query && filter === "active" ? (
              <button className="btn small" onClick={() => setCreatingWs(true)}>+ New Workstream</button>
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
