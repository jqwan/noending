import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { onEvent, EVT_TERMINALS, EVT_SYNCED, type Route } from "../app/routes";
import Icon from "../components/Icon";
import AgentIcon from "../components/AgentIcon";
import { AGENT_LABELS, type Agent, type TerminalSummary, type Session, type WorkstreamCardData, type ProjectCardData, type AgentStatusEntry } from "../types";
import { sessionDisplayTitle } from "../features/sessions/SessionTable";
import { capsOf, FORMAT_CAPS } from "../features/sessions/sessionFormats";
import { DesktopContinueButton } from "../features/sessions/SessionHeaderActions";
import { openSessionTerminal, useAgentStatus } from "../features/sessions/continueDesktop";
import { useViewState } from "../hooks/useViewState";
import { preferredTerminal, sessionDestination, terminalDestination } from "../features/sessions/sessionNavigation";

export type SidebarFilterKey = "recent5Days" | "unarchived" | "canContinue";

export const SIDEBAR_FILTER_OPTIONS: Array<{ key: SidebarFilterKey; label: string }> = [
  { key: "recent5Days", label: "最近5天" },
  { key: "unarchived", label: "未归档" },
  { key: "canContinue", label: "可终端继续" },
];

export const FIVE_DAYS_MS = 5 * 24 * 60 * 60 * 1000;
export const SEVEN_DAYS_MS = 7 * 24 * 60 * 60 * 1000;

export function sessionConversationTime(session: { last_conversation_at?: string | null }): number | null {
  if (!session.last_conversation_at) return null;
  const time = new Date(session.last_conversation_at).getTime();
  return isNaN(time) ? null : time;
}

export function sessionCreatedTime(session: {
  started_at?: string | null;
  last_conversation_at?: string | null;
  last_activity_at?: string | null;
  terminals?: TerminalSummary[];
}): number {
  if (session.started_at) {
    const t = Date.parse(session.started_at);
    if (!isNaN(t) && t > 0) return t;
  }
  if (session.terminals && session.terminals.length > 0 && session.terminals[0].created_at) {
    const t = Date.parse(session.terminals[0].created_at);
    if (!isNaN(t) && t > 0) return t;
  }
  if (session.last_conversation_at) {
    const t = Date.parse(session.last_conversation_at);
    if (!isNaN(t) && t > 0) return t;
  }
  if (session.last_activity_at) {
    const t = Date.parse(session.last_activity_at);
    if (!isNaN(t) && t > 0) return t;
  }
  return 0;
}

export function isSessionWithinFiveDays(
  session: {
    last_conversation_at?: string | null;
    terminals?: TerminalSummary[];
  },
  now = Date.now(),
): boolean {
  const time = sessionConversationTime(session)
    ?? (session.terminals && session.terminals.length > 0 && session.terminals[0].created_at ? Date.parse(session.terminals[0].created_at) : null);
  if (time === null || isNaN(time)) return false;
  const diff = now - time;
  return diff >= -60000 && diff <= FIVE_DAYS_MS;
}

export function isSessionWithinSevenDays(session: Session, now = Date.now()): boolean {
  if (session.archived_at) return false;
  const time = sessionConversationTime(session);
  if (time === null) return false;
  const diff = now - time;
  return diff >= -60000 && diff <= SEVEN_DAYS_MS;
}

export function isSessionTerminalContinuable(
  session: Pick<Session, "agent" | "archived_at"> & { source_kind?: string; terminals?: TerminalSummary[] },
): boolean {
  if (session.terminals && session.terminals.length > 0) return true;
  const caps = capsOf(session.agent, session.source_kind);
  return caps.terminal && !session.archived_at;
}

export type SidebarSession = Pick<Session, "id" | "agent" | "title" | "cwd" | "owner_workstream_id" | "project_id" | "last_conversation_at" | "archived_at"> & {
  source_kind?: string;
  sessionId: string | null;
  terminals: TerminalSummary[];
  activityAt: number;
  started_at?: string | null;
  last_activity_at?: string | null;
};

export function mergeSidebarSessions(
  sessions: Session[],
  terminals: TerminalSummary[],
  currentSessionId?: string,
  filters: SidebarFilterKey[] = [],
): SidebarSession[] {
  const rows = new Map<string, SidebarSession>();
  for (const session of sessions) {
    rows.set(session.id, {
      ...session,
      sessionId: session.id,
      terminals: [],
      activityAt: sessionConversationTime(session) ?? 0,
      started_at: session.started_at,
      last_activity_at: session.last_activity_at,
    });
  }
  for (const terminal of terminals) {
    const key = terminal.session_id ?? `terminal:${terminal.terminal_id}`;
    let row = rows.get(key);
    if (!row) {
      row = {
        id: key,
        sessionId: terminal.session_id,
        agent: terminal.agent,
        title: terminal.session_title ?? "新会话",
        cwd: terminal.cwd,
        owner_workstream_id: terminal.owner_workstream_id,
        project_id: null,
        last_conversation_at: null,
        archived_at: null,
        started_at: terminal.created_at,
        last_activity_at: terminal.created_at,
        terminals: [],
        activityAt: 0,
      };
      rows.set(key, row);
    }
    row.terminals.push(terminal);
    row.activityAt = Math.max(row.activityAt, Date.parse(terminal.created_at) || 0);
  }

  const now = Date.now();
  const filterRecent = filters.includes("recent5Days");
  const filterUnarchived = filters.includes("unarchived");
  const filterCanContinue = filters.includes("canContinue");

  const result: SidebarSession[] = [];
  for (const row of rows.values()) {
    const isCurrent = Boolean(currentSessionId && row.sessionId === currentSessionId);
    if (filterRecent && !isSessionWithinFiveDays(row, now) && !isCurrent) {
      continue;
    }
    if (filterUnarchived && row.archived_at && !isCurrent) {
      continue;
    }
    if (filterCanContinue && !isSessionTerminalContinuable(row) && !isCurrent) {
      continue;
    }
    result.push(row);
  }

  return result.sort((a, b) => sessionCreatedTime(b) - sessionCreatedTime(a));
}

export interface SidebarSessionGroup {
  id: string;
  title: string;
  latestTime: number;
  sessions: SidebarSession[];
}

export function isAgentTerminalContinuable(agentKey: string): boolean {
  if (FORMAT_CAPS[agentKey]?.terminal) return true;
  if (agentKey === "antigravity" && FORMAT_CAPS.antigravity_cli?.terminal) return true;
  if (FORMAT_CAPS[`${agentKey}_cli`]?.terminal) return true;
  return false;
}

function getWorkstreamGroupTime(group: SidebarSessionGroup, workstreamMap: Map<string, WorkstreamCardData>): number {
  if (group.id !== "__unassigned_workstream__" && workstreamMap.has(group.id)) {
    const card = workstreamMap.get(group.id)!;
    const t = Date.parse(card.created_at);
    if (!isNaN(t) && t > 0) return t;
  }
  return group.latestTime;
}

function getProjectGroupTime(group: SidebarSessionGroup, projectMap: Map<string, ProjectCardData>): number {
  if (group.id !== "__unassigned_project__" && projectMap.has(group.id)) {
    const card = projectMap.get(group.id)!;
    if (card.created_at) {
      const t = Date.parse(card.created_at);
      if (!isNaN(t) && t > 0) return t;
    }
  }
  return group.latestTime;
}

function isAgentGroupContinuable(group: SidebarSessionGroup): boolean {
  if (isAgentTerminalContinuable(group.id)) return true;
  return group.sessions.some(s => s.terminals.length > 0);
}

export function groupSidebarSessions(
  sessions: SidebarSession[],
  groupBy: "workstream" | "project" | "agent",
  workstreamMap: Map<string, WorkstreamCardData>,
  projectMap: Map<string, ProjectCardData>,
): SidebarSessionGroup[] {
  const groupsMap = new Map<string, SidebarSessionGroup>();

  for (const s of sessions) {
    let groupId: string;
    let groupTitle: string;

    if (groupBy === "workstream") {
      if (s.owner_workstream_id && workstreamMap.has(s.owner_workstream_id)) {
        groupId = s.owner_workstream_id;
        groupTitle = workstreamMap.get(s.owner_workstream_id)!.title;
      } else if (s.owner_workstream_id) {
        groupId = s.owner_workstream_id;
        groupTitle = "未知任务";
      } else {
        groupId = "__unassigned_workstream__";
        groupTitle = "未归属任务";
      }
    } else if (groupBy === "project") {
      if (s.project_id && projectMap.has(s.project_id)) {
        groupId = s.project_id;
        groupTitle = projectMap.get(s.project_id)!.name;
      } else if (s.project_id) {
        groupId = s.project_id;
        groupTitle = "未知项目";
      } else {
        groupId = "__unassigned_project__";
        groupTitle = "未归属项目";
      }
    } else {
      const rawAgent = s.agent as string;
      const agentKey = rawAgent === "claude" ? "claude_code" : rawAgent;
      if (agentKey && agentKey in AGENT_LABELS) {
        groupId = agentKey;
        groupTitle = AGENT_LABELS[agentKey as Agent];
      } else if (agentKey) {
        groupId = agentKey;
        groupTitle = agentKey;
      } else {
        groupId = "__unknown_agent__";
        groupTitle = "未知代理";
      }
    }

    let group = groupsMap.get(groupId);
    if (!group) {
      group = {
        id: groupId,
        title: groupTitle,
        latestTime: 0,
        sessions: [],
      };
      groupsMap.set(groupId, group);
    }
    group.sessions.push(s);
  }

  // 1. 分组内会话列表按创建时间或者发现时间进行排序 (descending)
  for (const group of groupsMap.values()) {
    group.sessions.sort((a, b) => {
      const diff = sessionCreatedTime(b) - sessionCreatedTime(a);
      return diff !== 0 ? diff : a.id.localeCompare(b.id);
    });
    group.latestTime = group.sessions.length > 0 ? sessionCreatedTime(group.sessions[0]) : 0;
  }

  // 2. 项目/任务 也是按创建/发现时间排序，代理按是否可终端继续和名称进行排序
  const groupsList = Array.from(groupsMap.values());

  if (groupBy === "workstream") {
    return groupsList.sort((a, b) => {
      const timeA = getWorkstreamGroupTime(a, workstreamMap);
      const timeB = getWorkstreamGroupTime(b, workstreamMap);
      if (timeB !== timeA) return timeB - timeA;
      return a.title.localeCompare(b.title, "zh-Hans");
    });
  } else if (groupBy === "project") {
    return groupsList.sort((a, b) => {
      const timeA = getProjectGroupTime(a, projectMap);
      const timeB = getProjectGroupTime(b, projectMap);
      if (timeB !== timeA) return timeB - timeA;
      return a.title.localeCompare(b.title, "zh-Hans");
    });
  } else {
    return groupsList.sort((a, b) => {
      const canA = isAgentGroupContinuable(a);
      const canB = isAgentGroupContinuable(b);
      if (canA !== canB) {
        return canA ? -1 : 1;
      }
      return a.title.localeCompare(b.title, "zh-Hans");
    });
  }
}

function SidebarSessionRow({ session, route, navigate, agentStatus, loading }: {
  session: SidebarSession; route: Route; navigate: (r: Route) => void;
  agentStatus: Record<string, AgentStatusEntry> | null; loading: boolean;
}) {
  const s = session;
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const active = route.view === "session" && route.sessionId === s.sessionId || route.view === "terminal" && s.terminals.some(t => t.terminal_id === route.terminalId);
  const title = sessionDisplayTitle(s.title);
  const selected = s.sessionId ? preferredTerminal(s.sessionId, s.terminals) : s.terminals[0];
  const caps = capsOf(s.agent, s.source_kind);
  const terminalLabel = selected ? `终端 · ${selected.live ? "运行中" : "已退出"}` : "在终端中继续";
  const terminalCount = s.terminals.length;
  const runningCount = s.terminals.filter(t => t.live).length;
  const hasLiveTerminal = runningCount > 0;
  const isTerminalActive = route.view === "terminal" && s.terminals.some(t => t.terminal_id === route.terminalId);
  const counts = `${runningCount} 个运行中终端，${s.terminals.filter(t => !t.live).length} 个已退出终端`;
  const openTerminal = async () => {
    if (pending.current) return;
    if (!s.sessionId) { if (selected) navigate(terminalDestination(selected)); return; }
    pending.current = true;
    setBusy(true);
    try { await openSessionTerminal({ ...s, id: s.sessionId, title }, s.terminals, navigate); }
    finally { pending.current = false; setBusy(false); }
  };
  const onSelectSession = () => {
    navigate(s.sessionId ? sessionDestination(s.sessionId, s.terminals, { title, agent: s.agent }) : terminalDestination(s.terminals[0]));
  };
  const titleRef = useRef<HTMLSpanElement>(null);
  const [overflow, setOverflow] = useState(0);
  const [hovered, setHovered] = useState(false);

  const hasAction = Boolean(selected || caps.terminal || s.sessionId);
  const onMouseEnter = () => {
    setHovered(true);
    if (titleRef.current) {
      const available = hasAction ? titleRef.current.clientWidth - 36 : titleRef.current.clientWidth;
      const diff = titleRef.current.scrollWidth - available;
      setOverflow(diff > 2 ? diff : 0);
    }
  };

  const onMouseLeave = () => {
    setHovered(false);
    setOverflow(0);
  };

  const isScrolling = hovered && overflow > 0;
  const duration = Math.max(2.5, overflow / 30);

  return <div className={`nav-item sidebar-recent-item ${active ? "active" : ""}`}
    onMouseEnter={onMouseEnter}
    onMouseLeave={onMouseLeave}
    onClick={(e) => {
      if ((e.target as HTMLElement).closest(".sidebar-session-action")) return;
      onSelectSession();
    }}>
    <button className="sidebar-session-link" title={`${s.cwd ? `${title} · ${s.cwd}` : title}${s.archived_at ? " · 已归档" : ""}`}
      onClick={(e) => {
        e.stopPropagation();
        onSelectSession();
      }}>
      <AgentIcon agent={s.agent} size={14} />
      <span
        ref={titleRef}
        className={`sidebar-session-title${isScrolling ? " scrolling" : ""}`}
        style={isScrolling ? ({
          "--overflow-width": `${overflow}px`,
          "--scroll-duration": `${duration}s`,
        } as React.CSSProperties) : undefined}
      >
        <span className="sidebar-session-title-text">{title}</span>
      </span>
      {s.terminals.length > 0 && <span role="img" aria-label={counts} title={counts}
        className={`sidebar-session-dot ${hasLiveTerminal ? "running" : "exited"}`} />}
    </button>
    <span className="sidebar-session-actions">
      {(selected || caps.terminal) ? <button
        className={`sidebar-session-action terminal-action${hasLiveTerminal ? " terminal-running" : (terminalCount > 0 ? " terminal-exited" : "")}${isTerminalActive ? " active" : ""}`}
        aria-label={`${terminalLabel}：${title}`}
        title={selected ? `${terminalLabel} · ${counts}` : terminalLabel}
        disabled={busy || !selected && (loading || !s.sessionId || !!s.archived_at)}
        onClick={(e) => {
          e.stopPropagation();
          void openTerminal();
        }}>
        <Icon name="terminal" />
      </button> : s.sessionId && <DesktopContinueButton sessionId={s.sessionId} agent={s.agent} sourceKind={s.source_kind}
        archived={!!s.archived_at} agentStatus={agentStatus} className="sidebar-session-action" label={`在桌面应用中继续：${title}`} />}
    </span>
  </div>;
}

/**
 * 会话侧边栏：新建、会话看板、最近会话与保留终端的统一列表。
 * 工作区导航由最左侧固定图标栏提供；切换工作区时保留本栏的数据与分组状态。
 */
export default function Sidebar({ route, navigate, collapsed = false, hidden = false }: {
  route: Route;
  navigate: (r: Route) => void;
  collapsed?: boolean;
  hidden?: boolean;
}) {
  const agentStatus = useAgentStatus();
  const [terminalsLoading, setTerminalsLoading] = useState(true);
  const [terminals, setTerminals] = useState<TerminalSummary[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [workstreams, setWorkstreams] = useState<WorkstreamCardData[]>([]);
  const [projects, setProjects] = useState<ProjectCardData[]>([]);
  const [groupBy, setGroupBy] = useViewState<"workstream" | "project" | "agent">("sidebar.recent.groupBy", "workstream");
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});

  const refreshSeq = useRef(0);

  const refreshTerminals = useCallback(() => {
    const seq = ++refreshSeq.current;
    api
      .terminalList()
      .then((next) => {
        if (seq === refreshSeq.current) { setTerminals(next); setTerminalsLoading(false); }
      })
      .catch(error => { if (seq === refreshSeq.current) setTerminalsLoading(false); console.error(error); });
  }, []);

  const refreshSessions = useCallback(() => {
    Promise.all([
      api.listSessions(undefined, undefined, "all"),
      api.listWorkstreamCards(),
      api.listProjectCards(),
    ])
      .then(([sList, wsList, prjList]) => {
        setSessions(sList);
        setWorkstreams(wsList);
        setProjects(prjList);
      })
      .catch(console.error);
  }, []);

  useEffect(() => {
    refreshTerminals();
    return () => { ++refreshSeq.current; };
  }, [refreshTerminals]);
  useEffect(() => onEvent(EVT_TERMINALS, refreshTerminals), [refreshTerminals]);
  useEffect(refreshSessions, [refreshSessions]);
  useEffect(() => onEvent(EVT_TERMINALS, refreshSessions), [refreshSessions]);
  useEffect(() => onEvent(EVT_SYNCED, refreshSessions), [refreshSessions]);

  const [filters, setFilters] = useViewState<SidebarFilterKey[]>("sidebar.recent.filters", []);
  const [filterOpen, setFilterOpen] = useState(false);
  const filterRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!filterOpen) return;
    const onClickOutside = (e: MouseEvent) => {
      if (filterRef.current && !filterRef.current.contains(e.target as Node)) {
        setFilterOpen(false);
      }
    };
    document.addEventListener("mousedown", onClickOutside);
    return () => document.removeEventListener("mousedown", onClickOutside);
  }, [filterOpen]);

  const toggleFilter = (key: SidebarFilterKey) => {
    setFilters((prev) =>
      prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key],
    );
  };

  const workstreamMap = useMemo(() => {
    const map = new Map<string, WorkstreamCardData>();
    for (const ws of workstreams) {
      map.set(ws.id, ws);
    }
    return map;
  }, [workstreams]);

  const projectMap = useMemo(() => {
    const map = new Map<string, ProjectCardData>();
    for (const p of projects) {
      map.set(p.id, p);
    }
    return map;
  }, [projects]);

  const sessionGroups = useMemo(() => {
    return groupSidebarSessions(
      mergeSidebarSessions(
        sessions,
        terminals,
        route.view === "session" ? route.sessionId : undefined,
        filters,
      ),
      groupBy,
      workstreamMap,
      projectMap,
    );
  }, [sessions, terminals, route, filters, groupBy, workstreamMap, projectMap]);

  const toggleGroup = (groupId: string) => {
    setCollapsedGroups((prev) => ({
      ...prev,
      [groupId]: !prev[groupId],
    }));
  };

  return (
    <aside className={`sidebar${collapsed ? " collapsed" : ""}`} hidden={collapsed || hidden} aria-label="会话侧边栏">
      <div className="sidebar-scroll">
        <button className={`nav-item ${route.view === "new-session" ? "active" : ""}`} onClick={() => navigate({ view: "new-session" })}>
          <Icon name="chatPlus" />新建
        </button>

        <div className="nav-section sidebar-section-header">
          <span>工作区</span>
          <div className="sidebar-filter-wrapper" ref={filterRef}>
            <button
              type="button"
              className={`sidebar-filter-trigger${filters.length > 0 ? " active" : ""}`}
              onClick={() => setFilterOpen((prev) => !prev)}
              aria-label="筛选会话"
              title={filters.length > 0 ? `已筛选 ${filters.length} 项` : "筛选会话"}
              aria-expanded={filterOpen}
            >
              <Icon name="filter" />
              {filters.length > 0 && <span className="sidebar-filter-badge">{filters.length}</span>}
            </button>
            {filterOpen && (
              <div className="sidebar-filter-menu" role="menu">
                <div className="sidebar-filter-menu-header">
                  <span>筛选</span>
                  {filters.length > 0 && (
                    <button
                      type="button"
                      className="sidebar-filter-clear-btn"
                      onClick={() => setFilters([])}
                    >
                      重置
                    </button>
                  )}
                </div>
                {SIDEBAR_FILTER_OPTIONS.map((opt) => {
                  const checked = filters.includes(opt.key);
                  return (
                    <button
                      key={opt.key}
                      type="button"
                      className={`sidebar-filter-item${checked ? " checked" : ""}`}
                      onClick={() => toggleFilter(opt.key)}
                      role="menuitemcheckbox"
                      aria-checked={checked}
                    >
                      <span className="sidebar-filter-check">
                        {checked && <Icon name="check" />}
                      </span>
                      <span className="sidebar-filter-label">{opt.label}</span>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </div>
        <div className="sidebar-recent-switcher">
          <div className="sidebar-pill-seg" role="radiogroup" aria-label="会话归类方式">
            <button
              type="button"
              className={`sidebar-pill-btn ${groupBy === "workstream" ? "active" : ""}`}
              onClick={() => setGroupBy("workstream")}
              role="radio"
              aria-checked={groupBy === "workstream"}
              aria-label="按任务归类"
              title="按任务归类"
            >
              <Icon name="tasks" />
              <span>任务</span>
            </button>
            <button
              type="button"
              className={`sidebar-pill-btn ${groupBy === "project" ? "active" : ""}`}
              onClick={() => setGroupBy("project")}
              role="radio"
              aria-checked={groupBy === "project"}
              aria-label="按项目归类"
              title="按项目归类"
            >
              <Icon name="project" />
              <span>项目</span>
            </button>
            <button
              type="button"
              className={`sidebar-pill-btn ${groupBy === "agent" ? "active" : ""}`}
              onClick={() => setGroupBy("agent")}
              role="radio"
              aria-checked={groupBy === "agent"}
              aria-label="按代理归类"
              title="按代理归类"
            >
              <Icon name="bot" />
              <span>代理</span>
            </button>
          </div>
        </div>

        {sessionGroups.length === 0 ? (
          <div className="sidebar-recent-empty">
            {filters.length > 0 ? "无匹配会话" : "暂无会话"}
          </div>
        ) : (
          sessionGroups.map((group) => {
            const isCollapsed = Boolean(collapsedGroups[group.id]);
            const isKnownAgent = groupBy === "agent" && group.id in AGENT_LABELS;
            const iconName =
              groupBy === "workstream"
                ? (isCollapsed ? "tasksCollapsed" : "tasks")
                : groupBy === "project"
                ? "project"
                : "bot";

            return (
              <div className="sidebar-recent-group" key={group.id}>
                <button
                  type="button"
                  className={`sidebar-recent-group-header ${isCollapsed ? "collapsed" : ""}`}
                  onClick={() => toggleGroup(group.id)}
                  aria-expanded={!isCollapsed}
                  title={`${group.title} (${group.sessions.length})`}
                >
                  <span className="sidebar-group-icon">
                    {isKnownAgent ? (
                      <AgentIcon agent={group.id as Agent} size={14} />
                    ) : (
                      <Icon name={iconName} />
                    )}
                  </span>
                  <span className="sidebar-recent-group-title truncate">{group.title}</span>
                  <span className="sidebar-recent-count">{group.sessions.length}</span>
                </button>
                {!isCollapsed && (
                  <div className="sidebar-recent-group-items">
                    {group.sessions.map(s => <SidebarSessionRow key={s.id} session={s} route={route} navigate={navigate} agentStatus={agentStatus} loading={terminalsLoading} />)}
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>

    </aside>
  );
}
