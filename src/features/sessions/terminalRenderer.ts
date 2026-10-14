import type { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";

/** On loss, dispose the addon to reinstall xterm's DOM renderer. Retrying a
 * broken GPU repeatedly can lose more contexts across the cached terminals. */
export function connectTerminalRenderer(term: Terminal): () => void {
  const addon = new WebglAddon();
  let stopped = false;
  const loss = addon.onContextLoss(() => {
    stop();
    term.refresh(0, term.rows - 1);
    term.element?.dispatchEvent(new Event("terminal-renderer-changed"));
  });
  const stop = () => {
    if (stopped) return;
    stopped = true;
    loss.dispose();
    addon.dispose();
  };
  try { term.loadAddon(addon); }
  catch (error) {
    stop();
    console.warn("Terminal WebGL unavailable; using DOM renderer", error);
  }
  return stop;
}
