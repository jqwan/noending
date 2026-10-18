import { lazy, Suspense } from "react";
import { workstreamDetailCache } from "../features/workstreams/workstreamDetailCache";
import { sessionDetailCache } from "../features/sessions/sessionDetailCache";
import { projectDetailCache } from "../features/projects/projectDetailCache";
import type { Route } from "./routes";

const NewSessionView = lazy(() => import("../features/sessions/NewSessionView"));
const WorkstreamsView = lazy(() => import("../features/workstreams/WorkstreamsView"));
const WorkstreamDetailView = lazy(() => import("../features/workstreams/WorkstreamDetailView"));
const SessionsView = lazy(() => import("../features/sessions/SessionsView"));
const SessionDetailView = lazy(() => import("../features/sessions/SessionDetailView"));
const SessionConversationView = lazy(() => import("../features/sessions/SessionConversationView"));
const SessionTerminalView = lazy(() => import("../features/sessions/SessionTerminalView"));
const ProjectsView = lazy(() => import("../features/projects/ProjectsView"));
const ProjectDetail = lazy(() => import("../features/projects/ProjectDetail"));
const SettingsView = lazy(() => import("../features/settings/SettingsView"));
const AgentsView = lazy(() => import("../features/agents/AgentsView"));
const ExtensionsView = lazy(() => import("../features/extensions/ExtensionsView"));
const ExtensionDetailView = lazy(() => import("../features/extensions/ExtensionDetailView"));


export function preloadCommonRoutes() {
  const common = [
    () => import("../features/sessions/SessionsView"),
    () => import("../features/sessions/SessionConversationView"),
    () => import("../features/sessions/SessionDetailView"),
    () => import("../features/sessions/SessionTerminalView"),
    () => import("../features/projects/ProjectsView"),
    () => import("../features/workstreams/WorkstreamsView"),
    () => import("../features/agents/AgentsView"),
  ];
  for (const load of common) {
    try {
      void load();
    } catch {
      // Ignore background preload failures
    }
  }
}

export default function Router({ route, navigate, goBack, actionSeq }: {
  route: Route;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
  actionSeq: number;
}) {
  return (
    <Suspense fallback={null}>
      {renderRoute(route, navigate, goBack, actionSeq)}
    </Suspense>
  );
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
    case "extensions":
      return <ExtensionsView key={`${route.projectId ?? "user"}:${route.tab ?? "skill"}`} initialProjectId={route.projectId} initialTab={route.tab} navigate={navigate} />;
    case "extension":
      return <ExtensionDetailView key={`${route.resourceId}:${route.projectId ?? ""}:${route.workspacePathId ?? ""}`} {...route} navigate={navigate} />;
    case "settings":
      return <SettingsView section={(route.section as any) ?? "general"} navigate={navigate} />;
    default:
      return <div className="main">Unknown view</div>;
  }
}
