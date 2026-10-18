import type { IngestSource, Project, SessionListItem } from "../../types";

export interface SessionsCache {
  normalSessions: SessionListItem[] | null;
  archivedSessions: SessionListItem[] | null;
  projects: Project[];
  sources: IngestSource[] | null;
  workstreamTitleById: Map<string, string>;
}

export const sessionsCache: SessionsCache = {
  normalSessions: null,
  archivedSessions: null,
  projects: [],
  sources: null,
  workstreamTitleById: new Map(),
};

export function clearSessionsCache() {
  sessionsCache.normalSessions = null;
  sessionsCache.archivedSessions = null;
  sessionsCache.projects = [];
  sessionsCache.sources = null;
  sessionsCache.workstreamTitleById = new Map();
}
