import { useEffect, useRef, useState, type ClipboardEvent, type RefObject } from "react";
import { api } from "../../api";
import type { MailboxMessage, TerminalNotifications } from "../../types";
import { Modal } from "../../components/common";
import Icon from "../../components/Icon";
import { useViewState } from "../../hooks/useViewState";
import { useMailboxMessages } from "./MailboxMessages";

export default function TerminalMailbox({ terminalId, notification, error: loadError, apply, refresh, inputRef }: {
  terminalId: string; notification: TerminalNotifications | null; error: string;
  apply: (n: TerminalNotifications) => void; refresh: () => Promise<void>;
  inputRef?: RefObject<HTMLTextAreaElement>;
}) {
  const session = notification?.session_id ?? null;
  const { page, error: listError, refresh: refreshMessages, more } = useMailboxMessages(session, "unread");
  const [draft, setDraft] = useViewState(`mailbox.${session ?? terminalId}.draft`, "");
  const [editing, setEditing] = useState<{ message: MailboxMessage; text: string } | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [skip, setSkip] = useState(false);
  const [skipHint, setSkipHint] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef(false);
  useEffect(() => { let active = true; void api.mailboxNotificationHint().then(v => { if (active) setSkipHint(v); }).catch(e => { if (active) setError(String(e)); }); return () => { active = false; }; }, []);
  useEffect(() => {
    if (notification?.enabled) { setEditing(null); setDeleting(null); }
    if (page && editing && !page.items.some(m => m.id === editing.message.id && m.revision === editing.message.revision)) setEditing(null);
    if (page && deleting && !page.items.some(m => m.id === deleting)) setDeleting(null);
  }, [notification?.enabled, page, editing, deleting]);
  const run = async (fn: () => Promise<unknown>) => {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError("");
    try { await fn(); }
    catch (e) { setError(String(e)); }
    finally { await Promise.all([refresh(), refreshMessages()]); pending.current = false; setBusy(false); }
  };
  const toggle = async (enabled: boolean) => {
    if (!notification) return;
    if (enabled && skip) { await api.mailboxNotificationHint(true); setSkipHint(true); }
    apply(await api.terminalAutoNotifications(terminalId, notification.identity_revision, enabled)); setConfirm(false);
  };
  const edit = (m: MailboxMessage, body: string | null) => run(async () => {
    if (!notification) return;
    await api.mailboxEdit(terminalId, notification.identity_revision, m.id, m.revision, body);
    setEditing(null); setDeleting(null);
  });
  const pasteImage = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = Array.from(event.clipboardData.files).filter(file => file.type.startsWith("image/"));
    if (!files.length) return;
    event.preventDefault();
    if (pending.current || !session) return;
    const input = event.currentTarget, start = input.selectionStart, end = input.selectionEnd;
    void run(async () => {
      const paths: string[] = [];
      for (const file of files) {
        if (!["image/png", "image/jpeg", "image/webp"].includes(file.type) || file.size > 20 * 1024 * 1024) throw new Error("仅支持不超过 20 MiB 的 PNG、JPEG 和 WebP 图片");
        const data = await new Promise<string>((resolve, reject) => {
          const reader = new FileReader(); reader.onload = () => resolve(String(reader.result).split(",")[1]); reader.onerror = () => reject(new Error("读取图片失败")); reader.readAsDataURL(file);
        });
        paths.push(await api.terminalCacheImage(terminalId, { mime: file.type, data_base64: data }));
      }
      setDraft(current => `${current.slice(0, start)}${paths.join("\n")}${current.slice(end)}`);
    });
  };
  const enabled = notification?.enabled ?? false;
  const mutable = !enabled && !notification?.writing;
  return <section className="terminal-mailbox" aria-label="信箱">
    <div className="mailbox-head"><h3>信箱{page ? `（${page.unread}）` : ""}</h3>
      <div className="mailbox-actions">
        <button className="btn small" disabled={!session || busy} aria-pressed={enabled} onClick={() => {
          if (enabled) void run(() => toggle(false));
          else if (skipHint) void run(() => toggle(true));
          else setConfirm(true);
        }}>{enabled ? "关闭自动通知" : "开启自动通知"}</button>
        <button className="btn small ghost" disabled={!page?.unread || busy} onClick={() => void run(() => api.mailboxRemind(terminalId))}>提醒查收</button>
      </div>
    </div>
    {enabled && <p className="small muted">自动通知已开启；请通过信箱输入消息。操作终端授权或菜单前请关闭自动通知。</p>}
    {!enabled && session && <p className="small muted">手动提醒会立即插队，请保持终端聊天输入框可用且没有草稿。</p>}
    {!session && <p className="small muted">终端绑定会话后可使用信箱。</p>}
    {page?.items.map(m => <article className="mailbox-item" key={m.id} aria-label="未读消息">
      <div className="small muted">{m.sender_session_id ? `会话 ${m.sender_session_id}` : "用户"} · {new Date(m.created_at).toLocaleString()}</div>
      {editing?.message.id === m.id ? <div>
        <textarea aria-label="编辑信箱消息" value={editing.text} onChange={e => setEditing({ ...editing, text: e.target.value })} disabled={busy} />
        <div className="mailbox-actions"><button className="btn small" disabled={busy || !editing.text.trim()} onClick={() => void edit(editing.message, editing.text)}>保存</button><button className="btn small ghost" onClick={() => setEditing(null)}>取消</button></div>
      </div> : <p className="mailbox-body">{m.body}</p>}
      {mutable && <div className="mailbox-actions">
          <button className="btn ghost icon-button small" aria-label="编辑消息" disabled={busy} onClick={() => { setDeleting(null); setEditing({ message: m, text: m.body }); }}><Icon name="edit" /></button>
          {deleting === m.id ? <><button className="btn small danger" disabled={busy} onClick={() => void edit(m, null)}>确认永久删除</button><button className="btn small ghost" onClick={() => setDeleting(null)}>取消</button></>
            : <button className="btn ghost icon-button small" aria-label="删除消息" disabled={busy} onClick={() => { setEditing(null); setDeleting(m.id); }}><Icon name="trash" /></button>}
      </div>}
    </article>)}
    {page?.items.length === 0 && <p className="small muted">暂无未读消息</p>}
    {page?.next_cursor != null && <button className="btn small" onClick={() => void more()}>加载更多</button>}
    <form className="mailbox-compose" onSubmit={e => { e.preventDefault(); if (!session || !draft.trim()) return; const submitted = draft; void run(async () => { await api.mailboxSend(session, submitted); setDraft(current => current === submitted ? "" : current); }); }}>
      <label htmlFor={`mailbox-input-${terminalId}`} className="small">发送消息</label>
      <textarea ref={inputRef} id={`mailbox-input-${terminalId}`} value={draft} disabled={!session || busy} rows={4} onPaste={pasteImage} onChange={e => setDraft(e.target.value)} placeholder="输入文字或粘贴图片路径" />
      <button className="btn small primary" type="submit" disabled={!session || busy || !draft.trim()}>发送到信箱</button>
    </form>
    {(error || loadError || listError || notification?.error) && <p role="alert" className="small warning">{error || loadError || listError || notification?.error}</p>}
    {confirm && <Modal title="开启自动通知" onClose={() => !busy && setConfirm(false)} actions={<><button className="btn" disabled={busy} onClick={() => setConfirm(false)}>取消</button><button className="btn primary" disabled={busy} onClick={() => void run(() => toggle(true))}>开启自动通知</button></>}>
      <p>请保持 Agent 聊天输入框可用且没有草稿，并退出授权或设置菜单。开启后终端输入由 NoEnding 管理，通过侧栏信箱发送消息。</p>
      <label><input type="checkbox" checked={skip} onChange={e => setSkip(e.target.checked)} />不再提示</label>
    </Modal>}
  </section>;
}
