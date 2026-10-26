import Icon from "../../components/Icon";
import { timeAgo } from "../../components/common";
import type { CollaborationRoomView, SessionListItem } from "../../types";
import { WAKE_LABEL, memberStatus } from "./collaborationState";

function memberName(sessionId: string, sessions: SessionListItem[]): string {
  const local = sessions.find(s => s.id === sessionId);
  return local?.title || sessionId.slice(0, 8);
}

/**
 * 本次运行的成员状态。
 *
 * 「正在监听」只说明参与通道有有效心跳，不说明模型一定能响应；
 * 处理中不套 2 分钟限制。后台进程残留不会被这段文案写成「工作正常」。
 */
export default function CollaborationMembers({
  room,
  sessions,
  now,
  busySessionId,
  onWake,
  onCopyPrompt,
}: {
  room: CollaborationRoomView;
  sessions: SessionListItem[];
  now: number;
  busySessionId: string | null;
  onWake: (sessionId: string) => void;
  onCopyPrompt: (sessionId: string) => void;
}) {
  const coordinator = room.room?.coordinator_session_id ?? null;
  if (!room.members.length) {
    return <p className="small muted" style={{ margin: 0 }}>本次运行还没有成员状态。</p>;
  }
  return (
    <ul className="collab-member-list" aria-label="成员状态">
      {room.members.map(member => {
        const status = memberStatus(member, now);
        const name = memberName(member.session_id, sessions);
        const busy = busySessionId === member.session_id;
        return (
          <li key={member.session_id} className="collab-member" aria-label={name}>
            <div className="collab-member-head">
              <Icon name="agent" />
              <span className="collab-member-name truncate" title={name}>{name}</span>
              {coordinator === member.session_id && <span className="badge small">负责人</span>}
              <span className={`badge small collab-tone-${status.tone}`}>{status.label}</span>
              {member.unread_count > 0 && <span className="badge muted small">未读 {member.unread_count}</span>}
            </div>
            <div className="collab-member-meta small muted">
              <span>{WAKE_LABEL[member.wake_method]}</span>
              <span aria-label="最近查收">
                {member.last_read_at ? `最近查收 ${timeAgo(member.last_read_at)}` : "尚未查收"}
                {member.last_read_seq != null ? ` · 到 #${member.last_read_seq}` : ""}
              </span>
              {member.last_heartbeat_at && <span>心跳 {timeAgo(member.last_heartbeat_at)}</span>}
            </div>
            {member.error && <p className="small warning" role="alert">{member.error}</p>}
            <div className="collab-member-actions">
              {member.wake_status === "failed" && (
                <button type="button" className="btn small" disabled={busy} onClick={() => onWake(member.session_id)}>
                  {busy ? "打开中…" : "重试打开"}
                </button>
              )}
              {member.wake_status === "pending" && member.wake_method === "desktop" && (
                <button type="button" className="btn small" disabled={busy} onClick={() => onWake(member.session_id)}>
                  {busy ? "打开中…" : "打开应用"}
                </button>
              )}
              {member.wake_method === "desktop" && (
                <button type="button" className="btn small ghost" onClick={() => onCopyPrompt(member.session_id)}>
                  <Icon name="copy" /> 复制加入提示
                </button>
              )}
            </div>
          </li>
        );
      })}
    </ul>
  );
}
