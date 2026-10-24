import type { Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";
import type { createTerminalRenderer } from "./terminalPresentation";
import type { TerminalNotifications } from "../../types";

type LiveTerminal = {
  term: Terminal;
  fit: FitAddon;
  renderer: ReturnType<typeof createTerminalRenderer>;
  dimensions: { cols: number; rows: number };
  unlisteners: (() => void)[];
  exited: boolean;
  managed?: boolean;
  manualCursorInactiveStyle?: Terminal["options"]["cursorInactiveStyle"];
  inputRevision?: number;
  inputReady?: boolean;
  inputReadOnly?: boolean;
  onManagedFocus?: (() => void) | null;
  onExit: (() => void) | null;
};

/** 按 terminal_id 常驻，切页只移走 DOM，保留输出、尺寸和监听。
 *  返回时搬回原节点，避免重放不同尺寸的 PTY 字节流破坏 TUI 布局。
 *  已退出终端也保留供查看；手动移除和重连成功时显式释放。 */
export const liveTerminals = new Map<string, LiveTerminal>();

/** Preserve terminal protocol replies; disableStdin would also suppress them. */
export function applyNotificationInputOwnership(terminalId: string, notification: TerminalNotifications) {
  const live = liveTerminals.get(terminalId);
  if (!live || notification.input_revision < (live.inputRevision ?? -1)) return;
  const readOnly = notification.enabled || notification.writing || live.exited;
  if (live.inputReadOnly !== readOnly) {
    if (readOnly) live.manualCursorInactiveStyle = live.term.options.cursorInactiveStyle;
    live.term.options.cursorInactiveStyle = readOnly ? "none" : live.manualCursorInactiveStyle ?? "outline";
  }
  live.inputReadOnly = readOnly;
  live.managed = notification.enabled || notification.writing;
  live.inputRevision = notification.input_revision;
  live.inputReady = true;
  if (live.term.textarea) live.term.textarea.readOnly = readOnly;
}

export function disposeTerminal(terminalId: string): void {
  const entry = liveTerminals.get(terminalId);
  if (!entry) return;
  liveTerminals.delete(terminalId);
  entry.onExit = null;
  entry.unlisteners.forEach((stop) => stop());
  entry.term.dispose();
}
