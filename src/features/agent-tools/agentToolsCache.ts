import { api } from "../../api";
import type { AgentTool, AgentToolStatusEntry, IngestSource, IngestTaskStatus } from "../../types";
import { createReadResource } from "../../hooks/readResource";

export const toolStatusResource = createReadResource(() => api.getToolStatus());

export interface AgentToolsCache {
  toolStatus: Record<string, AgentToolStatusEntry> | null;
  sources: IngestSource[] | null;
  status: IngestTaskStatus | null;
  sessionCounts: Record<string, number> | null;
  latestActivities: Partial<Record<AgentTool, string>> | null;
}

export const agentToolsCache: AgentToolsCache = {
  get toolStatus() { return toolStatusResource.get("agent-tools") ?? null; },
  set toolStatus(value) { if (value === null) toolStatusResource.clear("agent-tools"); else toolStatusResource.set("agent-tools", value); },
  sources: null,
  status: null,
  sessionCounts: null,
  latestActivities: null,
};

export function clearAgentToolsCache() {
  agentToolsCache.toolStatus = null;
  agentToolsCache.sources = null;
  agentToolsCache.status = null;
  agentToolsCache.sessionCounts = null;
  agentToolsCache.latestActivities = null;
}

let preloadPromise: Promise<void> | null = null;

/**
 * 提前预热 AgentTool 看板数据，使得用户点击或进入该界面时可直接使用缓存，实现首帧完整渲染、零闪烁与零布局抖动。
 */
export function preloadAgentToolsCache(): Promise<void> | undefined {
  if (Object.values(agentToolsCache).every(value => value !== null)) return;
  if (!preloadPromise) {
    preloadPromise = Promise.all([
      toolStatusResource.revalidate("agent-tools"),
      api.listIngestSources().then((res) => { agentToolsCache.sources = res; }),
      api.getIngestionStatus().then((res) => { agentToolsCache.status = res; }),
      api.getSessionCountsByTool().then((res) => { agentToolsCache.sessionCounts = res; }),
      api.getToolLatestActivities().then((res) => { agentToolsCache.latestActivities = res; }),
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
