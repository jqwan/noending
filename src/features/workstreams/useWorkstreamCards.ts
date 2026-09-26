import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { useRefreshSignal } from "../../components/common";
import { useBaseExperience } from "../../app/experience";
import type { Agent, WorkstreamCardData, WorkstreamReviewSummary } from "../../types";

/** Cards + default agent + review summaries, shared by Home and Workstreams pages. */
export function useWorkstreamCards() {
  const [loadError, setLoadError] = useState("");
  const [cards, setCards] = useState<WorkstreamCardData[] | null>(null);
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [reviewSummaries, setReviewSummaries] = useState<WorkstreamReviewSummary[] | null>(null);
  const { intelligenceEnabled } = useBaseExperience();

  const refresh = useCallback(() => {
    setLoadError("");
    api.listWorkstreamCards().then(setCards).catch(e => setLoadError(String(e)));
    api.getDefaultAgent().then(setDefaultAgent).catch(console.error);
    // 要求 review 五支在 Base Experience 下不可达 —— 不是"发了请求再藏起来"。
    // 返回形状按 冻结，所以这里只跳过请求，留下 null。
    if (intelligenceEnabled) {
      api.listWorkstreamReviewSummaries().then(setReviewSummaries).catch(console.error);
    }
  }, [intelligenceEnabled]);

  useEffect(refresh, [refresh]);
  useRefreshSignal(refresh);

  return { loadError, cards, defaultAgent, reviewSummaries, refresh };
}

