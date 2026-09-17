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

/**
 * 页面动作直接携带在 Route 上（palette → 页面 Modal）：
 * 目标页已挂载时也能收到（AppShell 每次带 action 的导航都会递增
 * actionSeq，页面以它为 effect 依赖）。不引入第二套 pending/event 桥。
 */
export type ViewAction = "new";

export type WorkstreamEntry = "review" | "conflicts";

export type Route =
  | { view: "home" }
  | { view: "workstreams"; action?: ViewAction }
  | { view: "workstream"; workstreamId: string; entry?: WorkstreamEntry }
  | { view: "sessions"; action?: ViewAction }
  | { view: "session"; sessionId: string }
  | { view: "assistant"; scope?: AssistantScope }
  | { view: "projects" }
  | { view: "project"; projectId: string }
  | { view: "settings"; section?: SettingsSection }
  | { view: "search"; query: string };

/** 后台同步 / reconcile 完成的全局刷新信号（UI state，不进 domain）。 */
export const EVT_SYNCED = "noending:sync";

export function onEvent(evt: string, cb: () => void) {
  const h = () => cb();
  window.addEventListener(evt, h);
  return () => window.removeEventListener(evt, h);
}
