/** Route model — 手写 union，集中在此管理。 */

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

/** 会话页的历史消息入口。终端实例由 terminal_id 路由标识，与详情、历史消息共用顶部导航。 */
export type SessionEntry = "conversation";

/** 任务详情的子页：详情与协作室共用标题栏，省略即详情。 */
export type WorkstreamEntry = "detail" | "collaboration";

export type Route =
  | { view: "new-session"; workstreamId?: string; agent?: Agent; projectId?: string }
  | { view: "workstreams"; action?: ViewAction; scope?: WorkstreamScope }
  | { view: "workstream"; workstreamId: string; initialTitle?: string; entry?: WorkstreamEntry }
  | { view: "sessions"; action?: ViewAction; scope?: SessionScope }
  | {
      view: "session";
      sessionId: string;
      entry?: SessionEntry;
      initialTitle?: string;
      initialAgent?: Agent;
      initialTotal?: number;
    }
  /** 会话的内嵌终端面板（也容纳尚未绑定的实例）：内嵌新建与「先跳后启」的共同落点。
   *  会话身份由后端绑定（预指定 id 精确匹配 / 会话页按需校验匹配），绑定
   *  事实经 `terminal-bound` 事件推给视图，点亮会话详情入口。从会话页跳转
   *  时携带已知的身份种子（initialTitle/initialAgent/initialSessionId），
   *  首帧即终帧——标题不闪、入口即刻可点；内嵌新建不传，从「新会话」起步。 */
  | { view: "terminal"; terminalId: string; initialTitle?: string; initialAgent?: Agent; initialSessionId?: string }
  | { view: "agents"; agent?: Agent }
  | { view: "skills"; projectId?: string }
  | { view: "skill"; resourceId: string; initialTitle?: string; projectId?: string; workspacePathId?: string }
  | { view: "packages" }
  | { view: "package"; packageId: string; initialTitle?: string; agent?: Agent }
  | { view: "projects"; scope?: "unarchived" | "archived" }
  | { view: "project"; projectId: string; initialTitle?: string }
  | { view: "settings"; section?: SettingsSection | LegacySettingsSection };

/** 后台摄入完成的全局刷新信号（UI state，不进 domain）。 */
export const EVT_SYNCED = "noending:sync";

/** 未声明范围的同步通知保守刷新；已知的界面修改可以排除无关侧栏读取。 */
export type SidebarCollection = "sessions" | "projects";
export function notifyDataChanged(sidebar: SidebarCollection[]) {
  window.dispatchEvent(new CustomEvent(EVT_SYNCED, { detail: { sidebar } }));
}
export function affectsSidebar(event: Event, collection: SidebarCollection) {
  const scopes = (event as CustomEvent<{ sidebar?: SidebarCollection[] }>).detail?.sidebar;
  return !Array.isArray(scopes) || scopes.includes(collection);
}

/** 终端集合或状态变化（spawn / exit / bind / close）的全局刷新信号。后端 registry
 *  发 Tauri 事件 `terminals-changed`，AppShell 桥接成 window 事件。 */
export const EVT_TERMINALS = "noending:terminals";
export const EVT_TERMINAL_ACTIVITY = "noending:terminal-activity";

export function onEvent(evt: string, cb: () => void) {
  const h = () => cb();
  window.addEventListener(evt, h);
  return () => window.removeEventListener(evt, h);
}
