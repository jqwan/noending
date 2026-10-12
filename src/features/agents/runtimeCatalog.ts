import { readViewPreference, writeViewPreference, viewState } from "../../hooks/useViewState";
import { AGENT_LABELS, type Agent, type AgentRuntimeDiscovery } from "../../types";

export interface RuntimeCatalog {
  at: number;
  version: string | null;
  executable: string | null;
  value: AgentRuntimeDiscovery;
}

const key = (agent: Agent) => `agent.catalog.${agent}`;
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(item => typeof item === "string");
const nullableString = (value: unknown) => value === null || typeof value === "string";

function isCatalog(raw: unknown): raw is RuntimeCatalog | null {
  if (raw === null) return true;
  if (!raw || typeof raw !== "object") return false;
  const catalog = raw as RuntimeCatalog;
  const value = catalog.value;
  return Number.isFinite(catalog.at) && nullableString(catalog.version) && nullableString(catalog.executable)
    && Boolean(value && ["dynamic", "suggested"].includes(value.model_source)
      && strings(value.effort_levels) && strings(value.warnings)
      && value.capabilities && ["model", "provider", "effort"].every(field =>
        ["unsupported", "free_form", "discoverable", "suggested"].includes(value.capabilities[field as keyof typeof value.capabilities]))
      && Array.isArray(value.models) && value.models.every(model => model && typeof model.id === "string"
        && nullableString(model.display_name) && nullableString(model.provider) && strings(model.supported_efforts)));
}

export function readRuntimeCatalog(agent: Agent) {
  return readViewPreference(key(agent), null, isCatalog);
}

/** A successful discovery replaces the entire catalog, including entries removed by the CLI. */
export function saveRuntimeCatalog(agent: Agent, catalog: RuntimeCatalog) {
  writeViewPreference(key(agent), catalog);
}

export function removeRuntimeCatalog(agent?: Agent) {
  for (const item of agent ? [agent] : Object.keys(AGENT_LABELS) as Agent[]) {
    viewState.delete(key(item));
    try { localStorage.removeItem(`noending.view.${key(item)}`); } catch { /* Memory invalidation still works. */ }
  }
}
