import { useState } from "react";
import { timeAgo } from "../../components/common";
import type { WorkstreamReviewSummary, WorkstreamReviewWindow } from "../../types";
import {
  contextChangeActorLabel,
  contextChangeBadgeClass,
  contextChangeKindLabel,
} from "./contextChangeHelpers";

interface Props {
  window: WorkstreamReviewWindow;
  summary: WorkstreamReviewSummary;
  dirty: boolean;
  marking: boolean;
  onMarkReviewed: () => void;
  onRefresh: () => void;
  onOpenItem: (itemId: string) => void;
  onOpenConflict: (conflictId: string) => void;
}

export default function SinceLastReview({
  window,
  summary,
  dirty,
  marking,
  onMarkReviewed,
  onRefresh,
  onOpenItem,
  onOpenConflict,
}: Props) {
  const [expanded, setExpanded] = useState(false);

  // If there are no unseen changes, do not display the panel at all.
  if (window.unseen_changes.length === 0 && summary.unseen_change_count === 0) {
    return null;
  }

  const changes = window.unseen_changes;
  const visibleChanges = expanded ? changes : changes.slice(0, 5);

  const relativeTime = summary.last_unseen_change_at
    ? timeAgo(summary.last_unseen_change_at)
    : timeAgo(summary.reviewed_at);

  return (
    <div className="since-last-review-card">
      <div className="since-last-review-head">
        <div className="since-last-review-title">Since your last review</div>
        <div className="since-last-review-time">{relativeTime}</div>
      </div>

      <div className="since-last-review-stats">
        <span style={{ fontWeight: 600 }}>
          {summary.unseen_change_count} change{summary.unseen_change_count === 1 ? "" : "s"}
        </span>
        {summary.new_facts > 0 && <span>+ {summary.new_facts} new</span>}
        {summary.updated_facts > 0 && <span>↻ {summary.updated_facts} updated</span>}
        {summary.resolved_items > 0 && <span>✓ {summary.resolved_items} resolved</span>}
        {summary.superseded_items > 0 && <span>⇄ {summary.superseded_items} superseded</span>}
        {summary.open_conflict_count > 0 && (
          <span style={{ color: "var(--warning)", fontWeight: 550, marginLeft: 4 }}>
            ⚠ {summary.open_conflict_count} conflict{summary.open_conflict_count === 1 ? "" : "s"} detected
          </span>
        )}
      </div>

      {dirty && (
        <div className="since-last-review-dirty-banner">
          <span>New changes are available</span>
          <button className="btn small" onClick={onRefresh}>
            Refresh
          </button>
        </div>
      )}

      <div className="since-last-review-list">
        {visibleChanges.map((change) => {
          const isConflict = change.kind === "conflict_created";
          const handleClick = () => {
            if (isConflict && change.conflict_id) {
              onOpenConflict(change.conflict_id);
            } else if (change.item_id) {
              onOpenItem(change.item_id);
            }
          };

          return (
            <div
              className="since-last-review-row"
              key={change.id}
              onClick={handleClick}
              title={isConflict ? "点击审查此冲突" : "点击在 Context 中定位此条目"}
            >
              <span
                className={`badge ${contextChangeBadgeClass(change.kind)}`}
                style={{ fontSize: 10.5, padding: "1px 6px", flexShrink: 0 }}
              >
                {contextChangeKindLabel(change.kind)}
              </span>
              <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                {change.title}
              </span>
              <span className="muted small" style={{ fontSize: 11, flexShrink: 0 }}>
                {timeAgo(change.created_at)} · {contextChangeActorLabel(change.actor)}
              </span>
              {isConflict ? (
                <button
                  className="btn small ghost"
                  style={{ padding: "1px 6px", fontSize: 11, color: "var(--warning)" }}
                  onClick={(e) => {
                    e.stopPropagation();
                    if (change.conflict_id) onOpenConflict(change.conflict_id);
                  }}
                >
                  Review
                </button>
              ) : change.item_id ? (
                <button
                  className="btn small ghost"
                  style={{ padding: "1px 6px", fontSize: 11 }}
                  onClick={(e) => {
                    e.stopPropagation();
                    onOpenItem(change.item_id!);
                  }}
                >
                  View
                </button>
              ) : null}
            </div>
          );
        })}
      </div>

      <div className="since-last-review-footer">
        <div>
          {changes.length > 5 && (
            <button className="link small" onClick={() => setExpanded((v) => !v)}>
              {expanded ? "Show less" : `View all ${changes.length} changes`}
            </button>
          )}
        </div>
        <button
          className="btn small primary"
          disabled={marking}
          onClick={onMarkReviewed}
          title="将当前已观察的变化标记为已审阅"
        >
          {marking ? "Marking..." : "Mark reviewed"}
        </button>
      </div>
    </div>
  );
}
