import React from "react";
import { timeAgo } from "../../components/common";
import type { ContextChange } from "../../types";
import {
  contextChangeActorLabel,
  contextChangeBadgeClass,
  contextChangeKindLabel,
} from "./contextChangeHelpers";

interface Props {
  changes: ContextChange[];
}

export default function RecentChangesTimeline({ changes }: Props) {
  const rows = changes ?? [];

  return (
    <div className="rail-section" style={{ marginTop: 24 }}>
      <div className="rail-head">
        <div className="section-label" style={{ margin: 0 }}>Recent Changes</div>
      </div>
      {rows.length === 0 ? (
        <div className="muted small" style={{ padding: "8px 0" }}>暂无变更记录</div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column" }}>
          {rows.slice(0, 10).map((change) => (
            <div className="activity-row" key={change.id}>
              <span className="when">{timeAgo(change.created_at)}</span>
              <div style={{ flex: 1, minWidth: 0 }}>
                <span
                  className={`badge ${contextChangeBadgeClass(change.kind)}`}
                  style={{ marginRight: 6, fontSize: 10, padding: "0 5px" }}
                >
                  {contextChangeKindLabel(change.kind)}
                </span>
                <span style={{ fontSize: 12.5, color: "var(--text-primary)" }}>
                  {change.title}
                </span>
                <span className="muted small" style={{ marginLeft: 6, fontSize: 11 }}>
                  · {contextChangeActorLabel(change.actor)}
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

