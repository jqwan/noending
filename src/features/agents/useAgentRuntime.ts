import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { Agent, AgentRuntimeDiscovery, AgentRuntimeSettings } from "../../types";

const catalogs = new Map<Agent, { at: number; value: AgentRuntimeDiscovery }>();
const pending = new Map<Agent, Promise<AgentRuntimeDiscovery>>();
const CACHE_MS = 5 * 60 * 1000;

/** Re-detection invalidates advisory data without touching saved overrides. */
export function clearRuntimeCatalog(agent?: Agent) {
  if (agent) catalogs.delete(agent);
  else catalogs.clear();
}

/** Catalogs are advisory, cached in memory, and fetched only on user action. */
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
    const cached = catalogs.get(agent);
    if (!force && cached && Date.now() - cached.at < CACHE_MS) {
      setCatalog({ agent, value: cached.value });
      return;
    }
    setLoading(true);
    setError("");
    try {
      let request = pending.get(agent);
      if (!request) {
        request = api.refreshAgentRuntimeOptions(agent);
        pending.set(agent, request);
        void request.finally(() => { if (pending.get(agent) === request) pending.delete(agent); }).catch(() => {});
      }
      const value = await request;
      catalogs.set(agent, { at: Date.now(), value });
      if (mounted.current && current.current === agent) setCatalog({ agent, value });
    } catch (e) {
      if (mounted.current && current.current === agent) setError(String(e));
    } finally {
      if (mounted.current && current.current === agent) setLoading(false);
    }
  }, [agent]);

  const settings = loaded?.agent === agent ? loaded : null;
  const options = catalog?.agent === agent ? catalog.value : catalogs.get(agent)?.value;
  const invalidate = () => { clearRuntimeCatalog(agent); setCatalog(null); };
  const acceptSettings = (value: AgentRuntimeSettings) => { setLoaded(value); setError(""); };
  return { settings, options: settings ? { ...settings, ...options } : null, error, loading, refresh, setLoaded: acceptSettings, invalidate };
}
