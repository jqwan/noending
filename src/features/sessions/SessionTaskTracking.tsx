import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import Icon from "../../components/Icon";
import { useViewState } from "../../hooks/useViewState";
import { EVT_SYNCED, type Route } from "../../app/routes";
import type { SessionTracking, Workstream } from "../../types";
import WorkstreamFormModal from "../workstreams/WorkstreamFormModal";

/** State survives navigation; the window stays inside the session content so hiding it hides the window too. */
export default function SessionTaskTracking({ sessionId, navigate }: {
  sessionId: string | null;
  navigate: (route: Route) => void;
}) {
  const [open, setOpen] = useViewState(`session.${sessionId}.tracking.open`, false);
  const [collapsed, setCollapsed] = useViewState(`session.${sessionId}.tracking.collapsed`, false);
  const [hideUntracked, setHideUntracked] = useState(false);
  const [newTaskOpen, setNewTaskOpen] = useState(false);
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

  const allTasks = useMemo(() => {
    if (!data) return [];
    const map = new Map<string, Workstream>();
    for (const t of data.tracked_workstreams) map.set(t.id, t);
    for (const t of data.available_workstreams) {
      if (!map.has(t.id)) map.set(t.id, t);
    }
    return Array.from(map.values());
  }, [data]);

  const trackedIds = useMemo(() => {
    if (!data) return new Set<string>();
    const set = new Set<string>(data.session.tracked_workstream_ids);
    for (const t of data.tracked_workstreams) set.add(t.id);
    return set;
  }, [data]);

  const displayedTasks = useMemo(() => {
    if (hideUntracked) {
      return allTasks.filter(t => trackedIds.has(t.id));
    }
    return allTasks;
  }, [allTasks, trackedIds, hideUntracked]);

  const trackedCount = trackedIds.size;

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
      <aside className={`session-tracking-panel${collapsed ? " collapsed" : ""}`} aria-label="跟踪任务面板">
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
              className={`btn ghost icon-button small${hideUntracked ? " on" : ""}`}
              title={hideUntracked ? "显示未跟踪任务" : "隐藏未跟踪任务"}
              aria-label={hideUntracked ? "显示未跟踪任务" : "隐藏未跟踪任务"}
              aria-pressed={hideUntracked}
              disabled={busy || !data}
              onClick={() => setHideUntracked(v => !v)}
            >
              <Icon name="filter" />
            </button>
            <button
              className="btn ghost icon-button small"
              title="新建任务"
              aria-label="新建任务"
              disabled={busy || !data}
              onClick={() => setNewTaskOpen(true)}
            >
              <Icon name="plus" />
            </button>
            <button
              className="btn ghost icon-button small"
              title={collapsed ? "展开跟踪任务面板" : "收起跟踪任务面板"}
              aria-label={collapsed ? "展开跟踪任务面板" : "收起跟踪任务面板"}
              aria-expanded={!collapsed}
              onClick={() => setCollapsed(v => !v)}
            >
              <Icon name={collapsed ? "expandAll" : "collapseAll"} />
            </button>
          </div>
        </div>

        {!collapsed && (
          <div className="session-tracking-list">
            {displayedTasks.map(task => {
              const isTracked = trackedIds.has(task.id);
              return (
                <div className={`session-tracking-row${isTracked ? " tracked" : ""}`} key={task.id}>
                  <label className="session-tracking-check-label" title={isTracked ? "取消跟踪" : "添加跟踪"}>
                    <input
                      type="checkbox"
                      disabled={busy}
                      checked={isTracked}
                      aria-label={task.title}
                      onChange={() => void change([task.id], !isTracked)}
                    />
                  </label>
                  <button
                    className="session-tracking-link"
                    title={task.title}
                    onClick={() => navigate({ view: "workstream", workstreamId: task.id, initialTitle: task.title })}
                  >
                    {task.title}
                  </button>
                  {task.visibility === "archived" && <span className="badge">已归档</span>}
                  {isTracked && (
                    <button
                      className="btn ghost icon-button small session-tracking-remove"
                      disabled={busy}
                      title="取消跟踪"
                      aria-label={`取消跟踪：${task.title}`}
                      onClick={() => void change([task.id], false)}
                    >
                      <Icon name="close" />
                    </button>
                  )}
                </div>
              );
            })}
            {data && displayedTasks.length === 0 && (
              <p className="muted small">
                {hideUntracked
                  ? "暂无跟踪任务。"
                  : (data.session.project_id ? "该项目暂无可用任务，可点击上方「+」新建。" : "请先为会话关联工作目录及项目。")}
              </p>
            )}
            {!data && !error && <p className="muted small">正在读取任务…</p>}
          </div>
        )}

        {error && <p role="alert" className="warning session-tracking-error">{error}</p>}

        {newTaskOpen && (
          <WorkstreamFormModal
            initialProjectId={data?.session.project_id ?? undefined}
            onClose={() => setNewTaskOpen(false)}
            onCreated={async (created) => {
              setNewTaskOpen(false);
              await change([created.id], true);
            }}
          />
        )}
      </aside>
    )}
  </>;
}
