import { useEffect, useRef, useState } from "react";
import SessionTaskTracking from "./SessionTaskTracking";
import Icon from "../../components/Icon";
import type { Route, SessionEntry } from "../../app/routes";
import type { Agent } from "../../types";
import { capsOf } from "./sessionFormats";
import { openSessionTerminal } from "./continueDesktop";
import { preferredTerminal, rememberSessionPage, terminalDestination } from "./sessionNavigation";
import { useSessionTerminals } from "./useSessionTerminals";

/** One titlebar navigation group for history, detail and all bound terminals. */
export default function SessionSubpageTabs({ sessionId, entry, terminalId, terminalLive, title, agent, sourceKind, archived = false, navigate }: {
  sessionId: string | null;
  entry?: SessionEntry;
  terminalId?: string;
  terminalLive?: boolean;
  title?: string;
  agent?: Agent;
  sourceKind?: string;
  archived?: boolean;
  navigate: (r: Route) => void;
}) {
  const { terminals, loading } = useSessionTerminals();
  const peers = terminals.filter(t => sessionId ? t.session_id === sessionId : t.terminal_id === terminalId)
    .sort((a, b) => a.created_at.localeCompare(b.created_at) || a.terminal_id.localeCompare(b.terminal_id));
  const selected = terminalId ? peers.find(t => t.terminal_id === terminalId)
    : sessionId ? preferredTerminal(sessionId, peers) : undefined;
  const live = terminalId ? terminalLive ?? selected?.live : selected?.live;
  const [open, setOpen] = useState(false);
  const [launching, setLaunching] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const launchPending = useRef(false);
  const canTerminal = !!terminalId || peers.length > 0 || !!agent && capsOf(agent, sourceKind).terminal;

  useEffect(() => {
    setOpen(false);
    if (sessionId) rememberSessionPage(sessionId, terminalId
      ? { view: "terminal", terminalId, initialTitle: title, initialAgent: agent, initialSessionId: sessionId }
      : { view: "session", sessionId, entry, initialTitle: title, initialAgent: agent });
  }, [sessionId, terminalId, entry, title, agent]);
  useEffect(() => {
    if (!open) return;
    const pointer = (e: MouseEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") { setOpen(false); root.current?.querySelector<HTMLButtonElement>(".terminal-dropdown")?.focus(); } };
    document.addEventListener("mousedown", pointer);
    document.addEventListener("keydown", key);
    return () => { document.removeEventListener("mousedown", pointer); document.removeEventListener("keydown", key); };
  }, [open]);

  const openTerminal = async () => {
    if (terminalId) return;
    if (selected) { navigate({ ...terminalDestination(selected), initialTitle: title ?? selected.session_title ?? undefined }); return; }
    if (!sessionId || !agent || archived || launchPending.current || loading) return;
    launchPending.current = true;
    setLaunching(true);
    try { await openSessionTerminal({ id: sessionId, title, agent, archived_at: archived ? "archived" : null }, peers, navigate); }
    finally { launchPending.current = false; setLaunching(false); }
  };
  const terminalLabel = terminalId || selected
    ? `终端 · ${live === undefined ? "正在连接" : live ? "运行中" : "已退出"}`
    : "在终端中继续";

  return <div className="session-navigation" ref={root} onKeyDown={e => {
    if (!open || !["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
    const items = [...(root.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]') ?? [])];
    if (!items.length) return;
    e.preventDefault();
    const current = items.findIndex(item => item === document.activeElement);
    const next = e.key === "Home" ? 0 : e.key === "End" ? items.length - 1
      : (current + (e.key === "ArrowDown" ? 1 : current < 0 ? 0 : -1) + items.length) % items.length;
    items[next].focus();
  }}>
    <div className="settings-seg icon-seg" role="group" aria-label="会话子页">
      <button className={!terminalId && entry === "conversation" ? "on" : ""} aria-pressed={!terminalId && entry === "conversation"}
        aria-label="历史消息" title="历史消息" disabled={!sessionId}
        onClick={() => sessionId && navigate({ view: "session", sessionId, entry: "conversation", initialTitle: title, initialAgent: agent })}><Icon name="chat" /></button>
      <button className={!terminalId && entry === undefined ? "on" : ""} aria-pressed={!terminalId && entry === undefined}
        aria-label="会话详情" title="会话详情" disabled={!sessionId}
        onClick={() => sessionId && navigate({ view: "session", sessionId, initialTitle: title, initialAgent: agent })}><Icon name="info" /></button>
      {canTerminal && <>
        <button className={`${terminalId ? "on " : ""}${live === true ? "terminal-running" : live === false ? "terminal-exited" : ""}`}
          aria-pressed={!!terminalId} aria-label={terminalLabel} title={terminalLabel}
          disabled={launching || !terminalId && !selected && (loading || archived || !sessionId)}
          onClick={() => void openTerminal()}><Icon name="terminal" /></button>
        {peers.length > 1 && <button className="terminal-dropdown" aria-label="切换终端" title="切换终端"
          aria-expanded={open} aria-haspopup="menu" onClick={() => setOpen(value => !value)}><Icon name="chevronDown" /></button>}
      </>}
    </div>
    <span className="session-action-divider" aria-hidden="true" />
    <SessionTaskTracking key={sessionId} sessionId={sessionId} navigate={navigate}/>
    {open && <div className="continue-split-menu terminal-menu" role="menu" aria-label="会话终端">
      {peers.map(t => <button key={t.terminal_id} className="continue-menu-item" role="menuitemradio"
        aria-checked={t.terminal_id === (terminalId ?? selected?.terminal_id)}
        title={t.cwd ?? undefined} onClick={() => { setOpen(false); navigate({ ...terminalDestination(t), initialTitle: title ?? t.session_title ?? undefined }); }}>
        <span className={t.live ? "terminal-running" : "terminal-exited"}><Icon name="terminal" /></span>
        <span className="menu-label">{t.cwd ?? "默认工作目录"}<small>{t.live ? "运行中" : "已退出"}{Number.isFinite(Date.parse(t.created_at)) ? ` · ${new Date(t.created_at).toLocaleString(undefined, { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" })}` : ""}</small></span>
        {t.terminal_id === (terminalId ?? selected?.terminal_id) && <Icon name="check" />}
      </button>)}
    </div>}
  </div>;
}
