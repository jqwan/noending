import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import type { MailboxPage } from "../../types";
import Icon from "../../components/Icon";
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
export function MailboxHistory({ sessionId }: { sessionId: string }) {
  const { page, error, more } = useMailboxMessages(sessionId, "all");
  return <section aria-label="会话信箱" className="card detail-section mailbox-history">
    <h2 className="detail-section-title"><Icon name="inbox" /><span>信箱</span></h2>
    {error && <div role="alert" className="badge warn mailbox-history-error">{error}</div>}
    {!page && !error && <div className="skeleton mailbox-history-loading" />}
    <div className="mailbox-history-list">
      {page?.items.map(m => <article className="event mailbox-history-entry" key={m.id}>
        <div className="head mailbox-history-meta">
          <div className="mailbox-history-sender">
            <span className="who">{m.sender_session_id ? "会话" : "用户"}</span>
            {m.sender_session_id && <span className="from mono" title={m.sender_session_id}>{m.sender_session_id}</span>}
          </div>
          <div className="mailbox-history-status">
            <time className="when" dateTime={m.created_at}>{formatDateTime(m.created_at)}</time>
            <span className={`badge${m.read_at ? "" : " accent"}`}>{m.read_at ? "已读" : "未读"}</span>
          </div>
        </div>
        <div className="body">{m.body}</div>
      </article>)}
    </div>
    {page?.items.length === 0 && <div className="l1-none">暂无信箱消息</div>}
    {page?.next_cursor != null && <div className="mailbox-history-footer"><button className="btn small ghost" onClick={() => void more()}>加载更多</button></div>}
  </section>;
}
