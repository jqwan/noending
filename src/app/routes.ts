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
export type SessionScope = "unarchived" | "archived";
export type WorkstreamScope = "unarchived" | "archived";

export type WorkstreamEntry = "review" | "conflicts";

/** Session 的页内子入口：`conversation` 是整屏的消息阅读界面。终端不再是
 *  会话的子页——它是一等独立视图（见下方 `view:"terminal"`）。 */
export type SessionEntry = "conversation";

export type Route =
  | { view: "new-session"; workstreamId?: string; agent?: Agent; projectId?: string }
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
  /** 内嵌终端视图（一等独立路由）：内嵌新建与「先跳后启」的共同落点。
   *  会话身份由后端绑定（预指定 id 精确匹配 / 会话页按需校验匹配），绑定
   *  事实经 `terminal-bound` 事件推给视图，点亮会话详情入口。从会话页跳转
   *  时携带已知的身份种子（initialTitle/initialAgent/initialSessionId），
   *  首帧即终帧——标题不闪、入口即刻可点；内嵌新建不传，从「新会话」起步。 */
  | { view: "terminal"; terminalId: string; initialTitle?: string; initialAgent?: Agent; initialSessionId?: string }
  | { view: "agents"; agent?: Agent }
  | { view: "assistant"; scope?: AssistantScope }
  | { view: "projects" }
  | { view: "project"; projectId: string }
  | { view: "settings"; section?: SettingsSection | LegacySettingsSection }
  | { view: "search"; query: string };

/** 后台摄入完成的全局刷新信号（UI state，不进 domain）。 */
export const EVT_SYNCED = "noending:sync";

/** 活终端集合变化（spawn / exit / bind）的全局刷新信号。后端 registry
 *  发 Tauri 事件 `terminals-changed`，AppShell 桥接成 window 事件。 */
export const EVT_TERMINALS = "noending:terminals";

export function onEvent(evt: string, cb: () => void) {
  const h = () => cb();
  window.addEventListener(evt, h);
  return () => window.removeEventListener(evt, h);
}
