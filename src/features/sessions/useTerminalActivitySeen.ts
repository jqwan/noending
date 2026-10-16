import { useEffect } from "react";
import { api } from "../../api";
import type { TerminalSummary } from "../../types";

/** Viewing history/details, a different terminal, or a background window is not a read. */
export function useTerminalActivitySeen(terminal: TerminalSummary | undefined) {
  const id = terminal?.terminal_id;
  const revision = terminal?.activity?.revision;
  const unread = terminal?.activity?.unread;
  useEffect(() => {
    if (!id || revision === undefined || !unread) return;
    let stopped = false;
    let pending = false;
    let acknowledged = false;
    const acknowledge = () => {
      if (stopped || pending || acknowledged || document.visibilityState !== "visible" || !document.hasFocus()) return;
      pending = true;
      void api.terminalActivitySeen(id, revision).then(() => { acknowledged = true; })
        .catch(() => { /* Keep unread on failure; a later focus retries. */ })
        .finally(() => { pending = false; });
    };
    acknowledge();
    document.addEventListener("visibilitychange", acknowledge);
    window.addEventListener("focus", acknowledge);
    return () => {
      stopped = true;
      document.removeEventListener("visibilitychange", acknowledge);
      window.removeEventListener("focus", acknowledge);
    };
  }, [id, revision, unread]);
}
