import NewSessionView from "../features/sessions/NewSessionView";
import WorkstreamsView from "../features/workstreams/WorkstreamsView";
import WorkstreamDetailView from "../features/workstreams/WorkstreamDetailView";
import SessionsView from "../features/sessions/SessionsView";
import SessionDetailView from "../features/sessions/SessionDetailView";
import SessionConversationView from "../features/sessions/SessionConversationView";
import SessionTerminalView from "../features/sessions/SessionTerminalView";
import AssistantView from "../features/assistant/AssistantView";
import ProjectsView from "../features/projects/ProjectsView";
import ProjectDetail from "../features/projects/ProjectDetail";
import SettingsView from "../features/settings/SettingsView";
import AgentsView from "../features/agents/AgentsView";
import SearchView from "../features/search/SearchView";
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
          initialTitle={route.initialTitle}
          initialAgent={route.initialAgent}
          initialTotal={route.initialTotal}
          navigate={navigate}
        />
      ) : (
        <SessionDetailView
          sessionId={route.sessionId}
          initialTitle={route.initialTitle}
          initialAgent={route.initialAgent}
          navigate={navigate}
          goBack={goBack}
        />
      );
    case "terminal":
      return (
        <SessionTerminalView
          key={route.terminalId}
          terminalId={route.terminalId}
          navigate={navigate}
        />
      );
    case "assistant":
      return <AssistantView scope={route.scope} navigate={navigate} />;
    case "projects":
      return <ProjectsView navigate={navigate} />;
    case "project":
      return <ProjectDetail projectId={route.projectId} navigate={navigate} />;
    case "agents":
      return <AgentsView navigate={navigate} initialAgent={route.agent} />;
    case "settings":
      return <SettingsView section={(route.section as any) ?? "general"} navigate={navigate} />;
    case "search":
      return <SearchView query={route.query} navigate={navigate} />;
    default:
      return <div className="main">Unknown view</div>;
  }
}
