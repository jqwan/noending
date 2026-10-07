/** Route model — 手写 union，集中在此管理。 */

export type AssistantScope =
  | { type: "workspace" }
  | { type: "project"; id: string }
  | { type: "workstream"; id: string }
  | { type: "session"; id: string };

import type { Agent } from "../types";

export type SettingsSection =
  | "general"
  | "appearance"
  | "advanced";

/** 兼容历史旧设置子入口；在 AppShell 导航入口统一归一化为 agents 面板 */
export type LegacySettingsSection = "agents" | "sources";

/**
 * 页面动作直接携带在 Route 上（palette → 页面 Modal），目标页已挂载时也能收到：
 * AppShell 每次带 action 的导航都递增 actionSeq，页面以它为 effect 依赖。
 * 不引入第二套 pending/event 桥。
 */
export type ViewAction = "new";
export type SessionScope = "active" | "trash";
export type WorkstreamScope = "active" | "trash";

export type WorkstreamEntry = "review" | "conflicts";

/** Session 的页内子入口：`conversation` 是整屏的消息阅读界面，`terminal` 是内嵌 TUI 终端。 */
export type SessionEntry = "conversation" | "terminal";

export type Route =
  | { view: "home" }
  | { view: "workstreams"; action?: ViewAction; scope?: WorkstreamScope }
  | { view: "workstream"; workstreamId: string; entry?: WorkstreamEntry }
  | { view: "sessions"; action?: ViewAction; scope?: SessionScope }
  | {
      view: "session";
      sessionId: string;
      entry?: SessionEntry;
      initialTitle?: string;
      initialAgent?: Agent;
      initialTotal?: number;
    }
  /** 未绑定会话的独立终端视图：内嵌新建的直接落点。摄入发现会话并完成
   *  绑定后，视图 replace 成该会话的终端子页。 */
  | { view: "terminal"; terminalId: string }
  | { view: "agents"; agent?: Agent }
  | { view: "assistant"; scope?: AssistantScope }
  | { view: "projects" }
  | { view: "project"; projectId: string }
  | { view: "settings"; section?: SettingsSection | LegacySettingsSection }
  | { view: "search"; query: string };

/** 后台摄入完成的全局刷新信号（UI state，不进 domain）。 */
export const EVT_SYNCED = "noending:sync";

export function onEvent(evt: string, cb: () => void) {
  const h = () => cb();
  window.addEventListener(evt, h);
  return () => window.removeEventListener(evt, h);
}
