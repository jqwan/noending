import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { onEvent, EVT_TERMINALS, EVT_SYNCED, type Route } from "../app/routes";
import Icon from "../components/Icon";
import AgentIcon from "../components/AgentIcon";
import { AGENT_LABELS, type Agent, type TerminalSummary, type Session, type WorkstreamCardData, type ProjectCardData, type AgentStatusEntry } from "../types";
import { sessionDisplayTitle } from "../features/sessions/SessionTable";
import { capsOf } from "../features/sessions/sessionFormats";
import { DesktopContinueButton } from "../features/sessions/SessionHeaderActions";
import { openSessionTerminal, useAgentStatus } from "../features/sessions/continueDesktop";
import { useViewState } from "../hooks/useViewState";
import { preferredTerminal, sessionDestination, terminalDestination } from "../features/sessions/sessionNavigation";

const SEVEN_DAYS_MS = 7 * 24 * 60 * 60 * 1000;

export function sessionConversationTime(session: Pick<Session, "last_conversation_at">): number | null {
  if (!session.last_conversation_at) return null;
  const time = new Date(session.last_conversation_at).getTime();
  return isNaN(time) ? null : time;
}

export function isSessionWithinSevenDays(session: Session, now = Date.now()): boolean {
  if (session.archived_at) return false;
  const time = sessionConversationTime(session);
  if (time === null) return false;
  const diff = now - time;
  return diff >= -60000 && diff <= SEVEN_DAYS_MS;
}

type SidebarSession = Pick<Session, "id" | "agent" | "title" | "cwd" | "owner_workstream_id" | "project_id" | "last_conversation_at" | "archived_at"> & {
  source_kind?: string;
  sessionId: string | null;
  terminals: TerminalSummary[];
  activityAt: number;
};

export function mergeSidebarSessions(sessions: Session[], terminals: TerminalSummary[], currentSessionId?: string): SidebarSession[] {
  const rows = new Map<string, SidebarSession>();
  for (const session of sessions) {
    if (isSessionWithinSevenDays(session) || session.id === currentSessionId || terminals.some(t => t.session_id === session.id)) {
      rows.set(session.id, { ...session, sessionId: session.id, terminals: [], activityAt: sessionConversationTime(session) ?? 0 });
    }
  }
  for (const terminal of terminals) {
    const key = terminal.session_id ?? `terminal:${terminal.terminal_id}`;
    let row = rows.get(key);
    if (!row) {
      row = { id: key, sessionId: terminal.session_id, agent: terminal.agent, title: terminal.session_title ?? "新会话", cwd: terminal.cwd,
        owner_workstream_id: null, project_id: null, last_conversation_at: null, archived_at: null, terminals: [], activityAt: 0 };
      rows.set(key, row);
    }
    row.terminals.push(terminal);
    row.activityAt = Math.max(row.activityAt, Date.parse(terminal.created_at) || 0);
  }
  return [...rows.values()].sort((a, b) => b.activityAt - a.activityAt);
}

export interface SidebarSessionGroup {
  id: string;
  title: string;
  latestTime: number;
  sessions: SidebarSession[];
}

export function groupSidebarSessions(
  sessions: SidebarSession[],
  groupBy: "workstream" | "project" | "agent",
  workstreamMap: Map<string, WorkstreamCardData>,
  projectMap: Map<string, ProjectCardData>,
): SidebarSessionGroup[] {

  const groupsMap = new Map<string, SidebarSessionGroup>();

  for (const s of sessions) {
    const time = s.activityAt;

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
        latestTime: time,
        sessions: [],
      };
      groupsMap.set(groupId, group);
    } else {
      if (time > group.latestTime) {
        group.latestTime = time;
      }
    }
    group.sessions.push(s);
  }

  // Sort sessions within each group by timestamp desc
  for (const group of groupsMap.values()) {
    group.sessions.sort((a, b) => {
      const ta = sessionConversationTime(a) ?? 0;
      const tb = sessionConversationTime(b) ?? 0;
      return tb - ta;
    });
  }

  // Sort groups by latest session timestamp desc
  return Array.from(groupsMap.values()).sort((a, b) => b.latestTime - a.latestTime);
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
  const counts = `${s.terminals.filter(t => t.live).length} 个运行中终端，${s.terminals.filter(t => !t.live).length} 个已退出终端`;
  const openTerminal = async () => {
    if (pending.current) return;
    if (!s.sessionId) { if (selected) navigate(terminalDestination(selected)); return; }
    pending.current = true;
    setBusy(true);
    try { await openSessionTerminal({ ...s, id: s.sessionId, title }, s.terminals, navigate); }
    finally { pending.current = false; setBusy(false); }
  };
  return <div className={`nav-item sidebar-recent-item ${active ? "active" : ""}`}>
    <button className="sidebar-session-link" title={`${s.cwd ? `${title} · ${s.cwd}` : title}${s.archived_at ? " · 已归档" : ""}`}
      onClick={() => navigate(s.sessionId ? sessionDestination(s.sessionId, s.terminals, { title, agent: s.agent }) : terminalDestination(s.terminals[0]))}>
      <AgentIcon agent={s.agent} size={14} />
      {s.terminals.length > 0 && <span role="img" aria-label={counts} title={counts}
        className={`sidebar-session-dot ${s.terminals.some(t => t.live) ? "running" : "exited"}`} />}
      <span className="truncate">{title}</span>
    </button>
    <span className="sidebar-session-actions">
      {(selected || caps.terminal) ? <button className="sidebar-session-action"
        aria-label={`${terminalLabel}：${title}`} title={selected ? `${terminalLabel} · ${counts}` : terminalLabel}
        disabled={busy || !selected && (loading || !s.sessionId || !!s.archived_at)} onClick={() => void openTerminal()}>
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
    return groupSidebarSessions(mergeSidebarSessions(sessions, terminals, route.view === "session" ? route.sessionId : undefined), groupBy, workstreamMap, projectMap);
  }, [sessions, terminals, route, groupBy, workstreamMap, projectMap]);

  const toggleGroup = (groupId: string) => {
    setCollapsedGroups((prev) => ({
      ...prev,
      [groupId]: !prev[groupId],
    }));
  };

  return (
    <aside className={`sidebar${collapsed ? " collapsed" : ""}`} hidden={hidden} aria-label="会话侧边栏">
      <div className="sidebar-scroll">
        <button className={`nav-item ${route.view === "new-session" ? "active" : ""}`} onClick={() => navigate({ view: "new-session" })}>
          <Icon name="chatPlus" />新建
        </button>
        <button className={`nav-item ${route.view === "sessions" ? "active" : ""}`} onClick={() => navigate({ view: "sessions" })}>
          <Icon name="chat" />会话
        </button>

        <div className="nav-section">会话</div>
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
          <div className="sidebar-recent-empty">最近 7 天无新消息</div>
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
