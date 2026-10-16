import NewSessionView from "../features/sessions/NewSessionView";
import WorkstreamsView from "../features/workstreams/WorkstreamsView";
import WorkstreamDetailView, { workstreamDetailCache } from "../features/workstreams/WorkstreamDetailView";
import SessionsView from "../features/sessions/SessionsView";
import SessionDetailView, { sessionDetailCache } from "../features/sessions/SessionDetailView";
import SessionConversationView from "../features/sessions/SessionConversationView";
import SessionTerminalView from "../features/sessions/SessionTerminalView";
import ProjectsView from "../features/projects/ProjectsView";
import ProjectDetail, { projectDetailCache } from "../features/projects/ProjectDetail";
import SettingsView from "../features/settings/SettingsView";
import AgentsView from "../features/agents/AgentsView";
import type { Route } from "./routes";

export default function Router({ route, navigate, goBack, actionSeq }: {
  route: Route;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
  actionSeq: number;
}) {
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
    case "settings":
      return <SettingsView section={(route.section as any) ?? "general"} navigate={navigate} />;
    default:
      return <div className="main">Unknown view</div>;
  }
}
