import { useViewState, useViewScroll } from "../../hooks/useViewState";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import Icon from "../../components/Icon";
import { Modal, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SessionCards, {
  agentDisplayLabel,
  sessionDisplayTitle,
} from "./SessionTable";
import PermanentDeleteModal from "./PermanentDeleteModal";
import { AGENT_LABELS, type Agent, type IngestSource, type Project, type Session } from "../../types";
import type { Route, SessionScope } from "../../app/routes";

/**
 * Sessions = 执行记录页：第二天回来还能一眼找到并继续任意一次 Agent 会话，不承担
 * Workstream 浏览。搜索与筛选在前端做，规模大了再转后端。
 * 未归档和已归档使用相同的筛选与展示，归档只改变继续和删除动作。
 */
let cachedNormalSessions: Session[] | null = null;
let cachedArchivedSessions: Session[] | null = null;
let cachedProjects: Project[] = [];
let cachedSources: IngestSource[] | null = null;
let cachedWorkstreamTitleById = new Map<string, string>();

export function clearSessionsCache() {
  cachedNormalSessions = null;
  cachedArchivedSessions = null;
  cachedProjects = [];
  cachedSources = null;
  cachedWorkstreamTitleById = new Map();
}

export default function SessionsView({ navigate, scope }: {
  navigate: (r: Route) => void;
  scope?: SessionScope;
}) {
  const [archivedMode, setArchivedMode] = useState(scope === "archived");
  const [sessions, setSessions] = useState<Session[] | null>(() => archivedMode ? cachedArchivedSessions : cachedNormalSessions);
  const [projects, setProjects] = useState<Project[]>(cachedProjects);
  /** Workstream id → 标题：`session.owner_workstream_id` 只有一个 id，名字在这里解析。 */
  const [workstreamTitleById, setWorkstreamTitleById] = useState<Map<string, string>>(cachedWorkstreamTitleById);
  /** Session 来源只用来把"空"拆成两种真实情况：没启用来源 vs 启用了但还没发现。
   *  读失败时保持 null，文案退回中性说法，不把"读不到"说成"没启用"。 */
  const [sources, setSources] = useState<IngestSource[] | null>(cachedSources);
  const [loadFailed, setLoadFailed] = useState(false);
  const [query, setQuery] = useViewState("sessions.query", "");
  const [agent, setAgent] = useViewState<"all" | Agent>("sessions.agent", "all");
  const [projectId, setProjectId] = useViewState("sessions.projectId", "all");
  const [wsFilter, setWsFilter] = useViewState("sessions.wsFilter", "all");
  const [viewMode, setViewMode] = useViewState<"cards" | "list">("sessions.viewMode", "cards");
  const [archiveSessionId, setArchiveSessionId] = useState<string | null>(null);
  const [archiveBusy, setArchiveBusy] = useState(false);
  const [unarchiveBusy, setUnarchiveBusy] = useState(false);
  const [bulkPurgeOpen, setBulkPurgeOpen] = useState(false);
  const [bulkPurgeBusy, setBulkPurgeBusy] = useState(false);
  /** 删除 Modal 的目标 Session（已归档行 / 恢复冲突提示都可能打开它）。 */
  const [purgeSessionId, setPurgeSessionId] = useState<string | null>(null);
  const refresh = useCallback(() => {
    setLoadFailed(false);
    let cancelled = false;
    // 来源列表独立加载：它只为普通模式的空状态分类服务，读失败不该让整张表
    // 变成"读取失败"；已归档模式压根用不到它，不发请求。
    if (!archivedMode) {
      api.listIngestSources()
        .then((ss) => {
          if (!cancelled) {
            setSources(ss);
            cachedSources = ss;
          }
        })
        .catch((e) => {
          console.error(e);
          if (!cancelled) {
            setSources(null);
            cachedSources = null;
          }
        });
    }
    // 两个归档范围共用项目与任务筛选。
    const archivedList = () => api.listSessions(undefined, undefined, "archived");
    Promise.all([
      archivedMode ? archivedList() : api.listSessions(),
      api.listProjects(),
      api.listWorkstreams(),
    ])
      .then(([ss, ps, ws]) => {
        if (cancelled) return;
        const wsMap = new Map((ws ?? []).map((w) => [w.id, w.title]));
        setWorkstreamTitleById(wsMap);
        cachedWorkstreamTitleById = wsMap;
        setProjects(ps ?? []);
        cachedProjects = ps ?? [];
        if (!archivedMode) cachedNormalSessions = ss;
        else cachedArchivedSessions = ss;
        setSessions(ss);
        setLoadFailed(false);
      })
      .catch((e) => {
        if (cancelled) return;
        console.error(e);
        setLoadFailed(true);
      });
    return () => { cancelled = true; };
  }, [archivedMode]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);
  useEffect(() => {
    const isArchived = scope === "archived";
    setArchivedMode(isArchived);
    setSessions(isArchived ? cachedArchivedSessions : cachedNormalSessions);
  }, [scope]);

  const projectNameById = useMemo(
    () => new Map(projects.map((p) => [p.id, p.name])),
    [projects],
  );

  /** 筛选下拉里的任务选项：只列真的在列表里出现过的 Owner。 */
  const wsOptions = useMemo(() => {
    const seen = new Set<string>();
    for (const s of sessions ?? []) {
      if (s.owner_workstream_id) seen.add(s.owner_workstream_id);
    }
    return [...seen]
      .map((id) => [id, workstreamTitleById.get(id)?.trim() || "未命名任务"] as const)
      .sort((a, b) => a[1].localeCompare(b[1], "zh-Hans"));
  }, [sessions, workstreamTitleById]);

  const filtersActive =
    query.trim() !== "" || agent !== "all" || projectId !== "all"
    || wsFilter !== "all";

  const shown = useMemo(() => {
    if (!sessions) return null;
    const q = query.trim().toLowerCase();
    return sessions
      .filter((s) => (agent === "all" ? true : s.agent === agent))
      // 和 Project 徽章同一个判据：只有存在物理锚点的行才算这个 Project 的成员。
      // 徽章说"历史标签，已不决定任何事"的行不能一边显示成无归属、一边又被筛进来
      // （M34 / M29：一个视图不能两个真相）。
      .filter((s) =>
        projectId === "all" ? true : projectId === "none"
          ? !s.workspace_path_id || !s.project_id
          : !!s.workspace_path_id && s.project_id === projectId
      )
      .filter((s) => {
        if (wsFilter === "all") return true;
        // 归属是单值：命中就是这一个，未归属就是 null。
        return wsFilter === "unassigned"
          ? s.owner_workstream_id === null
          : s.owner_workstream_id === wsFilter;
      })
      .filter((s) => {
        if (q === "") return true;
        // 无标题 Session 也要被找到：占位文案与 root_agent_session_id / id 都进
        // 搜索面，用户照着报障里的 Session ID 能直接定位（AGENTS.md: provenance 可解析）。
        return [
          s.title,
          sessionDisplayTitle(s.title),
          s.cwd,
          s.root_agent_session_id,
          s.id,
          agentDisplayLabel(s.agent),
          s.project_id ? projectNameById.get(s.project_id) : null,
          s.owner_workstream_id ? workstreamTitleById.get(s.owner_workstream_id) : null,
        ]
          .filter(Boolean)
          .some((t) => (t as string).toLowerCase().includes(q));
      })
      .sort((a, b) =>
        (b.last_activity_at ?? b.started_at ?? "").localeCompare(
          a.last_activity_at ?? a.started_at ?? "",
        ),
      );
  }, [sessions, workstreamTitleById, query, agent, projectId, wsFilter, projectNameById]);

  /** 行内「继续」：直接在会话格式对应的桌面应用里打开（无预览）；格式没有
   *  桌面路由时按钮本来就是灰的，这里的报错是兜底。 */
  const resume = (sessionId: string) => {
    api.continueSessionDesktop(sessionId)
      .then((open) => showToast(open.note || "已在桌面应用中打开"))
      .catch((e) => showToast(`打开失败：${String(e)}`));
  };

  const archiveTarget = sessions?.find((s) => s.id === archiveSessionId) ?? null;
  const archive = async () => {
    if (!archiveTarget || archiveBusy) return;
    setArchiveBusy(true);
    try {
      await api.archiveSession(archiveTarget.id);
      showToast("已归档");
      setArchiveSessionId(null);
      cachedNormalSessions = null;
      cachedArchivedSessions = null;
      refresh();
    } catch (e) {
      console.error(e);
      showToast(`归档失败：${String(e)}`);
    } finally {
      setArchiveBusy(false);
    }
  };

  /** 删除全部已归档会话；源仍存在的会话下次同步会重新入库。 */
  const bulkPurge = async () => {
    if (!archivedMode || !sessions || sessions.length === 0 || bulkPurgeBusy) return;
    const targets = [...sessions];
    let purged = 0;
    let failed = 0;
    setBulkPurgeBusy(true);
    for (const session of targets) {
      try {
        await api.permanentlyDeleteSession(session.id);
        purged += 1;
      } catch (e) {
        failed += 1;
        console.error(`删除会话失败：${session.id}`, e);
      }
    }
    setBulkPurgeBusy(false);
    setBulkPurgeOpen(false);
    cachedNormalSessions = null;
    cachedArchivedSessions = null;
    refresh();
    showToast(
      failed === 0
        ? `已删除 ${purged} 个会话`
        : `已删除 ${purged} 个会话，${failed} 个失败`,
    );
  };

  /** 恢复失败要把后端的拒绝原因原样给出。 */
  const unarchive = async (s: Session) => {
    if (unarchiveBusy || bulkPurgeBusy) return;
    setUnarchiveBusy(true);
    try {
      await api.restoreSession(s.id);
      showToast(`已恢复「${sessionDisplayTitle(s.title)}」`);
      cachedNormalSessions = null;
      cachedArchivedSessions = null;
      refresh();
    } catch (e) {
      console.error(e);
      showToast(String(e));
    } finally { setUnarchiveBusy(false); }
  };

  const clearFilters = () => {
    setQuery("");
    setAgent("all");
    setProjectId("all");
    setWsFilter("all");
  };

  /** 空库的三种情况分开说话：没启用来源 / 启用的来源目录不在了 / 还没跑过。
   *  来源读不到时退回中性说法，不宣称没被证实的事。 */
  const noSourcesEnabled = sources !== null && sources.every((s) => !s.enabled);
  const missingSourcePath = sources?.find((s) => s.enabled && !s.exists)?.path ?? "";
  const emptyTitle = noSourcesEnabled ? "还没有启用任何会话来源" : "还没有发现本地会话";
  const emptyHint = noSourcesEnabled ? "请启用会话来源。" : missingSourcePath
    ? `来源目录不存在：${missingSourcePath}` : "新建会话，或检查会话来源。";

  const loadFailedState = (
    <EmptyState
      title={archivedMode ? "读取已归档失败。" : "读取会话失败。"}
      actions={<button className="btn small" onClick={refresh}>重试</button>}
    />
  );

  const scrollRef = useViewScroll("sessions.scroll", sessions !== null);

  return (
    <div className="main board-page" ref={scrollRef}>
      <PageHeader
        title="会话"
        actions={
          <>
            {/* 两个归档范围共用同一套卡片和列表。 */}
            <div className="settings-seg archive-scope" role="group" aria-label="会话列表范围">
              <button
                className={archivedMode ? "" : "on"}
                aria-pressed={!archivedMode}
                aria-label="未归档"
                title="未归档"
                onClick={() => navigate({ view: "sessions", scope: "unarchived" })}
              >
                <Icon name="chat" /> 未归档
              </button>
              <button
                className={archivedMode ? "on" : ""}
                aria-pressed={archivedMode}
                aria-label="已归档"
                title="已归档"
                onClick={() => navigate({ view: "sessions", scope: "archived" })}
              >
                <Icon name="archive" /> 已归档
              </button>
            </div>
            {archivedMode ? <button className="btn ghost danger" aria-label="删除全部" title="删除全部已归档会话" disabled={!sessions?.length || bulkPurgeBusy || unarchiveBusy || archiveBusy} onClick={() => setBulkPurgeOpen(true)}><Icon name="trash" /> 删除全部</button> : <button className="btn ghost icon-button" aria-label="新建会话" title="新建会话" onClick={() => navigate({ view: "new-session" })}>
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
            aria-label="搜索会话"
            placeholder="搜索会话…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />

          <div className="toolbar ws-controls">
            <label className="ws-control">
              <span className="muted small">Agent</span>
              <select value={agent} onChange={(e) => setAgent(e.target.value as "all" | Agent)}>
                <option value="all">全部 Agent</option>
                {Object.entries(AGENT_LABELS).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
              </select>
            </label>
            <label className="ws-control">
              <span className="muted small">任务</span>
              <select value={wsFilter} onChange={(e) => setWsFilter(e.target.value)}>
                <option value="all">全部任务</option>
                {wsOptions.map(([id, title]) => <option key={id} value={id}>{title}</option>)}
                <option value="unassigned">未归属任务</option>
              </select>
            </label>
            <label className="ws-control">
              <span className="muted small">项目</span>
              <select
                value={projectId}
                onChange={(e) => setProjectId(e.target.value)}
                title="按工作目录所属项目筛选"
              >
                <option value="all">全部项目</option>
                {projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
              </select>
            </label>
            {filtersActive && (
              <button className="btn small ghost" onClick={clearFilters}>清除筛选</button>
            )}
            {shown && (
              <span className="muted small">
                {filtersActive ? `显示 ${shown.length} / 共 ${sessions?.length ?? 0} 条` : `共 ${shown.length} 条`}
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

          {shown === null && !loadFailed && (
            viewMode === "cards" ? (
              <div className="board-grid" role="status" aria-busy="true">
                {Array.from({ length: 6 }).map((_, i) => (
                  <div
                    key={i}
                    className="skeleton card"
                    style={{ minHeight: 160, borderRadius: "var(--radius-lg)" }}
                  />
                ))}
              </div>
            ) : (
              <div className="session-list" aria-busy="true">
                {Array.from({ length: 6 }).map((_, i) => (
                  <div
                    key={i}
                    className="skeleton"
                    style={{ height: 68, borderRadius: "var(--radius-md)", marginBottom: 8 }}
                  />
                ))}
              </div>
            )
          )}
          {loadFailed && loadFailedState}
          {shown !== null && shown.length === 0 && (sessions?.length ?? 0) === 0 && !loadFailed && (
            <EmptyState
              title={archivedMode ? "还没有已归档的会话" : emptyTitle}
              hint={archivedMode ? "归档的会话会显示在这里，可以取消归档或永久删除。" : emptyHint}
              actions={!archivedMode ? (
                <>
                  <button className="btn small" onClick={() => navigate({ view: "agents" })}>
                    配置会话来源
                  </button>
                  <button className="btn small" onClick={() => navigate({ view: "new-session" })}>新建会话</button>
                </>
              ) : undefined}
            />
          )}
          {shown !== null && shown.length === 0 && (sessions?.length ?? 0) > 0 && (
            <EmptyState
              title="没有符合当前筛选条件的会话"
              hint={query.trim()
                ? `没有匹配「${query.trim()}」的会话。可以换个关键词，或直接清除筛选。`
                : "调整或清除筛选条件即可看到全部会话。"}
              actions={<button className="btn small" onClick={clearFilters}>清除筛选</button>}
            />
          )}
          {shown !== null && shown.length > 0 && (
            <SessionCards
              sessions={shown}
              workstreamTitleById={workstreamTitleById}
              projectNameById={projectNameById}
              viewMode={viewMode}
              onOpen={(id) => navigate({ view: "session", sessionId: id })}
              onResume={resume}
              onArchive={setArchiveSessionId}
              onRestore={(id) => { const target = sessions?.find((s) => s.id === id); if (target) void unarchive(target); }}
              onDelete={setPurgeSessionId}
              busy={bulkPurgeBusy || unarchiveBusy || archiveBusy}
              navigate={navigate}
            />
          )}
      </>

      {archiveTarget && (
        <Modal
          title="归档"
          onClose={() => { if (!archiveBusy) setArchiveSessionId(null); }}
        >
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            <b>{sessionDisplayTitle(archiveTarget.title)}</b> 会移至「已归档」，归档后不能通过 NoEnding 继续。搜索、同步和摘要更新保持可用。
          </p>
          <div className="card hairline" style={{ marginBottom: 12 }}>
            <p style={{ margin: 0 }}>Agent 原始会话不会被删除，之后可以从已归档恢复。</p>
          </div>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setArchiveSessionId(null)} disabled={archiveBusy}>取消</button>
            <button className="btn primary" onClick={archive} disabled={archiveBusy}>
              {archiveBusy ? "处理中…" : "归档"}
            </button>
          </div>
        </Modal>
      )}

      {archivedMode && bulkPurgeOpen && sessions && sessions.length > 0 && (
        <Modal
          title="删除全部"
          onClose={() => { if (!bulkPurgeBusy) setBulkPurgeOpen(false); }}
        >
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            确定要删除已归档的 <b>{sessions.length} 个会话</b> 的本地数据吗？
          </p>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
            只删除 NoEnding 本地数据，不会删除 Agent 数据。
          </div>
          <p className="small" style={{ margin: "0 0 14px", maxWidth: "72ch" }}>
            每个会话都会删除 NoEnding 的本地副本；源文件不会被删除，所以源仍存在的会话
            会在后续同步中作为<b>新会话</b>重新入库（新 id、无所属任务、无摘要）。
          </p>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setBulkPurgeOpen(false)} disabled={bulkPurgeBusy}>取消</button>
            <button className="btn danger" onClick={bulkPurge} disabled={bulkPurgeBusy}>
              {bulkPurgeBusy ? "删除中…" : "删除全部"}
            </button>
          </div>
        </Modal>
      )}

      {purgeSessionId && (
        <PermanentDeleteModal
          sessionId={purgeSessionId}
          onClose={() => { setPurgeSessionId(null); refresh(); }}
          onDeleted={() => { cachedNormalSessions = null; cachedArchivedSessions = null; setPurgeSessionId(null); refresh(); }}
        />
      )}
    </div>
  );
}
