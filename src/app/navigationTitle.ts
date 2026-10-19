import { api } from "../api";
import { sessionDetailCache, sessionContextCache } from "../features/sessions/sessionDetailCache";
import { sessionDisplayTitle } from "../features/sessions/sessionDisplay";
import { projectDetailCache } from "../features/projects/projectDetailCache";
import { workstreamDetailCache } from "../features/workstreams/workstreamDetailCache";

import { sessionPages } from "../features/sessions/sessionNavigation";
import type { Route } from "./routes";
import { isRemovedRoute } from "./removedTargets";

/** Known titles navigate synchronously. ID-only links resolve before changing pages. */
export function prepareNavigationTitle(route: Route): Route | Promise<Route> {
  if (route.view === "session") {
    const cached = sessionDetailCache.get(route.sessionId)?.session;
    const remembered = sessionPages.get(route.sessionId);
    const seed = remembered && (remembered.view === "session" || remembered.view === "terminal") ? remembered : undefined;
    const initialTitle = route.initialTitle || (cached ? sessionDisplayTitle(cached.title) : seed?.initialTitle);
    const initialAgent = route.initialAgent ?? cached?.agent ?? seed?.initialAgent;
    if (!sessionContextCache.has(route.sessionId)) {
      void api.getSessionContext(route.sessionId).then(ctx => {
        if (!isRemovedRoute(route)) sessionContextCache.set(route.sessionId, ctx);
      }).catch(() => {});
    }
    if (initialTitle && initialAgent) return { ...route, initialTitle, initialAgent };
    return api.getSessionDetail(route.sessionId).then(detail => {
      if (!isRemovedRoute(route)) sessionDetailCache.set(route.sessionId, detail);
      return { ...route, initialTitle: sessionDisplayTitle(detail.session.title), initialAgent: detail.session.agent };
    });
  }
  if (route.view === "workstream") {
    const initialTitle = route.initialTitle ?? workstreamDetailCache.get(route.workstreamId)?.ctx.workstream.title;
    if (initialTitle !== undefined) return { ...route, initialTitle };
    return api.getWorkstreamContext(route.workstreamId).then(ctx => {
      const cached = workstreamDetailCache.get(route.workstreamId);
      if (!isRemovedRoute(route)) workstreamDetailCache.set(route.workstreamId, { ctx, ctxState: cached?.ctxState ?? null });
      return { ...route, initialTitle: ctx.workstream.title };
    });
  }
  if (route.view === "project") {
    const initialTitle = route.initialTitle ?? projectDetailCache.get(route.projectId)?.project.name;
    if (initialTitle !== undefined) return { ...route, initialTitle };
    return api.getProjectDetail(route.projectId).then(detail => {
      if (!isRemovedRoute(route)) projectDetailCache.set(route.projectId, detail);
      return { ...route, initialTitle: detail.project.name };
    });
  }
  return route;
}
