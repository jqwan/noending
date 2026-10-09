import type { Route } from "../../app/routes";
import type { TerminalSummary } from "../../types";

// UI preferences only; native identity and terminal lifetime remain backend facts.
export const sessionPages = new Map<string, Route>();
export const sessionTerminals = new Map<string, string>();
export function rememberSessionPage(sessionId: string, route: Route) {
  sessionPages.set(sessionId, route);
  if (route.view === "terminal") sessionTerminals.set(sessionId, route.terminalId);
}

export function terminalDestination(terminal: TerminalSummary): Route {
  return { view: "terminal", terminalId: terminal.terminal_id, initialAgent: terminal.agent,
    initialSessionId: terminal.session_id ?? undefined, initialTitle: terminal.session_title ?? undefined };
}

export function preferredTerminal(sessionId: string, terminals: TerminalSummary[]) {
  const remembered = terminals.find(t => t.terminal_id === sessionTerminals.get(sessionId));
  return remembered ?? terminals.find(t => t.live) ?? terminals[0];
}

export function sessionDestination(sessionId: string, terminals: TerminalSummary[]): Route {
  const bound = terminals.filter(t => t.session_id === sessionId);
  const remembered = sessionPages.get(sessionId);
  if (remembered?.view === "session") return remembered;
  if (remembered?.view === "terminal" && bound.some(t => t.terminal_id === remembered.terminalId)) return remembered;
  const terminal = preferredTerminal(sessionId, bound.filter(t => t.live));
  return terminal ? terminalDestination(terminal)
    : { view: "session", sessionId, entry: "conversation" };
}
