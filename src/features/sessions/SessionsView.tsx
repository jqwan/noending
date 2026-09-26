import { useViewState, useViewScroll } from "../../hooks/useViewState";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import AgentIcon from "../../components/AgentIcon";
import Icon from "../../components/Icon";
import { Modal, useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SessionCards, {
  agentDisplayLabel,
  cwdDisplayLabel,
  ellipsisTail,
  formatDateTime,
  sessionDisplayTitle,
} from "./SessionTable";
import PermanentDeleteModal from "./PermanentDeleteModal";
import NewSessionModal from "./NewSessionModal";
import ResumeSessionModal from "./ResumeSessionModal";
import { AGENT_LABELS, type Agent, type IngestSource, type Project, type Session } from "../../types";
import type { Route, SessionScope, ViewAction } from "../../app/routes";

type AssignedFilter = "all" | "assigned" | "unassigned";

/**
 * Sessions = 执行记录页：第二天回来还能一眼找到并继续任意一次 Agent 会话，不承担
 * Workstream 浏览。搜索与筛选在前端做，规模大了再转后端。
 * 回收站不是第三种筛选，而是换一个数据面（scope=trash），行渲染与动作都专属。
 */
export default function SessionsView({ navigate, scope, action, actionSeq }: {
  navigate: (r: Route) => void;
  scope?: SessionScope;
  action?: ViewAction;
  actionSeq: number;
}) {
  const [trashMode, setTrashMode] = useState(scope === "trash");
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  /** Workstream id → 标题：`session.owner_workstream_id` 只有一个 id，名字在这里解析。 */
  const [workstreamTitleById, setWorkstreamTitleById] = useState<Map<string, string>>(new Map());
  /** Session 来源只用来把"空"拆成两种真实情况：没启用来源 vs 启用了但还没发现。
   *  读失败时保持 null，文案退回中性说法，不把"读不到"说成"没启用"。 */
  const [sources, setSources] = useState<IngestSource[] | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [query, setQuery] = useViewState("sessions.query", "");
  const [agent, setAgent] = useViewState<"all" | Agent>("sessions.agent", "all");
  const [projectId, setProjectId] = useViewState("sessions.projectId", "all");
  const [wsFilter, setWsFilter] = useViewState("sessions.wsFilter", "all");
  const [assigned, setAssigned] = useViewState<AssignedFilter>("sessions.assigned", "all");
  const [creating, setCreating] = useState(false);
  const [resumeModalSessionId, setResumeModalSessionId] = useState<string | null>(null);
  const [trashSessionId, setTrashSessionId] = useState<string | null>(null);
  const [trashBusy, setTrashBusy] = useState(false);
  const [bulkPurgeOpen, setBulkPurgeOpen] = useState(false);
  const [bulkPurgeBusy, setBulkPurgeBusy] = useState(false);
  /** 永久删除 Modal 的目标 Session（回收站行 / 恢复冲突提示都可能打开它）。 */
  const [purgeSessionId, setPurgeSessionId] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setLoadFailed(false);
    let cancelled = false;
    // 来源列表独立加载：它只为普通模式的空状态分类服务，读失败不该让整张表
    // 变成"读取失败"；回收站模式压根用不到它，不发请求。
    if (!trashMode) {
      api.listIngestSources()
        .then((ss) => { if (!cancelled) setSources(ss); })
        .catch((e) => { console.error(e); if (!cancelled) setSources(null); });
    }
    // 两个数据面各自的后端 scope：普通 = active，
    // 回收站 = trash。projects / workstreams 只服务普通模式的筛选列，回收站行不显示它们。
    const trashList = () => api.listSessions(undefined, undefined, "trash");
    Promise.all([
      trashMode ? trashList() : api.listSessions(),
      trashMode ? Promise.resolve(null) : api.listProjects(),
      trashMode ? Promise.resolve(null) : api.listWorkstreams(),
    ])
      .then(([ss, ps, ws]) => {
        if (cancelled) return;
        if (!trashMode) {
          setWorkstreamTitleById(new Map((ws ?? []).map((w) => [w.id, w.title])));
          setProjects(ps ?? []);
        }
        setSessions(ss);
        setLoadFailed(false);
      })
      .catch((e) => {
        if (cancelled) return;
        console.error(e);
        setLoadFailed(true);
      });
    return () => { cancelled = true; };
  }, [trashMode]);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);
  useEffect(() => setTrashMode(scope === "trash"), [scope]);
  // 页面动作随 Route 到达（palette → New Session）：actionSeq 让「已在 Sessions 页」
  // 的重复命令同样触发。
  useEffect(() => {
    if (action === "new") setCreating(true);
  }, [action, actionSeq]);

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
    || wsFilter !== "all" || assigned !== "all";

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
        if (assigned === "all") return true;
        const has = s.owner_workstream_id !== null;
        return assigned === "assigned" ? has : !has;
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
  }, [sessions, workstreamTitleById, query, agent, projectId, wsFilter, assigned, projectNameById]);

  const resume = (sessionId: string) => {
    setResumeModalSessionId(sessionId);
  };

  const trashTarget = sessions?.find((s) => s.id === trashSessionId) ?? null;
  const trash = async () => {
    if (!trashTarget || trashBusy) return;
    setTrashBusy(true);
    try {
      await api.trashSession(trashTarget.id);
      showToast("已移入回收站");
      setTrashSessionId(null);
      refresh();
    } catch (e) {
      console.error(e);
      showToast(`移入回收站失败：${String(e)}`);
    } finally {
      setTrashBusy(false);
    }
  };

  /** 全部永久删除：无状态的本地清除。逐个读预览，只有 `can_permanently_delete`
   *  （trashed + fresh root missing）的会话才执行；Root 源仍存在或无法确认的原地保留。 */
  const bulkPurge = async () => {
    if (!trashMode || !sessions || sessions.length === 0 || bulkPurgeBusy) return;
    const targets = [...sessions];
    let purged = 0;
    let skipped = 0;
    let failed = 0;
    setBulkPurgeBusy(true);
    for (const session of targets) {
      try {
        const preview = await api.getSessionLocalDeletePreview(session.id);
        if (!preview.can_permanently_delete) {
          skipped += 1;
          continue;
        }
        await api.permanentlyDeleteSession(session.id);
        purged += 1;
      } catch (e) {
        failed += 1;
        console.error(`永久删除会话失败：${session.id}`, e);
      }
    }
    setBulkPurgeBusy(false);
    setBulkPurgeOpen(false);
    refresh();
    showToast(
      failed === 0 && skipped === 0
        ? `已永久删除 ${purged} 个会话`
        : `已清理 ${purged} 个会话${skipped > 0 ? `，${skipped} 个因 Root 源仍存在或无法确认而保留` : ""}${failed > 0 ? `，${failed} 个处理失败` : ""}`,
    );
  };

  /** 恢复失败要把后端的拒绝原因原样给出。 */
  const restoreFromTrash = async (s: Session) => {
    try {
      await api.restoreSession(s.id);
      showToast(`已恢复「${sessionDisplayTitle(s.title)}」`);
      refresh();
    } catch (e) {
      console.error(e);
      showToast(String(e));
    }
  };

  const clearFilters = () => {
    setQuery("");
    setAgent("all");
    setProjectId("all");
    setWsFilter("all");
    setAssigned("all");
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
      title={trashMode ? "读取回收站失败。" : "读取会话失败。"}
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
            {/* 回收站开关：同一页面的两个数据面，用分段控件而非筛选器——
                两边行为（列、动作）不同，不是同一张表的条件过滤。 */}
            <div className="settings-seg" role="group" aria-label="会话列表范围">
              <button
                className={trashMode ? "" : "on"}
                aria-pressed={!trashMode}
                aria-label="会话列表"
                title="会话列表"
                onClick={() => navigate({ view: "sessions", scope: "active" })}
              >
                <Icon name="chat" />
              </button>
              <button
                className={trashMode ? "on" : ""}
                aria-pressed={trashMode}
                aria-label="回收站"
                title="回收站"
                onClick={() => navigate({ view: "sessions", scope: "trash" })}
              >
                <Icon name="archive" />
              </button>
            </div>
            <button className="btn ghost icon-button" aria-label="新建会话" title="新建会话" onClick={() => setCreating(true)}>
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
                <option value="none">无项目</option>
              </select>
            </label>
            <label className="ws-control">
              <span className="muted small">归属状态</span>
              <select value={assigned} onChange={(e) => setAssigned(e.target.value as AssignedFilter)}>
                <option value="all">全部</option>
                <option value="assigned">已归属</option>
                <option value="unassigned">未归属</option>
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
          </div>

          </div>

          {shown === null && !loadFailed && <div className="muted">加载中…</div>}
          {loadFailed && loadFailedState}
          {shown !== null && shown.length === 0 && (sessions?.length ?? 0) === 0 && !loadFailed && (
            <EmptyState
              title={emptyTitle}
              hint={emptyHint}
              actions={
                <>
                  <button className="btn small" onClick={() => navigate({ view: "settings", section: "sources" })}>
                    配置会话来源
                  </button>
                  <button className="btn small" onClick={() => setCreating(true)}>新建会话</button>
                </>
              }
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
              onOpen={(id) => navigate({ view: "session", sessionId: id })}
              onResume={resume}
              onTrash={setTrashSessionId}
            />
          )}
        </>
      )}

      {trashMode && (
        <>
          <div className="muted small" style={{ margin: "4px 0 14px", maxWidth: "72ch" }}>
            回收站里的会话不出现在会话列表、搜索与继续入口中。Agent 原始会话始终保留；
            「永久删除」只清理 NoEnding 的本地数据，不会删除 Agent 数据。
          </div>

          {sessions === null && !loadFailed && <div className="muted">加载中…</div>}
          {loadFailed && loadFailedState}
          {sessions !== null && sessions.length === 0 && !loadFailed && (
            <EmptyState
              title="回收站是空的。"
              hint="「移入回收站」的会话会留在这里：可以随时恢复，也可以在这里永久删除。"
            />
          )}
          {sessions !== null && sessions.length > 0 && (
            <>
              <div className="session-trash-actions">
                <span className="muted small">共 {sessions.length} 个会话</span>
                <button className="btn small danger" onClick={() => setBulkPurgeOpen(true)}>
                  全部永久删除
                </button>
              </div>
              <TrashSessionTable
                sessions={sessions}
                onOpen={(id) => navigate({ view: "session", sessionId: id })}
                onRestore={restoreFromTrash}
                onPurge={(s) => setPurgeSessionId(s.id)}
              />
            </>
          )}
        </>
      )}

      {trashTarget && (
        <Modal
          title="移入回收站"
          onClose={() => { if (!trashBusy) setTrashSessionId(null); }}
        >
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            <b>{sessionDisplayTitle(trashTarget.title)}</b> 会从会话列表、搜索与继续入口中消失，出现在回收站里。
          </p>
          <div className="card hairline" style={{ marginBottom: 12 }}>
            <p style={{ margin: 0 }}>Agent 原始会话不会被删除，之后可以从回收站恢复。</p>
          </div>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setTrashSessionId(null)} disabled={trashBusy}>取消</button>
            <button className="btn primary" onClick={trash} disabled={trashBusy}>
              {trashBusy ? "处理中…" : "移入回收站"}
            </button>
          </div>
        </Modal>
      )}

      {trashMode && bulkPurgeOpen && sessions && sessions.length > 0 && (
        <Modal
          title="全部永久删除"
          onClose={() => { if (!bulkPurgeBusy) setBulkPurgeOpen(false); }}
        >
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            确定要永久删除回收站中的 <b>{sessions.length} 个会话</b> 吗？
          </p>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
            只删除 NoEnding 本地数据，不会删除 Agent 数据。
          </div>
          <p className="small" style={{ margin: "0 0 14px", maxWidth: "72ch" }}>
            每个会话会先读取删除预览：Root 源会话已不存在的才执行；
            Root 源仍存在或无法确认的会话原地保留在回收站。
          </p>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setBulkPurgeOpen(false)} disabled={bulkPurgeBusy}>取消</button>
            <button className="btn danger" onClick={bulkPurge} disabled={bulkPurgeBusy}>
              {bulkPurgeBusy ? "删除中…" : "全部永久删除"}
            </button>
          </div>
        </Modal>
      )}

      {creating && <NewSessionModal onClose={() => setCreating(false)} />}
      {resumeModalSessionId && (
        <ResumeSessionModal
          sessionId={resumeModalSessionId}
          onClose={() => setResumeModalSessionId(null)}
        />
      )}
      {purgeSessionId && (
        <PermanentDeleteModal
          sessionId={purgeSessionId}
          onClose={() => { setPurgeSessionId(null); refresh(); }}
          onDeleted={() => { setPurgeSessionId(null); refresh(); }}
        />
      )}
    </div>
  );
}

/** 回收站表格：行是 Agent、标题、工作目录、移入时间、恢复 / 永久删除五段。
 *  不做筛选与搜索（回收站规模小），点行仍可进详情页。 */
function TrashSessionTable({ sessions, onOpen, onRestore, onPurge }: {
  sessions: Session[];
  onOpen: (sessionId: string) => void;
  onRestore: (s: Session) => void;
  onPurge: (s: Session) => void;
}) {
  const rows = [...sessions].sort((a, b) =>
    (b.trashed_at ?? "").localeCompare(a.trashed_at ?? ""));

  return (
    <table className="session-table">
      <thead>
        <tr>
          <th style={{ width: 110 }}>Agent</th>
          <th>会话</th>
          <th style={{ width: 210 }}>移入回收站</th>
          <th style={{ width: 150 }}>操作</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((s) => {
          const title = sessionDisplayTitle(s.title);
          const cwd = (s.cwd ?? "").trim();
          return (
            <tr
              key={s.id}
              onClick={() => onOpen(s.id)}
              title={`打开会话：${title}`}
            >
              <td className="cell-agent">
                <span className="cell-agent-content">
                  <AgentIcon agent={s.agent} />
                  {agentDisplayLabel(s.agent)}
                </span>
              </td>
              <td className="cell-title" title={title}>
                <div style={{ whiteSpace: "normal" }}>
                  <div>{ellipsisTail(title, 40)}</div>
                  <div className="muted small mono">{cwdDisplayLabel(cwd, 30)}</div>
                </div>
              </td>
              <td title={s.trashed_at ?? undefined}>
                移入回收站：{formatDateTime(s.trashed_at)}
              </td>
              <td onClick={(e) => e.stopPropagation()}>
                <div className="row" style={{ gap: 6 }}>
                  <button className="btn small" onClick={() => onRestore(s)}>恢复</button>
                  <button className="btn small" onClick={() => onPurge(s)}>永久删除</button>
                </div>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
