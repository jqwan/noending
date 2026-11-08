import { createElement, Suspense, type ComponentType } from "react";
import { workstreamDetailCache } from "../features/workstreams/workstreamDetailCache";
import { sessionDetailCache } from "../features/sessions/sessionDetailCache";
import { projectDetailCache } from "../features/projects/projectDetailCache";
import type { Route } from "./routes";
import { PageHeaderHost } from "../layout/PageHeader";

/** 渲染与预载共享同一份模块状态，预载完成后首帧可同步渲染。 */
function deferredRoute<P extends object>(load: () => Promise<{ default: ComponentType<P> }>) {
  let component: ComponentType<P> | null = null;
  let pending: Promise<void> | null = null;
  let failure: unknown;
  const preload = (): Promise<void> | undefined => {
    if (component) return;
    if (!pending) {
      failure = undefined;
      pending = load().then(module => { component = module.default; }).catch(error => {
        failure = error; throw error;
      }).finally(() => { pending = null; });
    }
    return pending;
  };
  function View(props: P) {
    if (component) return createElement(component, props);
    if (failure) throw failure;
    throw preload();
  }
  return { View, preload };
}

const modules = {
  "new-session": deferredRoute(() => import("../features/sessions/NewSessionView")),
  "workstreams": deferredRoute(() => import("../features/workstreams/WorkstreamsView")),
  "workstream": deferredRoute(() => import("../features/workstreams/WorkstreamDetailView")),
  "sessions": deferredRoute(() => import("../features/sessions/SessionsView")),
  "session": deferredRoute(() => import("../features/sessions/SessionDetailView")),
  "conversation": deferredRoute(() => import("../features/sessions/SessionConversationView")),
  "terminal": deferredRoute(() => import("../features/sessions/SessionTerminalView")),
  "projects": deferredRoute(() => import("../features/projects/ProjectsView")),
  "project": deferredRoute(() => import("../features/projects/ProjectDetail")),
  "settings": deferredRoute(() => import("../features/settings/SettingsView")),
  "agents": deferredRoute(() => import("../features/agents/AgentsView")),
  "skills": deferredRoute(() => import("../features/skills/SkillsView")),
  "skill": deferredRoute(() => import("../features/skills/SkillDetailView")),
  "packages": deferredRoute(() => import("../features/packages/PackagesView")),
  "package": deferredRoute(() => import("../features/packages/PackageDetailView")),
};

const NewSessionView = modules["new-session"].View;
const WorkstreamsView = modules["workstreams"].View;
const WorkstreamDetailView = modules["workstream"].View;
const SessionsView = modules["sessions"].View;
const SessionDetailView = modules["session"].View;
const SessionConversationView = modules["conversation"].View;
const SessionTerminalView = modules["terminal"].View;
const ProjectsView = modules["projects"].View;
const ProjectDetail = modules["project"].View;
const SettingsView = modules["settings"].View;
const AgentsView = modules["agents"].View;
const SkillsView = modules["skills"].View;
const SkillDetailView = modules["skill"].View;
const PackagesView = modules["packages"].View;
const PackageDetailView = modules["package"].View;

export function prepareRouteModule(route: Route) {
  const key = route.view === "session" && route.entry === "conversation" ? "conversation" : route.view;
  return modules[key].preload();
}

export function preloadCommonRoutes() {
  for (const module of Object.values(modules)) void module.preload()?.catch(() => {});
  void import("../features/agents/agentsCache").then(m => m.preloadAgentsCache()).catch(() => {});
}

export default function Router({ route, navigate, goBack, actionSeq }: {
  route: Route;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
  actionSeq: number;
}) {
  const content = (
    <Suspense fallback={null}>
      {renderRoute(route, navigate, goBack, actionSeq)}
    </Suspense>
  );
  return route.view === "session" || route.view === "terminal" ? <PageHeaderHost>{content}</PageHeaderHost> : content;
}

function renderRoute(
  route: Route,
  navigate: (r: Route) => void,
  goBack: (fallback?: Route) => void,
  actionSeq: number,
) {
  switch (route.view) {
    case "new-session":
      return (
        <NewSessionView
          key={`${route.workstreamId ?? ""}:${route.agent ?? ""}:${route.projectId ?? ""}`}
          navigate={navigate}
          workstreamId={route.workstreamId}
          agent={route.agent}
          projectId={route.projectId}
        />
      );
    case "workstreams":
      return (
        <WorkstreamsView
          navigate={navigate}
          action={route.action}
          scope={route.scope}
          actionSeq={actionSeq}
        />
      );
    case "workstream":
      return (
        <WorkstreamDetailView
          key={route.workstreamId}
          initialTitle={route.initialTitle ?? workstreamDetailCache.get(route.workstreamId)?.ctx.workstream.title}
          workstreamId={route.workstreamId}
          entry={route.entry}
          navigate={navigate}
          goBack={goBack}
        />
      );
    case "sessions":
      return (
        <SessionsView
          navigate={navigate}
          scope={route.scope}
        />
      );
    case "session":
      return route.entry === "conversation" ? (
        <SessionConversationView
          key={route.sessionId}
          sessionId={route.sessionId}
          initialTitle={route.initialTitle ?? sessionDetailCache.get(route.sessionId)?.session.title ?? undefined}
          initialAgent={route.initialAgent ?? sessionDetailCache.get(route.sessionId)?.session.agent}
          initialTotal={route.initialTotal}
          navigate={navigate}
        />
      ) : (
        <SessionDetailView
          key={route.sessionId}
          sessionId={route.sessionId}
          initialTitle={route.initialTitle ?? sessionDetailCache.get(route.sessionId)?.session.title ?? undefined}
          initialAgent={route.initialAgent ?? sessionDetailCache.get(route.sessionId)?.session.agent}
          navigate={navigate}
        />
      );
    case "terminal":
      return (
        <SessionTerminalView
          key={route.terminalId}
          terminalId={route.terminalId}
          initialTitle={route.initialTitle}
          initialAgent={route.initialAgent}
          initialSessionId={route.initialSessionId}
          navigate={navigate}
        />
      );
    case "projects":
      return <ProjectsView navigate={navigate} scope={route.scope} />;
    case "project":
      return <ProjectDetail key={route.projectId} projectId={route.projectId} initialTitle={route.initialTitle ?? projectDetailCache.get(route.projectId)?.project.name} navigate={navigate} />;
    case "agents":
      return <AgentsView key={route.agent ?? "overview"} navigate={navigate} initialAgent={route.agent} />;
    case "skills":
      return <SkillsView key={route.projectId ?? "user"} initialProjectId={route.projectId} navigate={navigate} />;
    case "skill":
      return <SkillDetailView key={`${route.resourceId}:${route.projectId ?? ""}:${route.workspacePathId ?? ""}`} {...route} navigate={navigate} />;
    case "packages":
      return <PackagesView navigate={navigate} />;
    case "package":
      return <PackageDetailView key={route.packageId} packageId={route.packageId} initialTitle={route.initialTitle} agent={route.agent} navigate={navigate} />;
    case "settings":
      return <SettingsView section={(route.section as any) ?? "general"} navigate={navigate} />;
    default:
      return <div className="main">Unknown view</div>;
  }
}
