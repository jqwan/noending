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
      cachedCards = cs;
      setCards(cs);
    }).catch(e => setLoadError(String(e)));
    api.getDefaultAgent().then((a) => {
      cachedDefaultAgent = a;
      setDefaultAgent(a);
    }).catch(console.error);

  }, []);

  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  return { loadError, cards, defaultAgent, refresh };
}

