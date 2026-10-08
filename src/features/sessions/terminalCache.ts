import type { Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";

type LiveTerminal = {
  term: Terminal;
  fit: FitAddon;
  unlisteners: (() => void)[];
  exited: boolean;
  onExit: (() => void) | null;
};

/** 按 terminal_id 常驻，切页只移走 DOM，保留输出、尺寸和监听。
 *  返回时搬回原节点，避免重放不同尺寸的 PTY 字节流破坏 TUI 布局。
 *  已退出终端也保留供查看；手动移除和重连成功时显式释放。 */
export const liveTerminals = new Map<string, LiveTerminal>();

export function disposeTerminal(terminalId: string): void {
  const entry = liveTerminals.get(terminalId);
  if (!entry) return;
  liveTerminals.delete(terminalId);
  entry.onExit = null;
  entry.unlisteners.forEach((stop) => stop());
  entry.term.dispose();
}
