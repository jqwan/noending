import type { CollaborationMessage, SessionListItem } from "../../types";
import { formatDateTime } from "../sessions/SessionTable";

const KIND_LABEL: Record<CollaborationMessage["kind"], string | null> = {
  text: null,
  start: "开始协作",
  stop: "停止协作",
};

function senderName(sender: string | null, sessions: SessionListItem[]): string {
  if (!sender) return "你";
  const local = sessions.find(s => s.id === sender);
  // 会话被永久删除后仍保留真实来源，不转换成「用户」。
  return local?.title || `会话 ${sender.slice(0, 8)}`;
}

/**
 * 协作消息。房间内单份正文，按 seq 升序；只有追加，更正用新消息。
 * 输入框在未开启时承载首条消息——开启与写消息由后端在同一事务里完成。
 */
export default function RoomMessages({
  messages,
  earlierCursor,
  sessions,
  sending,
  draft,
  onDraftChange,
  onSend,
  onLoadEarlier,
  sendLabel,
  placeholder,
  disabled = false,
}: {
  messages: CollaborationMessage[];
  earlierCursor: string | null;
  sessions: SessionListItem[];
  sending: boolean;
  draft: string;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onLoadEarlier: () => void;
  sendLabel: string;
  placeholder: string;
  disabled?: boolean;
}) {
  return (
    <div className="collab-messages">
      {earlierCursor && (
        <button type="button" className="btn small ghost" onClick={onLoadEarlier}>加载更早的消息</button>
      )}
      {messages.length === 0 ? (
        <p className="small muted" style={{ margin: 0 }}>还没有消息。</p>
      ) : (
        <ol className="collab-message-list" aria-label="协作消息">
          {messages.map(message => {
            const kind = KIND_LABEL[message.kind];
            return (
              <li key={message.id} className={`collab-message${message.sender_session_id ? "" : " mine"}`}
                title={message.kind === "text" ? undefined : message.body}>
                <div className="collab-message-head small muted">
                  <span className="collab-message-sender">{senderName(message.sender_session_id, sessions)}</span>
                  {kind && <span className="badge muted small">{kind}</span>}
                  <span title={message.created_at}>{formatDateTime(message.created_at)}</span>
                  <span className="mono">#{message.seq}</span>
                </div>
                {/* 开始/停止通知的正文是定位信息（JSON），不是给人读的话；只留徽章，原文进 title。 */}
                {message.kind === "text" && <div className="collab-message-body">{message.body}</div>}
              </li>
            );
          })}
        </ol>
      )}
      <form className="collab-compose" onSubmit={e => { e.preventDefault(); onSend(); }}>
        <label className="small" htmlFor="collab-input">{placeholder}</label>
        <textarea
          id="collab-input"
          rows={3}
          value={draft}
          disabled={disabled || sending}
          onChange={e => onDraftChange(e.target.value)}
          placeholder="简短说明进展、阻碍或需要对方处理的点；正文只在本房间保存一份"
        />
        <button type="submit" className="btn small primary" disabled={disabled || sending || !draft.trim()}>
          {sending ? "发送中…" : sendLabel}
        </button>
      </form>
    </div>
  );
}
