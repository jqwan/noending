import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "../api";
import Sidebar from "../layout/Sidebar";
import Router from "./Router";
import CommandPalette from "../components/CommandPalette";
import ToastHost from "../components/Toast";
import { LaunchDetailsHost } from "../features/launcher/LaunchResultModal";
import { EVT_SYNCED, type Route } from "./routes";

type NavigationState = { entries: Route[]; index: number };

function withoutAction(route: Route): Route {
  if (!("action" in route) || !route.action) return route;
  const { action: _action, ...base } = route;
  return base as Route;
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
  return JSON.stringify(left) === JSON.stringify(right);
}

/**
 * AppShell：只负责骨架 —— Sidebar、RouterOutlet、
 * CommandPalette、全局事件与 Toast。业务数据由各页面自己加载。
 */
export default function AppShell() {
  const [navigation, setNavigation] = useState<NavigationState>({
    entries: [{ view: "home" }],
    index: 0,
  });
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
  const route = navigation.entries[navigation.index];
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

  const navigate = useCallback((r: Route) => {
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
  }, []);

  const goBack = useCallback((fallback?: Route) => {
    setNavigation((current) => {
      if (current.index === 0) {
        return fallback ? { entries: [fallback], index: 0 } : current;
      }
      const entries = current.entries.slice();
      entries[current.index - 1] = withoutAction(entries[current.index - 1]);
      return { entries, index: current.index - 1 };
    });
  }, []);

  const goForward = useCallback(() => {
    setNavigation((current) => {
      if (current.index >= current.entries.length - 1) return current;
      const entries = current.entries.slice();
      entries[current.index + 1] = withoutAction(entries[current.index + 1]);
      return { entries, index: current.index + 1 };
    });
  }, []);

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

  // ⌘K / Ctrl+K opens the command palette
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
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, []);

  return (
    <div style={{ "--sidebar-width": `${sidebarWidth}px` } as CSSProperties} className={`app${isMac ? " mac-titlebar" : ""}${sidebarCollapsed ? " sidebar-collapsed" : ""}`}>
      <header className="app-titlebar">
        {isMac && <div className="window-drag-region" data-tauri-drag-region />}
        <div className="history-controls" aria-label="页面导航">
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
          <button
            className="history-button"
            aria-label="返回上一页"
            title="返回上一页（⌥← / ⌘[）"
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
            title="前进到下一页（⌥→ / ⌘]）"
            disabled={navigation.index >= navigation.entries.length - 1}
            onClick={goForward}
          >
            <svg className="history-icon" viewBox="0 0 18 18" aria-hidden="true">
              <path d="M4 9h10M9 4l5 5-5 5" />
            </svg>
          </button>
        </div>
      </header>
      <Sidebar
        route={route}
        navigate={navigate}
        onSearch={() => setPaletteOpen(true)}
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
            resizeSidebar(e.clientX - e.currentTarget.parentElement!.getBoundingClientRect().left);
          }
        }}
        onPointerUp={(e) => {
          if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
        }}
      />}
      <div className="app-content">
        <Router route={route} navigate={navigate} goBack={goBack} actionSeq={actionSeq} />
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
