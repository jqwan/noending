import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { onEvent, EVT_TERMINALS, EVT_SYNCED, type Route } from "../app/routes";
import Icon from "../components/Icon";
import SidebarLogo from "../components/SidebarLogo";
import AgentIcon from "../components/AgentIcon";
import type { TerminalSummary, Session, WorkstreamCardData, ProjectCardData } from "../types";
import { sessionDisplayTitle } from "../features/sessions/SessionTable";
import { timeAgo } from "../components/common";
import { useViewState } from "../hooks/useViewState";
import { disposeTerminal } from "../features/sessions/terminalCache";

const SEVEN_DAYS_MS = 7 * 24 * 60 * 60 * 1000;

export function isSessionWithinSevenDays(session: Session, now = Date.now()): boolean {
  if (session.archived_at) return false;
  const timeStr = session.last_conversation_at || session.last_activity_at;
  if (!timeStr) return false;
  const ts = new Date(timeStr).getTime();
  if (isNaN(ts)) return false;
  const diff = now - ts;
  return diff >= -60000 && diff <= SEVEN_DAYS_MS;
}

export interface SidebarRecentGroup {
  id: string;
  title: string;
  latestTime: number;
  sessions: Session[];
}

export function groupRecentSessions(
  sessions: Session[],
  groupBy: "workstream" | "project",
  workstreamMap: Map<string, WorkstreamCardData>,
  projectMap: Map<string, ProjectCardData>,
  now = Date.now(),
): SidebarRecentGroup[] {
  const recent = sessions.filter((s) => isSessionWithinSevenDays(s, now));

  const groupsMap = new Map<string, SidebarRecentGroup>();

  for (const s of recent) {
    const timeStr = s.last_conversation_at || s.last_activity_at;
    const time = timeStr ? new Date(timeStr).getTime() : 0;

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
    } else {
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
      const ta = new Date(a.last_conversation_at || a.last_activity_at || 0).getTime();
      const tb = new Date(b.last_conversation_at || b.last_activity_at || 0).getTime();
      return tb - ta;
    });
  }

  // Sort groups by latest session timestamp desc
  return Array.from(groupsMap.values()).sort((a, b) => b.latestTime - a.latestTime);
}

/**
 * Sidebar：Brand→新会话、Search、工作区一级导航（Workstreams / Projects / Sessions /
 * Assistant）、运行中（含保留的已退出终端）、最近活动（最近 7 天有新消息的会话，按任务/项目归类）、
 * 底部固定 Settings。
 */
export default function Sidebar({ route, navigate, onSearch, collapsed = false }: {
  route: Route;
  navigate: (r: Route) => void;
  onSearch: () => void;
  collapsed?: boolean;
}) {
  const [terminals, setTerminals] = useState<TerminalSummary[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [workstreams, setWorkstreams] = useState<WorkstreamCardData[]>([]);
  const [projects, setProjects] = useState<ProjectCardData[]>([]);
  const [groupBy, setGroupBy] = useViewState<"workstream" | "project">("sidebar.recent.groupBy", "workstream");
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});

  const refreshSeq = useRef(0);
  const currentRoute = useRef(route);
  currentRoute.current = route;

  const refreshTerminals = useCallback(() => {
    const seq = ++refreshSeq.current;
    api
      .terminalList()
      .then((next) => {
        if (seq === refreshSeq.current) setTerminals(next);
      })
      .catch(console.error);
  }, []);

  const refreshRecent = useCallback(() => {
    Promise.all([
      api.listSessions(),
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

  /** 「运行中」子项的显式关闭：杀掉内嵌 Agent 并移出列表。关闭时正看着
   *  这个终端 → 跳回它的会话（未绑定则回会话看板），不让用户停在尸体上。 */
  const closeTerminal = async (t: TerminalSummary) => {
    try {
      const closed = await api.terminalClose(t.terminal_id);
      disposeTerminal(t.terminal_id);
      setTerminals((current) => current.filter((item) => item.terminal_id !== t.terminal_id));
      const displayed = currentRoute.current;
      if (displayed.view === "terminal" && displayed.terminalId === t.terminal_id) {
        if (closed.session_id) navigate({ view: "session", sessionId: closed.session_id });
        else navigate({ view: "sessions" });
      }
    } catch (e) {
      console.error(e);
    }
  };

  useEffect(() => {
    refreshTerminals();
    return () => { ++refreshSeq.current; };
  }, [refreshTerminals]);
  useEffect(() => onEvent(EVT_TERMINALS, refreshTerminals), [refreshTerminals]);
  useEffect(refreshRecent, [refreshRecent]);
  useEffect(() => onEvent(EVT_TERMINALS, refreshRecent), [refreshRecent]);
  useEffect(() => onEvent(EVT_SYNCED, refreshRecent), [refreshRecent]);

  const workspaceActive = (v: "workstreams" | "projects" | "sessions" | "agents" | "assistant") => {
    if (route.view === v) return "active";
    // Workstream Detail → Workstreams 保持弱高亮
    if (v === "workstreams" && route.view === "workstream") return "weak";
    if (v === "sessions" && route.view === "session") return "active";
    // Project Detail → Projects 弱高亮，与 Workstreams 同一模式
    if (v === "projects" && route.view === "project") return "weak";
    return "";
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

  const recentGroups = useMemo(() => {
    return groupRecentSessions(sessions, groupBy, workstreamMap, projectMap);
  }, [sessions, groupBy, workstreamMap, projectMap]);

  const toggleGroup = (groupId: string) => {
    setCollapsedGroups((prev) => ({
      ...prev,
      [groupId]: !prev[groupId],
    }));
  };

  return (
    <div className={`sidebar${collapsed ? " collapsed" : ""}`}>
      <button className="brand" onClick={() => navigate({ view: "new-session" })} title="新会话">
        <SidebarLogo size={22} />
        <span className="brand-name">NoEnding</span>
      </button>

      <div className="sidebar-scroll">
        <button className={`nav-item ${route.view === "new-session" ? "active" : ""}`} onClick={() => navigate({ view: "new-session" })}>
          <Icon name="plus" />新会话
        </button>
        <button className="nav-item" onClick={onSearch}>
          <Icon name="search" />搜索
          <span style={{ flex: 1 }} />
          <span className="kbd">{/Macintosh|Mac OS X/.test(navigator.userAgent) ? "⌘K" : "Ctrl K"}</span>
        </button>

        <div className="nav-section">工作区</div>
        <button className={`nav-item ${workspaceActive("workstreams")}`}
          onClick={() => navigate({ view: "workstreams" })}>
          <Icon name="tasks" />任务
        </button>
        {/* Projects 成为一等导航项：Workstream=我正在做什么，
            Project=我在哪里做，Session=我做过哪些执行 */}
        <button className={`nav-item ${workspaceActive("projects")}`}
          onClick={() => navigate({ view: "projects" })}>
          <Icon name="folder" />项目
        </button>
        <button className={`nav-item ${workspaceActive("sessions")}`}
          onClick={() => navigate({ view: "sessions" })}>
          <Icon name="chat" />会话
        </button>
        <button className={`nav-item ${workspaceActive("agents")}`}
          onClick={() => navigate({ view: "agents" })}>
          <Icon name="bot" />代理
        </button>
        <button className={`nav-item ${workspaceActive("assistant")}`}
          onClick={() => navigate({ view: "assistant" })}>
          <Icon name="spark" />助手
        </button>

        {terminals.length > 0 && (
          <>
            <div className="nav-section">运行中</div>
            {terminals.map((t) => {
              const active = route.view === "terminal" && route.terminalId === t.terminal_id;
              const title = t.session_id && t.session_title
                ? sessionDisplayTitle(t.session_title)
                : "新会话";
              return (
                <div className="sidebar-task" key={t.terminal_id}>
                  <button
                    className={`nav-item ${active ? "active" : ""}${t.live ? "" : " terminal-exited"}`}
                    title={`${t.cwd ? `${title} · ${t.cwd}` : title}${t.live ? "" : " · 已退出"}`}
                    onClick={() => navigate({ view: "terminal", terminalId: t.terminal_id })}
                  >
                    <AgentIcon agent={t.agent} size={15} />
                    <span className="truncate">{title}</span>
                  </button>
                  <button
                    className="pin-button terminal-close"
                    aria-label={`${t.live ? "关闭" : "移除"}终端：${title}`}
                    title={t.live ? "关闭终端" : "移除终端"}
                    onClick={() => void closeTerminal(t)}
                  >
                    <Icon name="close" />
                  </button>
                </div>
              );
            })}
          </>
        )}

        {/* 最近活动 */}
        <div className="nav-section">最近活动</div>
        <div className="sidebar-recent-switcher">
          <div className="sidebar-pill-seg" role="radiogroup" aria-label="最近活动归类方式">
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
              <Icon name="folder" />
              <span>项目</span>
            </button>
          </div>
        </div>

        {recentGroups.length === 0 ? (
          <div className="sidebar-recent-empty">最近 7 天无新消息</div>
        ) : (
          recentGroups.map((group) => {
            const isCollapsed = Boolean(collapsedGroups[group.id]);
            const iconName =
              groupBy === "workstream"
                ? (isCollapsed ? "tasksCollapsed" : "tasks")
                : (isCollapsed ? "folder" : "folderOpen");

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
                    <Icon name={iconName} />
                  </span>
                  <span className="sidebar-recent-group-title truncate">{group.title}</span>
                  <span className="sidebar-recent-count">{group.sessions.length}</span>
                </button>
                {!isCollapsed && (
                  <div className="sidebar-recent-group-items">
                    {group.sessions.map((s) => {
                      const active = route.view === "session" && route.sessionId === s.id;
                      const title = sessionDisplayTitle(s.title);
                      const timeStr = s.last_conversation_at || s.last_activity_at;
                      const tooltip = s.cwd ? `${title} · ${s.cwd}` : title;
                      return (
                        <button
                          key={s.id}
                          type="button"
                          className={`nav-item sidebar-recent-item ${active ? "active" : ""}`}
                          title={tooltip}
                          onClick={() => navigate({ view: "session", sessionId: s.id })}
                        >
                          <AgentIcon agent={s.agent} size={14} />
                          <span className="truncate" style={{ flex: 1 }}>{title}</span>
                          {timeStr && <span className="sidebar-recent-time">{timeAgo(timeStr)}</span>}
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>

      <div className="sidebar-footer">
        <button
          className={`nav-item ${route.view === "settings" ? "active" : ""}`}
          onClick={() => navigate({ view: "settings" })}
        >
          <Icon name="settings" />设置
        </button>
      </div>

    </div>
  );
}
