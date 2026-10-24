import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import type { TerminalNotifications } from "../../types";
import { applyNotificationInputOwnership } from "./terminalCache";

export function useMailbox(terminalId: string) {
  const [notification, setNotification] = useState<TerminalNotifications | null>(null);
  const [error, setError] = useState("");
  const current = useRef<TerminalNotifications | null>(null);
  const target = useRef(terminalId);
  target.current = terminalId;
  const apply = useCallback((next: TerminalNotifications) => {
    if (next.terminal_id !== target.current || next.terminal_id !== terminalId || next.revision < (current.current?.revision ?? -1)) return;
    current.current = next;
    applyNotificationInputOwnership(terminalId, next);
    setNotification(next); setError("");
  }, [terminalId]);
  const refresh = useCallback(async () => {
    try { apply(await api.terminalNotifications(terminalId)); }
    catch (e) { if (target.current === terminalId) setError(String(e)); }
  }, [terminalId, apply]);
  useEffect(() => {
    let active = true;
    const stops: (() => void)[] = [];
    current.current = null; setNotification(null);
    void Promise.all(["terminal-notifications-changed", "cli-data-changed", "terminal-activity", "terminals-changed"].map(name =>
      listen(name, () => { if (active) void refresh(); }).then(stop => { if (active) stops.push(stop); else stop(); })
    )).then(() => { if (active) void refresh(); }).catch(e => { if (active) setError(String(e)); });
    return () => { active = false; stops.forEach(stop => stop()); };
  }, [terminalId, refresh]);
  return { notification, error, apply, refresh };
}
export function isTerminalReply(data: string) {
  return /^\x1b\][\s\S]*(?:\x07|\x1b\\)$/.test(data)
    || /^\x1b\[[\x20-\x3f]*[cnRtIO]$/.test(data);
}
