import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import DialogActions from "../../components/DialogActions";
import Notice from "../../components/Notice";
import AgentIcon from "../../components/AgentIcon";
import CompactPath from "../../components/CompactPath";
import { recentFirst } from "../../components/detailOrder";
import { sessionDisplayTitle } from "../sessions/sessionDisplay";
import type { SessionListItem } from "../../types";

export interface SessionTrackingChange { sessionId: string; tracked: boolean }

/** 统一管理关联，只提交用户改动；不改变项目归属或当前协作成员。 */
export default function WorkstreamSessionPickerModal({ projectIds, linkedSessions, lockedIds = [], onClose, onConfirm }: {
  projectIds: string[];
  linkedSessions: SessionListItem[];
  lockedIds?: string[];
  onClose: () => void;
  onConfirm: (changes: SessionTrackingChange[]) => Promise<void>;
}) {
  const [sessions, setSessions] = useState<SessionListItem[]>([]);
  const [edits, setEdits] = useState<Record<string, boolean>>({});
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef(false);
  const request = useRef(0);
  const projectKey = JSON.stringify([...new Set(projectIds)].sort());
  const load = useCallback(async () => {
    const current = ++request.current;
    const ids: string[] = JSON.parse(projectKey);
    setLoading(true); setError(""); setSessions([]);
    try {
      const pages = await Promise.all(ids.map(id => api.listSessions(id, undefined, "all")));
      if (current !== request.current) return;
      setSessions(recentFirst([...new Map(pages.flat().map(session => [session.id, session])).values()], session => session.last_conversation_at ?? session.last_activity_at ?? session.started_at));
    } catch (e) { if (current === request.current) setError(String(e)); }
    finally { if (current === request.current) setLoading(false); }
  }, [projectKey]);
  useEffect(() => { void load(); return () => { ++request.current; }; }, [load]);
  const linkedIds = new Set(linkedSessions.map(session => session.id));
  const candidates = recentFirst([...new Map([...sessions, ...linkedSessions].map(session => [session.id, session])).values()], session => session.last_conversation_at ?? session.last_activity_at ?? session.started_at);
  const changes = Object.entries(edits)
    .filter(([id, tracked]) => tracked !== linkedIds.has(id) && (tracked || !lockedIds.includes(id)) && (!tracked || candidates.some(session => session.id === id && !!session.project_id && projectIds.includes(session.project_id))))
    .map(([sessionId, tracked]) => ({ sessionId, tracked }));
  const filtered = candidates.filter(session => `${session.title ?? ""} ${session.cwd ?? ""}`.toLowerCase().includes(query.trim().toLowerCase()));
  const save = async () => {
    if (pending.current || !changes.length) return;
    pending.current = true; setBusy(true); setError("");
    try { await onConfirm(changes); onClose(); }
    catch (e) { setError(String(e)); }
    finally { pending.current = false; setBusy(false); }
  };
  return <Modal title="管理关联会话" onClose={() => { if (!pending.current) onClose(); }}>
    <div className="workstream-project-picker-modal">
      <p className="small muted">勾选建立关联，取消勾选解除关联。正在协作的成员需先停止协作。</p>
      <input type="text" autoFocus aria-label="搜索已有会话" placeholder="搜索会话名称或目录…" value={query} disabled={busy} onChange={event => setQuery(event.target.value)} />
      <div className="workstream-project-modal-list" role="group" aria-label="项目中的已有会话">
        {loading ? <p className="small muted" role="status">正在读取会话…</p>
          : error && candidates.length === 0 ? null
          : filtered.length === 0 ? <p className="small muted">{candidates.length ? "没有匹配的会话。" : "关联项目中暂无会话。"}</p>
          : filtered.map(session => {
            const linked = linkedIds.has(session.id);
            const locked = linked && lockedIds.includes(session.id);
            const title = sessionDisplayTitle(session.title);
            return <label className="workstream-project-item" key={session.id} title={`${title}${session.cwd ? `\n${session.cwd}` : ""}`}>
              <input type="checkbox" aria-label={title} disabled={busy || locked || (!linked && (!session.project_id || !projectIds.includes(session.project_id)))} checked={locked || (edits[session.id] ?? linked)} onChange={event => setEdits(current => ({ ...current, [session.id]: event.target.checked }))} />
              <AgentIcon agent={session.agent} />
              <span className="detail-list-copy workstream-project-item-name"><span className="truncate">{title}</span>{session.cwd && <span className="small muted"><CompactPath path={session.cwd} /></span>}</span>
              {locked ? <span className="badge accent small" title="停止协作后可取消关联">协作中</span> : linked && <span className="badge muted small">已关联</span>}
              {session.archived_at && <span className="badge muted small">已归档</span>}
            </label>;
          })}
      </div>
      {error && <Notice role="alert">{error}{!busy && !sessions.length && <button type="button" className="btn small ghost" onClick={() => void load()}>重试</button>}</Notice>}
      <DialogActions>
        <button type="button" className="btn" disabled={busy} onClick={onClose}>取消</button>
        <button type="button" className="btn primary" disabled={busy || loading || changes.length === 0} onClick={() => void save()}>{busy ? "保存中…" : "保存"}</button>
      </DialogActions>
    </div>
  </Modal>;
}
