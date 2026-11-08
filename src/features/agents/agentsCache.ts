import { api } from "../../api";
import type { Agent, AgentStatusEntry, IngestSource, IngestTaskStatus } from "../../types";
import { createReadResource } from "../../hooks/readResource";

export const agentStatusResource = createReadResource(() => api.getAgentStatus());

export interface AgentsCache {
  agentStatus: Record<string, AgentStatusEntry> | null;
  sources: IngestSource[] | null;
  status: IngestTaskStatus | null;
  sessionCounts: Record<string, number> | null;
  latestActivities: Partial<Record<Agent, string>> | null;
}

export const agentsCache: AgentsCache = {
  get agentStatus() { return agentStatusResource.get("agents") ?? null; },
  set agentStatus(value) { if (value === null) agentStatusResource.clear("agents"); else agentStatusResource.set("agents", value); },
  sources: null,
  status: null,
  sessionCounts: null,
  latestActivities: null,
};

export function clearAgentsCache() {
  agentsCache.agentStatus = null;
  agentsCache.sources = null;
  agentsCache.status = null;
  agentsCache.sessionCounts = null;
  agentsCache.latestActivities = null;
}

let preloadPromise: Promise<void> | null = null;

/**
 * 提前预热 Agent 看板数据，使得用户点击或进入该界面时可直接使用缓存，实现首帧完整渲染、零闪烁与零布局抖动。
 */
export function preloadAgentsCache(): Promise<void> | undefined {
  if (Object.values(agentsCache).every(value => value !== null)) return;
  if (!preloadPromise) {
    preloadPromise = Promise.all([
      agentStatusResource.revalidate("agents"),
      api.listIngestSources().then((res) => { agentsCache.sources = res; }),
      api.getIngestionStatus().then((res) => { agentsCache.status = res; }),
      api.getSessionCountsByAgent().then((res) => { agentsCache.sessionCounts = res; }),
      api.getAgentLatestActivities().then((res) => { agentsCache.latestActivities = res; }),
    ])
      .then(() => {})
      .catch((err) => {
        console.error("Failed to preload agents cache:", err);
      })
      .finally(() => {
        preloadPromise = null;
      });
  }
  return preloadPromise;
}
