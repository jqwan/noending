import { useCallback, useEffect, useLayoutEffect, useRef, useState, useTransition, type CSSProperties } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "../api";
import Sidebar from "../layout/Sidebar";
import NavigationRail, { isSessionRoute } from "../layout/NavigationRail";
import WindowsWindowControls from "../layout/WindowsWindowControls";
import Router, { preloadCommonRoutes } from "./Router";
import { rememberSessionPage } from "../features/sessions/sessionNavigation";
import CommandPalette from "../components/CommandPalette";
import ErrorBoundary from "../components/ErrorBoundary";
import { prepareNavigation } from "./navigation";
import { clearReadResources } from "../hooks/readResource";
import { useAutoHideScrollbars } from "../hooks/useAutoHideScrollbars";
import { useLoadingDelay } from "../hooks/useLoadingDelay";
import ToastHost, { showToast } from "../components/Toast";
import Icon from "../components/Icon";
import PanelToggle from "../components/PanelToggle";
import PanelResizer from "../components/PanelResizer";
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
 * CommandPalette、全局事件与 Toast。导航先准备代码和首屏数据，页面保留后台刷新能力。
 */
export default function AppShell() {
  useAutoHideScrollbars();
  const [, startTransition] = useTransition();
  const [preparing, setPreparing] = useState(false);
  const showNavigationProgress = useLoadingDelay(preparing);
  const [navigation, setNavigation] = useState<NavigationState>({
    entries: [{ view: "new-session" }],
    index: 0,
  });

  // 空闲时预热路由；未预热的目标由公共导航等待模块就绪。
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
  const pendingDestination = useRef<Route | null>(null);
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
  const isWindows = /Windows/.test(navigator.userAgent);

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

  const prepareDestination = useCallback((target: Route, commit: (route: Route) => void) => {
    if (isRemovedRoute(target)) return;
    const request = ++navigationRequest.current;
    pendingDestination.current = target;
    const ready = (route: Route) => {
      if (request === navigationRequest.current) {
        setPreparing(false);
        pendingDestination.current = null;
        if (!isRemovedRoute(route)) commit(route);
      }
    };
    const fail = (error: unknown) => {
      if (request === navigationRequest.current) { setPreparing(false); pendingDestination.current = null; showToast(`无法打开：${String(error)}`); }
    };
    try {
      const prepared = prepareNavigation(normalizeRoute(target));
      if (prepared instanceof Promise) { setPreparing(true); void prepared.then(ready).catch(fail); }
      else ready(prepared);
    } catch (error) { fail(error); }
  }, []);

  const navigate = useCallback((target: Route) => {
    prepareDestination(target, r => {
      startTransition(() => {
        setNavigation(current => {
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
    });
  }, [prepareDestination, startTransition]);

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
      if (pendingDestination.current && isRemovedRoute(pendingDestination.current)) {
        ++navigationRequest.current; pendingDestination.current = null; setPreparing(false);
      }
      clearReadResources([...(targets.sessionIds ?? []), ...(targets.workstreamIds ?? []), ...(targets.projectIds ?? [])]);
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
    const index = navigation.index - 1;
    const target = index < 0 ? fallback : withoutAction(navigation.entries[index]);
    if (!target) { ++navigationRequest.current; pendingDestination.current = null; setPreparing(false); return; }
    prepareDestination(target, ready => startTransition(() => setNavigation(current => {
      if (current !== navigation) return current;
      if (index < 0) return { entries: [ready], index: 0 };
      const entries = current.entries.slice();
      entries[index] = ready;
      return { entries, index };
    })));
  }, [navigation, prepareDestination, startTransition]);

  const goForward = useCallback(() => {
    const index = navigation.index + 1;
    if (index >= navigation.entries.length) { ++navigationRequest.current; pendingDestination.current = null; setPreparing(false); return; }
    prepareDestination(withoutAction(navigation.entries[index]), ready => startTransition(() => setNavigation(current => {
      if (current !== navigation) return current;
      const entries = current.entries.slice();
      entries[index] = ready;
      return { entries, index };
    })));
  }, [navigation, prepareDestination, startTransition]);

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
    <div style={{ "--sidebar-width": `${sidebarWidth}px` } as CSSProperties} className={`app${isMac ? " mac-titlebar" : ""}${isWindows ? " windows-titlebar" : ""}${sidebarCollapsed ? " sidebar-collapsed without-session-sidebar" : ""}`}>
      {showNavigationProgress && preparing && <div className="navigation-progress" role="status" aria-label="正在打开页面" />}
      <header className="app-titlebar">
        {(isMac || isWindows) && <div className="window-drag-region" data-tauri-drag-region />}
        <div className="history-controls" aria-label="页面导航">
          <button
            className="history-button"
            aria-label="返回上一页"
            title="返回上一页"
            disabled={navigation.index === 0}
            onClick={() => goBack()}
          >
            <Icon name="arrowLeft" />
          </button>
          <button
            className="history-button"
            aria-label="前进到下一页"
            title="前进到下一页"
            disabled={navigation.index >= navigation.entries.length - 1}
            onClick={goForward}
          >
            <Icon name="arrowRight" />
          </button>
          <button
            className="history-button"
            aria-label="查找"
            title={isMac ? "查找 (⌘K)" : "查找 (Ctrl+K)"}
            onClick={() => setPaletteOpen(true)}
          >
            <Icon name="search" />
          </button>
          <PanelToggle side="left" open={!sidebarCollapsed} label="侧边栏"
            controls="session-sidebar" onToggle={() => setSidebarCollapsed(value => !value)} />
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
      {isWindows && <WindowsWindowControls />}
      <NavigationRail route={route} navigate={navigate} />
      <Sidebar
        route={route}
        navigate={navigate}
        collapsed={sidebarCollapsed}
      />
      {!sidebarCollapsed && <PanelResizer side="left" label="侧边栏宽度" controls="session-sidebar" value={sidebarWidth}
        min={200} max={360} defaultValue={248} onResize={resizeSidebar} className="sidebar-resizer" />}
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
