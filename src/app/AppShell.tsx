import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import Sidebar from "../layout/Sidebar";
import Router from "./Router";
import CommandPalette from "../components/CommandPalette";
import ToastHost from "../components/Toast";
import { LaunchDetailsHost } from "../features/launcher/LaunchResultModal";
import { EVT_SYNCED, type Route } from "./routes";
import { refreshBaseExperience } from "./experience";

/**
 * AppShell（实施方案 §4/§80）：只负责骨架 —— Sidebar、RouterOutlet、
 * CommandPalette、全局事件与 Toast。业务数据由各页面自己加载。
 */
export default function AppShell() {
  const [route, setRoute] = useState<Route>({ view: "home" });
  const [paletteOpen, setPaletteOpen] = useState(false);
  // 带 action 的导航每次都递增，页面据此响应「已在目标页」的重复命令
  const [actionSeq, setActionSeq] = useState(0);
  const seqRef = useRef(0);

  const navigate = useCallback((r: Route) => {
    setRoute(r);
    if ("action" in r && r.action) setActionSeq(++seqRef.current);
  }, []);

  // Base Experience 开关：整个应用读一次，判断集中在 experience.tsx（§11.9）。
  useEffect(() => {
    refreshBaseExperience();
  }, []);

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

  // Background sync / reconcile completion → 通知页面做定向刷新（§86），
  // 不做整页刷新：页面通过 window 事件自行 invalidate。
  useEffect(() => {
    const un1 = listen("sync-completed", () => emitSynced());
    const un2 = listen("reconcile-completed", () => emitSynced());
    // Projects Experience v0.2 §12 — 工作区刷新完成后同样扇出刷新信号，
    // Sidebar 最近列表等自行 invalidate。
    const un3 = listen("workspace-reconcile-completed", () => emitSynced());
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
    };
  }, []);

  return (
    <div className="app">
      <Sidebar route={route} navigate={navigate} onSearch={() => setPaletteOpen(true)} />
      <Router route={route} navigate={navigate} actionSeq={actionSeq} />
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
