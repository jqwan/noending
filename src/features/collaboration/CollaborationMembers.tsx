import Icon from "../../components/Icon";
import { timeAgo } from "../../components/common";
import type { CollaborationRoomView, SessionListItem } from "../../types";
import { WAKE_LABEL, memberStatus } from "./collaborationState";

function memberName(sessionId: string, sessions: SessionListItem[]): string {
  const local = sessions.find(s => s.id === sessionId);
  return local?.title || sessionId.slice(0, 8);
}

function elapsed(since: string | null, now: number): string | null {
  const start = since ? Date.parse(since) : Number.NaN;
  if (!Number.isFinite(start) || start > now) return null;
  const seconds = Math.floor((now - start) / 1000);
  if (seconds < 60) return `${seconds}秒`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}分钟`;
  return `${Math.floor(minutes / 60)}小时${minutes % 60}分钟`;
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
  disabled = false,
}: {
  room: CollaborationRoomView;
  sessions: SessionListItem[];
  now: number;
  busySessionId: string | null;
  onWake: (sessionId: string) => void;
  onCopyPrompt: (sessionId: string) => void;
  disabled?: boolean;
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
        const processingTime = status.tone === "processing" ? elapsed(member.phase_since, now) : null;
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
              {processingTime && <span aria-label="处理时长">已处理 {processingTime}</span>}
              <span aria-label="最近查收">
                {member.last_read_at ? `最近查收 ${timeAgo(member.last_read_at)}` : "尚未查收"}
                {member.last_read_seq != null ? ` · 到 #${member.last_read_seq}` : ""}
              </span>
              {member.last_heartbeat_at && <span>心跳 {timeAgo(member.last_heartbeat_at)}</span>}
            </div>
            {member.error && status.tone === "failed" && <p className="small warning" role="alert">{member.error}</p>}
            <div className="collab-member-actions">
              {status.tone === "failed" && (
                <button type="button" className="btn small" disabled={disabled || busySessionId !== null} onClick={() => onWake(member.session_id)}>
                  {busy ? "打开中…" : "重试打开"}
                </button>
              )}
              {(status.tone === "disconnected" || status.tone === "left" || (member.wake_status === "pending" && member.wake_method === "desktop")) && (
                <button type="button" className="btn small" disabled={disabled || busySessionId !== null} onClick={() => onWake(member.session_id)}>
                  {busy ? "打开中…" : "打开会话"}
                </button>
              )}
              <button type="button" className="btn small ghost" onClick={() => onCopyPrompt(member.session_id)}>
                <Icon name="copy" /> 复制加入提示
              </button>
            </div>
            {member.connection_state === "pending" && member.wake_method === "terminal" && member.wake_status === "opened" && (
              <p className="small muted">复用现有终端时，请复制加入提示发送给会话。</p>
            )}
          </li>
        );
      })}
    </ul>
  );
}
