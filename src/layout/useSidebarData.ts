import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { onEvent, EVT_TERMINALS, EVT_TERMINAL_ACTIVITY, EVT_SYNCED } from "../app/routes";
import { EVT_REMOVED, removedTargets, type RemovedTargets } from "../app/removedTargets";
import { useViewPreference, preferenceValues, booleanPreference, booleanRecordPreference } from "../hooks/useViewState";
import { useTerminalResumeStates } from "../features/sessions/useTerminalResumeStates";
import { capsOf } from "../features/sessions/sessionFormats";
import type { TerminalSummary, SessionListItem, ProjectCardData, TerminalResumeState } from "../types";
import {
  PINNED_STORAGE_KEY,
  MAX_GROUP_SESSIONS_DEFAULT,
  getStoredPinnedSessions,
  compareSidebarSessions,
  mergeSidebarSessions,
  groupSidebarSessions,
  isSidebarFilters,
  type SidebarFilterKey,
  type SidebarGroupBy,
  type SidebarSortKey,
  type SidebarSession,
  type SidebarSessionGroup,
} from "./sidebarModel";

export interface UseSidebarDataOptions {
  collapsed?: boolean;
  hidden?: boolean;
}

export interface UseSidebarDataResult {
  terminalsLoading: boolean;
  terminals: TerminalSummary[];
  sessions: SessionListItem[];
  projects: ProjectCardData[];
  projectMap: Map<string, ProjectCardData>;
  groupBy: SidebarGroupBy;
  setGroupBy: (groupBy: SidebarGroupBy) => void;
  collapsedGroups: Record<string, boolean>;
  expandedGroupSessions: Record<string, boolean>;
  setExpandedGroupSessions: React.Dispatch<React.SetStateAction<Record<string, boolean>>>;
  flatVisibleCount: number;
  setFlatVisibleCount: React.Dispatch<React.SetStateAction<number>>;
  filters: SidebarFilterKey[];
  setFilters: React.Dispatch<React.SetStateAction<SidebarFilterKey[]>>;
  toggleFilter: (key: SidebarFilterKey) => void;
  clearFilters: () => void;
  runningOnly: boolean;
  setRunningOnly: (val: boolean | ((prev: boolean) => boolean)) => void;
  showSessionIcons: boolean;
  setShowSessionIcons: (val: boolean | ((prev: boolean) => boolean)) => void;
  sortBy: SidebarSortKey;
  setSortBy: (sortBy: SidebarSortKey) => void;
  pinnedSessionIds: string[];
  pinnedCollapsed: boolean;
  setPinnedCollapsed: (val: boolean | ((prev: boolean) => boolean)) => void;
  isSessionPinned: (s: { id: string; sessionId: string | null }) => boolean;
  togglePin: (id: string, sessionId?: string | null) => void;
  pinnedSessions: SidebarSession[];
  resumeStates: Record<string, TerminalResumeState>;
  sessionGroups: SidebarSessionGroup[];
  allGroupsCollapsed: boolean;
  toggleGroup: (groupId: string) => void;
  toggleAllGroups: () => void;
}

export function useSidebarData({ collapsed = false, hidden = false }: UseSidebarDataOptions = {}): UseSidebarDataResult {
  const statePrefix = "sidebar";
  const [terminalsLoading, setTerminalsLoading] = useState(true);
  const [terminals, setTerminals] = useState<TerminalSummary[]>([]);
  const [sessions, setSessions] = useState<SessionListItem[]>([]);
  const [projects, setProjects] = useState<ProjectCardData[]>([]);

  const [groupBy, setGroupBy] = useViewPreference<SidebarGroupBy>(
    `${statePrefix}.recent.groupBy.v2`,
    "project",
    preferenceValues("project", "agent", "none"),
  );
  const [collapsedGroups, setCollapsedGroups] = useViewPreference<Record<string, boolean>>(
    `${statePrefix}.recent.collapsedGroups`,
    {},
    booleanRecordPreference,
  );
  const [expandedGroupSessions, setExpandedGroupSessions] = useState<Record<string, boolean>>({});
  const [flatVisibleCount, setFlatVisibleCount] = useState(100);

  useEffect(() => {
    setExpandedGroupSessions({});
  }, [groupBy]);

  const refreshSeq = useRef(0);

  const refreshTerminals = useCallback(() => {
    const seq = ++refreshSeq.current;
    api
      .terminalList()
      .then((next) => {
        if (seq === refreshSeq.current) {
          setTerminals(
            next.filter(
              (t) =>
                !removedTargets.terminals.has(t.terminal_id) &&
                !removedTargets.sessions.has(t.session_id ?? ""),
            ),
          );
          setTerminalsLoading(false);
        }
      })
      .catch((error) => {
        if (seq === refreshSeq.current) setTerminalsLoading(false);
        console.error(error);
      });
  }, []);

  const refreshSessions = useCallback(() => {
    Promise.all([api.listSessions(undefined, undefined, "all"), api.listProjectCards()])
      .then(([sList, prjList]) => {
        setSessions(sList.filter((s) => !removedTargets.sessions.has(s.id)));
        setProjects(prjList.filter((p) => !removedTargets.projects.has(p.id)));
      })
      .catch(console.error);
  }, []);

  useEffect(() => {
    refreshTerminals();
    return () => {
      ++refreshSeq.current;
    };
  }, [refreshTerminals]);

  useEffect(() => onEvent(EVT_TERMINALS, refreshTerminals), [refreshTerminals]);
  useEffect(() => onEvent(EVT_TERMINAL_ACTIVITY, refreshTerminals), [refreshTerminals]);
  useEffect(refreshSessions, [refreshSessions]);
  useEffect(() => onEvent(EVT_TERMINALS, refreshSessions), [refreshSessions]);
  useEffect(() => onEvent(EVT_SYNCED, refreshSessions), [refreshSessions]);

  const [filters, setFilters] = useViewPreference<SidebarFilterKey[]>(
    `${statePrefix}.recent.filters`,
    [],
    isSidebarFilters,
  );
  const [runningOnly, setRunningOnly] = useViewPreference<boolean>(
    `${statePrefix}.recent.runningOnly`,
    false,
    booleanPreference,
  );
  const [showSessionIcons, setShowSessionIcons] = useViewPreference<boolean>(
    `${statePrefix}.recent.showSessionIcons`,
    true,
    booleanPreference,
  );

  const toggleFilter = useCallback((key: SidebarFilterKey) => {
    setFilters((prev) => (prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key]));
  }, [setFilters]);

  const clearFilters = useCallback(() => {
    setFilters([]);
  }, [setFilters]);

  const projectMap = useMemo(() => {
    const map = new Map<string, ProjectCardData>();
    for (const p of projects) {
      map.set(p.id, p);
    }
    return map;
  }, [projects]);

  const [pinnedSessionIds, setPinnedSessionIds] = useState<string[]>(getStoredPinnedSessions);
  const [pinnedCollapsed, setPinnedCollapsed] = useViewPreference<boolean>(
    `${statePrefix}.pinned.collapsed`,
    false,
    booleanPreference,
  );

  useEffect(() => {
    const removed = (event: Event) => {
      const targets = (event as CustomEvent<RemovedTargets>).detail;
      setSessions((rows) => rows.filter((s) => !removedTargets.sessions.has(s.id)));
      setTerminals((rows) =>
        rows.filter(
          (t) =>
            !removedTargets.terminals.has(t.terminal_id) &&
            !removedTargets.sessions.has(t.session_id ?? ""),
        ),
      );
      setProjects((rows) => rows.filter((p) => !removedTargets.projects.has(p.id)));
      if (targets.sessionIds?.length || targets.terminalIds?.length) {
        setPinnedSessionIds((ids) => {
          const next = ids.filter(
            (id) =>
              !removedTargets.sessions.has(id) &&
              !removedTargets.terminals.has(id.replace(/^terminal:/, "")),
          );
          try {
            localStorage.setItem(PINNED_STORAGE_KEY, JSON.stringify(next));
          } catch {
            /* In-memory removal still applies. */
          }
          return next;
        });
      }
    };
    window.addEventListener(EVT_REMOVED, removed);
    return () => window.removeEventListener(EVT_REMOVED, removed);
  }, []);

  const isSessionPinned = useCallback(
    (s: { id: string; sessionId: string | null }) => {
      return (
        pinnedSessionIds.includes(s.id) ||
        (Boolean(s.sessionId) && pinnedSessionIds.includes(s.sessionId!))
      );
    },
    [pinnedSessionIds],
  );

  const togglePin = useCallback((id: string, sessionId?: string | null) => {
    setPinnedSessionIds((prev) => {
      const isPinned = prev.includes(id) || (Boolean(sessionId) && prev.includes(sessionId!));
      let next: string[];
      if (isPinned) {
        next = prev.filter((item) => item !== id && item !== sessionId);
      } else {
        next = [...prev, sessionId || id];
      }
      try {
        localStorage.setItem(PINNED_STORAGE_KEY, JSON.stringify(next));
      } catch (err) {
        console.error(err);
      }
      return next;
    });
  }, []);

  const [sortBy, setSortBy] = useViewPreference<SidebarSortKey>(
    `${statePrefix}.recent.sortBy`,
    "createdAt",
    preferenceValues("createdAt", "updatedAt"),
  );

  const allMergedSessions = useMemo(() => {
    return mergeSidebarSessions(sessions, terminals, [], sortBy);
  }, [sessions, terminals, sortBy]);

  const pinnedSessions = useMemo(() => {
    if (pinnedSessionIds.length === 0) return [];
    return allMergedSessions
      .filter((s) => isSessionPinned(s))
      .sort((a, b) => compareSidebarSessions(a, b, sortBy));
  }, [allMergedSessions, pinnedSessionIds, isSessionPinned, sortBy]);

  const resumeCheckIds = useMemo(() => {
    if (collapsed || hidden) return [];
    let candidates: SidebarSession[];
    if (filters.includes("canContinue")) {
      candidates = allMergedSessions;
    } else {
      const groups = groupSidebarSessions(
        mergeSidebarSessions(sessions, terminals, filters, sortBy).filter(
          (s) => !isSessionPinned(s) && (!runningOnly || s.terminals.some((t) => t.live)),
        ),
        groupBy,
        sortBy,
        projectMap,
      );
      candidates = groups.flatMap((group) => {
        if (groupBy === "none") return group.sessions.slice(0, flatVisibleCount);
        if (collapsedGroups[`${groupBy}:${group.id}`]) return [];
        return expandedGroupSessions[group.id]
          ? group.sessions
          : group.sessions.slice(0, MAX_GROUP_SESSIONS_DEFAULT);
      });
    }
    if (!pinnedCollapsed) candidates = [...candidates, ...pinnedSessions];
    return candidates
      .filter((s) => s.sessionId && !s.archived_at && capsOf(s.agent, s.source_kind).terminal)
      .map((s) => s.sessionId!);
  }, [
    collapsed,
    hidden,
    filters,
    allMergedSessions,
    sessions,
    terminals,
    sortBy,
    isSessionPinned,
    runningOnly,
    groupBy,
    projectMap,
    flatVisibleCount,
    collapsedGroups,
    expandedGroupSessions,
    pinnedCollapsed,
    pinnedSessions,
  ]);

  const resumeStates = useTerminalResumeStates(resumeCheckIds);

  const sessionGroups = useMemo(() => {
    return groupSidebarSessions(
      mergeSidebarSessions(
        sessions,
        terminals,
        filters,
        sortBy,
        resumeStates,
      ).filter((s) => !isSessionPinned(s) && (!runningOnly || s.terminals.some((t) => t.live))),
      groupBy,
      sortBy,
      projectMap,
    ).filter((group) => group.sessions.length > 0);
  }, [sessions, terminals, filters, runningOnly, groupBy, sortBy, projectMap, isSessionPinned, resumeStates]);

  const collapseKey = useCallback((groupId: string) => `${groupBy}:${groupId}`, [groupBy]);

  const toggleGroup = useCallback((groupId: string) => {
    const key = `${groupBy}:${groupId}`;
    setCollapsedGroups((prev) => {
      const nextCollapsed = !prev[key];
      if (nextCollapsed) {
        setExpandedGroupSessions((exp) => {
          if (!exp[groupId]) return exp;
          const next = { ...exp };
          delete next[groupId];
          return next;
        });
      }
      return {
        ...prev,
        [key]: nextCollapsed,
      };
    });
  }, [groupBy, setCollapsedGroups]);

  const allGroupsCollapsed =
    sessionGroups.length > 0 &&
    sessionGroups.every((g) => Boolean(collapsedGroups[collapseKey(g.id)]));

  const toggleAllGroups = useCallback(() => {
    setCollapsedGroups((prev) => {
      const next = { ...prev };
      for (const group of sessionGroups) {
        const key = `${groupBy}:${group.id}`;
        if (allGroupsCollapsed) delete next[key];
        else next[key] = true;
      }
      return next;
    });
    setExpandedGroupSessions({});
  }, [allGroupsCollapsed, groupBy, sessionGroups, setCollapsedGroups]);

  return {
    terminalsLoading,
    terminals,
    sessions,
    projects,
    projectMap,
    groupBy,
    setGroupBy,
    collapsedGroups,
    expandedGroupSessions,
    setExpandedGroupSessions,
    flatVisibleCount,
    setFlatVisibleCount,
    filters,
    setFilters,
    toggleFilter,
    clearFilters,
    runningOnly,
    setRunningOnly,
    showSessionIcons,
    setShowSessionIcons,
    sortBy,
    setSortBy,
    pinnedSessionIds,
    pinnedCollapsed,
    setPinnedCollapsed,
    isSessionPinned,
    togglePin,
    pinnedSessions,
    resumeStates,
    sessionGroups,
    allGroupsCollapsed,
    toggleGroup,
    toggleAllGroups,
  };
}
