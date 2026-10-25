import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import type { MailboxMessage, MailboxPage } from "../../types";
import Icon from "../../components/Icon";
import { Modal } from "../../components/common";
import { formatDateTime } from "./SessionTable";

export function useMailboxMessages(session: string | null, filter: "all" | "unread") {
  const [page, setPage] = useState<MailboxPage | null>(null);
  const [error, setError] = useState("");
  const request = useRef(0);
  const refresh = useCallback(async () => {
    const token = ++request.current;
    if (!session) { setPage(null); return; }
    try { const next = await api.mailboxList(session, filter); if (request.current === token) { setPage(next); setError(""); } }
    catch (e) { if (request.current === token) setError(String(e)); }
  }, [session, filter]);
  const more = async () => {
    if (!session || page?.next_cursor == null) return;
    const token = ++request.current;
    try {
      const next = await api.mailboxList(session, filter, page.next_cursor);
      if (request.current === token) setPage(old => ({ ...next, items: [...(old?.items ?? []), ...next.items] }));
    } catch (e) { if (request.current === token) setError(String(e)); }
  };
  useEffect(() => {
    let active = true;
    const stops: (() => void)[] = [];
    setPage(null); void refresh();
    void Promise.all(["mailbox-changed", "cli-data-changed"].map(name => listen(name, () => { if (active) void refresh(); })
      .then(stop => { if (active) stops.push(stop); else stop(); }))).then(() => { if (active) void refresh(); })
      .catch(e => { if (active) setError(String(e)); });
    return () => { active = false; ++request.current; stops.forEach(stop => stop()); };
  }, [refresh]);
  return { page, error, refresh, more };
}
function MailboxMetadata({ message }: { message: MailboxMessage }) {
  return <span className="mailbox-history-meta">
    <span className="mailbox-history-sender">
      <span>{message.sender_session_id ? "会话" : "用户"}</span>
      {message.sender_session_id && <span className="mono mailbox-sender-id" title={message.sender_session_id}>{message.sender_session_id}</span>}
    </span>
    <span className="mailbox-history-status">
      <time dateTime={message.created_at}>{formatDateTime(message.created_at)}</time>
      <span className={`badge${message.read_at ? "" : " accent"}`}>{message.read_at ? "已读" : "未读"}</span>
    </span>
  </span>;
}

export function MailboxMessagePreview({ message }: { message: MailboxMessage }) {
  const [open, setOpen] = useState(false);
  const characters = Array.from(message.body.replace(/\s+/g, " ").trim());
  const preview = characters.slice(0, 120).join("") + (characters.length > 120 ? "…" : "");
  return <>
    <button type="button" className="mailbox-message-preview" aria-label={`查看消息：${preview}`} aria-haspopup="dialog" onClick={() => setOpen(true)}>
      <MailboxMetadata message={message} />
      <span className="mailbox-preview-line"><span className="mailbox-preview-text">{preview}</span><Icon name="chevronRight" /></span>
    </button>
    {open && <Modal title="信箱消息" onClose={() => setOpen(false)}>
      <div className="mailbox-message-detail">
        <MailboxMetadata message={message} />
        <div className="small muted mailbox-message-id">消息 ID：<span className="mono">{message.id}</span></div>
        <div className="mailbox-body">{message.body}</div>
      </div>
    </Modal>}
  </>;
}

export function MailboxHistory({ sessionId }: { sessionId: string }) {
  const { page, error, more } = useMailboxMessages(sessionId, "all");
  return <section aria-label="会话信箱" className="card detail-section mailbox-history">
    <h2 className="detail-section-title"><Icon name="inbox" /><span>信箱{page ? `（${page.total}）` : ""}</span></h2>
    {error && <div role="alert" className="badge warn mailbox-history-error">{error}</div>}
    {!page && !error && <div className="skeleton mailbox-history-loading" />}
    <div className="mailbox-history-list" role="region" aria-label="信箱消息列表" tabIndex={0}>
      {page?.items.map(m => <article className="mailbox-history-entry" key={`${sessionId}:${m.id}`}>
        <MailboxMessagePreview message={m} />
      </article>)}
      {page?.items.length === 0 && <div className="l1-none">暂无信箱消息</div>}
      {page?.next_cursor != null && <div className="mailbox-history-footer"><button className="btn small ghost" onClick={() => void more()}>加载更多</button></div>}
    </div>
  </section>;
}
