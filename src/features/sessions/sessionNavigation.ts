import { isRemovedRoute } from "../../app/removedTargets";
import type { Route } from "../../app/routes";
import type { TerminalSummary, AgentTool } from "../../types";

// UI preferences only; native identity and terminal lifetime remain backend facts.
export const sessionPages = new Map<string, Route>();
export const sessionTerminals = new Map<string, string>();
export function rememberSessionPage(sessionId: string, route: Route) {
  if (isRemovedRoute(route) || isRemovedRoute({ view: "session", sessionId })) return;
  sessionPages.set(sessionId, route);
  if (route.view === "terminal") sessionTerminals.set(sessionId, route.terminalId);
}

export function terminalDestination(terminal: TerminalSummary): Extract<Route, { view: "terminal" }> {
  return { view: "terminal", terminalId: terminal.terminal_id, initialTool: terminal.agent,
    initialSessionId: terminal.session_id ?? undefined, initialTitle: terminal.session_title ?? undefined };
}

export function preferredTerminal(sessionId: string, terminals: TerminalSummary[]) {
  const remembered = terminals.find(t => t.terminal_id === sessionTerminals.get(sessionId));
  return remembered ?? terminals.find(t => t.live) ?? terminals[0];
}

export function sessionDestination(sessionId: string, terminals: TerminalSummary[], seed?: { title: string; agent: AgentTool }): Route {
  const bound = terminals.filter(t => t.session_id === sessionId);
  const remembered = sessionPages.get(sessionId);
  if (remembered?.view === "session") return { ...remembered, initialTitle: seed?.title ?? remembered.initialTitle, initialTool: seed?.agent ?? remembered.initialTool };
  if (remembered?.view === "terminal" && bound.some(t => t.terminal_id === remembered.terminalId)) return { ...remembered, initialTitle: seed?.title ?? remembered.initialTitle, initialTool: seed?.agent ?? remembered.initialTool, initialSessionId: sessionId };
  const terminal = preferredTerminal(sessionId, bound.filter(t => t.live));
  return terminal ? { ...terminalDestination(terminal), initialTitle: seed?.title ?? terminal.session_title ?? undefined }
    : { view: "session", sessionId, initialTitle: seed?.title, initialTool: seed?.agent };
}
