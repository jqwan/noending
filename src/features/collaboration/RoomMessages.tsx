import type { CollaborationMessage, SessionListItem } from "../../types";
import SessionMessage from "../sessions/SessionMessage";
import { formatDateTime } from "../sessions/SessionTable";
import "../sessions/sessions.css";

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
 * 协作消息：与会话详情页的「最近消息」同形——自己靠右、成员靠左，
 * Markdown 走预览、长文点开弹窗。整段历史一次展示，不另设分页入口。
 *
 * 开始／停止通知的正文是坐标 JSON，不渲染成气泡，只留一行居中提示。
 */
export default function RoomMessages({
  messages,
  sessions,
  sending,
  draft,
  onDraftChange,
  onSend,
  sendLabel,
  placeholder,
  disabled = false,
}: {
  messages: CollaborationMessage[];
  sessions: SessionListItem[];
  sending: boolean;
  draft: string;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  sendLabel: string;
  placeholder: string;
  disabled?: boolean;
}) {
  return (
    <div className="collab-messages">
      {messages.length === 0 ? (
        <p className="small muted" style={{ margin: 0 }}>还没有消息。</p>
      ) : (
        <ol className="collab-message-list" aria-label="协作消息">
          {messages.map(message => (
            <li key={message.id} className="collab-message">
              {message.kind === "text" ? (
                <SessionMessage
                  ordinal={message.seq}
                  msg={{
                    sequence: message.seq,
                    role: message.sender_session_id ? "assistant" : "user",
                    content: message.body,
                    ts: message.created_at,
                    who: senderName(message.sender_session_id, sessions),
                    turnFinal: true,
                  }}
                />
              ) : (
                <p className="small muted collab-notice" title={message.body}>
                  {KIND_LABEL[message.kind]} · {senderName(message.sender_session_id, sessions)}
                  {" · "}{formatDateTime(message.created_at)}
                </p>
              )}
            </li>
          ))}
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
