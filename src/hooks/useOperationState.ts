import { useEffect } from "react";
import { api } from "../api";
import { EVT_SYNCED } from "../app/routes";
import type { OperationStatus } from "../types";
import { createReadResource } from "./readResource";

type Snapshot = { operations: OperationStatus[]; checking: boolean; error: string | null };
const initial: Snapshot = { operations: [], checking: true, error: null };
const status = createReadResource<Snapshot>(async (): Promise<Snapshot> => {
  try { return { operations: await api.getOperationStatus(), checking: false, error: null }; }
  catch (error) { return { ...(status.get("operations") ?? initial), checking: false, error: String(error) }; }
});
let consumers = 0;
let timer: ReturnType<typeof setInterval> | undefined;

/** 所有页面共享最后一次结果；提交后强制新读，迟到的旧请求不会覆盖它。 */
export function refreshOperationStatus(fresh = false): Promise<void> {
  return (fresh ? status.refresh("operations") : status.revalidate("operations")).then(() => {});
}
const refresh = () => { void refreshOperationStatus(); };
export function prepareOperationStatus() { return status.ensure("operations"); }
function matches(operation: OperationStatus, kind: "sync" | "summary", targetKind?: string, id?: string) {
  if (operation.kind !== kind) return false;
  if (!targetKind) return true;
  return operation.targets.some(target => {
    if (target.kind === targetKind && target.id === id) return true;
    return kind === "sync" && target.kind === "all";
  });
}
export function useOperationState(kind: "sync" | "summary", targetKind?: string, id?: string) {
  const state = status.useValue("operations") ?? initial;
  useEffect(() => {
    if (++consumers === 1) {
      refresh();
      timer = setInterval(refresh, 750);
      window.addEventListener("focus", refresh);
      window.addEventListener(EVT_SYNCED, refresh);
    }
    return () => {
      if (--consumers === 0) {
        clearInterval(timer);
        window.removeEventListener("focus", refresh);
        window.removeEventListener(EVT_SYNCED, refresh);
      }
    };
  }, []);
  const active = state.operations.filter(operation => matches(operation, kind, targetKind, id));
  return {
    busy: active.length > 0,
    queued: active.length > 0 && active.every(operation => operation.state === "queued"),
    checking: state.checking || state.error !== null,
    error: state.error,
  };
}
