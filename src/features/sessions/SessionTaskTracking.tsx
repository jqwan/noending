import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import SessionTrackedTasks from "./SessionTrackedTasks";
import { EVT_SYNCED, type Route } from "../../app/routes";
import type { SessionTracking } from "../../types";
import SessionTaskTrackingModal from "./SessionTaskTrackingModal";

/** State survives navigation; the window stays inside the session content so hiding it hides the window too. */
export default function SessionTaskTracking({ sessionId, navigate }: {
  sessionId: string | null;
  navigate: (route: Route) => void;
}) {
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
    if (!sessionId) return;
    setError("");
    void refresh();
    const synced = () => { void refresh(); };
    window.addEventListener(EVT_SYNCED, synced);
    return () => { ++serial.current; window.removeEventListener(EVT_SYNCED, synced); };
  }, [sessionId, refresh]);

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

  return <>
    <SessionTrackedTasks tasks={data?.tracked_workstreams ?? null} navigate={navigate}
      bound={Boolean(sessionId)} busy={busy} error={error}
      onAdd={() => setModalOpen(true)} onRemove={id => void change([id], false)} />
        {modalOpen && sessionId && (
          <SessionTaskTrackingModal
            sessionId={sessionId}
            onClose={() => setModalOpen(false)}
          />
        )}
      </>;
}
