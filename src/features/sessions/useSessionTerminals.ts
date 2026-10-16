import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { EVT_TERMINALS, EVT_TERMINAL_ACTIVITY, onEvent } from "../../app/routes";
import type { TerminalSummary } from "../../types";

export function useSessionTerminals() {
  const [terminals, setTerminals] = useState<TerminalSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const sequence = useRef(0);
  const refresh = useCallback(() => {
    const mine = ++sequence.current;
    const fetcher = typeof api.terminalList === "function" ? api.terminalList() : Promise.resolve([]);
    void fetcher.then(rows => {
      if (sequence.current === mine) { setTerminals(rows); setLoading(false); }
    }).catch(() => { if (sequence.current === mine) setLoading(false); });
  }, []);
  useEffect(() => {
    refresh();
    const stop = onEvent(EVT_TERMINALS, refresh);
    const stopActivity = onEvent(EVT_TERMINAL_ACTIVITY, refresh);
    return () => { ++sequence.current; stop(); stopActivity(); };
  }, [refresh]);
  return { terminals, loading };
}
