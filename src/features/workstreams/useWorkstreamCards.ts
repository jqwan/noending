import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { useRefreshSignal } from "../../components/common";
import type { Agent, WorkstreamCardData, WorkstreamReviewSummary } from "../../types";

let cachedCards: WorkstreamCardData[] | null = null;
let cachedDefaultAgent: Agent | null = null;
let cachedReviewSummaries: WorkstreamReviewSummary[] | null = null;

export function clearWorkstreamCardsCache() {
  cachedCards = null;
  cachedDefaultAgent = null;
  cachedReviewSummaries = null;
}

/** Cards + default agent + review summaries, shared by Home and Workstreams pages. */
export function useWorkstreamCards() {
  const [loadError, setLoadError] = useState("");
  const [cards, setCards] = useState<WorkstreamCardData[] | null>(cachedCards);
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(cachedDefaultAgent);
  const [reviewSummaries, setReviewSummaries] = useState<WorkstreamReviewSummary[] | null>(cachedReviewSummaries);

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
    api.listWorkstreamReviewSummaries().then((rs) => {
      cachedReviewSummaries = rs;
      setReviewSummaries(rs);
    }).catch(console.error);
  }, []);

  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  return { loadError, cards, defaultAgent, reviewSummaries, refresh };
}

