import Icon from "../../components/Icon";
import SessionMiniList from "../sessions/SessionMiniList";
import type { Session } from "../../types";
import type { Route } from "../../app/routes";

/** 显式跟踪该任务的会话；一个会话可以同时出现在多个任务中。 */
export default function WorkstreamSessions({ sessions, navigate, onNewSession, allowActions = true }: {
  sessions: Session[];
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
    <section className="rail-section">
      <div className="rail-head">
        <div className="section-label" style={{ margin: 0 }}>会话</div>
        {allowActions && onNewSession && (
          <button className="btn ghost icon-button" title="新建会话" aria-label="新建会话"
            onClick={onNewSession}>
            <Icon name="plus" />
          </button>
        )}
      </div>

      <SessionMiniList sessions={sorted} emptyText="还没有会话跟踪这项任务。" navigate={navigate} />
    </section>
  );
}
