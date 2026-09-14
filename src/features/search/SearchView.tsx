import React, { useEffect, useState } from "react";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import type { SearchHit } from "../../types";

export default function SearchView({ query, navigate }: { query: string; navigate: (r: Route) => void }) {
  const [q, setQ] = useState(query);
  const [hits, setHits] = useState<SearchHit[]>([]);

  useEffect(() => { setQ(query); }, [query]);
  useEffect(() => {
    if (!q.trim()) { setHits([]); return; }
    const t = setTimeout(() => {
      api.search(q).then(setHits).catch(console.error);
    }, 200);
    return () => clearTimeout(t);
  }, [q]);

  return (
    <div className="main narrow">
      <h1>Search</h1>
      <p className="page-sub">优先展示当前 Context，其次历史与原始 Session。</p>
      <input type="text" style={{ marginBottom: 18 }} autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder="搜索 Workstream / Context / Session 消息…" />

      {hits.map((h) => (
        <div className="card clickable" key={h.kind + h.ref_id}
          onClick={() => {
            if (h.kind === "workstream") navigate({ view: "workstream", workstreamId: h.ref_id });
            else if (h.kind === "item") navigate({ view: "workstream", workstreamId: h.parent_id });
            else if (h.kind === "event") navigate({ view: "session", sessionId: h.parent_id });
            else if (h.kind === "project") navigate({ view: "project", projectId: h.ref_id });
          }}>
          <div className="row">
            <span className="badge">{h.kind}</span>
            <strong className="small">{h.title.slice(0, 80)}</strong>
          </div>
          {h.snippet && <div className="muted small" style={{ marginTop: 4 }}>{h.snippet}</div>}
        </div>
      ))}
      {q && hits.length === 0 && <div className="empty">没有匹配结果。</div>}
    </div>
  );
}
