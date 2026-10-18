import React, { useRef, useState } from "react";
import type { Route } from "../app/routes";
import Icon from "../components/Icon";
import AgentIcon from "../components/AgentIcon";
import type { AgentStatusEntry, TerminalResumeState } from "../types";
import { sessionDisplayTitle } from "../features/sessions/sessionDisplay";
import { capsOf } from "../features/sessions/sessionFormats";
import { DesktopContinueButton } from "../features/sessions/SessionHeaderActions";
import { openSessionTerminal, terminalContinueState, terminalForContinue } from "../features/sessions/continueDesktop";
import { preferredTerminal, sessionDestination, terminalDestination } from "../features/sessions/sessionNavigation";
import TerminalActivityIndicator from "../features/sessions/TerminalActivityIndicator";
import type { SidebarSession } from "./sidebarModel";

export default function SidebarSessionRow({
  session,
  route,
  navigate,
  agentStatus,
  loading,
  isPinned,
  onTogglePin,
  showIcon = true,
  resumeState,
}: {
  session: SidebarSession;
  route: Route;
  navigate: (r: Route) => void;
  agentStatus: Record<string, AgentStatusEntry> | null;
  loading: boolean;
  isPinned?: boolean;
  onTogglePin?: () => void;
  showIcon?: boolean;
  resumeState?: TerminalResumeState;
}) {
  const s = session;
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const active =
    (route.view === "session" && route.sessionId === s.sessionId) ||
    (route.view === "terminal" && s.terminals.some((t) => t.terminal_id === route.terminalId));
  const title = sessionDisplayTitle(s.title);
  const selected = s.sessionId ? terminalForContinue(s.sessionId, s.terminals) : s.terminals[0];
  const statusPeers = [...s.terminals].sort(
    (a, b) => a.created_at.localeCompare(b.created_at) || a.terminal_id.localeCompare(b.terminal_id),
  );
  const statusTerminal =
    (route.view === "terminal" ? s.terminals.find((t) => t.terminal_id === route.terminalId) : undefined) ??
    (s.sessionId
      ? route.view === "session" && route.sessionId === s.sessionId
        ? terminalForContinue(s.sessionId, statusPeers)
        : preferredTerminal(s.sessionId, statusPeers)
      : selected);
  const caps = capsOf(s.agent, s.source_kind);
  const availability = terminalContinueState(s.sessionId, s.terminals, resumeState, !!s.archived_at);
  const connecting = busy && !selected?.live;
  const terminalLabel = connecting
    ? "终端 · 正在连接"
    : selected
    ? `终端 · ${selected.live ? "已连接" : "已退出"}`
    : "在终端中继续";
  const terminalCount = s.terminals.length;
  const runningCount = s.terminals.filter((t) => t.live).length;
  const hasLiveTerminal = runningCount > 0;
  const isTerminalActive = route.view === "terminal" && s.terminals.some((t) => t.terminal_id === route.terminalId);
  const counts = `${runningCount} 个已连接终端，${s.terminals.filter((t) => !t.live).length} 个已退出终端`;

  const openTerminal = async () => {
    if (pending.current) return;
    if (!s.sessionId) {
      if (selected) navigate(terminalDestination(selected));
      return;
    }
    pending.current = true;
    setBusy(true);
    try {
      await openSessionTerminal({ ...s, id: s.sessionId, title }, navigate);
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };

  const onSelectSession = () => {
    navigate(
      s.sessionId
        ? sessionDestination(s.sessionId, s.terminals, { title, agent: s.agent })
        : terminalDestination(s.terminals[0]),
    );
  };

  const titleRef = useRef<HTMLSpanElement>(null);
  const textRef = useRef<HTMLSpanElement>(null);
  const [overflow, setOverflow] = useState(0);
  const [hovered, setHovered] = useState(false);

  const hasAction = Boolean(selected || caps.terminal || s.sessionId);
  const onMouseEnter = () => {
    setHovered(true);
    if (titleRef.current) {
      const actionsWidth = hasAction && onTogglePin ? 58 : hasAction || onTogglePin ? 34 : 0;
      const available = actionsWidth > 0 ? titleRef.current.clientWidth - actionsWidth : titleRef.current.clientWidth;
      const textWidth = textRef.current
        ? textRef.current.getBoundingClientRect().width || textRef.current.scrollWidth
        : 0;
      const contentWidth =
        textWidth > 0 ? textWidth : titleRef.current.scrollWidth > titleRef.current.clientWidth ? titleRef.current.scrollWidth : 0;
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
    <div
      className={`nav-item sidebar-recent-item ${active ? "active" : ""}`}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      onClick={(e) => {
        if ((e.target as HTMLElement).closest(".sidebar-session-action")) return;
        onSelectSession();
      }}
    >
      <button
        className="sidebar-session-link"
        title={`${s.cwd ? `${title} · ${s.cwd}` : title}${s.archived_at ? " · 已归档" : ""}`}
        onClick={(e) => {
          e.stopPropagation();
          onSelectSession();
        }}
      >
        {showIcon && <AgentIcon agent={s.agent} size={14} />}
        <span
          ref={titleRef}
          className={`sidebar-session-title${isScrolling ? " scrolling" : ""}`}
          style={
            isScrolling
              ? ({
                  "--overflow-width": `${overflow}px`,
                  "--scroll-duration": `${duration}s`,
                } as React.CSSProperties)
              : undefined
          }
        >
          <span ref={textRef} className="sidebar-session-title-text">
            {title}
          </span>
        </span>
      </button>
      {statusTerminal && (
        <span className="sidebar-session-activity">
          <TerminalActivityIndicator terminal={statusTerminal} />
        </span>
      )}
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
        {selected || caps.terminal ? (
          <button
            className={`sidebar-session-action terminal-action${
              connecting
                ? " terminal-connecting"
                : hasLiveTerminal
                ? " terminal-running"
                : terminalCount > 0
                ? " terminal-exited"
                : ""
            }${isTerminalActive ? " active" : ""}`}
            aria-label={`${terminalLabel}：${title}`}
            title={
              connecting
                ? "正在连接终端"
                : availability.disabled
                ? availability.title
                : selected?.live
                ? `${terminalLabel} · ${counts}`
                : availability.title
            }
            aria-busy={connecting}
            disabled={busy || loading || availability.disabled}
            onClick={(e) => {
              e.stopPropagation();
              void openTerminal();
            }}
          >
            <Icon name={connecting ? "refresh" : "terminal"} />
          </button>
        ) : (
          s.sessionId && (
            <DesktopContinueButton
              sessionId={s.sessionId}
              agent={s.agent}
              sourceKind={s.source_kind}
              archived={!!s.archived_at}
              agentStatus={agentStatus}
              className="sidebar-session-action"
              label={`在桌面应用中继续：${title}`}
            />
          )
        )}
      </span>
    </div>
  );
}
