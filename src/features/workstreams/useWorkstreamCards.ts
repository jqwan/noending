import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { useRefreshSignal } from "../../components/common";
import type { Agent, WorkstreamCardData } from "../../types";

/** Cards + default agent, shared by Home and Workstreams pages. */
export function useWorkstreamCards() {
  const [cards, setCards] = useState<WorkstreamCardData[] | null>(null);
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);

  const refresh = useCallback(() => {
    api.listWorkstreamCards().then(setCards).catch(console.error);
    api.getDefaultAgent().then(setDefaultAgent).catch(console.error);
  }, []);

  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  return { cards, defaultAgent, refresh };
}
