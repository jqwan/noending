import { useLayoutEffect, useRef, useState } from "react";
import Icon from "../../components/Icon";
import type { CollaborationMessage } from "../../types";
import SessionMessage from "../sessions/SessionMessage";
import { formatDateTime } from "../sessions/SessionTable";
import "../sessions/sessions.css";
import { useLoadingDelay } from "../../hooks/useLoadingDelay";

const KIND_LABEL: Record<CollaborationMessage["kind"], string | null> = {
  text: null,
  start: "开始协作",
  stop: "停止协作",
};

function senderName(message: CollaborationMessage): string {
  if (!message.sender_session_id) return "你";
  return message.sender_title || `会话 ${message.sender_session_id.slice(0, 8)}`;
}

/**
 * 协作消息：与会话历史消息同形——自己靠右、成员靠左，
 * Markdown 走预览、长文点开弹窗。消息区独立滚动，阅读旧消息时不抢滚动位置。
 *
 * 开始／停止通知的正文是坐标 JSON，不渲染成气泡，只留一行居中提示。
 */
export default function RoomMessages({
  messages,
  sending,
  draft,
  onDraftChange,
  onSend,
  onSetup,
  sendLabel,
  placeholder,
  disabled = false,
  loading = false,
}: {
  messages: CollaborationMessage[];
  sending: boolean;
  draft: string;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onSetup?: () => void;
  sendLabel: string;
  placeholder: string;
  disabled?: boolean;
  loading?: boolean;
}) {
  const showLoading = useLoadingDelay(loading);
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
        <div className="conversation-scroll" ref={scrollRef} role="region" aria-label="协作消息记录" aria-busy={loading}
          onScroll={() => {
            const el = scrollRef.current;
            if (!el) return;
            following.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 64;
            if (following.current) setHasNew(false);
          }}>
          {loading ? (showLoading && <p role="status" className="small muted">正在读取协作室…</p>) : messages.length === 0 ? (
            <div className="collab-empty muted">
              <Icon name="chat" />
              <p>还没有协作消息。</p>
              {onSetup && <button type="button" className="btn primary" disabled={sending} onClick={onSetup}>开始协作</button>}
            </div>
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
                        who: senderName(message),
                        turnFinal: true,
                      }}
                    />
                  ) : (
                    <p className="small muted collab-notice" title={message.body}>
                      {KIND_LABEL[message.kind]} · {senderName(message)}
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
      <form className="collab-compose" aria-label="发送协作消息" onSubmit={e => { e.preventDefault(); submit(); }}>
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
          <div className="collab-compose-hint small muted">
            {!loading && <span>{messages.length} 条消息</span>}
            <span>Enter {onSetup ? "选择成员" : "发送"}，Shift + Enter 换行</span>
          </div>
          <button type="submit" className="btn small primary" disabled={disabled || sending || !draft.trim()}>
            {sending ? "发送中…" : sendLabel}
          </button>
        </div>
      </form>
    </div>
  );
}
