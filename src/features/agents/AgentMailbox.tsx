import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import CustomSelect from "../../components/CustomSelect";
import { DetailList, DetailListRow, DetailLoading, DetailSection } from "../../components/DetailSection";
import DialogActions from "../../components/DialogActions";
import Icon from "../../components/Icon";
import Notice from "../../components/Notice";
import { Modal, timeAgo, useRefreshSignal } from "../../components/common";
import type { AgentMailboxChange, AgentMailboxMessage, AgentMailboxPage, SessionListItem } from "../../types";

function status(letter: AgentMailboxMessage) {
  if (letter.completed_at) return "已处理";
  return letter.handler_session_id ? "处理中" : "待接手";
}

export default function AgentMailbox({
  agentId, name, active, archived, sessions,
}: {
  agentId: string;
  name: string;
  active: boolean;
  archived: boolean;
  sessions: SessionListItem[];
}) {
  const [page, setPage] = useState<AgentMailboxPage | null>(null);
  const [opened, setOpened] = useState<AgentMailboxMessage | null>(null);
  const [composing, setComposing] = useState(false);
  const [draft, setDraft] = useState("");
  const [target, setTarget] = useState("");
  const [listError, setListError] = useState("");
  const [error, setError] = useState("");
  const [moreLoading, setMoreLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const writePending = useRef(false);
  const request = useRef(0);
  const openId = opened?.id;
  const listed = page?.items.find(letter => letter.id === openId);
  const selected = listed && opened && listed.revision > opened.revision ? listed : opened;
  const sendEnabled = !archived && Boolean(draft.trim());
  const sessionName = (id: string) => sessions.find(session => session.id === id)?.title || id;
  const eligible = sessions.filter(session => !session.archived_at);
  const handler = (letter: AgentMailboxMessage) => letter.handler_session_id
    ? `由 ${sessionName(letter.handler_session_id)} 处理` : "";
  const distinctTarget = target !== selected?.handler_session_id && eligible.some(session => session.id === target);

  const load = useCallback(async (cursor?: number) => {
    if (!active) return;
    const sequence = ++request.current;
    setMoreLoading(cursor !== undefined);
    try {
      const next = await api.agentMailboxList(agentId, cursor);
      if (sequence !== request.current) return;
      setPage(current => {
        if (cursor === undefined || !current) return next;
        const seen = new Set(current.items.map(letter => letter.id));
        return { ...next, items: [...current.items, ...next.items.filter(letter => !seen.has(letter.id))] };
      });
      setListError("");
    } catch (error) {
      if (sequence === request.current) setListError(String(error));
    } finally {
      if (sequence === request.current) setMoreLoading(false);
    }
  }, [active, agentId]);

  const refresh = useCallback(() => { void load(); }, [load]);
  const refreshOpenLetter = useCallback(() => {
    if (!active || !openId) return;
    void api.agentMailboxRead(agentId, openId).then(letter => {
      setOpened(current => current?.id === letter.id && letter.revision > current.revision ? letter : current);
    }, () => {});
  }, [active, agentId, openId]);
  useEffect(() => {
    refresh();
    return () => { request.current += 1; };
  }, [refresh]);
  useRefreshSignal(refresh);
  useRefreshSignal(refreshOpenLetter);

  async function write(operation: () => Promise<void>) {
    if (writePending.current) return;
    writePending.current = true;
    setBusy(true);
    setError("");
    try {
      await operation();
    } catch (error) {
      setError(String(error));
      refresh();
      refreshOpenLetter();
    } finally {
      writePending.current = false;
      setBusy(false);
    }
  }

  function send(event: React.FormEvent) {
    event.preventDefault();
    if (!sendEnabled) return;
    void write(async () => {
      await api.agentMailboxSend(agentId, draft);
      setDraft("");
      setComposing(false);
      refresh();
    });
  }

  function change(operation: AgentMailboxChange["operation"]) {
    if (!selected) return;
    void write(async () => {
      const saved = await api.agentMailboxChange(agentId, {
        message_id: selected.id,
        revision: selected.revision,
        operation,
        ...(operation === "transfer" && { target_session: target }),
      });
      setOpened(current => current?.id === saved.id && saved.revision > current.revision ? saved : current);
      setPage(current => {
        const previous = current?.items.find(letter => letter.id === saved.id);
        if (!current || !previous || previous.revision >= saved.revision) return current;
        return {
          ...current,
          unread: current.unread - Number(!previous.read_at && Boolean(saved.read_at)),
          items: current.items.map(letter => letter.id === saved.id ? saved : letter),
        };
      });
      setTarget("");
    });
  }

  return (
    <DetailSection sectionId="agent-mailbox" label="智能体信箱"
      title={<><Icon name="inbox" /><span>信箱{page && page.unread > 0 ? `（${page.unread} 封未读）` : ""}</span></>}
      actions={<>
        <button type="button" className="btn ghost icon-button small" title="刷新信箱" aria-label="刷新信箱"
          onClick={() => { refresh(); refreshOpenLetter(); }}><Icon name="refresh" /></button>
        {!archived && <button type="button" className="btn ghost icon-button small" title={`写信给${name}`} aria-label={`写信给${name}`}
          onClick={() => { setError(""); setComposing(true); }}><Icon name="chatPlus" /></button>}
      </>}>
      {listError && <Notice tone="error">{listError} <button type="button" className="btn small" onClick={refresh}>重试读取</button></Notice>}
      {!page && !listError && <DetailLoading>正在读取信箱…</DetailLoading>}
      {page && page.items.length === 0 && <p className="detail-empty">还没有来信。</p>}
      {page && page.items.length > 0 && <DetailList label="智能体来信列表">
        {page.items.map(letter => (
          <DetailListRow key={letter.id} icon={<Icon name="inbox" />}
            title={Array.from(letter.body.trim().replace(/\s+/g, " ")).slice(0, 72).join("")}
            titleHint="查看来信" description={[handler(letter), timeAgo(letter.created_at)].filter(Boolean).join(" · ")}
            meta={<>{!letter.read_at && <span className="badge accent">未读</span>}<span className="badge">{status(letter)}</span></>}
            onOpen={() => { setError(""); setOpened(letter); setTarget(""); }} />
        ))}
      </DetailList>}
      {page?.next_cursor != null && <button type="button" className="btn small ghost" disabled={moreLoading}
        onClick={() => void load(page.next_cursor ?? undefined)}>加载更多</button>}

      {active && composing && <Modal title={`写信给${name}`} onClose={() => { if (!busy) setComposing(false); }}>
        <form onSubmit={send}>
          <textarea aria-label="信件正文" rows={5} value={draft} disabled={busy} onChange={event => setDraft(event.target.value)} />
          {error && <Notice tone="error">{error}</Notice>}
          <DialogActions>
            <button type="button" className="btn" disabled={busy} onClick={() => setComposing(false)}>取消</button>
            <button type="submit" className="btn primary" disabled={busy || !sendEnabled}>发信</button>
          </DialogActions>
        </form>
      </Modal>}

      {active && selected && <Modal title="查看来信" onClose={() => { if (!busy) setOpened(null); }}>
        <p className="small muted">{selected.sender_session_id ? `发件对话 ${sessionName(selected.sender_session_id)}` : "你发出的信"} · {timeAgo(selected.created_at)}</p>
        <p className="detail-description">{selected.body}</p>
        <p className="small muted">{[status(selected), handler(selected)].filter(Boolean).join(" · ")}</p>
        {!archived && !selected.completed_at && eligible.length > 0 && <div className="agent-editor-actions">
          <CustomSelect ariaLabel="接手对话" value={target} disabled={busy}
            options={[{ value: "", label: "选择接手对话" }, ...eligible.map(session => ({ value: session.id, label: session.title || session.id }))]}
            onChange={setTarget} />
          <button type="button" className="btn small" disabled={busy || !distinctTarget}
            onClick={() => change("transfer")}>{selected.handler_session_id ? "转交" : "指定接手对话"}</button>
        </div>}
        {error && <Notice tone="error">{error}</Notice>}
        {(!selected.read_at || (!selected.completed_at && selected.handler_session_id)) && <DialogActions>
          {!selected.read_at && <button type="button" className="btn" disabled={busy} onClick={() => change("read")}>标为已读</button>}
          {!selected.completed_at && selected.handler_session_id && <>
            <button type="button" className="btn" disabled={busy} onClick={() => change("release")}>放回待处理</button>
            <button type="button" className="btn primary" disabled={busy} onClick={() => change("complete")}>标为已处理</button>
          </>}
        </DialogActions>}
      </Modal>}
    </DetailSection>
  );
}
