import React, { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import Sidebar from "../layout/Sidebar";
import LazyRouter from "../LazyRouter";
import CommandPalette from "../components/CommandPalette";
import { EVT_SYNCED, type Route } from "./routes";

/**
 * AppShell（实施方案 §4/§80）：只负责骨架 —— Sidebar、RouterOutlet、
 * CommandPalette 与全局事件。业务数据由各页面自己加载。
 */
export default function AppShell() {
  const [route, setRoute] = useState<Route>({ view: "home" });
  const [paletteOpen, setPaletteOpen] = useState(false);

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
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, []);

  return (
    <div className="app">
      <Sidebar route={route} navigate={setRoute} onSearch={() => setPaletteOpen(true)} />
      <LazyRouter route={route} navigate={setRoute} />
      {paletteOpen && (
        <CommandPalette onClose={() => setPaletteOpen(false)} navigate={setRoute} />
      )}
    </div>
  );
}

function emitSynced() {
  window.dispatchEvent(new CustomEvent(EVT_SYNCED));
}
