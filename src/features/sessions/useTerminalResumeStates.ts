import { useEffect } from "react";
import { api } from "../../api";
import { affectsSidebar, EVT_SYNCED, EVT_TERMINALS, onEvent } from "../../app/routes";
import type { TerminalResumeState } from "../../types";
import { createReadResource } from "../../hooks/readResource";

const resumeStates = createReadResource<Record<string, TerminalResumeState>>(async key => {
  const ids: string[] = JSON.parse(key);
  if (!ids.length) return {};
  try { return await api.getSessionTerminalStates(ids); }
  catch { return Object.fromEntries(ids.map(id => [id, { available: false, reason: "无法检查终端可用性，请稍后重试" }])); }
});

/** Batch fresh prerequisites separately from history loading; never block the page. */
export function useTerminalResumeStates(sessionIds: string[]) {
  const key = JSON.stringify([...new Set(sessionIds)].sort());
  const states = resumeStates.useValue(key);
  useEffect(() => {
    const refresh = () => {
      void resumeStates.refresh(key);
    };
    void resumeStates.revalidate(key);
    const synced = (event: Event) => { if (affectsSidebar(event, "sessions")) refresh(); };
    window.addEventListener(EVT_SYNCED, synced);
    const offTerminals = onEvent(EVT_TERMINALS, refresh);
    window.addEventListener("focus", refresh);
    return () => { window.removeEventListener(EVT_SYNCED, synced); offTerminals(); window.removeEventListener("focus", refresh); };
  }, [key]);
  return states ?? {};
}
