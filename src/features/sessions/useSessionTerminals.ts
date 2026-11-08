import { api } from "../../api";
import { EVT_TERMINALS, EVT_TERMINAL_ACTIVITY } from "../../app/routes";
import type { TerminalSummary } from "../../types";
import { createLiveReadResource } from "../../hooks/liveReadResource";

const terminalsResource = createLiveReadResource<TerminalSummary[]>(() =>
  typeof api.terminalList === "function" ? api.terminalList() : Promise.resolve([]),
  [{ name: EVT_TERMINALS }, { name: EVT_TERMINAL_ACTIVITY }]);
const empty: TerminalSummary[] = [];

export function useSessionTerminals(enabled = true) {
  const { value, loading } = terminalsResource.useValue(enabled);
  return { terminals: value ?? empty, loading };
}
