import { useState } from "react";
import type { Route, WorkstreamEntry } from "../../app/routes";
import type { WorkstreamCardData, WorkstreamReviewSummary } from "../../types";

interface Props {
  cards: WorkstreamCardData[];
  summaries: WorkstreamReviewSummary[];
  navigate: (route: Route) => void;
}

export default function ContextUpdatesSection({ cards, summaries, navigate }: Props) {
  const [expanded, setExpanded] = useState(false);

  const activeCards = cards.filter(
    (c) => c.lifecycle === "active" && c.visibility === "normal"
  );
  const summaryById = new Map(summaries.map((s) => [s.workstream_id, s]));

  const updates = activeCards
    .map((card) => ({
      card,
      summary: summaryById.get(card.id),
    }))
    .filter((x): x is { card: WorkstreamCardData; summary: WorkstreamReviewSummary } =>
      Boolean(x.summary && (x.summary.has_updates || x.summary.needs_attention))
    );

  if (updates.length === 0) return null;

  // Sorting order:
  // 1. needs_attention=true
  // 2. has_updates=true
  // 3. last_unseen_change_at DESC
  // 4. card.last_activity_at DESC
  updates.sort((a, b) => {
    if (a.summary.needs_attention !== b.summary.needs_attention) {
      return a.summary.needs_attention ? -1 : 1;
    }
    if (a.summary.has_updates !== b.summary.has_updates) {
      return a.summary.has_updates ? -1 : 1;
    }
    const aTime = a.summary.last_unseen_change_at ?? a.summary.reviewed_at ?? "";
    const bTime = b.summary.last_unseen_change_at ?? b.summary.reviewed_at ?? "";
    const timeCmp = bTime.localeCompare(aTime);
    if (timeCmp !== 0) return timeCmp;
    return (b.card.last_activity_at ?? "").localeCompare(a.card.last_activity_at ?? "");
  });

  const visible = expanded ? updates : updates.slice(0, 4);

  return (
    <div className="context-updates-section">
      <div className="section-label">Context Updates</div>
      <div className="context-updates-list">
        {visible.map(({ card, summary }) => {
          const entry: WorkstreamEntry =
            !summary.has_updates && summary.needs_attention ? "conflicts" : "review";

          const handleReview = () => {
            navigate({
              view: "workstream",
              workstreamId: card.id,
              entry,
            });
          };

          return (
            <div
              key={card.id}
              className="context-update-row"
              onClick={handleReview}
              title="前往 Workstream 审查上下文"
            >
              <div className="context-update-main">
                <span className="context-update-title">{card.title}</span>
                {card.project_name && (
                  <span className="muted small context-update-project">
                    {card.project_name}
                  </span>
                )}
              </div>

              <div className="context-update-signals">
                {summary.has_updates && !summary.needs_attention && (
                  <span className="badge">
                    {summary.unseen_change_count} change{summary.unseen_change_count === 1 ? "" : "s"}
                  </span>
                )}
                {!summary.has_updates && summary.needs_attention && (
                  <span className="badge warn">
                    ⚠ {summary.open_conflict_count} unresolved conflict{summary.open_conflict_count === 1 ? "" : "s"}
                  </span>
                )}
                {summary.has_updates && summary.needs_attention && (
                  <>
                    <span className="badge">
                      {summary.unseen_change_count} change{summary.unseen_change_count === 1 ? "" : "s"}
                    </span>
                    <span className="badge warn">
                      ⚠ {summary.open_conflict_count} unresolved conflict{summary.open_conflict_count === 1 ? "" : "s"}
                    </span>
                  </>
                )}
              </div>

              <div className="context-update-action">
                <button
                  className="btn small ghost"
                  onClick={(e) => {
                    e.stopPropagation();
                    handleReview();
                  }}
                >
                  Review →
                </button>
              </div>
            </div>
          );
        })}
      </div>

      {updates.length > 4 && (
        <button
          className="link small context-updates-more"
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? "收起" : `View ${updates.length - 4} more`}
        </button>
      )}
    </div>
  );
}
