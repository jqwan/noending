import Icon from "../../components/Icon";
import { timeAgo } from "../../components/common";
import type { CollaborationRoomView, SessionListItem } from "../../types";
import { WAKE_LABEL, memberStatus } from "./collaborationState";
import type { Route } from "../../app/routes";

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
 * 「等待消息」只说明参与通道有有效心跳，不说明模型一定能响应；
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
  navigate,
}: {
  room: CollaborationRoomView;
  sessions: SessionListItem[];
  now: number;
  busySessionId: string | null;
  onWake: (sessionId: string) => void;
  onCopyPrompt: (sessionId: string) => void;
  disabled?: boolean;
  navigate: (route: Route) => void;
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
        const retry = status.tone === "failed";
        const canWake = retry || status.tone === "disconnected" || status.tone === "left"
          || (member.wake_status === "pending" && member.wake_method === "desktop");
        const readLabel = `${member.last_read_at ? `最近查收 ${timeAgo(member.last_read_at)}` : "尚未查收"}${member.last_read_seq != null ? ` · 到 #${member.last_read_seq}` : ""}`;
        const statusHint = `${status.label}${member.last_heartbeat_at ? `\n最近心跳 ${timeAgo(member.last_heartbeat_at)}` : ""}`;
        const copyHint = member.connection_state === "pending" && member.wake_method === "terminal" && member.wake_status === "opened"
          ? "复制加入提示，发送给现有终端中的会话" : "复制加入提示";
        return (
          <li key={member.session_id} className="collab-member" aria-label={name}>
            <div className="collab-member-head">
              <span className="collab-member-wake muted" role="img" aria-label={WAKE_LABEL[member.wake_method]} title={WAKE_LABEL[member.wake_method]}>
                <Icon name={member.wake_method} />
              </span>
              <button type="button" className="link collab-member-name truncate" title={name}
                onClick={() => navigate({ view: "session", sessionId: member.session_id, initialTitle: name, initialAgent: sessions.find(session => session.id === member.session_id)?.agent })}>{name}</button>
              {coordinator === member.session_id && <span className="collab-member-coordinator" role="img" aria-label="负责人" title="负责人"><Icon name="spark" /></span>}
              <div className="collab-member-actions">
                {canWake && (
                  <button type="button" className="btn ghost icon-button small"
                    aria-label={busy ? "打开中…" : retry ? "重试打开" : "打开会话"}
                    title={busy ? "打开中…" : retry ? "重试打开" : "打开会话"}
                    disabled={disabled || busySessionId !== null} onClick={() => onWake(member.session_id)}>
                    <Icon name={retry ? "refresh" : "play"} />
                  </button>
                )}
                <button type="button" className="btn ghost icon-button small" aria-label="复制加入提示" title={copyHint}
                  onClick={() => onCopyPrompt(member.session_id)}><Icon name="copy" /></button>
              </div>
            </div>
            <div className="collab-member-meta small muted">
              <span className={`badge small collab-tone-${status.tone}`} title={statusHint}>{status.label}</span>
              {processingTime && <span title={`已处理 ${processingTime}`} aria-label={`已处理 ${processingTime}`}>{processingTime}</span>}
              <span className="collab-member-read" role="img" aria-label={readLabel} title={readLabel}>
                <Icon name={member.last_read_seq != null ? "check" : "inbox"} />
                {member.last_read_seq != null && <span>#{member.last_read_seq}</span>}
              </span>
              {member.unread_count > 0 && <span className="collab-member-unread" role="img" aria-label={`未读 ${member.unread_count}`} title={`未读 ${member.unread_count}`}>
                <Icon name="inbox" /><span>{member.unread_count}</span>
              </span>}
            </div>
            {member.error && status.tone === "failed" && <p className="small warning" role="alert">{member.error}</p>}
          </li>
        );
      })}
    </ul>
  );
}
