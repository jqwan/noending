import type { Agent, ExtensionInventory, ExtensionLocation, ExtensionResource } from "../../types";
import type { Route } from "../../app/routes";

export const EXTENSION_LABELS = { skill: "Skills", mcp: "MCP" };

export function resourceLocations(inventory: ExtensionInventory, resource: ExtensionResource) {
  return resource.locations.flatMap(id => inventory.locations.find(l => l.id === id) ?? []);
}

export function under(path: string, root: string) {
  const base = root.replace(/[\\/]+$/, "");
  return path === base || path.startsWith(`${base}/`) || path.startsWith(`${base}\\`);
}

export function locationSupportsAgent(location: ExtensionLocation, agent: Agent) {
  return location.agent === agent || (!location.agent
    && (agent === "codex" || agent === "pi" || location.scope === "project" && agent === "antigravity"));
}

export function extensionDetailRoute(resource: ExtensionResource, projectId?: string, workspacePathId?: string): Route {
  return { view: "extension", resourceId: resource.id, kind: resource.kind,
    initialTitle: resource.name, projectId, workspacePathId };
}
