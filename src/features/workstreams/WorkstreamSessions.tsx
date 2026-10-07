import Icon from "../../components/Icon";
import SessionMiniList from "../sessions/SessionMiniList";
import type { Session } from "../../types";
import type { Route } from "../../app/routes";

/**
 * Workstream 的 Sessions 段落，这一页的主角。列表口径是归属：
 * `owner_workstream_id == 当前 Workstream` 的 Sessions，所以同一个 Session
 * 不会同时出现在两个任务的列表里。展示与交互和项目详情的会话列表完全一致
 * （SessionMiniList：整行点击进详情，行内无动作）。
 */
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

      <SessionMiniList sessions={sorted} emptyText="还没有会话归属到这项任务。" navigate={navigate} />
    </section>
  );
}
