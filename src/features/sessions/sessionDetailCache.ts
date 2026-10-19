import type { SessionContextView, SessionDetail } from "../../types";

/**
 * 跨页面共享的会话详情与会话摘要缓存，用于路由预加载及详情页复用。
 * 独立于视图组件定义，避免引用缓存时拖入重量级视图实现。
 */
export const sessionDetailCache = new Map<string, SessionDetail>();
export const sessionContextCache = new Map<string, SessionContextView>();

export function clearSessionDetailCache() {
  sessionDetailCache.clear();
  sessionContextCache.clear();
}
