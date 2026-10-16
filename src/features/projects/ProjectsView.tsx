import { useObjectMatches, ObjectSearchFeedback } from "../search/useObjectMatches";
import { EVT_REMOVED, removedTargets } from "../../app/removedTargets";
import Icon from "../../components/Icon";
import CustomSelect from "../../components/CustomSelect";
import { useViewState, useViewScroll, useViewPreference, preferenceValues } from "../../hooks/useViewState";
import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import { showToast } from "../../components/Toast";
import { timeAgo, useRefreshSignal } from "../../components/common";
import { EVT_SYNCED, type Route } from "../../app/routes";
import ProjectLifecycleActions, { canDeleteProject, ProjectDeleteModal } from "./ProjectLifecycleActions";
import type { ProjectCardData, ProjectKind } from "../../types";
import { projectKindLabels, isDirectoryGroupProject, isProjectAllPathsMissing } from "./projectKind";

/**
 * Projects Board：Projects 是一等浏览页面——物理工作空间，由工作目录自动派生。
 * 数据来自**一次** `list_project_cards` 调用，不再是 1 + N 的 detail 读取。
 * 卡片强调「地方」（路径、可用性、任务 / 会话数、最近活动）；诊断信息（uuid、
 * git_id、内部 identity）属于 Detail。归档状态由用户管理。
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

let cachedProjectCards: ProjectCardData[] | null = null;

export function clearProjectCardsCache() {
  cachedProjectCards = null;
}

export default function ProjectsView({ navigate, scope }: { navigate: (r: Route) => void; scope?: "unarchived" | "archived" }) {
  const archivedMode = scope === "archived";
  const [bulkDeleteOpen, setBulkDeleteOpen] = useState(false);
  const [bulkDeleteBusy, setBulkDeleteBusy] = useState(false);
  const [cards, setCards] = useState<ProjectCardData[] | null>(cachedProjectCards);
  const [listError, setListError] = useState("");
  const [query, setQuery] = useViewState("projects.query", "");
  const [filter, setFilter] = useViewPreference<FilterKey>("projects.filter", "all", preferenceValues("all", "ok", "missing"));
  const [kindFilter, setKindFilter] = useViewPreference<ProjectKindFilter>("projects.kindFilter", "all", preferenceValues("all", "git", "single_directory", "directory_group"));
  const effectiveKindFilter = kindFilter === "all" || kindFilter in projectKindLabels ? kindFilter : "all";
  const [sort, setSort] = useViewPreference<SortKey>("projects.sort", "recent", preferenceValues("recent", "name", "paths"));
  const [viewMode, setViewMode] = useViewPreference<"cards" | "list">("projects.viewMode", "cards", preferenceValues("cards", "list"));

  const refresh = useCallback(() => {
    api
      .listProjectCards()
      .then((cs) => {
        const current = cs.filter(p => !removedTargets.projects.has(p.id));
        cachedProjectCards = current;
        setCards(current);
        setListError("");
      })
      .catch((e) => {
        setCards(null);
        setListError(`读取项目失败：${String(e)}`);
      });
  }, []);
  useEffect(() => {
    const remove = () => setCards(rows => rows?.filter(p => !removedTargets.projects.has(p.id)) ?? null);
    window.addEventListener(EVT_REMOVED, remove);
    return () => window.removeEventListener(EVT_REMOVED, remove);
  }, []);
  useEffect(refresh, [refresh]);
  // 单次卡片查询足够便宜，refresh 信号不再需要防抖。
  useRefreshSignal(refresh);

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

  const candidates = useMemo(() => cards?.filter((c) => c.visibility === (archivedMode ? "archived" : "normal"))
      .filter(FILTERS[filter])
      .filter((c) => effectiveKindFilter === "all" || c.kind === effectiveKindFilter) ?? null, [cards, filter, effectiveKindFilter, archivedMode]);
  const candidateIds = useMemo(() => candidates?.map(item => item.id) ?? [], [candidates]);
  const search = useObjectMatches("project", query, cards, candidateIds);
  const list = useMemo(() => {
    if (!candidates || search.pending || search.error) return null;
    return candidates
      .filter((c) => !search.ids || search.ids.has(c.id))
      .sort(SORTERS[sort]);
  }, [candidates, search, sort]);

  const deletableProjects = cards?.filter(canDeleteProject) ?? [];
  const bulkDelete = async () => {
    if (bulkDeleteBusy || deletableProjects.length === 0) return;
    const targets = [...deletableProjects];
    setBulkDeleteBusy(true);
    let deleted = 0;
    for (const project of targets) {
      try { await api.deleteProjectPermanently(project.id); deleted += 1; }
      catch (e) { console.error(e); }
    }
    setBulkDeleteBusy(false);
    setBulkDeleteOpen(false);
    clearProjectCardsCache();
    refresh();
    window.dispatchEvent(new Event(EVT_SYNCED));
    showToast(`已永久删除 ${deleted} 个项目${deleted < targets.length ? `，${targets.length - deleted} 个删除失败` : ""}`);
  };

  const scrollRef = useViewScroll("projects.scroll", cards !== null);

  return (
    <div className="main board-page" ref={scrollRef}>
      <PageHeader
        title="项目"
        actions={
          <>
            <div className="settings-seg icon-seg archive-scope" role="group" aria-label="项目列表范围">
              <button className={archivedMode ? "" : "on"} aria-pressed={!archivedMode} aria-label="未归档" title="未归档"
                onClick={() => navigate({ view: "projects", scope: "unarchived" })}><Icon name="project" /></button>
              <button className={archivedMode ? "on" : ""} aria-pressed={archivedMode} aria-label="已归档" title="已归档"
                onClick={() => navigate({ view: "projects", scope: "archived" })}><Icon name="archive" /></button>
            </div>
            <span className="session-action-divider" aria-hidden="true" />
            {archivedMode ? (
              <button className="btn ghost icon-button danger" aria-label="删除全部" title="删除全部可删除的已归档项目"
                disabled={deletableProjects.length === 0 || bulkDeleteBusy} onClick={() => setBulkDeleteOpen(true)}><Icon name="trash" /></button>
            ) : (
              <button className="btn ghost icon-button" disabled={addingProject} aria-label="新增项目" title="新增项目" onClick={handleAddProject}>
                <Icon name="plus" />
              </button>
            )}
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
        <div className="ws-controls-group">
          <div className="ws-control">
            <span className="muted small">筛选</span>
            <CustomSelect
              value={filter}
              onChange={(val) => setFilter(val as FilterKey)}
              ariaLabel="筛选"
              options={[
                { value: "all", label: "全部" },
                { value: "ok", label: "正常" },
                { value: "missing", label: "有目录缺失" },
              ]}
            />
          </div>
          <div className="ws-control">
            <span className="muted small">项目类型</span>
            <CustomSelect
              value={effectiveKindFilter}
              onChange={(val) => setKindFilter(val as ProjectKindFilter)}
              ariaLabel="项目类型"
              options={[
                { value: "all", label: "全部类型" },
                ...Object.entries(projectKindLabels).map(([kind, label]) => ({
                  value: kind,
                  label,
                })),
              ]}
            />
          </div>
          <div className="ws-control">
            <span className="muted small">排序</span>
            <CustomSelect
              value={sort}
              onChange={(val) => setSort(val as SortKey)}
              ariaLabel="排序"
              options={[
                { value: "recent", label: "最近活动" },
                { value: "name", label: "名称" },
                { value: "paths", label: "目录数" },
              ]}
            />
          </div>
          {(query || filter !== "all" || effectiveKindFilter !== "all") && (
            <button
              className="btn small ghost"
              onClick={() => {
                setQuery("");
                setFilter("all");
                setKindFilter("all");
              }}
            >
              清除筛选
            </button>
          )}
        </div>

        <div className="ws-controls-meta">
          {list !== null && (
            <span className="muted small">
              {(query || filter !== "all" || effectiveKindFilter !== "all")
                ? `显示 ${list.length} / 共 ${cards?.filter((c) => c.visibility === (archivedMode ? "archived" : "normal")).length ?? 0} 项`
                : `共 ${list.length} 项`}
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

      </div>

      <ObjectSearchFeedback search={search} />
      {cards === null && listError === "" && (
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
      {list !== null && list.length === 0 && cards !== null && cards.some((c) => c.visibility === (archivedMode ? "archived" : "normal")) && (
        <EmptyState
          title="没有匹配的项目"
          hint="换个关键词，或把筛选切回「全部」。"
        />
      )}
      {cards !== null && !cards.some((c) => c.visibility === (archivedMode ? "archived" : "normal")) && (
        <EmptyState
          title={archivedMode ? "没有已归档项目" : "还没有项目"}
          hint={archivedMode ? "归档的项目会显示在这里。" : "添加目录、打开会话或给任务添加工作路径后，项目会出现在这里。"}
        />
      )}

      {viewMode === "cards" ? (
        <div className="ws-grid board-grid">
          {list?.map((c) => <ProjectCard key={c.id} card={c} navigate={navigate} onChanged={refresh} />)}
        </div>
      ) : (
        <div className="project-list">
          {list?.map((c) => (
            <article className="project-list-row" key={`list-${c.id}`}>
              <button
                type="button"
                className="project-open"
                onClick={() => navigate({ view: "project", projectId: c.id, initialTitle: c.name })}
              >
                <div className="project-list-title" title={c.name}>
                  <Icon name="project" />
                  <span>{c.name}</span>
                  {c.missing_path_count > 0 && (
                    <span className="badge warn">{isProjectAllPathsMissing(c) ? "全部目录缺失" : "部分目录缺失"}</span>
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
                    <Icon name="folder" />
                    <span>{c.representative_paths[0]}</span>
                    {c.path_count > 1 && (
                      <span className="muted" style={{ marginLeft: 6 }}>
                        另有 {c.path_count - 1} 个目录
                      </span>
                    )}
                  </div>
                )}
              </button>
              <div className="project-list-actions" onClick={(e) => e.stopPropagation()}>
                <ProjectLifecycleActions project={c} onChanged={refresh} />
                {(c.purpose === "assistant" || !isDirectoryGroupProject(c)) && (
                  <button
                    type="button"
                    className="btn small ghost icon-button"
                    aria-label="新建会话"
                    title="新建会话"
                    onClick={() => navigate({ view: "new-session", projectId: c.id })}
                  >
                    <Icon name="plus" />
                  </button>
                )}
              </div>
            </article>
          ))}
        </div>
      )}
      {bulkDeleteOpen && <ProjectDeleteModal names={deletableProjects.map((p) => p.name)} busy={bulkDeleteBusy}
        onClose={() => setBulkDeleteOpen(false)} onConfirm={() => void bulkDelete()} />}

    </div>
  );
}

/** 信息密度受控的卡片：名字、可用性、两条代表路径、计数、最近活动。 */
function ProjectCard({ card, navigate, onChanged }: {
  card: ProjectCardData;
  onChanged: () => void;
  navigate: (r: Route) => void;
}) {
  return (
    <article
      className="ws-card full project-card"
      tabIndex={0}
      role="button"
      onClick={() => navigate({ view: "project", projectId: card.id, initialTitle: card.name })}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          navigate({ view: "project", projectId: card.id, initialTitle: card.name });
        }
      }}
    >
      <div className="ws-card-head">
        <div className="ws-card-title" title={card.name}>
          <Icon name="project" />
          <span className="truncate">{card.name}</span>
        </div>
        {card.missing_path_count > 0 && (
          <div className="ws-card-side">
            <span className="badge warn">{isProjectAllPathsMissing(card) ? "全部目录缺失" : "部分目录缺失"}</span>
          </div>
        )}
      </div>

      <div className="ws-card-body">
        <div className="board-stats">
          <span title="工作目录"><Icon name="folder" />{card.path_count}<span>目录</span></span>
          <span title="与本项目有工作路径关联的任务"><Icon name="tasks" />{card.workstream_count}<span>任务</span></span>
          <span title="会话"><Icon name="chat" />{card.session_count}<span>会话</span></span>
        </div>
      </div>
      <div className="ws-card-meta">
        <span className="muted small">{timeAgo(card.last_activity_at)}</span>
        <div className="ws-card-actions" onClick={(e) => e.stopPropagation()}>
          <ProjectLifecycleActions project={card} onChanged={onChanged} />
          {(card.purpose === "assistant" || !isDirectoryGroupProject(card)) && (
            <button
              type="button"
              className="btn small ghost icon-button"
              aria-label="新建会话"
              title="新建会话"
              onClick={() => navigate({ view: "new-session", projectId: card.id })}
            >
              <Icon name="plus" />
            </button>
          )}
        </div>
      </div>
    </article>
  );
}
