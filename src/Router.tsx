import React from "react";
import { Route } from "./App";
import ProjectsView from "./features/projects/ProjectsView";
import ProjectDetail from "./features/projects/ProjectDetail";
import WorkstreamView from "./features/workstreams/WorkstreamView";
import SessionsView from "./features/sessions/SessionsView";
import SessionDetailView from "./features/sessions/SessionDetailView";
import AssistantView from "./features/assistant/AssistantView";
import SearchView from "./features/search/SearchView";

export default function Router({ route, navigate, refreshSidebar }: {
  route: Route;
  navigate: (r: Route) => void;
  refreshSidebar: () => void;
}) {
  switch (route.view) {
    case "home":
      return <ProjectsView navigate={navigate} refreshSidebar={refreshSidebar} />;
    case "project":
      return <ProjectDetail projectId={route.projectId} navigate={navigate} refreshSidebar={refreshSidebar} />;
    case "workstream":
      return <WorkstreamView workstreamId={route.workstreamId} navigate={navigate} refreshSidebar={refreshSidebar} />;
    case "sessions":
      return <SessionsView navigate={navigate} />;
    case "session":
      return <SessionDetailView sessionId={route.sessionId} navigate={navigate} />;
    case "assistant":
      return <AssistantView navigate={navigate} />;
    case "search":
      return <SearchView query={route.query} navigate={navigate} />;
    default:
      return <div className="main">Unknown view</div>;
  }
}
