import { AGENT_LABELS, type Agent, type ExtensionInventory, type ExtensionLocation, type ExtensionResource } from "../../types";
import type { Route } from "../../app/routes";
import { EXTENSION_AGENTS } from "./extensionAgents";

export const EXTENSION_LABELS = { skill: "Skills", mcp: "MCP", package: "插件包" };

export function resourceLocations(inventory: ExtensionInventory, resource: ExtensionResource) {
  return resource.locations.flatMap(id => inventory.locations.find(l => l.id === id) ?? []);
}

export function locationSupportsAgent(location: ExtensionLocation, agent: Agent) {
  return location.agent === agent || (!location.agent
    && (agent === "codex" || agent === "pi" || location.scope === "project" && agent === "antigravity"));
}

export function resourceAgents(locations: ExtensionLocation[]) {
  return EXTENSION_AGENTS.filter(agent => locations.some(l => locationSupportsAgent(l, agent)));
}

export function resourceSource(resource: ExtensionResource, locations: ExtensionLocation[]) {
  if (locations.some(l => l.package)) return "插件包提供";
  if (!resource.managed) return "独立配置";
  return [...new Set(locations.filter(l => !l.writable).map(l => l.label))].join(" · ") || "由应用管理";
}

export function extensionDetailRoute(resource: ExtensionResource, projectId?: string, workspacePathId?: string): Route {
  return { view: "extension", resourceId: resource.id, kind: resource.kind,
    initialTitle: resource.name, projectId, workspacePathId };
}

export function agentNames(locations: ExtensionLocation[]) {
  return resourceAgents(locations).map(agent => AGENT_LABELS[agent]).join(" · ");
}
