import React, { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import PageHeader from "../../layout/PageHeader";
import EmptyState from "../../components/EmptyState";
import AgentIcon from "../../components/AgentIcon";
import { useRefreshSignal } from "../../components/common";
import { showToast } from "../../components/Toast";
import SessionTable, {
  agentDisplayLabel,
  cwdDisplayLabel,
  ellipsisTail,
  formatDateTime,
  sessionDisplayTitle,
} from "./SessionTable";
import PermanentDeleteModal from "./PermanentDeleteModal";
import NewSessionModal from "./NewSessionModal";
import ResumeSessionModal from "./ResumeSessionModal";
import { AGENT_LABELS, type Agent, type IngestSource, type Project, type Session, type SessionBindingRow } from "../../types";
import type { Route, ViewAction } from "../../app/routes";

type AssignedFilter = "all" | "assigned" | "unassigned";

/**
 * Sessions = 执行记录页（整体设计方案 §38-§41）：用户第二天回来还能一眼找到并继续
 * 任意一次 Agent 会话。不承担 Workstream 浏览。搜索与筛选在前端做，规模大了再转后端。
 *
 * Session Lifecycle & Deletion v0.1（§35）加入「回收站」：不是第三种筛选，而是
 * 换一个数据面——普通模式读 scope=active（与旧行为完全一致），回收站读 scope=trash，
 * 行渲染与操作都是回收站专属（恢复 / 永久删除），正常列表不出现这些动作。
 */
export default function SessionsView({ navigate, action, actionSeq }: {
  navigate: (r: Route) => void;
  action?: ViewAction;
  actionSeq: number;
}) {
  const [trashMode, setTrashMode] = useState(false);
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [bindings, setBindings] = useState<Map<string, SessionBindingRow[]>>(new Map());
  /** 回收站条数：页头「回收站 (N)」徽标。普通模式也读——本地查询，足够便宜。 */
  const [trashCount, setTrashCount] = useState<number | null>(null);
  /**
   * Session 来源只用来把"空"拆成两种真实情况：一个来源都没启用 vs
   * 启用了但还没发现 Session（§23）。读失败时保持 null，文案退回中性说法——
   * 不能把"读不到"说成"没启用"。
   */
  const [sources, setSources] = useState<IngestSource[] | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [query, setQuery] = useState("");
  const [agent, setAgent] = useState<"all" | Agent>("all");
  const [projectId, setProjectId] = useState("all");
  const [wsFilter, setWsFilter] = useState("all");
  const [assigned, setAssigned] = useState<AssignedFilter>("all");
  const [creating, setCreating] = useState(false);
  const [resumeModalSessionId, setResumeModalSessionId] = useState<string | null>(null);
  /** 永久删除 Modal 的目标 Session（回收站行 / 恢复冲突提示都可能打开它）。 */
  const [purgeSessionId, setPurgeSessionId] = useState<string | null>(null);

  const refresh = useCallback(() => {
    let cancelled = false;
    // 来源列表独立加载：它只为普通模式的空状态分类服务，读失败不该让整张表
    // 变成"读取失败"；回收站模式压根用不到它，不发请求。
    if (!trashMode) {
      api.listIngestSources()
        .then((ss) => { if (!cancelled) setSources(ss); })
        .catch((e) => { console.error(e); if (!cancelled) setSources(null); });
    }
    // 两个数据面各自的后端 scope（§11）：普通 = active（listAllSessions 的默认），
    // 回收站 = trash。projects / bindings 只服务普通模式的筛选列，回收站行不显示它们。
    const trashList = () => api.listSessions(undefined, undefined, "trash");
    Promise.all([
      trashMode ? trashList() : api.listAllSessions(),
      trashMode ? Promise.resolve(null) : api.listProjects(),
      trashMode ? Promise.resolve(null) : api.listSessionBindings(),
      trashMode ? Promise.resolve(null) : trashList().then((rows) => rows.length),
    ])
      .then(([ss, ps, rows, tc]) => {
        if (cancelled) return;
        if (!trashMode) {
          const m = new Map<string, SessionBindingRow[]>();
          for (const r of rows ?? []) {
            const list = m.get(r.session_id) ?? [];
            list.push(r);
            m.set(r.session_id, list);
          }
          setBindings(m);
          setProjects(ps ?? []);
          if (typeof tc === "number") setTrashCount(tc);
        } else {
          setTrashCount(ss.length);
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
  // 页面动作随 Route 到达（palette → New Session）：
  // actionSeq 让「已在 Sessions 页」的重复命令同样触发。
  useEffect(() => {
    if (action === "new") setCreating(true);
  }, [action, actionSeq]);

  const projectNameById = useMemo(
    () => new Map(projects.map((p) => [p.id, p.name])),
    [projects],
  );

  const wsOptions = useMemo(() => {
    const seen = new Map<string, string>();
    for (const rows of bindings.values()) {
      for (const r of rows) seen.set(r.workstream_id, r.workstream_title);
    }
    return [...seen.entries()].sort((a, b) => a[1].localeCompare(b[1], "zh-Hans"));
  }, [bindings]);

  const filtersActive =
    query.trim() !== "" || agent !== "all" || projectId !== "all"
    || wsFilter !== "all" || assigned !== "all";

  const shown = useMemo(() => {
    if (!sessions) return null;
    const q = query.trim().toLowerCase();
    return sessions
      .filter((s) => (agent === "all" ? true : s.agent === agent))
      // 和 Project 徽章同一个判据：只有存在物理锚点的行才算这个 Project 的成员。
      // 缓存列单方面说"是"、而徽章说"历史标签，已不决定任何事"的行，不能一边
      // 显示成无归属、一边又被筛进列表（M34 / §42.3-M29：一个视图不能两个真相）。
      .filter((s) =>
        projectId === "all" ? true : !!s.workspace_path_id && s.project_id === projectId
      )
      .filter((s) => {
        if (wsFilter === "all") return true;
        const rows = bindings.get(s.id) ?? [];
        return wsFilter === "unassigned"
          ? rows.length === 0
          : rows.some((r) => r.workstream_id === wsFilter);
      })
      .filter((s) => {
        if (assigned === "all") return true;
        const has = (bindings.get(s.id) ?? []).length > 0;
        return assigned === "assigned" ? has : !has;
      })
      .filter((s) => {
        if (q === "") return true;
        // 无标题 Session 也要被找到：占位文案与 agent_session_id / id 都进搜索面，
        // 用户照着报障里的 Session ID 能直接定位（AGENTS.md: provenance 可解析）。
        return [
          s.title,
          sessionDisplayTitle(s.title),
          s.cwd,
          s.agent_session_id,
          s.id,
          agentDisplayLabel(s.agent),
          s.project_id ? projectNameById.get(s.project_id) : null,
          (bindings.get(s.id) ?? []).map((b) => b.workstream_title).join(" "),
        ]
          .filter(Boolean)
          .some((t) => (t as string).toLowerCase().includes(q));
      })
      .sort((a, b) =>
        (b.last_activity_at ?? b.started_at ?? "").localeCompare(
          a.last_activity_at ?? a.started_at ?? "",
        ),
      );
  }, [sessions, bindings, query, agent, projectId, wsFilter, assigned, projectNameById]);

  const resume = (sessionId: string) => {
    setResumeModalSessionId(sessionId);
  };

  /** 恢复失败要把后端的拒绝原因原样给出（例如还有未取消的删除任务）。 */
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

  /**
   * 空库的三种情况分开说话（§23「无 Session」不是一条提示能覆盖的）：
   * 一个来源都没启用 / 启用的来源目录不在了 / 来源正常但确实还没跑过。
   * 来源读不到时退回中性说法，不宣称任何一件没被证实的事。
   */
  const noSourcesEnabled = sources !== null && sources.every((s) => !s.enabled);
  const missingSourcePath = sources?.find((s) => s.enabled && !s.exists)?.path ?? "";
  const emptyTitle = noSourcesEnabled ? "还没有启用任何 Session 来源" : "还没有发现本地 Session";
  const emptyHint = noSourcesEnabled
    ? "NoEnding 只读取 Agent 自己目录里的 Session 文件，不会修改它们。到「设置 → Session 来源」勾选要扫描的目录，应用启动时会自动发现本地 Codex、Claude Code 和 Pi 的 Session。"
    : missingSourcePath
      ? `已启用的来源里有目录当前不存在：${missingSourcePath}。接回移动盘或换一台机器时，到「设置 → Session 来源」调整目录即可。`
      : sources === null
        ? "NoEnding 只读取 Agent 自己目录里的 Session 文件，不会修改它们。应用启动时会自动发现本地 Codex、Claude Code 和 Pi 的 Session；也可以新建一个 Session 立刻开始。"
        : "已启用的来源里还没有可发现的 Session。Agent 跑过之后应用会在启动时自动发现；也可以新建一个 Session 立刻开始。";

  const loadFailedState = (
    <EmptyState
      title={trashMode ? "读取回收站失败。" : "读取 Sessions 失败。"}
      hint="本地数据没有被修改。可以重试，或到「设置 → 数据与高级」查看数据库位置。"
      actions={<button className="btn small" onClick={refresh}>重试</button>}
    />
  );

  return (
    <div className="main">
      <PageHeader
        title="Sessions"
        sub="来自 Codex、Claude Code 和 Pi 的本地执行记录。长期主题由 Workstream 承载。"
        actions={
          <>
            {/* 回收站开关（§35）：同一页面的两个数据面，用分段控件而不是筛选器，
                因为两边的行为（列、动作）不同，不是同一张表的条件过滤。 */}
            <div className="settings-seg" role="group" aria-label="Session 列表范围">
              <button
                className={trashMode ? "" : "on"}
                onClick={() => setTrashMode(false)}
              >
                会话列表
              </button>
              <button
                className={trashMode ? "on" : ""}
                onClick={() => setTrashMode(true)}
              >
                回收站{trashCount !== null ? ` (${trashCount})` : ""}
              </button>
            </div>
            <button className="btn primary" onClick={() => setCreating(true)}>新建 Session</button>
          </>
        }
      />

      {!trashMode && (
        <>
          <input
            type="text"
            className="ws-search"
            placeholder="搜索 Sessions…（标题、工作目录、Workstream）"
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
              <span className="muted small">Workstream</span>
              <select value={wsFilter} onChange={(e) => setWsFilter(e.target.value)}>
                <option value="all">全部 Workstream</option>
                {wsOptions.map(([id, title]) => <option key={id} value={id}>{title}</option>)}
                <option value="unassigned">未关联 Workstream</option>
              </select>
            </label>
            <label className="ws-control">
              <span className="muted small">Project</span>
              <select
                value={projectId}
                onChange={(e) => setProjectId(e.target.value)}
                title="Project 是从工作目录派生出来的分组视图，不是 Session 的所有权：这里不能指派 Project。"
              >
                <option value="all">全部 Project</option>
                {projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
                <option value="none">还没有 Project（工作目录未派生）</option>
              </select>
            </label>
            <label className="ws-control">
              <span className="muted small">关联状态</span>
              <select value={assigned} onChange={(e) => setAssigned(e.target.value as AssignedFilter)}>
                <option value="all">全部</option>
                <option value="assigned">已关联</option>
                <option value="unassigned">未关联</option>
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

          {/* v0.2 起 Session 的 Project 由它自己的工作目录派生（方案 §1.10），
              所以这一页不再有「设置 Project」这个动作：分组还能筛，归属不能选。 */}
          <div className="muted small" style={{ marginTop: -10, marginBottom: 14 }}>
            Project 由 Session 的工作目录自动派生，不需要也不能手工指派；按 Project 筛选只是换一种看法。
            想长期推进一件事，请关联 Workstream。
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
                    配置 Session 来源
                  </button>
                  <button className="btn small" onClick={() => setCreating(true)}>新建 Session</button>
                </>
              }
            />
          )}
          {shown !== null && shown.length === 0 && (sessions?.length ?? 0) > 0 && (
            <EmptyState
              title="没有符合当前筛选条件的 Session"
              hint={query.trim()
                ? `没有匹配「${query.trim()}」的 Session。可以换个关键词，或直接清除筛选。`
                : "调整或清除筛选条件即可看到全部 Session。"}
              actions={<button className="btn small" onClick={clearFilters}>清除筛选</button>}
            />
          )}
          {shown !== null && shown.length > 0 && (
            <SessionTable
              sessions={shown}
              bindings={bindings}
              projectNameById={projectNameById}
              onOpen={(id) => navigate({ view: "session", sessionId: id })}
              onResume={resume}
            />
          )}
        </>
      )}

      {trashMode && (
        <>
          <div className="muted small" style={{ margin: "4px 0 14px", maxWidth: "72ch" }}>
            回收站里的 Session 不出现在会话列表、搜索与继续入口中。Agent 原始会话与
            NoEnding 数据都原样保留；只有「永久删除」会连同 Agent 保存的原始会话一起删除。
          </div>

          {sessions === null && !loadFailed && <div className="muted">加载中…</div>}
          {loadFailed && loadFailedState}
          {sessions !== null && sessions.length === 0 && !loadFailed && (
            <EmptyState
              title="回收站是空的。"
              hint="「移入回收站」的 Session 会留在这里：可以随时恢复，也可以在这里永久删除。"
            />
          )}
          {sessions !== null && sessions.length > 0 && (
            <TrashSessionTable
              sessions={sessions}
              onOpen={(id) => navigate({ view: "session", sessionId: id })}
              onRestore={restoreFromTrash}
              onPurge={(s) => setPurgeSessionId(s.id)}
            />
          )}
        </>
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

/**
 * 回收站表格（Session Lifecycle & Deletion §35）：行就是规格里的五段信息——
 * Agent、标题、工作目录、移入时间、恢复 / 永久删除。这里不做筛选与搜索
 * （回收站规模小，规格也不要求）；正常列表的筛选工具在回收站里一律隐藏。
 * 点行仍然可以进详情页：那里对回收站 Session 有专属的横幅与动作。
 */
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
          <th>Session</th>
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
              title={`打开 Session：${title}`}
            >
              <td className="cell-agent">
                <AgentIcon agent={s.agent} />
                {agentDisplayLabel(s.agent)}
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
