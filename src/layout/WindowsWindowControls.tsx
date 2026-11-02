import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { showToast } from "../components/Toast";

/** Windows chrome shares the existing page header; macOS keeps its traffic lights. */
export default function WindowsWindowControls() {
  const [maximized, setMaximized] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const [focused, setFocused] = useState(true);

  useEffect(() => {
    const window = getCurrentWindow();
    let disposed = false;
    let revision = 0;
    const unlisten: (() => void)[] = [];
    const refresh = async () => {
      const request = ++revision;
      const [maximized, fullscreen] = await Promise.all([window.isMaximized(), window.isFullscreen()]);
      if (!disposed && request === revision) {
        setMaximized(maximized);
        setFullscreen(fullscreen);
      }
    };
    const report = (error: unknown) => console.warn("[window] Unable to read window state", error);
    const subscribe = (subscription: Promise<() => void>) => {
      void subscription.then(stop => disposed ? stop() : unlisten.push(stop)).catch(report);
    };
    subscribe(window.onResized(() => { void refresh().catch(report); }));
    subscribe(window.onFocusChanged(({ payload }) => { if (!disposed) setFocused(payload); }));
    void refresh().catch(report);
    return () => {
      disposed = true;
      ++revision;
      unlisten.forEach(stop => stop());
    };
  }, []);

  const act = (action: "minimize" | "toggleMaximize" | "close") => {
    void getCurrentWindow()[action]().catch(error => showToast(`窗口操作失败：${String(error)}`));
  };

  return <div className="windows-window-controls" role="group" aria-label="窗口控制"
    data-focused={focused} hidden={fullscreen} data-tauri-drag-region="false">
    <button type="button" aria-label="最小化窗口" title="最小化" onClick={() => act("minimize")}>
      <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M1 6.5h10" /></svg>
    </button>
    <button type="button" aria-label={maximized ? "还原窗口" : "最大化窗口"}
      title={maximized ? "还原" : "最大化"} onClick={() => act("toggleMaximize")}>
      <svg viewBox="0 0 12 12" aria-hidden="true">
        {maximized ? <path d="M3.5 3.5v-2h7v7h-2m-7-5h7v7h-7z" /> : <path d="M1.5 1.5h9v9h-9z" />}
      </svg>
    </button>
    <button type="button" className="window-close" aria-label="关闭窗口" title="关闭 (Alt+F4)" onClick={() => act("close")}>
      <svg viewBox="0 0 12 12" aria-hidden="true"><path d="m1.5 1.5 9 9m0-9-9 9" /></svg>
    </button>
  </div>;
}
