import { useLayoutEffect, useRef, useState } from "react";
import Icon from "../../components/Icon";
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
 * Markdown 走预览、长文点开弹窗。消息区独立滚动，阅读旧消息时不抢滚动位置。
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
  const scrollRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const [hasNew, setHasNew] = useState(false);
  const latestId = messages[messages.length - 1]?.id;

  const toLatest = () => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
    following.current = true;
    setHasNew(false);
  };

  useLayoutEffect(() => {
    if (following.current) toLatest();
    else setHasNew(true);
  }, [latestId]);

  const submit = () => {
    if (disabled || sending || !draft.trim()) return;
    toLatest();
    onSend();
  };

  return (
    <div className="collab-messages">
      <div className="collab-history">
        <div className="collab-message-scroll" ref={scrollRef} role="region" aria-label="协作消息记录"
          onScroll={() => {
            const el = scrollRef.current;
            if (!el) return;
            following.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 64;
            if (following.current) setHasNew(false);
          }}>
          {messages.length === 0 ? (
            <div className="collab-empty muted"><Icon name="chat" /><p>还没有消息，发一句话开始协作。</p></div>
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
        </div>
        {hasNew && <button type="button" className="btn small collab-latest" onClick={toLatest}>有新消息 · 回到最新消息</button>}
      </div>
      <form className="collab-compose" onSubmit={e => { e.preventDefault(); submit(); }}>
        <textarea
          id="collab-input"
          aria-label={placeholder}
          rows={3}
          value={draft}
          disabled={disabled || sending}
          onChange={e => onDraftChange(e.target.value)}
          onKeyDown={e => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing && e.keyCode !== 229) {
              e.preventDefault();
              submit();
            }
          }}
          placeholder="发送消息，简要说明进展或需要协助的事…"
        />
        <div className="collab-compose-actions">
          <span className="small muted">Enter 发送，Shift + Enter 换行</span>
          <button type="submit" className="btn small primary" disabled={disabled || sending || !draft.trim()}>
            {sending ? "发送中…" : sendLabel}
          </button>
        </div>
      </form>
    </div>
  );
}
