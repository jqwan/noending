import { useSyncExternalStore } from "react";
import { api } from "../api";
import { EVT_SYNCED } from "../app/routes";
import type { OperationStatus } from "../types";

type Snapshot = { operations: OperationStatus[]; checking: boolean; error: string | null };
let snapshot: Snapshot = { operations: [], checking: true, error: null };
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | undefined;
let epoch = 0;
let pending: Promise<void> | null = null;
function publish(next: Snapshot) {
  if (JSON.stringify(snapshot) === JSON.stringify(next)) return;
  snapshot = next;
  listeners.forEach(listener => listener());
}
/** One bounded poll for all mounted consumers; it also observes standalone CLI/MCP processes. */
export function refreshOperationStatus(fresh = false): Promise<void> {
  if (pending) {
    if (!fresh) return pending;
    // A check begun before submission may still report idle. Query again
    // after it resolves before releasing the submitting button's local guard.
    const previous = pending;
    return previous.then(() => {
      if (pending === previous) pending = null;
      return refreshOperationStatus();
    });
  }
  const requestEpoch = epoch;
  const request = (async () => {
    try {
      const operations = await api.getOperationStatus();
      if (epoch === requestEpoch) publish({ operations, checking: false, error: null });
    } catch (error) {
      if (epoch === requestEpoch) publish({ ...snapshot, checking: false, error: String(error) });
    }
  })();
  pending = request;
  void request.finally(() => { if (pending === request) pending = null; });
  return request;
}
const refresh = () => { void refreshOperationStatus(); };
function subscribe(listener: () => void) {
  listeners.add(listener);
  if (listeners.size === 1) {
    publish({ ...snapshot, checking: true });
    refresh();
    timer = setInterval(refresh, 750);
    window.addEventListener("focus", refresh);
    window.addEventListener(EVT_SYNCED, refresh);
  }
  return () => {
    listeners.delete(listener);
    if (!listeners.size) {
      clearInterval(timer);
      window.removeEventListener("focus", refresh);
      window.removeEventListener(EVT_SYNCED, refresh);
      ++epoch;
      pending = null;
      snapshot = { operations: [], checking: true, error: null };
    }
  };
}
function matches(operation: OperationStatus, kind: "sync" | "summary", targetKind?: string, id?: string) {
  if (operation.kind !== kind) return false;
  if (!targetKind) return true;
  return operation.targets.some(target => {
    if (target.kind === targetKind && target.id === id) return true;
    return kind === "sync" && target.kind === "all";
  });
}
export function useOperationState(kind: "sync" | "summary", targetKind?: string, id?: string) {
  const state = useSyncExternalStore(subscribe, () => snapshot);
  const active = state.operations.filter(operation => matches(operation, kind, targetKind, id));
  return {
    busy: active.length > 0,
    queued: active.length > 0 && active.every(operation => operation.state === "queued"),
    checking: state.checking || state.error !== null,
    error: state.error,
  };
}
