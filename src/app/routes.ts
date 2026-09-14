/** Route model (实施方案 §5) — 手写 union 保留，但集中在此管理。 */

export type AssistantScope =
  | { type: "workspace" }
  | { type: "project"; id: string }
  | { type: "workstream"; id: string }
  | { type: "session"; id: string };

export type SettingsSection =
  | "general"
  | "agents"
  | "sources"
  | "sync"
  | "appearance"
  | "advanced";

export type Route =
  | { view: "home" }
  | { view: "workstreams" }
  | { view: "workstream"; workstreamId: string }
  | { view: "sessions" }
  | { view: "session"; sessionId: string }
  | { view: "assistant"; scope?: AssistantScope }
  | { view: "projects" }
  | { view: "project"; projectId: string }
  | { view: "settings"; section?: SettingsSection }
  | { view: "search"; query: string };

/** UI 事件总线：palette / 跨页命令触发页面内 Modal（UI state，不进 domain）。 */
export const EVT_NEW_WORKSTREAM = "noending:new-workstream";
export const EVT_NEW_SESSION = "noending:new-session";
export const EVT_SYNCED = "noending:sync";

export function emit(evt: string) {
  window.dispatchEvent(new CustomEvent(evt));
}

export function onEvent(evt: string, cb: () => void) {
  const h = () => cb();
  window.addEventListener(evt, h);
  return () => window.removeEventListener(evt, h);
}

/**
 * 跨页命令（palette → 页面 Modal）：目标页可能尚未挂载，先登记，
 * 页面挂载时 consume；已挂载的页面走 onEvent 即时通道。
 */
let pendingCommand: string | null = null;

export function requestCommand(evt: string) {
  pendingCommand = evt;
}

export function consumeCommand(evt: string): boolean {
  if (pendingCommand === evt) {
    pendingCommand = null;
    return true;
  }
  return false;
}
