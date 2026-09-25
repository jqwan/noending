import PageHeader from "../../layout/PageHeader";
import { useEffect, useState } from "react";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import type { SearchHit } from "../../types";

/** 结果类型徽标（search/mod.rs:12 的 kind 定义域）。领域词按 §2 词表保留。 */
const HIT_KIND_LABELS: Record<string, string> = {
  item: "Context",
  workstream: "任务",
  project: "项目",
  session: "会话",
  event: "消息",
};

export default function SearchView({ query, navigate }: { query: string; navigate: (r: Route) => void }) {
  const [q, setQ] = useState(query);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [loading, setLoading] = useState(false);
  const [retry, setRetry] = useState(0);
  const [failed, setFailed] = useState(false);

  useEffect(() => { setQ(query); }, [query]);
  useEffect(() => {
    let cancelled = false;
    setFailed(false);
    setHits([]);
    setLoading(!!q.trim());
    if (!q.trim()) return;
    const t = setTimeout(() => {
      api.search(q).then(h => { if (!cancelled) setHits(h); })
        .catch(() => { if (!cancelled) setFailed(true); })
        .finally(() => { if (!cancelled) setLoading(false); });
    }, 200);
    return () => { cancelled = true; clearTimeout(t); };
  }, [q, retry]);

  return (
    <div className="main narrow">
      <PageHeader title="搜索" />
      <input aria-label="搜索" type="text" style={{ marginBottom: 18 }} autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder="搜索任务、会话、Context 消息…" />

      {failed && (
        <div className="empty" role="alert">搜索失败 <button className="btn small" onClick={() => setRetry(value => value + 1)}>重试</button></div>
      )}

      {loading && <div role="status" className="muted">搜索中…</div>}
      {hits.map((h) => (
        <button type="button" className="search-result" key={h.kind + h.ref_id}
          onClick={() => {
            if (h.kind === "workstream") navigate({ view: "workstream", workstreamId: h.ref_id });
            else if (h.kind === "session") navigate({ view: "session", sessionId: h.ref_id });
            else if (h.kind === "item") navigate({ view: "workstream", workstreamId: h.parent_id });
            else if (h.kind === "event") navigate({ view: "session", sessionId: h.parent_id });
            else if (h.kind === "project") navigate({ view: "project", projectId: h.ref_id });
          }}>
          <div className="row">
            <span className="badge">{HIT_KIND_LABELS[h.kind] ?? h.kind}</span>
            <strong className="small">{h.title.slice(0, 80)}</strong>
          </div>
          {h.snippet && <div className="muted small" style={{ marginTop: 4 }}>{h.snippet}</div>}
        </button>
      ))}
      {q.trim() && !loading && !failed && hits.length === 0 && <div className="empty">没有匹配结果。</div>}
    </div>
  );
}
