import HomeView from "../features/home/HomeView";
import WorkstreamsView from "../features/workstreams/WorkstreamsView";
import WorkstreamDetailView from "../features/workstreams/WorkstreamDetailView";
import SessionsView from "../features/sessions/SessionsView";
import SessionDetailView from "../features/sessions/SessionDetailView";
import AssistantView from "../features/assistant/AssistantView";
import ProjectsView from "../features/projects/ProjectsView";
import ProjectDetail from "../features/projects/ProjectDetail";
import SettingsView from "../features/settings/SettingsView";
import SearchView from "../features/search/SearchView";
import type { Route } from "./routes";

export default function Router({ route, navigate, goBack, actionSeq }: {
  route: Route;
  navigate: (r: Route) => void;
  goBack: (fallback?: Route) => void;
  actionSeq: number;
}) {
  switch (route.view) {
    case "home":
      return <HomeView navigate={navigate} />;
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
          action={route.action}
          actionSeq={actionSeq}
        />
      );
    case "session":
      return <SessionDetailView sessionId={route.sessionId} navigate={navigate} goBack={goBack} />;
    case "assistant":
      return <AssistantView scope={route.scope} navigate={navigate} />;
    case "projects":
      return <ProjectsView navigate={navigate} />;
    case "project":
      return <ProjectDetail projectId={route.projectId} navigate={navigate} />;
    case "settings":
      return <SettingsView section={route.section ?? "general"} navigate={navigate} />;
    case "search":
      return <SearchView query={route.query} navigate={navigate} />;
    default:
      return <div className="main">Unknown view</div>;
  }
}
