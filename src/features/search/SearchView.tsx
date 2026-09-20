import { useEffect, useState } from "react";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import type { SearchHit } from "../../types";

/** 结果类型徽标（search/mod.rs:12 的 kind 定义域）。领域词按 §2 词表保留。 */
const HIT_KIND_LABELS: Record<string, string> = {
  item: "Context",
  workstream: "Workstream",
  project: "Project",
  event: "消息",
};

export default function SearchView({ query, navigate }: { query: string; navigate: (r: Route) => void }) {
  const [q, setQ] = useState(query);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [failed, setFailed] = useState(false);

  useEffect(() => { setQ(query); }, [query]);
  useEffect(() => {
    if (!q.trim()) { setHits([]); return; }
    const t = setTimeout(() => {
      api.search(q).then((h) => { setHits(h); setFailed(false); })
        .catch((e) => { console.error(e); setFailed(true); });
    }, 200);
    return () => clearTimeout(t);
  }, [q]);

  return (
    <div className="main narrow">
      <h1>搜索</h1>
      <p className="page-sub">优先展示当前 Context，其次历史与原始 Session。</p>
      <input type="text" style={{ marginBottom: 18 }} autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder="搜索 Workstream、Session、Context 消息…" />

      {failed && (
        <div className="empty">搜索失败。本地数据没有被修改，换个关键词或稍后重试。</div>
      )}

      {hits.map((h) => (
        <div className="card clickable" key={h.kind + h.ref_id}
          onClick={() => {
            if (h.kind === "workstream") navigate({ view: "workstream", workstreamId: h.ref_id });
            else if (h.kind === "item") navigate({ view: "workstream", workstreamId: h.parent_id });
            else if (h.kind === "event") navigate({ view: "session", sessionId: h.parent_id });
            else if (h.kind === "project") navigate({ view: "project", projectId: h.ref_id });
          }}>
          <div className="row">
            <span className="badge">{HIT_KIND_LABELS[h.kind] ?? h.kind}</span>
            <strong className="small">{h.title.slice(0, 80)}</strong>
          </div>
          {h.snippet && <div className="muted small" style={{ marginTop: 4 }}>{h.snippet}</div>}
        </div>
      ))}
      {q && !failed && hits.length === 0 && <div className="empty">没有匹配结果。</div>}
    </div>
  );
}
