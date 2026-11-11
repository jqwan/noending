import { api } from "../api";
import { sessionDetailCache, sessionContextCache } from "../features/sessions/sessionDetailCache";
import { sessionDisplayTitle } from "../features/sessions/sessionDisplay";
import { projectDetailCache } from "../features/projects/projectDetailCache";
import { workstreamDetailCache } from "../features/workstreams/workstreamDetailCache";

import { sessionPages } from "../features/sessions/sessionNavigation";
import type { Route } from "./routes";
import { isRemovedRoute, removedTargets } from "./removedTargets";
import { preloadAgentsCache } from "../features/agents/agentsCache";
import { projectCardsCache } from "../features/projects/projectCardsCache";
import { workstreamCardsCache } from "../features/workstreams/useWorkstreamCards";
import { sessionsCache } from "../features/sessions/sessionsCache";
import { skillDetailCache, skillProjectsCache, setSkillProjectsCache } from "../features/skills/skillDetailCache";
import { packageDetailCache } from "../features/packages/packageDetailCache";
import { projectSummaryCache } from "../features/projects/projectSummaryCache";
import { taskItemsResource, taskSummaryResource } from "../features/workstreams/taskResources";
import { prepareCollaborationMembers } from "../features/collaboration/collaborationResources";
import { mailboxes } from "../features/sessions/mailboxResources";
import { liveTerminals } from "../features/sessions/terminalCache";
import { extractionSettingsResource, workspaceSettingsResource } from "../features/settings/settingsResources";
import { viewState } from "../hooks/useViewState";
import { prepareOperationStatus } from "../hooks/useOperationState";

export function prepareNavigationData(route: Route): Route | Promise<Route> {
  if (route.view === "projects" && projectCardsCache.cards === null) {
    return api.listProjectCards().then(cards => {
      projectCardsCache.cards = cards.filter(card => !removedTargets.projects.has(card.id));
      return route;
    });
  }
  if (route.view === "workstreams" && workstreamCardsCache.cards === null) {
    return Promise.all([api.listWorkstreamCards(), api.getDefaultAgent().catch(() => null)]).then(([cards, agent]) => {
      workstreamCardsCache.cards = cards.filter(card => !removedTargets.workstreams.has(card.id));
      workstreamCardsCache.defaultAgent = agent;
      return route;
    });
  }
  if (route.view === "sessions" && (route.scope === "archived" ? sessionsCache.archivedSessions : sessionsCache.normalSessions) === null) {
    return Promise.all([
      route.scope === "archived" ? api.listSessions(undefined, undefined, "archived") : api.listSessions(),
      api.listProjects(), api.listWorkstreams(),
    ]).then(([sessions, projects, tasks]) => {
      const rows = sessions.filter(session => !removedTargets.sessions.has(session.id));
      if (route.scope === "archived") sessionsCache.archivedSessions = rows;
      else sessionsCache.normalSessions = rows;
      sessionsCache.projects = projects.filter(project => !removedTargets.projects.has(project.id));
      sessionsCache.workstreamTitleById = new Map(tasks.filter(task => !removedTargets.workstreams.has(task.id)).map(task => [task.id, task.title]));
      return route;
    });
  }
  if (route.view === "agents") {
    const pending = preloadAgentsCache();
    return pending ? pending.then(() => route) : route;
  }
  if (route.view === "session") {
    const cached = sessionDetailCache.get(route.sessionId)?.session;
    const remembered = sessionPages.get(route.sessionId);
    const seed = remembered && (remembered.view === "session" || remembered.view === "terminal") ? remembered : undefined;
    const initialTitle = route.initialTitle || (cached ? sessionDisplayTitle(cached.title) : seed?.initialTitle);
    const initialAgent = route.initialAgent ?? cached?.agent ?? seed?.initialAgent;
    if (cached) return { ...route, initialTitle, initialAgent };
    return api.getSessionDetail(route.sessionId).then(detail => {
      if (!isRemovedRoute(route)) sessionDetailCache.set(route.sessionId, detail);
      return { ...route, initialTitle: sessionDisplayTitle(detail.session.title), initialAgent: detail.session.agent };
    });
  }
  if (route.view === "workstream") {
    const initialTitle = route.initialTitle ?? workstreamDetailCache.get(route.workstreamId)?.ctx.workstream.title;
    if (workstreamDetailCache.has(route.workstreamId)) return { ...route, initialTitle };
    return api.getWorkstreamContext(route.workstreamId).then(ctx => {
      if (!isRemovedRoute(route)) workstreamDetailCache.set(route.workstreamId, { ctx });
      return { ...route, initialTitle: ctx.workstream.title };
    });
  }
  if (route.view === "project") {
    const initialTitle = route.initialTitle ?? projectDetailCache.get(route.projectId)?.project.name;
    if (projectDetailCache.has(route.projectId)) return { ...route, initialTitle };
    return api.getProjectDetail(route.projectId).then(detail => {
      if (!isRemovedRoute(route)) projectDetailCache.set(route.projectId, detail);
      return { ...route, initialTitle: detail.project.name };
    });
  }
  if (route.view === "skill" && !skillDetailCache.has(route.resourceId)) {
    return api.getSkillDetail(route.resourceId).then(detail => {
      if (!detail) throw new Error("技能已移除或来源已变化");
      if (!isRemovedRoute(route)) skillDetailCache.set(route.resourceId, detail);
      return route;
    });
  }
  if (route.view === "package" && !packageDetailCache.has(route.packageId)) {
    return api.getPackageDetail(route.packageId).then(detail => {
      if (!detail) throw new Error("插件包已移除或来源已变化");
      if (!isRemovedRoute(route)) packageDetailCache.set(route.packageId, detail);
      return route;
    });
  }
  return route;
}

/** 只预备首屏轻量读取；不额外预取历史消息、技能/插件包目录或 Agent 运行配置。
 * 辅助读取失败时允许打开页面，由对应卡片重新读取并提供错误、重试入口。
 */
export function prepareSupportingData(route: Route): Promise<void> | undefined {
  const reads: Promise<unknown>[] = [];
  const add = (read: Promise<unknown> | undefined) => { if (read) reads.push(read); };
  if (["session", "workstream", "project", "agents"].includes(route.view)) add(prepareOperationStatus());
  if (route.view === "session" && route.entry !== "conversation") {
    if (!sessionContextCache.has(route.sessionId)) add(Promise.resolve().then(() => api.getSessionContext(route.sessionId)).then(context => {
      if (!isRemovedRoute(route)) sessionContextCache.set(route.sessionId, context);
    }));
    add(mailboxes.all.ensure(route.sessionId));
  }
  if (route.view === "terminal") {
    const snapshot = liveTerminals.get(route.terminalId)?.snapshot;
    const sessionId = snapshot ? snapshot.session_id : route.initialSessionId;
    if (sessionId) add(mailboxes.unread.ensure(sessionId));
  }
  if (route.view === "workstream") {
    add(prepareCollaborationMembers(route.workstreamId));
    if (route.entry !== "collaboration") {
      add(taskItemsResource.ensure(route.workstreamId));
      add(taskSummaryResource.ensure(route.workstreamId));
    }
  }
  if (route.view === "project" && !projectSummaryCache.has(route.projectId)) {
    add(Promise.resolve().then(() => api.getProjectContextState(route.projectId)).then(summary => {
      if (!isRemovedRoute(route)) projectSummaryCache.set(route.projectId, summary);
    }));
  }
  if (route.view === "skills") {
    if (skillProjectsCache === null) add(Promise.resolve().then(() => api.listProjects()).then(setSkillProjectsCache));
    const filter = viewState.get(`skills.${route.projectId ?? ""}.project`) ?? route.projectId;
    if (typeof filter === "string" && filter && filter !== "user" && !projectDetailCache.has(filter)) {
      add(Promise.resolve().then(() => api.getProjectDetail(filter)).then(detail => {
        if (!removedTargets.projects.has(filter)) projectDetailCache.set(filter, detail);
      }));
    }
  }
  if (route.view === "settings") {
    add(extractionSettingsResource.ensure("settings"));
    add(workspaceSettingsResource.ensure("settings"));
  }
  return reads.length ? Promise.allSettled(reads).then(() => {}) : undefined;
}
