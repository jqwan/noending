import { useCallback, useEffect, useLayoutEffect, useRef, useState, useTransition, type CSSProperties } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "../api";
import Sidebar from "../layout/Sidebar";
import NavigationRail, { isSessionRoute } from "../layout/NavigationRail";
import Router, { preloadCommonRoutes } from "./Router";
import { rememberSessionPage } from "../features/sessions/sessionNavigation";
import CommandPalette from "../components/CommandPalette";
import ErrorBoundary from "../components/ErrorBoundary";
import { prepareNavigationTitle } from "./navigationTitle";
import { useAutoHideScrollbars } from "../hooks/useAutoHideScrollbars";
import ToastHost, { showToast } from "../components/Toast";
import Icon from "../components/Icon";
import { LaunchDetailsHost } from "../features/launcher/LaunchResultModal";
import { EVT_SYNCED, EVT_TERMINALS, EVT_TERMINAL_ACTIVITY, type Route } from "./routes";
import { EVT_REMOVED, isRemovedRoute, type RemovedTargets } from "./removedTargets";
import { sessionDetailCache, sessionContextCache } from "../features/sessions/sessionDetailCache";
import { clearSessionsCache } from "../features/sessions/sessionsCache";
import { clearProjectDetailCache } from "../features/projects/projectDetailCache";
import { clearProjectCardsCache } from "../features/projects/projectCardsCache";
import { clearWorkstreamCardsCache } from "../features/workstreams/useWorkstreamCards";
import { clearWorkstreamDetailCache } from "../features/workstreams/workstreamDetailCache";


type NavigationState = { entries: Route[]; index: number };

function withoutAction(route: Route): Route {
  if (!("action" in route) || !route.action) return route;
  const { action: _action, ...base } = route;
  return base as Route;
}

export function normalizeRoute(route: Route): Route {
  if (route.view === "sessions" && route.action === "new") {
    return { view: "new-session" };
  }
  if (route.view === "settings") {
    if (route.section === "agents" || route.section === "sources") {
      return { view: "agents" };
    }
  }
  return route;
}

function sameRoute(a: Route, b: Route): boolean {
  const left = withoutAction(a);
  const right = withoutAction(b);
  if (left.view === "sessions" && right.view === "sessions") {
    return (left.scope ?? "active") === (right.scope ?? "active");
  }
  if (left.view === "workstreams" && right.view === "workstreams") {
    return (left.scope ?? "active") === (right.scope ?? "active");
  }
  if (left.view === "session" && right.view === "session") {
    return left.sessionId === right.sessionId && left.entry === right.entry;
  }
  if (left.view === "terminal" && right.view === "terminal") return left.terminalId === right.terminalId;
  if (left.view === "project" && right.view === "project") return left.projectId === right.projectId;
  if (left.view === "workstream" && right.view === "workstream") return left.workstreamId === right.workstreamId;
  return JSON.stringify(left) === JSON.stringify(right);
}

/**
 * AppShell：只负责骨架 —— Sidebar、RouterOutlet、
 * CommandPalette、全局事件与 Toast。ID-only 导航先准备标题，业务数据仍由页面加载。
 */
export default function AppShell() {
  useAutoHideScrollbars();
  const [, startTransition] = useTransition();
  const [navigation, setNavigation] = useState<NavigationState>({
    entries: [{ view: "new-session" }],
    index: 0,
  });

  // 在后台空闲时预热常用路由分块，确保点击切换时零等待且无 Suspense 闪烁
  useEffect(() => {
    if (typeof window !== "undefined" && "requestIdleCallback" in window) {
      const id = (window as unknown as { requestIdleCallback: (cb: () => void, opts?: { timeout: number }) => number })
        .requestIdleCallback(() => preloadCommonRoutes(), { timeout: 2000 });
      return () => (window as unknown as { cancelIdleCallback: (id: number) => void }).cancelIdleCallback(id);
    }
    const timer = setTimeout(() => preloadCommonRoutes(), 120);
    return () => clearTimeout(timer);
  }, []);

  const [paletteOpen, setPaletteOpen] = useState(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(
    () => localStorage.getItem("noending.sidebarCollapsed") === "true",
  );
  const [sidebarWidth, setSidebarWidth] = useState(() => {
    const saved = Number(localStorage.getItem("noending.sidebarWidth"));
    return Number.isFinite(saved) && saved >= 200 ? Math.min(360, saved) : 248;
  });
  const resizeSidebar = (width: number) => {
    const next = Math.max(200, Math.min(360, width));
    setSidebarWidth(next);
    localStorage.setItem("noending.sidebarWidth", String(next));
  };
  // 带 action 的导航每次都递增，页面据此响应「已在目标页」的重复命令
  const [actionSeq, setActionSeq] = useState(0);
  const seqRef = useRef(0);
  const navigationRequest = useRef(0);
  useEffect(() => () => { ++navigationRequest.current; }, []);
  const route = navigation.entries[navigation.index];
  const sessionSection = isSessionRoute(route);
  const lastSessionRoute = useRef<Route>(route);
  const currentRoute = useRef(route);
  const sessionRoute = sessionSection ? route : lastSessionRoute.current;
  useLayoutEffect(() => {
    currentRoute.current = route;
    if (route.view === "session") rememberSessionPage(route.sessionId, route);
    if (isSessionRoute(route)) lastSessionRoute.current = withoutAction(route);
  }, [route]);
  const isMac = /Macintosh|Mac OS X/.test(navigator.userAgent);

  useEffect(() => {
    localStorage.setItem("noending.sidebarCollapsed", String(sidebarCollapsed));
  }, [sidebarCollapsed]);

  // Returning to the foreground is a freshness trigger, never a page-open
  // trigger. Ignore the initial focused event; startup already queued a pass.
  useEffect(() => {
    let wasBackgrounded = false;
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) {
        wasBackgrounded = true;
      } else if (wasBackgrounded) {
        wasBackgrounded = false;
        void api.appForeground().catch((error) => {
          console.error("Failed to queue foreground ingestion", error);
        });
      }
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  const navigate = useCallback((target: Route) => {
    if (isRemovedRoute(target)) return;
    const request = ++navigationRequest.current;
    const commit = (r: Route) => {
      if (request !== navigationRequest.current || isRemovedRoute(r)) return;
      startTransition(() => {
        setNavigation((current) => {
          const entries = current.entries.slice(0, current.index + 1);
          const currentRoute = entries[entries.length - 1];
          if (sameRoute(currentRoute, r)) {
            entries[entries.length - 1] = r;
            return { entries, index: entries.length - 1 };
          }
          entries[entries.length - 1] = withoutAction(currentRoute);
          entries.push(r);
          return { entries, index: entries.length - 1 };
        });
        if ("action" in r && r.action) setActionSeq(++seqRef.current);
      });
    };
    const prepared = prepareNavigationTitle(normalizeRoute(target));
    if (prepared instanceof Promise) {
      void prepared.then(commit).catch(error => {
        if (request === navigationRequest.current) showToast(`无法打开：${String(error)}`);
      });
    } else commit(prepared);
  }, []);

  // An async launch can finish after switching sections. Remember its result
  // for the session section without pulling the user away from another page.
  const navigateSession = useCallback((target: Route) => {
    if (isRemovedRoute(target)) return;
    if (isSessionRoute(currentRoute.current)) navigate(target);
    else if (isSessionRoute(target)) lastSessionRoute.current = withoutAction(normalizeRoute(target));
  }, [navigate]);

  useEffect(() => {
    const removed = (event: Event) => {
      const targets = (event as CustomEvent<RemovedTargets>).detail;
      for (const id of targets.sessionIds ?? []) {
        sessionDetailCache.delete(id);
        sessionContextCache.delete(id);
      }
      if (targets.sessionIds?.length) clearSessionsCache();
      if (targets.projectIds?.length) { clearProjectDetailCache(); clearProjectCardsCache(); }
      if (targets.workstreamIds?.length) { clearWorkstreamDetailCache(); clearWorkstreamCardsCache(); }
      const replacement: Route = targets.replacement && !isRemovedRoute(targets.replacement)
        ? targets.replacement : { view: "sessions" };
      if (isRemovedRoute(lastSessionRoute.current)) lastSessionRoute.current = isSessionRoute(replacement)
        ? replacement : { view: "sessions" };
      setNavigation(current => {
        const entries: Route[] = [];
        let index = 0;
        current.entries.forEach((entry, originalIndex) => {
          const invalid = isRemovedRoute(entry);
          if (invalid && originalIndex !== current.index) return;
          const next = invalid ? replacement : entry;
          if (!entries.length || !sameRoute(entries[entries.length - 1], next)) entries.push(next);
          if (originalIndex === current.index) index = entries.length - 1;
        });
        return { entries, index };
      });
    };
    window.addEventListener(EVT_REMOVED, removed);
    return () => window.removeEventListener(EVT_REMOVED, removed);
  }, []);

  const goBack = useCallback((fallback?: Route) => {
    ++navigationRequest.current;
    startTransition(() => {
      setNavigation((current) => {
        if (current.index === 0) {
          return fallback ? { entries: [fallback], index: 0 } : current;
        }
        const entries = current.entries.slice();
        entries[current.index - 1] = withoutAction(entries[current.index - 1]);
        return { entries, index: current.index - 1 };
      });
    });
  }, [startTransition]);

  const goForward = useCallback(() => {
    ++navigationRequest.current;
    startTransition(() => {
      setNavigation((current) => {
        if (current.index >= current.entries.length - 1) return current;
        const entries = current.entries.slice();
        entries[current.index + 1] = withoutAction(entries[current.index + 1]);
        return { entries, index: current.index + 1 };
      });
    });
  }, [startTransition]);

  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const back = (e.altKey && e.key === "ArrowLeft") || (e.metaKey && e.key === "[");
      const forward = (e.altKey && e.key === "ArrowRight") || (e.metaKey && e.key === "]");
      if (!back && !forward) return;
      e.preventDefault();
      if (back) goBack();
      else goForward();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [goBack, goForward]);

  // ⌘K / Ctrl+K opens object lookup.
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, []);

  // Background ingestion / reconcile completion → 通知页面做定向刷新，
  // 不做整页刷新：页面通过 window 事件自行 invalidate。
  useEffect(() => {
    // 后台摄入的唯一完成事件（commands/ingestion.rs）。
    const un1 = listen("ingestion-completed", () => emitSynced());
    // Projects Experience v0.2 工作区刷新完成后同样扇出刷新信号，
    // Sidebar 最近列表等自行 invalidate。
    const un2 = listen("workspace-reconcile-completed", () => emitSynced());
    const unCli = listen("cli-data-changed", () => emitSynced());
    const unMailboxError = listen<{session_id:string;error:string}>("mailbox-notification-error", ({payload}) => {
      showToast(`信箱唤醒失败：${payload.error}`);
    });
    // 终端集合或状态变化（spawn / exit / bind / close）→ Sidebar 的
    // 「运行中」列表自行重取；terminal-bound 有 payload，由终端视图直听。
    const un3 = listen("terminals-changed", () => {
      window.dispatchEvent(new CustomEvent(EVT_TERMINALS));
    });
    const unActivity = listen("terminal-activity", () => {
      window.dispatchEvent(new CustomEvent(EVT_TERMINAL_ACTIVITY));
    });
    return () => {
      unActivity.then(f => f());
      un1.then((f) => f());
      un2.then((f) => f());
      unCli.then((f) => f());
      unMailboxError.then(f => f());
      un3.then((f) => f());
    };
  }, []);

  return (
    <div style={{ "--sidebar-width": `${sidebarWidth}px` } as CSSProperties} className={`app${isMac ? " mac-titlebar" : ""}${sidebarCollapsed ? " sidebar-collapsed without-session-sidebar" : ""}`}>
      <header className="app-titlebar">
        {isMac && <div className="window-drag-region" data-tauri-drag-region />}
        <div className="history-controls" aria-label="页面导航">
          <button
            className="history-button"
            aria-label="返回上一页"
            title="返回上一页"
            disabled={navigation.index === 0}
            onClick={() => goBack()}
          >
            <svg className="history-icon" viewBox="0 0 18 18" aria-hidden="true">
              <path d="M14 9H4m5-5L4 9l5 5" />
            </svg>
          </button>
          <button
            className="history-button"
            aria-label="前进到下一页"
            title="前进到下一页"
            disabled={navigation.index >= navigation.entries.length - 1}
            onClick={goForward}
          >
            <svg className="history-icon" viewBox="0 0 18 18" aria-hidden="true">
              <path d="M4 9h10M9 4l5 5-5 5" />
            </svg>
          </button>
          <button
            className="history-button"
            aria-label="查找"
            title={isMac ? "查找 (⌘K)" : "查找 (Ctrl+K)"}
            onClick={() => setPaletteOpen(true)}
          >
            <Icon name="search" />
          </button>
          <button
            className="history-button"
            aria-label={sidebarCollapsed ? "展开侧边栏" : "收起侧边栏"}
            title={sidebarCollapsed ? "展开侧边栏" : "收起侧边栏"}
            onClick={() => setSidebarCollapsed((value) => !value)}
          >
            <svg className="history-icon" viewBox="0 0 18 18" aria-hidden="true">
              <rect x="3" y="4" width="12" height="10" rx="2.5" />
              <path d="M7 4v10" />
            </svg>
          </button>
          {sidebarCollapsed && (
            <button
              className="history-button"
              aria-label="新建会话"
              title="新建会话"
              onClick={() => navigate({ view: "new-session" })}
            >
              <Icon name="chatPlus" />
            </button>
          )}
        </div>
        <div className="titlebar-divider" aria-hidden="true" />
      </header>
      <NavigationRail route={route} navigate={navigate} />
      <Sidebar
        route={route}
        navigate={navigate}
        collapsed={sidebarCollapsed}
      />
      {!sidebarCollapsed && <div
        className="sidebar-resizer" role="separator" tabIndex={0}
        aria-label="侧边栏宽度" aria-orientation="vertical"
        aria-valuemin={200} aria-valuemax={360} aria-valuenow={sidebarWidth}
        onDoubleClick={() => resizeSidebar(248)}
        onKeyDown={(e) => {
          if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) return;
          e.preventDefault();
          resizeSidebar(e.key === "Home" ? 200 : e.key === "End" ? 360 : sidebarWidth + (e.key === "ArrowRight" ? 8 : -8));
        }}
        onPointerDown={(e) => { e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId); }}
        onPointerMove={(e) => {
          if (e.currentTarget.hasPointerCapture(e.pointerId)) {
            const sidebar = e.currentTarget.parentElement!.querySelector(".sidebar")!;
            resizeSidebar(e.clientX - sidebar.getBoundingClientRect().left);
          }
        }}
        onPointerUp={(e) => {
          if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
        }}
      />}
      <div className="app-content">
        {/* 兜底只包内容区：页面（含它自己的弹窗）渲染抛错时侧栏与标题栏还在，
            用户能直接切走；resetKey 让路由一换就复位。 */}
        <div className="session-content" hidden={!sessionSection}>
          <ErrorBoundary resetKey={JSON.stringify(withoutAction(sessionRoute))}>
            <Router route={sessionRoute} navigate={navigateSession} goBack={goBack} actionSeq={actionSeq} />
          </ErrorBoundary>
        </div>
        {!sessionSection && <ErrorBoundary resetKey={JSON.stringify(withoutAction(route))}>
          <Router route={route} navigate={navigate} goBack={goBack} actionSeq={actionSeq} />
        </ErrorBoundary>}
      </div>
      {paletteOpen && (
        <CommandPalette onClose={() => setPaletteOpen(false)} navigate={navigate} />
      )}
      <ToastHost />
      <LaunchDetailsHost />
    </div>
  );
}

function emitSynced() {
  window.dispatchEvent(new CustomEvent(EVT_SYNCED));
}
