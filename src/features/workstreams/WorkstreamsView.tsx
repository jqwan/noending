import { useEffect, useMemo, useState } from "react";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import WorkstreamCard, { cardSearchFields, searchFieldHint } from "./WorkstreamCard";
import NewWorkstreamModal from "./NewWorkstreamModal";
import { useWorkstreamCards } from "./useWorkstreamCards";
import { useBaseExperience } from "../../app/experience";
import type { WorkstreamCardData } from "../../types";
import type { Route, ViewAction } from "../../app/routes";

type SortKey = "recent" | "created" | "name";
/**
 * 筛选按**两个正交维度**表达（方案 §1.13）：lifecycle（进行中 / 已完成）与
 * visibility（normal / archived）。回收站就是 archived —— 它不是第三种状态，
 * 也不改变 lifecycle，所以「回收站」只能按 visibility 判，
 * 不能像旧代码那样把「非进行中」统统算成已归档。
 */
type FilterKey = "all" | "active" | "completed" | "trash";

const FILTERS: Record<FilterKey, (c: WorkstreamCardData) => boolean> = {
  all: () => true,
  active: (c) => c.lifecycle === "active" && c.visibility === "normal",
  completed: (c) => c.lifecycle === "completed" && c.visibility === "normal",
  trash: (c) => c.visibility === "archived",
};

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
  const { cards, defaultAgent } = useWorkstreamCards();
  const { intelligenceEnabled } = useBaseExperience();
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
    const q = query.trim().toLowerCase();
    return cards
      .filter(FILTERS[filter])
      .filter((c) =>
        q === ""
          ? true
          : cardSearchFields(c, intelligenceEnabled)
              .filter(Boolean)
              .some((s) => s!.toLowerCase().includes(q)),
      )
      .sort(SORTERS[sort]);
  }, [cards, query, sort, filter, intelligenceEnabled]);

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
        placeholder={searchFieldHint(intelligenceEnabled)}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />

      <div className="toolbar ws-controls">
        <label className="ws-control">
          <span className="muted small">筛选</span>
          <select value={filter} onChange={(e) => setFilter(e.target.value as FilterKey)}>
            <option value="all">全部</option>
            <option value="active">进行中</option>
            <option value="completed">已完成</option>
            <option value="trash">回收站</option>
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
          title={
            query
              ? "没有匹配的 Workstream。"
              : filter === "active"
                ? "还没有进行中的 Workstream。"
                : filter === "completed"
                  ? "还没有标记为已完成的 Workstream。"
                  : filter === "trash"
                    ? "回收站是空的。"
                    : "还没有 Workstream。"
          }
          hint={
            query
              ? undefined
              : filter === "trash"
                ? "「移入回收站」的 Workstream 会留在这里：工作路径、Session 绑定与 Context 都原样保留，随时可以恢复；永久删除只能从这一步出发。"
                : filter === "completed"
                  ? "「已完成」只是分类标签，不改变任何行为；在 Workstream 详情页可以随时切回「进行中」。"
                  : "为一件想跨 Session 继续的事情创建一个 Workstream。工作路径是可选的 —— 它决定 Project 归属与新建 Session 的默认目录。"
          }
          actions={
            !query && filter !== "trash" && filter !== "completed" ? (
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
