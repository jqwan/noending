import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import { onEvent, EVT_TERMINALS, type Route } from "../app/routes";
import Icon from "../components/Icon";
import SidebarLogo from "../components/SidebarLogo";
import AgentIcon from "../components/AgentIcon";
import type { TerminalSummary } from "../types";
import { sessionDisplayTitle } from "../features/sessions/SessionTable";

/**
 * Sidebar：Brand→新会话、Search、工作区一级导航（Workstreams / Projects / Sessions /
 * Assistant）、运行中（活内嵌终端，点击进入终端视图交互）、底部固定 Settings。
 * Project 不逐个铺在导航上，整体进入 Projects Board。
 *
 * 「运行中」取代了旧的最近任务列表：跑着的会话终端才是此刻真正需要的入口，
 * 未绑定的显示「新会话」，绑定后显示会话名。数据是 registry 运行时事实，
 * 只在 spawn / exit / bind（terminals-changed 事件）时重取，无轮询。
 */
export default function Sidebar({ route, navigate, onSearch, collapsed = false }: {
  route: Route;
  navigate: (r: Route) => void;
  onSearch: () => void;
  collapsed?: boolean;
}) {
  const [terminals, setTerminals] = useState<TerminalSummary[]>([]);

  const refresh = useCallback(() => {
    api
      .terminalList()
      .then(setTerminals)
      .catch(console.error);
  }, []);

  /** 「运行中」子项的显式关闭：杀掉内嵌 Agent 并移出列表。关闭时正看着
   *  这个终端 → 跳回它的会话（未绑定则回会话看板），不让用户停在尸体上。 */
  const closeTerminal = async (t: TerminalSummary) => {
    try {
      await api.terminalClose(t.terminal_id);
      if (route.view === "terminal" && route.terminalId === t.terminal_id) {
        if (t.session_id) navigate({ view: "session", sessionId: t.session_id });
        else navigate({ view: "sessions" });
      }
    } catch (e) {
      console.error(e);
    }
  };

  useEffect(refresh, [refresh]);
  useEffect(() => onEvent(EVT_TERMINALS, refresh), [refresh]);

  const workspaceActive = (v: "workstreams" | "projects" | "sessions" | "agents" | "assistant") => {
    if (route.view === v) return "active";
    // Workstream Detail → Workstreams 保持弱高亮
    if (v === "workstreams" && route.view === "workstream") return "weak";
    if (v === "sessions" && route.view === "session") return "active";
    // Project Detail → Projects 弱高亮，与 Workstreams 同一模式
    if (v === "projects" && route.view === "project") return "weak";
    return "";
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
              const title = t.session_title
                ? sessionDisplayTitle(t.session_title)
                : "新会话";
              return (
                <div className="sidebar-task" key={t.terminal_id}>
                  <button
                    className={`nav-item ${active ? "active" : ""}`}
                    title={t.cwd ? `${title} · ${t.cwd}` : title}
                    onClick={() => navigate({ view: "terminal", terminalId: t.terminal_id })}
                  >
                    <AgentIcon agent={t.agent} size={15} />
                    <span className="truncate">{title}</span>
                  </button>
                  <button
                    className="pin-button terminal-close"
                    aria-label={`关闭终端：${title}`}
                    title="关闭终端"
                    onClick={() => void closeTerminal(t)}
                  >
                    <Icon name="close" />
                  </button>
                </div>
              );
            })}
          </>
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
