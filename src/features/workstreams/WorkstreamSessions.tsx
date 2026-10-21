import Icon from "../../components/Icon";
import SessionMiniList from "../sessions/SessionMiniList";
import type { SessionListItem } from "../../types";
import type { Route } from "../../app/routes";

/** 显式跟踪该任务的会话；一个会话可以同时出现在多个任务中。 */
export default function WorkstreamSessions({ sessions, navigate, onNewSession, allowActions = true }: {
  sessions: SessionListItem[];
  navigate: (r: Route) => void;
  onNewSession?: () => void;
  allowActions?: boolean;
}) {
  const sorted = [...sessions].sort((a, b) =>
    (b.last_activity_at ?? b.started_at ?? "").localeCompare(
      a.last_activity_at ?? a.started_at ?? "",
    ),
  );

  return (
    <section className="card detail-section">
      <div className="row between" style={{ alignItems: "center" }}>
        <h2 className="detail-section-title">
          <Icon name="chat" />
          <span>关联会话（{sessions.length}）</span>
        </h2>
        {allowActions && onNewSession && (
          <button className="btn ghost icon-button small" title="新建会话" aria-label="新建会话"
            onClick={onNewSession}>
            <Icon name="plus" />
          </button>
        )}
      </div>

      <SessionMiniList sessions={sorted} emptyText="还没有会话跟踪这项任务。" navigate={navigate} />
    </section>
  );
}
