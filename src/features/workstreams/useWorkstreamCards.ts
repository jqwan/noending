import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { useRefreshSignal } from "../../components/common";
import type { Agent, WorkstreamCardData, WorkstreamReviewSummary } from "../../types";

/** Cards + default agent + review summaries, shared by Home and Workstreams pages. */
export function useWorkstreamCards() {
  const [loadError, setLoadError] = useState("");
  const [cards, setCards] = useState<WorkstreamCardData[] | null>(null);
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [reviewSummaries, setReviewSummaries] = useState<WorkstreamReviewSummary[] | null>(null);

  const refresh = useCallback(() => {
    setLoadError("");
    api.listWorkstreamCards().then(setCards).catch(e => setLoadError(String(e)));
    api.getDefaultAgent().then(setDefaultAgent).catch(console.error);
    api.listWorkstreamReviewSummaries().then(setReviewSummaries).catch(console.error);
  }, []);

  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  return { loadError, cards, defaultAgent, reviewSummaries, refresh };
}

