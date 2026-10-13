import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { useViewState } from "../../hooks/useViewState";
import { EVT_SYNCED, type Route } from "../../app/routes";
import type { SessionTracking } from "../../types";
import SessionTaskTrackingModal from "./SessionTaskTrackingModal";

/** State survives navigation; the window stays inside the session content so hiding it hides the window too. */
export default function SessionTaskTracking({ sessionId, navigate }: {
  sessionId: string | null;
  navigate: (route: Route) => void;
}) {
  const [open, setOpen] = useViewState(`session.${sessionId}.tracking.open`, false);
  const [modalOpen, setModalOpen] = useState(false);
  const [data, setData] = useState<SessionTracking | null>(null);
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

  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [open, setOpen]);

  const change = async (ids: string[], tracked: boolean) => {
    if (!sessionId || pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      for (const id of ids) await api.setSessionTaskTracking(sessionId, id, tracked);
    } catch (reason) {
      if (active.current) setError(String(reason));
    } finally {
      pending.current = false;
      window.dispatchEvent(new Event(EVT_SYNCED));
      if (active.current) {
        setBusy(false);
      }
    }
  };

  const trackedCount = data?.tracked_workstreams.length ?? 0;

  return <>
    <button
      className={`btn ghost icon-button${open ? " on" : ""}`}
      title="跟踪任务"
      aria-label="跟踪任务"
      aria-expanded={open}
      disabled={!sessionId}
      onClick={() => setOpen(value => !value)}
    >
      <Icon name="tasks" />
    </button>
    {open && sessionId && (
      <aside className="session-tracking-panel" aria-label="跟踪任务面板">
        <div className="session-tracking-head">
          <div className="session-tracking-head-title">
            <Icon name="tasks" />
            <h3>跟踪任务</h3>
            {trackedCount > 0 && (
              <span className="badge">{trackedCount}</span>
            )}
          </div>
          <div className="session-tracking-actions">
            <button
              className="btn ghost icon-button small"
              title="添加跟踪任务"
              aria-label="添加跟踪任务"
              disabled={busy || !data}
              onClick={() => setModalOpen(true)}
            >
              <Icon name="plus" />
            </button>
            <button
              className="btn ghost icon-button small"
              title="关闭面板"
              aria-label="关闭面板"
              onClick={() => setOpen(false)}
            >
              <Icon name="close" />
            </button>
          </div>
        </div>

        <div className="session-tracking-list">
          {data?.tracked_workstreams.map(task => (
            <div className="session-tracking-row" key={task.id}>
              <button
                className="session-tracking-link"
                title={task.title}
                onClick={() => navigate({ view: "workstream", workstreamId: task.id, initialTitle: task.title })}
              >
                <Icon name="tasks" />
                <span className="truncate">{task.title}</span>
              </button>
              {task.visibility === "archived" && <span className="badge">已归档</span>}
              <button
                className="btn ghost icon-button small session-tracking-remove"
                disabled={busy}
                title="取消跟踪"
                aria-label={`取消跟踪：${task.title}`}
                onClick={() => void change([task.id], false)}
              >
                <Icon name="close" />
              </button>
            </div>
          ))}
          {data && data.tracked_workstreams.length === 0 && (
            <p className="muted small">未跟踪任何任务，点击右上角「+」添加。</p>
          )}
          {!data && !error && <p className="muted small">正在读取任务…</p>}
        </div>

        {error && <p role="alert" className="warning session-tracking-error">{error}</p>}

        {modalOpen && (
          <SessionTaskTrackingModal
            sessionId={sessionId}
            onClose={() => setModalOpen(false)}
          />
        )}
      </aside>
    )}
  </>;
}
