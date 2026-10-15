import { EVT_SYNCED, EVT_TERMINALS, type Route } from "./routes";
import { sessionPages, sessionTerminals } from "../features/sessions/sessionNavigation";
import { disposeTerminal } from "../features/sessions/terminalCache";
import { viewState } from "../hooks/useViewState";

export const EVT_REMOVED = "noending:removed";
export type RemovedTargets = {
  sessionIds?: readonly string[];
  terminalIds?: readonly string[];
  workstreamIds?: readonly string[];
  projectIds?: readonly string[];
  replacement?: Route;
};

// IDs are never reused. Reject late async navigation as well as old history entries.
export const removedTargets = {
  sessions: new Set<string>(), terminals: new Set<string>(),
  workstreams: new Set<string>(), projects: new Set<string>(),
};

export function isRemovedRoute(route: Route): boolean {
  switch (route.view) {
    case "session": return removedTargets.sessions.has(route.sessionId);
    case "terminal": return removedTargets.terminals.has(route.terminalId);
    case "workstream": return removedTargets.workstreams.has(route.workstreamId);
    case "project": return removedTargets.projects.has(route.projectId);
    default: return false;
  }
}

/** Publish only after backend success. Failed deletes/closes leave all UI state intact. */
export function invalidateTargets(targets: RemovedTargets) {
  for (const id of targets.sessionIds ?? []) {
    removedTargets.sessions.add(id);
    sessionPages.delete(id);
    sessionTerminals.delete(id);
  }
  for (const id of targets.terminalIds ?? []) {
    removedTargets.terminals.add(id);
    disposeTerminal(id);
  }
  for (const id of targets.workstreamIds ?? []) removedTargets.workstreams.add(id);
  for (const id of targets.projectIds ?? []) removedTargets.projects.add(id);
  for (const [id, route] of sessionPages) if (isRemovedRoute(route)) sessionPages.delete(id);
  for (const [id, terminalId] of sessionTerminals) if (removedTargets.terminals.has(terminalId)) sessionTerminals.delete(id);
  const prefixes = [
    ...(targets.sessionIds ?? []).map(id => `session.${id}.`),
    ...(targets.workstreamIds ?? []).map(id => `workstream.${id}.`),
    ...(targets.projectIds ?? []).map(id => `project.${id}.`),
  ];
  for (const key of viewState.keys()) {
    if (prefixes.some(prefix => key.startsWith(prefix))) viewState.delete(key);
  }
  window.dispatchEvent(new CustomEvent<RemovedTargets>(EVT_REMOVED, { detail: targets }));
  window.dispatchEvent(new CustomEvent(EVT_SYNCED));
  if (targets.terminalIds?.length) window.dispatchEvent(new CustomEvent(EVT_TERMINALS));
}
