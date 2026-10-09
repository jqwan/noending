import { api } from "../api";
import { sessionDetailCache } from "../features/sessions/SessionDetailView";
import { sessionDisplayTitle } from "../features/sessions/SessionTable";
import { projectDetailCache } from "../features/projects/ProjectDetail";
import { workstreamDetailCache } from "../features/workstreams/WorkstreamDetailView";
import { sessionPages } from "../features/sessions/sessionNavigation";
import type { Route } from "./routes";

/** Known titles navigate synchronously. ID-only links resolve before changing pages. */
export function prepareNavigationTitle(route: Route): Route | Promise<Route> {
  if (route.view === "session") {
    const cached = sessionDetailCache.get(route.sessionId)?.session;
    const remembered = sessionPages.get(route.sessionId);
    const seed = remembered && (remembered.view === "session" || remembered.view === "terminal") ? remembered : undefined;
    const initialTitle = route.initialTitle || (cached ? sessionDisplayTitle(cached.title) : seed?.initialTitle);
    const initialAgent = route.initialAgent ?? cached?.agent ?? seed?.initialAgent;
    if (initialTitle && initialAgent) return { ...route, initialTitle, initialAgent };
    return api.getSessionDetail(route.sessionId).then(detail => {
      sessionDetailCache.set(route.sessionId, detail);
      return { ...route, initialTitle: sessionDisplayTitle(detail.session.title), initialAgent: detail.session.agent };
    });
  }
  if (route.view === "workstream") {
    const initialTitle = route.initialTitle ?? workstreamDetailCache.get(route.workstreamId)?.ctx.workstream.title;
    if (initialTitle !== undefined) return { ...route, initialTitle };
    return api.getWorkstreamContext(route.workstreamId).then(ctx => {
      const cached = workstreamDetailCache.get(route.workstreamId);
      workstreamDetailCache.set(route.workstreamId, { ctx, ctxState: cached?.ctxState ?? null, paths: cached?.paths ?? null });
      return { ...route, initialTitle: ctx.workstream.title };
    });
  }
  if (route.view === "project") {
    const initialTitle = route.initialTitle ?? projectDetailCache.get(route.projectId)?.project.name;
    if (initialTitle !== undefined) return { ...route, initialTitle };
    return api.getProjectDetail(route.projectId).then(detail => {
      projectDetailCache.set(route.projectId, detail);
      return { ...route, initialTitle: detail.project.name };
    });
  }
  return route;
}
