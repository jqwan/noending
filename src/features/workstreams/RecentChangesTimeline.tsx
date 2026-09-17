import React from "react";
import { timeAgo } from "../../components/common";
import type { ContextChange } from "../../types";

interface Props {
  changes: ContextChange[];
}

function labelForKind(kind: ContextChange["kind"]): string {
  switch (kind) {
    case "added":
      return "+ 新增";
    case "edited":
      return "✎ 编辑";
    case "resolved":
      return "✓ 完成";
    case "superseded":
      return "⇄ 替代";
    case "deleted":
      return "✕ 删除";
    case "conflict_created":
      return "⚠ 冲突";
    case "conflict_resolved":
      return "✓ 裁决";
    default:
      return kind;
  }
}

function badgeClassForKind(kind: ContextChange["kind"]): string {
  switch (kind) {
    case "conflict_created":
      return "warn";
    case "conflict_resolved":
    case "resolved":
      return "accent";
    default:
      return "";
  }
}

function actorLabel(actor: string): string {
  if (actor === "user") return "用户";
  if (actor === "system") return "系统";
  if (actor === "sync" || actor.startsWith("sync:")) return "Sync";
  return actor;
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
                  className={`badge ${badgeClassForKind(change.kind)}`}
                  style={{ marginRight: 6, fontSize: 10, padding: "0 5px" }}
                >
                  {labelForKind(change.kind)}
                </span>
                <span style={{ fontSize: 12.5, color: "var(--text-primary)" }}>
                  {change.title}
                </span>
                <span className="muted small" style={{ marginLeft: 6, fontSize: 11 }}>
                  · {actorLabel(change.actor)}
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
