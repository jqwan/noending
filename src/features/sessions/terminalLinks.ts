import type { Terminal } from "@xterm/xterm";
import { WebLinksAddon } from "@xterm/addon-web-links";

/** Ordinary web links and native OSC 8 links share the system-browser opener.
 * No editor references or workbench command URIs are interpreted here. */
export function connectTerminalLinks(term: Terminal, mac: boolean, open: (url: string) => Promise<void>): () => void {
  const activate = (event: MouseEvent, text: string) => {
    if (!(mac ? event.metaKey : event.ctrlKey)) return;
    let url: URL;
    try { url = new URL(text); } catch { return; }
    if (url.protocol !== "http:" && url.protocol !== "https:") return;
    event.preventDefault();
    void open(url.href).catch(() => {});
  };
  term.options.linkHandler = { activate, allowNonHttpProtocols: false };
  const addon = new WebLinksAddon(activate);
  term.loadAddon(addon);
  return () => addon.dispose();
}
