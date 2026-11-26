import { EVT_REMOVED, removedTargets } from "../../app/removedTargets";
import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { useRefreshSignal } from "../../components/common";
import type { WorkstreamCardData } from "../../types";

export const workstreamCardsCache = {
  cards: null as WorkstreamCardData[] | null,

};

export function clearWorkstreamCardsCache() {
  workstreamCardsCache.cards = null;

}

/** Cards for the task board. */
export function useWorkstreamCards() {
  const [loadError, setLoadError] = useState("");
  const [cards, setCards] = useState<WorkstreamCardData[] | null>(workstreamCardsCache.cards);


  const refresh = useCallback(() => {
    setLoadError("");
    api.listWorkstreamCards().then((cs) => {
      const current = cs.filter(w => !removedTargets.workstreams.has(w.id));
      workstreamCardsCache.cards = current;
      setCards(current);
    }).catch(e => setLoadError(String(e)));
  }, []);

  useEffect(() => {
    const remove = () => setCards(rows => rows?.filter(w => !removedTargets.workstreams.has(w.id)) ?? null);
    window.addEventListener(EVT_REMOVED, remove);
    return () => window.removeEventListener(EVT_REMOVED, remove);
  }, []);
  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  return { loadError, cards, refresh };
}

