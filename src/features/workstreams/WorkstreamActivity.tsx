import React from "react";
import { timeAgo } from "../../components/common";
import { KIND_LABELS, type ContextItem, type ContextItemRevision } from "../../types";

/**
 * 右栏 Activity（实施方案 §29）：第一版只做「Recent Changes」伪聚合 ——
 * 来自 Context 条目的最近修订，不为此新建 event timeline 后端。
 */
export default function WorkstreamActivity({ items }: {
  items: [ContextItem, ContextItemRevision][];
}) {
  const rows = [...items]
    .sort((a, b) => b[0].updated_at.localeCompare(a[0].updated_at))
    .slice(0, 6);

  if (rows.length === 0) return null;

  return (
    <div className="rail-section">
      <div className="rail-head">
        <div className="section-label" style={{ margin: 0 }}>Activity</div>
      </div>
      {rows.map(([item, rev]) => (
        <div className="activity-row" key={item.id}>
          <span className="when">{timeAgo(item.updated_at)}</span>
          <span style={{ minWidth: 0 }}>
            <span className="badge" style={{ marginRight: 6 }}>{KIND_LABELS[item.kind] ?? item.kind}</span>
            {rev.title}
          </span>
        </div>
      ))}
    </div>
  );
}
