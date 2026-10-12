import { useCallback, useEffect, useRef, useState } from "react";
import { readRuntimeCatalog, saveRuntimeCatalog, removeRuntimeCatalog } from "./runtimeCatalog";
import { api } from "../../api";
import type { Agent, AgentRuntimeDiscovery, AgentRuntimeSettings } from "../../types";

const generations = new Map<Agent, number>();
const pending = new Map<Agent, Promise<AgentRuntimeDiscovery>>();
const CACHE_MS = 5 * 60 * 1000;

/** Re-detection invalidates advisory data without touching saved overrides. */
export function clearRuntimeCatalog(agent?: Agent) {
  removeRuntimeCatalog(agent);
  for (const item of agent ? [agent] : [...pending.keys()]) {
    generations.set(item, (generations.get(item) ?? 0) + 1);
    pending.delete(item);
  }
}

/** Catalogs are advisory, persisted for immediate display, and refreshed only on user action. */
export function useAgentRuntime(agent: Agent, enabled = true) {
  const [loaded, setLoaded] = useState<AgentRuntimeSettings | null>(null);
  const [catalog, setCatalog] = useState<{ agent: Agent; value: AgentRuntimeDiscovery } | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const current = useRef(agent);
  current.current = agent;
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  useEffect(() => {
    setLoading(false);
    if (!enabled) return;
    let cancelled = false;
    setError("");
    api.getAgentRuntimeSettings(agent).then((value) => {
      if (!cancelled) setLoaded(value);
    }).catch((e) => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, [agent, enabled]);

  const refresh = useCallback(async (force = false) => {
    const settings = loaded?.agent === agent ? loaded : null;
    if (!settings) return;
    const cached = readRuntimeCatalog(agent);
    if (!force && cached && cached.version === settings.version && cached.executable === settings.executable && Date.now() - cached.at < CACHE_MS) {
      setCatalog({ agent, value: cached.value });
      return;
    }
    setLoading(true);
    setError("");
    const generation = generations.get(agent) ?? 0;
    try {
      let request = pending.get(agent);
      if (!request) {
        request = api.refreshAgentRuntimeOptions(agent);
        pending.set(agent, request);
        void request.finally(() => { if (pending.get(agent) === request) pending.delete(agent); }).catch(() => {});
      }
      const value = await request;
      if ((generations.get(agent) ?? 0) !== generation) return;
      if (value.model_source === "unavailable" || value.model_source === "not_loaded") {
        throw new Error(value.warnings.join("；") || "无法获取模型列表");
      }
      saveRuntimeCatalog(agent, { at: Date.now(), version: settings.version, executable: settings.executable, value });
      if (mounted.current && current.current === agent) setCatalog({ agent, value });
    } catch (e) {
      if ((generations.get(agent) ?? 0) === generation && mounted.current && current.current === agent) setError(String(e));
    } finally {
      if (mounted.current && current.current === agent) setLoading(false);
    }
  }, [agent, loaded?.agent, loaded?.version, loaded?.executable]);

  const settings = loaded?.agent === agent ? loaded : null;
  const options = catalog?.agent === agent ? catalog.value : readRuntimeCatalog(agent)?.value;
  const invalidate = () => { clearRuntimeCatalog(agent); setCatalog(null); };
  const acceptSettings = (value: AgentRuntimeSettings) => { setLoaded(value); setError(""); };
  return { settings, options: settings ? { ...settings, ...options } : null, error, loading, refresh, setLoaded: acceptSettings, invalidate };
}
