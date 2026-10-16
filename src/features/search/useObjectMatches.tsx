import { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import type { NavigationItem } from "../../types";

/** Boards use the same fields as lookup, without its preview limit. */
export function useObjectMatches(kind: NavigationItem["kind"], query: string, revision: unknown, candidateIds: readonly string[]) {
  const text = query.trim();
  const [attempt, setAttempt] = useState(0);
  const [result, setResult] = useState<{
    kind: NavigationItem["kind"]; text: string; revision: unknown; attempt: number;
    ids: Set<string>; error: string; candidateIds: readonly string[];
  } | null>(null);
  useEffect(() => {
    if (!text) return;
    let active = true;
    const timer = window.setTimeout(() => {
      api.matchObjects(kind, text, candidateIds).then(
        ids => { if (active) setResult({ kind, text, revision, attempt, candidateIds, ids: new Set(ids), error: "" }); },
        error => { if (active) setResult({ kind, text, revision, attempt, candidateIds, ids: new Set(), error: String(error) }); },
      );
    }, 180);
    return () => { active = false; window.clearTimeout(timer); };
  }, [kind, text, revision, attempt, candidateIds]);
  return useMemo(() => {
    const current = result?.kind === kind && result.text === text && result.revision === revision && result.attempt === attempt && result.candidateIds === candidateIds;
    return {
      ids: text && current ? result.ids : null,
      pending: Boolean(text && !current),
      error: text && current ? result.error : "",
      retry: () => setAttempt(value => value + 1),
    };
  }, [kind, text, revision, attempt, candidateIds, result]);
}

export function ObjectSearchFeedback({ search }: { search: ReturnType<typeof useObjectMatches> }) {
  if (search.pending) return <div role="status" className="muted small">正在查找…</div>;
  if (search.error) return <div role="alert">查找失败：{search.error} <button className="btn small" onClick={search.retry}>重试</button></div>;
  return null;
}
