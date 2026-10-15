import { EVT_REMOVED, removedTargets } from "../../app/removedTargets";
import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { useRefreshSignal } from "../../components/common";
import type { Agent, WorkstreamCardData } from "../../types";

let cachedCards: WorkstreamCardData[] | null = null;
let cachedDefaultAgent: Agent | null = null;

export function clearWorkstreamCardsCache() {
  cachedCards = null;
  cachedDefaultAgent = null;
}

/** Cards and default agent for the task board. */
export function useWorkstreamCards() {
  const [loadError, setLoadError] = useState("");
  const [cards, setCards] = useState<WorkstreamCardData[] | null>(cachedCards);
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(cachedDefaultAgent);

  const refresh = useCallback(() => {
    setLoadError("");
    api.listWorkstreamCards().then((cs) => {
      const current = cs.filter(w => !removedTargets.workstreams.has(w.id));
      cachedCards = current;
      setCards(current);
    }).catch(e => setLoadError(String(e)));
    api.getDefaultAgent().then((a) => {
      cachedDefaultAgent = a;
      setDefaultAgent(a);
    }).catch(console.error);

  }, []);

  useEffect(() => {
    const remove = () => setCards(rows => rows?.filter(w => !removedTargets.workstreams.has(w.id)) ?? null);
    window.addEventListener(EVT_REMOVED, remove);
    return () => window.removeEventListener(EVT_REMOVED, remove);
  }, []);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  return { loadError, cards, defaultAgent, refresh };
}

