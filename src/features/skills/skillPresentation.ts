import type { Agent, SkillInventory, SkillLocation, SkillResource } from "../../types";
import type { Route } from "../../app/routes";
import { displayPath } from "../../pathDisplay";


export function displayScanWarning(warning: string) {
  const separator = warning.indexOf("：");
  return separator < 0 ? warning : displayPath(warning.slice(0, separator)) + warning.slice(separator);
}

export function resourceLocations(inventory: SkillInventory, resource: SkillResource) {
  return resource.locations.flatMap(id => inventory.locations.find(l => l.id === id) ?? []);
}

export function under(path: string, root: string) {
  const base = root.replace(/[\\/]+$/, "");
  return path === base || path.startsWith(`${base}/`) || path.startsWith(`${base}\\`);
}

export function locationSupportsAgent(location: SkillLocation, agent: Agent) {
  // Mirrors supports_user_agent/supports_project_agent in the backend: the
  // shared (agent-less) root covers everyone but Claude and WorkBuddy at
  // user level, plus Antigravity only at project level.
  const sharedReader = agent === "codex" || agent === "pi"
    || agent === "zcode" || agent === "dsh" || agent === "qoder";
  return location.agent === agent || (!location.agent
    && (sharedReader || location.scope === "project" && agent === "antigravity"));
}

export function skillDetailRoute(resource: SkillResource, projectId?: string, workspacePathId?: string): Route {
  return { view: "skill", resourceId: resource.id,
    initialTitle: resource.name, projectId, workspacePathId };
}
