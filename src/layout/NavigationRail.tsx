import { useEffect, useRef } from "react";
import Icon, { type IconName } from "../components/Icon";
import { withoutAction, type Route } from "../app/routes";
import { isRemovedRoute } from "../app/removedTargets";
import { preloadAgentToolsCache } from "../features/agent-tools/agentToolsCache";

export type RailSection = "sessions" | "workstreams" | "projects" | "agents" | "agent-tools" | "skills" | "packages" | "settings";

export function isSessionRoute(route: Route): boolean {
  return ["new-session", "sessions", "session", "terminal"].includes(route.view);
}

function getRouteSection(route: Route): RailSection | null {
  if (isSessionRoute(route)) return "sessions";
  if (route.view === "workstreams" || route.view === "workstream") return "workstreams";
  if (route.view === "projects") return route.agentOnly ? "agents" : "projects";
  if (route.view === "project") return route.fromAgents ? "agents" : "projects";
  if (route.view === "agent-tools") return "agent-tools";
  if (route.view === "skills" || route.view === "skill") return "skills";
  if (route.view === "packages" || route.view === "package") return "packages";
  if (route.view === "settings") return "settings";
  return null;
}

function getSectionRootRoute(section: RailSection): Route {
  switch (section) {
    case "sessions":
      return { view: "sessions" };
    case "workstreams":
      return { view: "workstreams" };
    case "projects":
      return { view: "projects" };
    case "agents":
      return { view: "projects", agentOnly: true };
    case "agent-tools":
      return { view: "agent-tools" };
    case "skills":
      return { view: "skills" };
    case "packages":
      return { view: "packages" };
    case "settings":
      return { view: "settings" };
  }
}

export default function NavigationRail({ route, navigate }: {
  route: Route;
  navigate: (route: Route) => void;
}) {
  const lastRoutes = useRef<Partial<Record<RailSection, Route>>>({});

  const currentSection = getRouteSection(route);
  if (currentSection && !lastRoutes.current[currentSection]) {
    lastRoutes.current[currentSection] = withoutAction(route);
  }

  useEffect(() => {
    const section = getRouteSection(route);
    if (section) {
      lastRoutes.current[section] = withoutAction(route);
    }
  }, [route]);

  const handleSectionClick = (targetSection: RailSection) => {
    const activeSection = getRouteSection(route);
    if (activeSection === targetSection) {
      // 点击当前界面的固定栏所属图标，跳转到所属看板界面
      navigate(getSectionRootRoute(targetSection));
    } else {
      // 点击固定栏其他图标，跳转到上次离开该图标界面时的某个子界面（若无记录则跳看板）
      const remembered = lastRoutes.current[targetSection];
      navigate(remembered && !isRemovedRoute(remembered) ? remembered : getSectionRootRoute(targetSection));
    }
  };

  const items: Array<{
    section: RailSection;
    label: string;
    icon: IconName;
    active: boolean;
  }> = [
    { section: "sessions", label: "会话", icon: "chat", active: currentSection === "sessions" },
    { section: "workstreams", label: "任务", icon: "tasks", active: ["workstreams", "workstream"].includes(route.view) },
    { section: "projects", label: "项目", icon: "project", active: currentSection === "projects" },
    { section: "agents", label: "智能体", icon: "spark", active: currentSection === "agents" },
    { section: "agent-tools", label: "运行工具", icon: "agent", active: route.view === "agent-tools" },
    { section: "skills", label: "技能", icon: "skill", active: currentSection === "skills" },
    { section: "packages", label: "插件包", icon: "puzzle", active: currentSection === "packages" },
  ];

  return (
    <nav className="navigation-rail" aria-label="工作区导航">
      <div className="navigation-rail-items">
        {items.map((item) => (
          <button
            key={item.section}
            className={`rail-button${item.active ? " active" : ""}`}
            aria-label={item.label}
            title={item.label}
            aria-current={item.active ? "page" : undefined}
            onClick={() => handleSectionClick(item.section)}
            onPointerEnter={() => {
              if (item.section === "agent-tools") {
                void preloadAgentToolsCache();
              }
            }}
          >
            <Icon name={item.icon} />
          </button>
        ))}
      </div>
      <button
        className={`rail-button${route.view === "settings" ? " active" : ""}`}
        aria-label="设置"
        title="设置"
        aria-current={route.view === "settings" ? "page" : undefined}
        onClick={() => handleSectionClick("settings")}
      >
        <Icon name="settings" />
      </button>
    </nav>
  );
}
