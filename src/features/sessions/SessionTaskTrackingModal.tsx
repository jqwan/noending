import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import Icon from "../../components/Icon";
import { EVT_SYNCED } from "../../app/routes";
import type { SessionTracking, Workstream } from "../../types";
import WorkstreamFormModal from "../workstreams/WorkstreamFormModal";

export default function SessionTaskTrackingModal({
  sessionId,
  onClose,
}: {
  sessionId: string;
  onClose: () => void;
}) {
  const [data, setData] = useState<SessionTracking | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [newTaskOpen, setNewTaskOpen] = useState(false);
  const serial = useRef(0);
  const pending = useRef(false);
  const active = useRef(true);

  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
      ++serial.current;
    };
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
    setError("");
    void refresh();
    const synced = () => {
      void refresh();
    };
    window.addEventListener(EVT_SYNCED, synced);
    return () => {
      ++serial.current;
      window.removeEventListener(EVT_SYNCED, synced);
    };
  }, [refresh]);

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

  return (
    <>
      <Modal
        title="选择跟踪任务"
        onClose={onClose}
        actions={
          <button
            type="button"
            className="btn ghost icon-button small"
            title="新建任务"
            aria-label="新建任务"
            disabled={busy || !data}
            onClick={() => setNewTaskOpen(true)}
          >
            <Icon name="plus" />
          </button>
        }
      >
        <div className="session-tracking-modal-body">
          {error && <p role="alert" className="warning session-tracking-error">{error}</p>}
          {!data && !error && <p className="muted small">正在读取可选任务…</p>}
          {data && allTasks.length === 0 && (
            <p className="muted small">
              {data.session.project_id
                ? "该项目暂无可用任务，可点击右上角「+」新建。"
                : "请先为会话关联工作目录及项目。"}
            </p>
          )}
          {allTasks.length > 0 && (
            <div className="session-tracking-modal-list">
              {allTasks.map((task) => {
                const isTracked = trackedIds.has(task.id);
                return (
                  <label
                    key={task.id}
                    className={`session-tracking-modal-row${isTracked ? " tracked" : ""}`}
                    title={isTracked ? "取消跟踪" : "添加跟踪"}
                  >
                    <input
                      type="checkbox"
                      disabled={busy}
                      checked={isTracked}
                      aria-label={task.title}
                      onChange={() => void change([task.id], !isTracked)}
                    />
                    <span className="session-tracking-modal-title">{task.title}</span>
                    {task.visibility === "archived" && <span className="badge">已归档</span>}
                  </label>
                );
              })}
            </div>
          )}
        </div>
      </Modal>

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
    </>
  );
}
