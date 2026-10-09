import Icon from "../components/Icon";
import type { Route } from "../app/routes";

export function isSessionRoute(route: Route): boolean {
  return ["new-session", "sessions", "session", "terminal"].includes(route.view);
}

export default function NavigationRail({ route, navigate, onSessions, onSearch }: {
  route: Route;
  navigate: (route: Route) => void;
  onSessions: () => void;
  onSearch: () => void;
}) {
  const items = [
    { view: "sessions", label: "首页", icon: "home", active: isSessionRoute(route) },
    { view: "search", label: "搜索", icon: "search", active: route.view === "search" },
    { view: "workstreams", label: "任务", icon: "tasks", active: ["workstreams", "workstream"].includes(route.view) },
    { view: "projects", label: "项目", icon: "project", active: ["projects", "project"].includes(route.view) },
    { view: "agents", label: "代理", icon: "bot", active: route.view === "agents" },
    { view: "assistant", label: "助手", icon: "spark", active: route.view === "assistant" },
  ] as const;
  return (
    <nav className="navigation-rail" aria-label="工作区导航">
      <div className="navigation-rail-items">
        {items.map((item) => (
          <button key={item.view} className={`rail-button${item.active ? " active" : ""}`}
            aria-label={item.label} title={item.label} aria-current={item.active ? "page" : undefined}
            onClick={() => {
              if (item.view === "sessions") onSessions();
              else if (item.view === "search") onSearch();
              else navigate({ view: item.view });
            }}>
            <Icon name={item.icon} />
          </button>
        ))}
      </div>
      <button className={`rail-button${route.view === "settings" ? " active" : ""}`}
        aria-label="设置" title="设置" aria-current={route.view === "settings" ? "page" : undefined}
        onClick={() => navigate({ view: "settings" })}>
        <Icon name="settings" />
      </button>
    </nav>
  );
}
