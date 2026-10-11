import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { useViewState } from "../../hooks/useViewState";
import { EVT_SYNCED, type Route } from "../../app/routes";
import type { SessionTracking } from "../../types";

/** State survives navigation; the window stays inside the session content so hiding it hides the window too. */
export default function SessionTaskTracking({ sessionId, navigate }: {
  sessionId: string | null;
  navigate: (route: Route) => void;
}) {
  const [open, setOpen] = useViewState(`session.${sessionId}.tracking.open`, false);
  const [data, setData] = useState<SessionTracking | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const serial = useRef(0);
  const pending = useRef(false);
  const active = useRef(true);
  useEffect(() => {
    active.current = true;
    return () => { active.current = false; ++serial.current; };
  }, []);

  const refresh = useCallback(async () => {
    const request = ++serial.current;
    if (!sessionId) return;
    try {
      const next = await api.getSessionTracking(sessionId);
      if (!active.current || request !== serial.current) return;
      setData(next);
      setSelected(ids => ids.filter(id => next.available_workstreams.some(task => task.id === id)));
    } catch (reason) {
      if (active.current && request === serial.current) setError(String(reason));
    }
  }, [sessionId]);
  useEffect(() => {
    if (!open) return;
    setError("");
    void refresh();
    const synced = () => { void refresh(); };
    window.addEventListener(EVT_SYNCED, synced);
    return () => { ++serial.current; window.removeEventListener(EVT_SYNCED, synced); };
  }, [open, refresh]);

  const change = async (ids: string[], tracked: boolean) => {
    if (!sessionId || pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      for (const id of ids) await api.setSessionTaskTracking(sessionId, id, tracked);
      if (active.current && tracked) { setSelected([]); setAdding(false); }
    } catch (reason) {
      if (active.current) setError(String(reason));
    } finally {
      pending.current = false;
      // A multi-selection can partially succeed; refresh the actual saved links.
      window.dispatchEvent(new Event(EVT_SYNCED));
      if (active.current) setBusy(false);
    }
  };
  return <>
    <button className={`btn ghost icon-button${open ? " on" : ""}`} title="跟踪任务" aria-label="跟踪任务"
      aria-expanded={open} disabled={!sessionId} onClick={() => setOpen(value => !value)}><Icon name="tasks" /></button>
    {open && sessionId && <aside className="session-tracking-panel" aria-label="跟踪任务面板">
      <div className="session-tracking-head">
        <h3>跟踪任务</h3>
        <button className={`btn ghost icon-button${adding ? " on" : ""}`} title="添加跟踪任务" aria-label="添加跟踪任务"
          aria-expanded={adding} disabled={busy || !data} onClick={() => setAdding(value => !value)}><Icon name="plus" /></button>
      </div>
      <div className="session-tracking-list">
        {data?.tracked_workstreams.map(task => <div className="session-tracking-row" key={task.id}>
          <button className="session-tracking-link" title={task.title}
            onClick={() => navigate({ view: "workstream", workstreamId: task.id, initialTitle: task.title })}>{task.title}</button>
          {task.visibility === "archived" && <span className="badge">已归档</span>}
          <button className="btn ghost icon-button" disabled={busy} title="取消跟踪" aria-label={`取消跟踪：${task.title}`}
            onClick={() => void change([task.id], false)}><Icon name="close" /></button>
        </div>)}
        {data && data.tracked_workstreams.length === 0 && <p className="muted small">暂无跟踪任务。</p>}
        {!data && !error && <p className="muted small">正在读取任务…</p>}
      </div>
      {adding && <div className="session-tracking-add">
        <div className="section-label">添加跟踪任务</div>
        {data?.available_workstreams.map(task => <label className="existing-path-option" key={task.id}>
          <input type="checkbox" disabled={busy} checked={selected.includes(task.id)} onChange={event => {
            const checked = event.target.checked;
            setSelected(ids => checked ? [...ids, task.id] : ids.filter(id => id !== task.id));
          }} /><span>{task.title}</span>
        </label>)}
        {data && data.available_workstreams.length === 0 && <p className="muted small">
          {data.session.project_id ? "该项目没有其他可跟踪任务。" : "请先为会话关联工作目录及项目。"}
        </p>}
        <button className="btn primary small" disabled={busy || selected.length === 0}
          onClick={() => void change(selected, true)}>添加跟踪</button>
      </div>}
      {error && <p role="alert" className="warning">{error}</p>}
    </aside>}
  </>;
}
