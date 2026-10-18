import { AGENT_LABELS, type Agent, type TerminalSummary, type SessionListItem, type ProjectCardData, type TerminalResumeState } from "../types";
import { FORMAT_CAPS } from "../features/sessions/sessionFormats";
import { terminalContinueState } from "../features/sessions/continueDesktop";

export type SidebarFilterKey = "recent5Days" | "unarchived" | "canContinue";

export const SIDEBAR_FILTER_OPTIONS: Array<{ key: SidebarFilterKey; label: string }> = [
  { key: "recent5Days", label: "最近5天" },
  { key: "unarchived", label: "未归档" },
  { key: "canContinue", label: "可终端继续" },
];

export function isSidebarFilters(value: unknown): value is SidebarFilterKey[] {
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
export const MAX_GROUP_SESSIONS_DEFAULT = 5;

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
