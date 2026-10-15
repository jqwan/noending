import { useEffect, useState } from "react";
import { api } from "../../api";
import { EVT_SYNCED, EVT_TERMINALS, onEvent } from "../../app/routes";
import type { TerminalResumeState } from "../../types";

/** Batch fresh prerequisites separately from history loading; never block the page. */
export function useTerminalResumeStates(sessionIds: string[]) {
  const key = JSON.stringify([...new Set(sessionIds)].sort());
  const [snapshot, setSnapshot] = useState<{ key: string; states: Record<string, TerminalResumeState> }>({ key: "", states: {} });
  useEffect(() => {
    const ids: string[] = JSON.parse(key);
    let active = true;
    let sequence = 0;
    const refresh = () => {
      const request = ++sequence;
      if (!ids.length) { setSnapshot({ key, states: {} }); return; }
      void api.getSessionTerminalStates(ids).then(states => {
        if (active && sequence === request) setSnapshot({ key, states });
      }).catch(() => {
        if (active && sequence === request) setSnapshot({ key, states: Object.fromEntries(ids.map(id =>
          [id, { available: false, reason: "无法检查终端可用性，请稍后重试" }])) });
      });
    };
    refresh();
    const offSync = onEvent(EVT_SYNCED, refresh);
    const offTerminals = onEvent(EVT_TERMINALS, refresh);
    window.addEventListener("focus", refresh);
    return () => { active = false; offSync(); offTerminals(); window.removeEventListener("focus", refresh); };
  }, [key]);
  return snapshot.key === key ? snapshot.states : {};
}
