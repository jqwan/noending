import TerminalActivityIndicator from "../features/sessions/TerminalActivityIndicator";
import { EVT_REMOVED, removedTargets, type RemovedTargets } from "../app/removedTargets";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { onEvent, EVT_TERMINALS, EVT_TERMINAL_ACTIVITY, EVT_SYNCED, type Route } from "../app/routes";
import Icon from "../components/Icon";
import AgentIcon from "../components/AgentIcon";
import { AGENT_LABELS, type Agent, type TerminalSummary, type SessionListItem, type ProjectCardData, type AgentStatusEntry, type TerminalResumeState } from "../types";
import { sessionDisplayTitle } from "../features/sessions/SessionTable";
import { capsOf, FORMAT_CAPS } from "../features/sessions/sessionFormats";
import { DesktopContinueButton } from "../features/sessions/SessionHeaderActions";
import { openSessionTerminal, terminalContinueState, terminalForContinue, useAgentStatus } from "../features/sessions/continueDesktop";
import { useViewPreference, preferenceValues, booleanPreference, booleanRecordPreference } from "../hooks/useViewState";
import { useTerminalResumeStates } from "../features/sessions/useTerminalResumeStates";
import { preferredTerminal, sessionDestination, terminalDestination } from "../features/sessions/sessionNavigation";

export type SidebarFilterKey = "recent5Days" | "unarchived" | "canContinue";

export const SIDEBAR_FILTER_OPTIONS: Array<{ key: SidebarFilterKey; label: string }> = [
  { key: "recent5Days", label: "最近5天" },
  { key: "unarchived", label: "未归档" },
  { key: "canContinue", label: "可终端继续" },
];

function isSidebarFilters(value: unknown): value is SidebarFilterKey[] {
  return Array.isArray(value) && value.every(item => SIDEBAR_FILTER_OPTIONS.some(option => option.key === item));
}

export type SidebarGroupBy = "project" | "agent" | "none";

export const SIDEBAR_GROUP_OPTIONS: Array<{
  key: SidebarGroupBy;
  label: string;
  fullLabel: string;
  icon: "tasks" | "project" | "bot" | "chat";
}> = [
  { key: "project", label: "项目", fullLabel: "按项目归类", icon: "project" },
  { key: "agent", label: "Agent", fullLabel: "按 Agent 归类", icon: "bot" },
  { key: "none", label: "会话", fullLabel: "不分组（全部会话）", icon: "chat" },
];

export type SidebarSortKey = "createdAt" | "updatedAt";

export const SIDEBAR_SORT_OPTIONS: Array<{ key: SidebarSortKey; label: string }> = [
  { key: "createdAt", label: "创建时间" },
  { key: "updatedAt", label: "最近更新" },
];

export const PINNED_STORAGE_KEY = "noending.sidebar.pinnedSessions";

export function getStoredPinnedSessions(): string[] {
  try {
    const raw = localStorage.getItem(PINNED_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((id): id is string => typeof id === "string") : [];
  } catch {
    return [];
  }
}

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

export function sessionUpdatedTime(session: {
  last_conversation_at?: string | null;
  started_at?: string | null;
  last_activity_at?: string | null;
  terminals?: TerminalSummary[];
}): number {
  if (session.last_conversation_at) {
    const t = Date.parse(session.last_conversation_at);
    if (!isNaN(t) && t > 0) return t;
  }
  return sessionCreatedTime(session);
}

export function compareSidebarSessions(
  a: SidebarSession,
  b: SidebarSession,
  sortBy: SidebarSortKey = "createdAt",
): number {
  const timeA = sortBy === "updatedAt" ? sessionUpdatedTime(a) : sessionCreatedTime(a);
  const timeB = sortBy === "updatedAt" ? sessionUpdatedTime(b) : sessionCreatedTime(b);
  const diff = timeB - timeA;
  return diff !== 0 ? diff : a.id.localeCompare(b.id);
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

export function isSessionWithinSevenDays(session: SessionListItem, now = Date.now()): boolean {
  if (session.archived_at) return false;
  const time = sessionConversationTime(session);
  if (time === null) return false;
  const diff = now - time;
  return diff >= -60000 && diff <= SEVEN_DAYS_MS;
}

export function isSessionTerminalContinuable(
  session: { id: string; archived_at?: string | null; terminals?: TerminalSummary[] },
  resume: TerminalResumeState | undefined,
): boolean {
  return !terminalContinueState(session.id, session.terminals ?? [], resume, !!session.archived_at).disabled;
}

export type SidebarSession = Pick<SessionListItem, "id" | "agent" | "title" | "cwd" | "project_id" | "last_conversation_at" | "archived_at"> & {
  source_kind?: string;
  sessionId: string | null;
  terminals: TerminalSummary[];
  activityAt: number;
  started_at?: string | null;
  last_activity_at?: string | null;
};

export function mergeSidebarSessions(
  sessions: SessionListItem[],
  terminals: TerminalSummary[],
  filters: SidebarFilterKey[] = [],
  sortBy: SidebarSortKey = "createdAt",
  resumeStates: Record<string, TerminalResumeState> = {},
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
    const hasTerminal = row.terminals.length > 0;
    if (filterRecent && !isSessionWithinFiveDays(row, now) && !hasTerminal) {
      continue;
    }
    if (filterUnarchived && row.archived_at && !hasTerminal) {
      continue;
    }
    if (filterCanContinue && !isSessionTerminalContinuable(row, resumeStates[row.id]) && !hasTerminal) {
      continue;
    }
    result.push(row);
  }

  return result.sort((a, b) => compareSidebarSessions(a, b, sortBy));
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

function getProjectGroupTime(
  group: SidebarSessionGroup,
  projectMap: Map<string, ProjectCardData>,
  sortBy: SidebarSortKey = "createdAt",
): number {
  if (group.id !== "__unassigned_project__" && projectMap.has(group.id)) {
    const card = projectMap.get(group.id)!;
    if (sortBy === "updatedAt") {
      const raw = card.last_activity_at || card.updated_at || card.created_at;
      if (raw) {
        const t = Date.parse(raw);
        if (!isNaN(t) && t > 0) return Math.max(t, group.latestTime);
      }
    } else {
      if (card.created_at) {
        const t = Date.parse(card.created_at);
        if (!isNaN(t) && t > 0) return Math.max(t, group.latestTime);
      }
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
  groupBy: SidebarGroupBy,
  sortBy: SidebarSortKey = "createdAt",
  projectMap: Map<string, ProjectCardData>,
): SidebarSessionGroup[] {
  if (groupBy === "none") {
    const sorted = [...sessions].sort((a, b) => compareSidebarSessions(a, b, sortBy));
    return [{
      id: "__flat__",
      title: "全部会话",
      latestTime: sorted.length > 0 ? (sortBy === "updatedAt" ? sessionUpdatedTime(sorted[0]) : sessionCreatedTime(sorted[0])) : 0,
      sessions: sorted,
    }];
  }

  const groupsMap = new Map<string, SidebarSessionGroup>();

  for (const s of sessions) {
    let groupId: string;
    let groupTitle: string;

    if (groupBy === "project") {
      if (s.project_id && projectMap.has(s.project_id)) {
        groupId = s.project_id;
        groupTitle = projectMap.get(s.project_id)!.name;
      } else if (s.project_id) {
        groupId = s.project_id;
        groupTitle = "未知项目";
      } else {
        groupId = "__unassigned_project__";
        groupTitle = "无项目";
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
        groupTitle = "未知 Agent";
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

  // 1. 分组内会话列表按配置的排序方式进行排序 (descending)
  for (const group of groupsMap.values()) {
    group.sessions.sort((a, b) => compareSidebarSessions(a, b, sortBy));
    group.latestTime = group.sessions.length > 0
      ? (sortBy === "updatedAt" ? sessionUpdatedTime(group.sessions[0]) : sessionCreatedTime(group.sessions[0]))
      : 0;
  }

  // 2. 项目和 Agent 分组按创建时间或最近更新排序；无项目/未知 Agent 固定沉底
  const groupsList = Array.from(groupsMap.values());

  if (groupBy === "project") {
    return groupsList.sort((a, b) => {
      const isUnassignedA = a.id === "__unassigned_project__";
      const isUnassignedB = b.id === "__unassigned_project__";
      if (isUnassignedA !== isUnassignedB) {
        return isUnassignedA ? 1 : -1;
      }
      const timeA = getProjectGroupTime(a, projectMap, sortBy);
      const timeB = getProjectGroupTime(b, projectMap, sortBy);
      if (timeB !== timeA) return timeB - timeA;
      return a.title.localeCompare(b.title, "zh-Hans");
    });
  } else {
    return groupsList.sort((a, b) => {
      const isUnknownA = a.id === "__unknown_agent__";
      const isUnknownB = b.id === "__unknown_agent__";
      if (isUnknownA !== isUnknownB) {
        return isUnknownA ? 1 : -1;
      }
      if (b.latestTime !== a.latestTime) {
        return b.latestTime - a.latestTime;
      }
      const canA = isAgentGroupContinuable(a);
      const canB = isAgentGroupContinuable(b);
      if (canA !== canB) {
        return canA ? -1 : 1;
      }
      return a.title.localeCompare(b.title, "zh-Hans");
    });
  }
}

function SidebarSessionRow({ session, route, navigate, agentStatus, loading, isPinned, onTogglePin, showIcon = true, resumeState }: {
  session: SidebarSession; route: Route; navigate: (r: Route) => void;
  agentStatus: Record<string, AgentStatusEntry> | null; loading: boolean;
  isPinned?: boolean; onTogglePin?: () => void;
  showIcon?: boolean;
  resumeState?: TerminalResumeState;
}) {
  const s = session;
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const active = route.view === "session" && route.sessionId === s.sessionId || route.view === "terminal" && s.terminals.some(t => t.terminal_id === route.terminalId);
  const title = sessionDisplayTitle(s.title);
  const selected = s.sessionId ? terminalForContinue(s.sessionId, s.terminals) : s.terminals[0];
  const statusPeers = [...s.terminals].sort((a, b) => a.created_at.localeCompare(b.created_at) || a.terminal_id.localeCompare(b.terminal_id));
  const statusTerminal = (route.view === "terminal" ? s.terminals.find(t => t.terminal_id === route.terminalId) : undefined)
    ?? (s.sessionId ? route.view === "session" && route.sessionId === s.sessionId
      ? terminalForContinue(s.sessionId, statusPeers) : preferredTerminal(s.sessionId, statusPeers) : selected);
  const caps = capsOf(s.agent, s.source_kind);
  const availability = terminalContinueState(s.sessionId, s.terminals, resumeState, !!s.archived_at);
  const connecting = busy && !selected?.live;
  const terminalLabel = connecting ? "终端 · 正在连接" : selected ? `终端 · ${selected.live ? "已连接" : "已退出"}` : "在终端中继续";
  const terminalCount = s.terminals.length;
  const runningCount = s.terminals.filter(t => t.live).length;
  const hasLiveTerminal = runningCount > 0;
  const isTerminalActive = route.view === "terminal" && s.terminals.some(t => t.terminal_id === route.terminalId);
  const counts = `${runningCount} 个已连接终端，${s.terminals.filter(t => !t.live).length} 个已退出终端`;
  const openTerminal = async () => {
    if (pending.current) return;
    if (!s.sessionId) { if (selected) navigate(terminalDestination(selected)); return; }
    pending.current = true;
    setBusy(true);
    try { await openSessionTerminal({ ...s, id: s.sessionId, title }, navigate); }
    finally { pending.current = false; setBusy(false); }
  };
  const onSelectSession = () => {
    navigate(s.sessionId ? sessionDestination(s.sessionId, s.terminals, { title, agent: s.agent }) : terminalDestination(s.terminals[0]));
  };
  const titleRef = useRef<HTMLSpanElement>(null);
  const textRef = useRef<HTMLSpanElement>(null);
  const [overflow, setOverflow] = useState(0);
  const [hovered, setHovered] = useState(false);

  const hasAction = Boolean(selected || caps.terminal || s.sessionId);
  const onMouseEnter = () => {
    setHovered(true);
    if (titleRef.current) {
      const actionsWidth = (hasAction && onTogglePin) ? 58 : (hasAction || onTogglePin ? 34 : 0);
      const available = actionsWidth > 0 ? titleRef.current.clientWidth - actionsWidth : titleRef.current.clientWidth;
      const textWidth = textRef.current
        ? (textRef.current.getBoundingClientRect().width || textRef.current.scrollWidth)
        : 0;
      const contentWidth = textWidth > 0 ? textWidth : (titleRef.current.scrollWidth > titleRef.current.clientWidth ? titleRef.current.scrollWidth : 0);
      const diff = Math.ceil(contentWidth - available);
      setOverflow(diff > 2 && contentWidth > available ? diff : 0);
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
      {showIcon && <AgentIcon agent={s.agent} size={14} />}
      <span
        ref={titleRef}
        className={`sidebar-session-title${isScrolling ? " scrolling" : ""}`}
        style={isScrolling ? ({
          "--overflow-width": `${overflow}px`,
          "--scroll-duration": `${duration}s`,
        } as React.CSSProperties) : undefined}
      >
        <span ref={textRef} className="sidebar-session-title-text">{title}</span>
      </span>
    </button>
    {statusTerminal && <span className="sidebar-session-activity">
      <TerminalActivityIndicator terminal={statusTerminal} />
    </span>}
    <span className="sidebar-session-actions">
      {onTogglePin && (
        <button
          type="button"
          className={`sidebar-session-action pin-action${isPinned ? " pinned" : ""}`}
          aria-label={isPinned ? `取消置顶：${title}` : `置顶会话：${title}`}
          title={isPinned ? "取消置顶" : "置顶会话"}
          onClick={(e) => {
            e.stopPropagation();
            onTogglePin();
          }}
        >
          <Icon name="pin" />
        </button>
      )}
      {(selected || caps.terminal) ? <button
        className={`sidebar-session-action terminal-action${connecting ? " terminal-connecting" : hasLiveTerminal ? " terminal-running" : (terminalCount > 0 ? " terminal-exited" : "")}${isTerminalActive ? " active" : ""}`}
        aria-label={`${terminalLabel}：${title}`}
        title={connecting ? "正在连接终端" : availability.disabled ? availability.title : selected?.live ? `${terminalLabel} · ${counts}` : availability.title}
        aria-busy={connecting} disabled={busy || loading || availability.disabled}
        onClick={(e) => {
          e.stopPropagation();
          void openTerminal();
        }}>
        <Icon name={connecting ? "refresh" : "terminal"} />
      </button> : s.sessionId && <DesktopContinueButton sessionId={s.sessionId} agent={s.agent} sourceKind={s.source_kind}
        archived={!!s.archived_at} agentStatus={agentStatus} className="sidebar-session-action" label={`在桌面应用中继续：${title}`} />}
    </span>
  </div>;
}

function SidebarGroupHeader({
  group,
  isCollapsed,
  isUnassignedGroup,
  isKnownAgent,
  iconName,
  onToggle,
}: {
  group: SidebarSessionGroup;
  isCollapsed: boolean;
  isUnassignedGroup: boolean;
  isKnownAgent: boolean;
  iconName: "tasks" | "tasksExpanded" | "project" | "projectOpen" | "bot" | "minus" | "inbox" | "chevronRight" | "chevronDown";
  onToggle: () => void;
}) {
  const titleRef = useRef<HTMLSpanElement>(null);
  const textRef = useRef<HTMLSpanElement>(null);
  const [overflow, setOverflow] = useState(0);
  const [hovered, setHovered] = useState(false);

  const onMouseEnter = () => {
    setHovered(true);
    if (titleRef.current) {
      const available = titleRef.current.clientWidth;
      const textWidth = textRef.current
        ? (textRef.current.getBoundingClientRect().width || textRef.current.scrollWidth)
        : 0;
      const contentWidth = textWidth > 0 ? textWidth : (titleRef.current.scrollWidth > titleRef.current.clientWidth ? titleRef.current.scrollWidth : 0);
      const diff = Math.ceil(contentWidth - available);
      setOverflow(diff > 2 && contentWidth > available ? diff : 0);
    }
  };

  const onMouseLeave = () => {
    setHovered(false);
    setOverflow(0);
  };

  const isScrolling = hovered && overflow > 0;
  const duration = Math.max(2.5, overflow / 30);

  return (
    <button
      type="button"
      className={`sidebar-recent-group-header ${isCollapsed ? "collapsed" : ""}${isUnassignedGroup ? " unassigned-group" : ""}`}
      onClick={onToggle}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      aria-expanded={!isCollapsed}
      title={`${group.title} (${group.sessions.length})`}
    >
      <span className={`sidebar-group-icon${isKnownAgent ? " agent-group-icon" : ""}`}>
        {isKnownAgent ? (
          <AgentIcon agent={group.id as Agent} size={14} />
        ) : (
          <Icon name={iconName} />
        )}
      </span>
      <span
        ref={titleRef}
        className={`sidebar-recent-group-title${isScrolling ? " scrolling" : " truncate"}`}
        style={isScrolling ? ({
          "--overflow-width": `${overflow}px`,
          "--scroll-duration": `${duration}s`,
        } as React.CSSProperties) : undefined}
      >
        <span ref={textRef} className="sidebar-recent-group-title-text">{group.title}</span>
      </span>
      <span className="sidebar-recent-count">{group.sessions.length}</span>
    </button>
  );
}

const MAX_GROUP_SESSIONS_DEFAULT = 5;

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
  const statePrefix = "sidebar";
  const agentStatus = useAgentStatus();
  const [terminalsLoading, setTerminalsLoading] = useState(true);
  const [terminals, setTerminals] = useState<TerminalSummary[]>([]);
  const [sessions, setSessions] = useState<SessionListItem[]>([]);
  const [projects, setProjects] = useState<ProjectCardData[]>([]);
  const [groupBy, setGroupBy] = useViewPreference<SidebarGroupBy>(`${statePrefix}.recent.groupBy.v2`, "project", preferenceValues("project", "agent", "none"));
  const [collapsedGroups, setCollapsedGroups] = useViewPreference<Record<string, boolean>>(`${statePrefix}.recent.collapsedGroups`, {}, booleanRecordPreference);
  const [expandedGroupSessions, setExpandedGroupSessions] = useState<Record<string, boolean>>({});
  const [flatVisibleCount, setFlatVisibleCount] = useState(100);

  useEffect(() => {
    setExpandedGroupSessions({});
  }, [groupBy]);

  const refreshSeq = useRef(0);

  const refreshTerminals = useCallback(() => {
    const seq = ++refreshSeq.current;
    api
      .terminalList()
      .then((next) => {
        if (seq === refreshSeq.current) { setTerminals(next.filter(t => !removedTargets.terminals.has(t.terminal_id) && !removedTargets.sessions.has(t.session_id ?? ""))); setTerminalsLoading(false); }
      })
      .catch(error => { if (seq === refreshSeq.current) setTerminalsLoading(false); console.error(error); });
  }, []);

  const refreshSessions = useCallback(() => {
    Promise.all([
      api.listSessions(undefined, undefined, "all"),
      api.listProjectCards(),
    ])
      .then(([sList, prjList]) => {
        setSessions(sList.filter(s => !removedTargets.sessions.has(s.id)));
        setProjects(prjList.filter(p => !removedTargets.projects.has(p.id)));
      })
      .catch(console.error);
  }, []);

  useEffect(() => {
    refreshTerminals();
    return () => { ++refreshSeq.current; };
  }, [refreshTerminals]);
  useEffect(() => onEvent(EVT_TERMINALS, refreshTerminals), [refreshTerminals]);
  useEffect(() => onEvent(EVT_TERMINAL_ACTIVITY, refreshTerminals), [refreshTerminals]);
  useEffect(refreshSessions, [refreshSessions]);
  useEffect(() => onEvent(EVT_TERMINALS, refreshSessions), [refreshSessions]);
  useEffect(() => onEvent(EVT_SYNCED, refreshSessions), [refreshSessions]);

  const [filters, setFilters] = useViewPreference<SidebarFilterKey[]>(`${statePrefix}.recent.filters`, [], isSidebarFilters);
  const [runningOnly, setRunningOnly] = useViewPreference<boolean>(`${statePrefix}.recent.runningOnly`, false, booleanPreference);
  const [showSessionIcons, setShowSessionIcons] = useViewPreference<boolean>(
    `${statePrefix}.recent.showSessionIcons`,
    true,
    booleanPreference,
  );
  const [filterOpen, setFilterOpen] = useState(false);
  const filterRef = useRef<HTMLDivElement>(null);

  const [groupOpen, setGroupOpen] = useState(false);
  const groupRef = useRef<HTMLDivElement>(null);

  const currentGroupOpt = useMemo(() => {
    return SIDEBAR_GROUP_OPTIONS.find((opt) => opt.key === groupBy) ?? SIDEBAR_GROUP_OPTIONS[0];
  }, [groupBy]);

  useEffect(() => {
    if (!filterOpen && !groupOpen) return;
    const onClickOutside = (e: MouseEvent) => {
      const target = e.target as Node;
      if (filterOpen && filterRef.current && !filterRef.current.contains(target)) {
        setFilterOpen(false);
      }
      if (groupOpen && groupRef.current && !groupRef.current.contains(target)) {
        setGroupOpen(false);
      }
    };
    document.addEventListener("mousedown", onClickOutside);
    return () => document.removeEventListener("mousedown", onClickOutside);
  }, [filterOpen, groupOpen]);

  const toggleFilter = (key: SidebarFilterKey) => {
    setFilters((prev) =>
      prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key],
    );
  };

  const projectMap = useMemo(() => {
    const map = new Map<string, ProjectCardData>();
    for (const p of projects) {
      map.set(p.id, p);
    }
    return map;
  }, [projects]);

  const [pinnedSessionIds, setPinnedSessionIds] = useState<string[]>(getStoredPinnedSessions);
  const [pinnedCollapsed, setPinnedCollapsed] = useViewPreference<boolean>(`${statePrefix}.pinned.collapsed`, false, booleanPreference);

  useEffect(() => {
    const removed = (event: Event) => {
      const targets = (event as CustomEvent<RemovedTargets>).detail;
      setSessions(rows => rows.filter(s => !removedTargets.sessions.has(s.id)));
      setTerminals(rows => rows.filter(t => !removedTargets.terminals.has(t.terminal_id) && !removedTargets.sessions.has(t.session_id ?? "")));
      setProjects(rows => rows.filter(p => !removedTargets.projects.has(p.id)));
      if (targets.sessionIds?.length || targets.terminalIds?.length) setPinnedSessionIds(ids => {
        const next = ids.filter(id => !removedTargets.sessions.has(id) && !removedTargets.terminals.has(id.replace(/^terminal:/, "")));
        try { localStorage.setItem(PINNED_STORAGE_KEY, JSON.stringify(next)); } catch { /* In-memory removal still applies. */ }
        return next;
      });
    };
    window.addEventListener(EVT_REMOVED, removed);
    return () => window.removeEventListener(EVT_REMOVED, removed);
  }, []);

  const isSessionPinned = useCallback(
    (s: { id: string; sessionId: string | null }) => {
      return pinnedSessionIds.includes(s.id) || (Boolean(s.sessionId) && pinnedSessionIds.includes(s.sessionId!));
    },
    [pinnedSessionIds],
  );

  const togglePin = useCallback((id: string, sessionId?: string | null) => {
    setPinnedSessionIds((prev) => {
      const isPinned = prev.includes(id) || (Boolean(sessionId) && prev.includes(sessionId!));
      let next: string[];
      if (isPinned) {
        next = prev.filter((item) => item !== id && item !== sessionId);
      } else {
        next = [...prev, sessionId || id];
      }
      try {
        localStorage.setItem(PINNED_STORAGE_KEY, JSON.stringify(next));
      } catch (err) {
        console.error(err);
      }
      return next;
    });
  }, []);

  const [sortBy, setSortBy] = useViewPreference<SidebarSortKey>(`${statePrefix}.recent.sortBy`, "createdAt", preferenceValues("createdAt", "updatedAt"));

  const currentSortOpt = useMemo(() => {
    return SIDEBAR_SORT_OPTIONS.find((opt) => opt.key === sortBy) ?? SIDEBAR_SORT_OPTIONS[0];
  }, [sortBy]);

  const allMergedSessions = useMemo(() => {
    return mergeSidebarSessions(
      sessions,
      terminals,
      [],
      sortBy,
    );
  }, [sessions, terminals, sortBy]);

  const pinnedSessions = useMemo(() => {
    if (pinnedSessionIds.length === 0) return [];
    return allMergedSessions
      .filter((s) => isSessionPinned(s))
      .sort((a, b) => compareSidebarSessions(a, b, sortBy));
  }, [allMergedSessions, pinnedSessionIds, isSessionPinned, sortBy]);

  // Only visible actions need filesystem/CLI prerequisites. The explicit
  // "can continue" filter needs all candidates checked to classify them.
  const resumeCheckIds = useMemo(() => {
    if (collapsed || hidden) return [];
    let candidates: SidebarSession[];
    if (filters.includes("canContinue")) {
      candidates = allMergedSessions;
    } else {
      const groups = groupSidebarSessions(
        mergeSidebarSessions(sessions, terminals, filters, sortBy)
          .filter(s => !isSessionPinned(s) && (!runningOnly || s.terminals.some(t => t.live))),
        groupBy, sortBy, projectMap,
      );
      candidates = groups.flatMap(group => {
        if (groupBy === "none") return group.sessions.slice(0, flatVisibleCount);
        if (collapsedGroups[`${groupBy}:${group.id}`]) return [];
        return expandedGroupSessions[group.id] ? group.sessions : group.sessions.slice(0, MAX_GROUP_SESSIONS_DEFAULT);
      });
    }
    if (!pinnedCollapsed) candidates = [...candidates, ...pinnedSessions];
    return candidates.filter(s => s.sessionId && !s.archived_at && capsOf(s.agent, s.source_kind).terminal)
      .map(s => s.sessionId!);
  }, [collapsed, hidden, filters, allMergedSessions, sessions, terminals, sortBy, isSessionPinned,
    runningOnly, groupBy, projectMap, flatVisibleCount, collapsedGroups, expandedGroupSessions, pinnedCollapsed, pinnedSessions]);
  const resumeStates = useTerminalResumeStates(resumeCheckIds);

  const sessionGroups = useMemo(() => {
    return groupSidebarSessions(
      mergeSidebarSessions(
        sessions,
        terminals,
        filters,
        sortBy,
        resumeStates,
      ).filter((s) => !isSessionPinned(s) && (!runningOnly || s.terminals.some((t) => t.live))),
      groupBy,
      sortBy,
      projectMap,
    ).filter((group) => group.sessions.length > 0);
  }, [sessions, terminals, filters, runningOnly, groupBy, sortBy, projectMap, isSessionPinned, resumeStates]);

  const collapseKey = (groupId: string) => `${groupBy}:${groupId}`;
  const toggleGroup = (groupId: string) => {
    const key = collapseKey(groupId);
    setCollapsedGroups((prev) => {
      const nextCollapsed = !prev[key];
      if (nextCollapsed) {
        setExpandedGroupSessions((exp) => {
          if (!exp[groupId]) return exp;
          const next = { ...exp };
          delete next[groupId];
          return next;
        });
      }
      return {
        ...prev,
        [key]: nextCollapsed,
      };
    });
  };

  const allGroupsCollapsed =
    sessionGroups.length > 0 &&
    sessionGroups.every((g) => Boolean(collapsedGroups[collapseKey(g.id)]));

  const toggleAllGroups = () => {
    setCollapsedGroups(prev => {
      const next = { ...prev };
      for (const group of sessionGroups) {
        const key = collapseKey(group.id);
        if (allGroupsCollapsed) delete next[key];
        else next[key] = true;
      }
      return next;
    });
    setExpandedGroupSessions({});
  };

  return (
    <aside className={`sidebar${collapsed ? " collapsed" : ""}`} hidden={collapsed || hidden} aria-label="会话侧边栏">
      <div className="sidebar-top">
        <button
          type="button"
          className={`sidebar-top-btn ${route.view === "new-session" ? "active" : ""}`}
          onClick={() => navigate({ view: "new-session" })}
        >
          <span className="sidebar-header-icon">
            <Icon name="chatPlus" />
          </span>
          <span className="sidebar-header-title">新建</span>
        </button>
      </div>
      <div className="sidebar-scroll">

        {pinnedSessions.length > 0 && (
          <div className="sidebar-pinned-section">
            <button
              type="button"
              className={`sidebar-recent-group-header sidebar-pinned-header ${pinnedCollapsed ? "collapsed" : ""}`}
              onClick={() => setPinnedCollapsed((prev) => !prev)}
              aria-expanded={!pinnedCollapsed}
              title={`置顶 (${pinnedSessions.length})`}
            >
              <span className="sidebar-header-icon pinned-group-icon">
                <Icon name="pin" />
              </span>
              <span className="sidebar-header-title truncate">置顶</span>
              <span className="sidebar-recent-count">{pinnedSessions.length}</span>
            </button>
            {!pinnedCollapsed && (
              <div className="sidebar-recent-group-items">
                {pinnedSessions.map((s) => (
                  <SidebarSessionRow
                    key={`pinned:${s.id}`}
                    session={s} resumeState={resumeStates[s.sessionId ?? ""]}
                    route={route}
                    navigate={navigate}
                    agentStatus={agentStatus}
                    loading={terminalsLoading}
                    isPinned={true}
                    onTogglePin={() => togglePin(s.id, s.sessionId)}
                    showIcon={showSessionIcons}
                  />
                ))}
              </div>
            )}
          </div>
        )}

        <div className="nav-section sidebar-section-header">
          <div className="sidebar-group-wrapper" ref={groupRef}>
            <button
              type="button"
              className={`sidebar-group-trigger${groupOpen ? " active" : ""}`}
              onClick={() => setGroupOpen((prev) => !prev)}
              aria-label={`会话归类：${currentGroupOpt.fullLabel}`}
              title={`切换会话归类方式（当前：${currentGroupOpt.fullLabel}）`}
              aria-expanded={groupOpen}
            >
              <span className="sidebar-header-icon">
                <Icon name={currentGroupOpt.icon} />
              </span>
              <span className="sidebar-header-title">{currentGroupOpt.label}</span>
              <Icon name="chevronDown" className="sidebar-group-arrow" />
            </button>
            {groupOpen && (
              <div className="sidebar-group-menu" role="menu">
                <div className="sidebar-group-menu-header">
                  <span>会话归类</span>
                </div>
                {SIDEBAR_GROUP_OPTIONS.map((opt) => {
                  const active = groupBy === opt.key;
                  return (
                    <button
                      key={opt.key}
                      type="button"
                      className={`sidebar-group-item${active ? " checked" : ""}`}
                      onClick={() => {
                        setGroupBy(opt.key);
                        setGroupOpen(false);
                      }}
                      role="menuitemradio"
                      aria-checked={active}
                    >
                      <span className="sidebar-group-item-icon">
                        <Icon name={opt.icon} />
                      </span>
                      <span className="sidebar-group-item-label">{opt.fullLabel}</span>
                      {active && (
                        <span className="sidebar-group-item-check">
                          <Icon name="check" />
                        </span>
                      )}
                    </button>
                  );
                })}
              </div>
            )}
          </div>

          <div className="sidebar-header-actions">
            <button
              type="button"
              className={`sidebar-filter-trigger sidebar-running-filter${runningOnly ? " active" : ""}`}
              onClick={() => setRunningOnly((prev) => !prev)}
              aria-label="仅显示运行中会话"
              aria-pressed={runningOnly}
              title={runningOnly ? "关闭运行中筛选（不影响置顶区）" : "仅显示有运行中终端的会话（不影响置顶区）"}
            >
              <Icon name="terminal" />
            </button>

            <button
              type="button"
              className="sidebar-collapse-all-btn"
              onClick={toggleAllGroups}
              disabled={groupBy === "none" || sessionGroups.length === 0}
              aria-label={allGroupsCollapsed ? "展开全部" : "收起全部"}
              title={groupBy === "none" ? "未分组" : (allGroupsCollapsed ? "展开全部" : "收起全部")}
            >
              <Icon name={allGroupsCollapsed ? "expandAll" : "collapseAll"} />
            </button>

            <div className="sidebar-filter-wrapper" ref={filterRef}>
              <button
                type="button"
                className={`sidebar-filter-trigger${filters.length > 0 ? " active" : ""}`}
                onClick={() => setFilterOpen((prev) => !prev)}
                aria-label="筛选与排序会话"
                title={filters.length > 0 ? `已筛选 ${filters.length} 项 · ${currentSortOpt.label}` : `筛选与排序（当前：${currentSortOpt.label}）`}
                aria-expanded={filterOpen}
              >
                <Icon name="sliders" />
              </button>
              {filterOpen && (
                <div className="sidebar-filter-menu" role="menu">
                  <div className="sidebar-filter-menu-header">
                    <span>排序方式</span>
                  </div>
                  {SIDEBAR_SORT_OPTIONS.map((opt) => {
                    const checked = sortBy === opt.key;
                    return (
                      <button
                        key={opt.key}
                        type="button"
                        className={`sidebar-filter-item radio${checked ? " checked" : ""}`}
                        onClick={() => setSortBy(opt.key)}
                        role="menuitemradio"
                        aria-checked={checked}
                      >
                        <span className="sidebar-filter-radio">
                          {checked && <span className="sidebar-filter-radio-dot" />}
                        </span>
                        <span className="sidebar-filter-label">{opt.label}</span>
                      </button>
                    );
                  })}

                  <div className="sidebar-filter-menu-divider" />

                  <div className="sidebar-filter-menu-header">
                    <span>筛选条件</span>
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

                  <div className="sidebar-filter-menu-divider" />

                  <div className="sidebar-filter-menu-header">
                    <span>显示设置</span>
                  </div>
                  <button
                    type="button"
                    className={`sidebar-filter-item${showSessionIcons ? " checked" : ""}`}
                    onClick={() => setShowSessionIcons((prev) => !prev)}
                    role="menuitemcheckbox"
                    aria-checked={showSessionIcons}
                  >
                    <span className="sidebar-filter-check">
                      {showSessionIcons && <Icon name="check" />}
                    </span>
                    <span className="sidebar-filter-label">显示会话图标</span>
                  </button>
                </div>
              )}
            </div>
          </div>
        </div>

        {sessionGroups.length === 0 ? (
          <div className="sidebar-recent-empty">
            {runningOnly ? "暂无运行中会话" : filters.length > 0 ? "无匹配会话" : pinnedSessions.length > 0 ? "暂无未置顶会话" : "暂无会话"}
          </div>
        ) : groupBy === "none" ? (
          <div className="sidebar-flat-session-items">
            {sessionGroups[0].sessions.slice(0, flatVisibleCount).map((s) => (
              <SidebarSessionRow
                key={s.id}
                session={s} resumeState={resumeStates[s.sessionId ?? ""]}
                route={route}
                navigate={navigate}
                agentStatus={agentStatus}
                loading={terminalsLoading}
                isPinned={isSessionPinned(s)}
                onTogglePin={() => togglePin(s.id, s.sessionId)}
                showIcon={showSessionIcons}
              />
            ))}
            {sessionGroups[0].sessions.length > flatVisibleCount && (
              <button className="btn small ghost" onClick={() => setFlatVisibleCount(count => count + 100)}>
                显示更多（剩余 {sessionGroups[0].sessions.length - flatVisibleCount}）
              </button>
            )}
          </div>
        ) : (
          sessionGroups.map((group) => {
            const isCollapsed = Boolean(collapsedGroups[collapseKey(group.id)]);
            const isUnassignedGroup =
              group.id === "__unassigned_project__";
            const isKnownAgent = groupBy === "agent" && group.id in AGENT_LABELS;
            const iconName = isUnassignedGroup
              ? (isCollapsed ? "chevronRight" : "chevronDown")
              : groupBy === "project"
              ? (isCollapsed ? "project" : "projectOpen")
              : "bot";

            return (
              <div className="sidebar-recent-group" key={group.id}>
                <SidebarGroupHeader
                  group={group}
                  isCollapsed={isCollapsed}
                  isUnassignedGroup={isUnassignedGroup}
                  isKnownAgent={isKnownAgent}
                  iconName={iconName}
                  onToggle={() => toggleGroup(group.id)}
                />
                {!isCollapsed && (() => {
                  const isExpanded = Boolean(expandedGroupSessions[group.id]);
                  const visibleSessions = isExpanded
                    ? group.sessions
                    : group.sessions.slice(0, MAX_GROUP_SESSIONS_DEFAULT);
                  const hasMore = group.sessions.length > MAX_GROUP_SESSIONS_DEFAULT;

                  return (
                    <div className={`sidebar-recent-group-items${showSessionIcons ? " has-icons" : ""}`}>
                      {visibleSessions.map((s) => (
                        <SidebarSessionRow
                          key={s.id}
                          session={s} resumeState={resumeStates[s.sessionId ?? ""]}
                          route={route}
                          navigate={navigate}
                          agentStatus={agentStatus}
                          loading={terminalsLoading}
                          isPinned={isSessionPinned(s)}
                          onTogglePin={() => togglePin(s.id, s.sessionId)}
                          showIcon={showSessionIcons}
                        />
                      ))}
                      {hasMore && !isExpanded && (
                        <button
                          type="button"
                          className="sidebar-group-expand-btn"
                          onClick={() =>
                            setExpandedGroupSessions((prev) => ({
                              ...prev,
                              [group.id]: true,
                            }))
                          }
                        >
                          展开显示
                        </button>
                      )}
                    </div>
                  );
                })()}
              </div>
            );
          })
        )}
      </div>

    </aside>
  );
}
