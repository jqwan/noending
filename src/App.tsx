import React, { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import { AGENT_LABELS, type Agent, type Project, type Workstream } from "./types";
import SidebarLogo from "./components/SidebarLogo";
import LazyRouter from "./LazyRouter";
import CommandPalette from "./components/CommandPalette";

export type Route =
  | { view: "home" }
  | { view: "assistant" }
  | { view: "search"; query: string }
  | { view: "project"; projectId: string }
  | { view: "workstream"; workstreamId: string }
  | { view: "session"; sessionId: string }
  | { view: "sessions" }
  | { view: "sources" };

function AgentBadge({ agent }: { agent: Agent }) {
  return <span className="badge">{AGENT_LABELS[agent]}</span>;
}

export default function App() {
  const [route, setRoute] = useState<Route>({ view: "home" });
  const [projects, setProjects] = useState<Project[]>([]);
  const [recent, setRecent] = useState<Workstream[]>([]);
  const [paletteOpen, setPaletteOpen] = useState(false);

  const refreshSidebar = () => {
    api.listProjects().then(setProjects).catch(console.error);
    api.listWorkstreams().then((ws) => setRecent(ws.slice(0, 6))).catch(console.error);
  };

  useEffect(refreshSidebar, []);

  // 入库在「会话数据源」页发起（后台执行）；完成后刷新全局数据。
  useEffect(() => {
    const un = listen("sync-completed", () => {
      refreshSidebar();
      window.dispatchEvent(new CustomEvent("noending:sync"));
    });
    return () => { un.then((f) => f()); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ⌘K / Ctrl+K opens the command palette
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, []);

  return (
    <div className="app">
      <Sidebar
        route={route}
        navigate={setRoute}
        projects={projects}
        recent={recent}
        onSearch={() => setPaletteOpen(true)}
      />
      <LazyMain route={route} navigate={setRoute} refreshSidebar={refreshSidebar} />
      {paletteOpen && <CommandPalette onClose={() => setPaletteOpen(false)} navigate={setRoute} />}
    </div>
  );
}

function Sidebar(props: {
  route: Route;
  navigate: (r: Route) => void;
  projects: Project[];
  recent: Workstream[];
  onSearch: () => void;
}) {
  const { route, navigate, projects, recent, onSearch } = props;
  const isActive = (v: string) =>
    route.view === v || (v === "project" && route.view === "workstream");
  return (
    <div className="sidebar">
      <div className="brand">
        <SidebarLogo size={22} />
        <span className="brand-name">NoEnding</span>
      </div>
      <div className="brand-tagline">对话会结束，上下文不会。</div>

      <button className="nav-item" onClick={onSearch}>
        搜索与跳转
        <span style={{ flex: 1 }} />
        <span className="kbd">⌘K</span>
      </button>
      <button className={`nav-item ${isActive("assistant") ? "active" : ""}`}
        onClick={() => navigate({ view: "assistant" })}>
        Assistant
      </button>
      <button className={`nav-item ${isActive("sessions") ? "active" : ""}`}
        onClick={() => navigate({ view: "sessions" })}>
        All Sessions
      </button>
      <button className={`nav-item ${isActive("sources") ? "active" : ""}`}
        onClick={() => navigate({ view: "sources" })}>
        会话数据源
      </button>

      <div className="nav-section">Projects</div>
      {projects.length === 0 && <div className="nav-item muted small">尚未创建</div>}
      {projects.map((p) => (
        <button key={p.id}
          className={`nav-item ${route.view === "project" && route.projectId === p.id ? "active" : ""}`}
          title={p.name}
          onClick={() => navigate({ view: "project", projectId: p.id })}>
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{p.name}</span>
        </button>
      ))}

      <div className="nav-section">Recent Workstreams</div>
      {recent.map((w) => (
        <button key={w.id}
          className={`nav-item ${route.view === "workstream" && route.workstreamId === w.id ? "active" : ""}`}
          title={w.title}
          onClick={() => navigate({ view: "workstream", workstreamId: w.id })}>
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 13 }}>
            {w.title}
          </span>
        </button>
      ))}

      <div className="spacer" />
    </div>
  );
}

const LazyMain = LazyRouter;

export { AgentBadge };
