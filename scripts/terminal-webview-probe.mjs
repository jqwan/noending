// Run against an isolated `pnpm dev` + NoEnding debug instance started with
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9224.
// No database commands, Agent input, or remote URLs are used by this probe.
// Usage: node scripts/terminal-webview-probe.mjs [baseline|adapted] [port]
const mode = process.argv[2] ?? 'adapted';
if (!['baseline', 'adapted'].includes(mode)) throw new Error('Expected baseline or adapted');
const port = Number(process.argv[3] ?? 9224);
const targets = await (await fetch(`http://localhost:${port}/json/list`)).json();
const target = targets.find(t => t.type === 'page' && t.url.startsWith('http://localhost:1420/'));
if (!target) throw new Error('No local NoEnding development WebView2 target');
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
const expression = `(${async function probe(mode) {
  if (!window.__TAURI_INTERNALS__ || !/Windows/.test(navigator.userAgent)) throw new Error('Requires real Windows Tauri WebView2');
  const { Terminal } = await import('/node_modules/.vite/deps/@xterm_xterm.js');
  const { FitAddon } = await import('/node_modules/.vite/deps/@xterm_addon-fit.js');
  const { WebglAddon } = await import('/node_modules/.vite/deps/@xterm_addon-webgl.js');
  const { createTerminalOutput } = await import('/src/features/sessions/terminalOutput.ts');
  const { connectTerminalRenderer } = await import('/src/features/sessions/terminalRenderer.ts');
  const { connectTerminalViewport } = await import('/src/features/sessions/terminalViewport.ts');
  const { Unicode11Addon } = await import('/node_modules/.vite/deps/@xterm_addon-unicode11.js');
  const { observeTerminalTheme } = await import('/src/features/sessions/terminalTheme.ts');
  const hosts = [], terms = [], cleanups = [];
  const savedTheme = document.documentElement.dataset.theme;
  const adapted = mode === 'adapted';
  function terminal(hidden = false) {
    const host = document.createElement('div');
    hosts.push(host);
    Object.assign(host.style, { position: 'fixed', left: '400px', top: '120px', width: '850px', height: '550px', zIndex: '9999', visibility: hidden ? 'hidden' : 'visible' });
    document.body.append(host);
    const term = new Terminal({ allowProposedApi: true,
      fontFamily: adapted ? "Consolas, 'Courier New', monospace" : 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
      fontSize: adapted ? 14 : 12.5, scrollback: 5000, screenReaderMode: false,
      minimumContrastRatio: adapted ? 1 : 4.5 });
    terms.push(term);
    const fit = new FitAddon(); term.loadAddon(fit); term.open(host);
    if (adapted) {
      term.loadAddon(new Unicode11Addon()); term.unicode.activeVersion = '11';
      cleanups.push(connectTerminalRenderer(term));
      cleanups.push(connectTerminalViewport(term));
    } else { term.loadAddon(new WebglAddon()); }
    fit.fit();
    const output = createTerminalOutput(term);
    cleanups.push(() => output.dispose());
    return { term, host, output };
  }
  try {
    const foreground = terminal();
    const metadata = { mode, userAgent: navigator.userAgent, dpr: devicePixelRatio,
      font: foreground.term.options.fontFamily, fontSize: foreground.term.options.fontSize,
      cols: foreground.term.cols, rows: foreground.term.rows, unicode: foreground.term.unicode.activeVersion };
    const payload = new TextEncoder().encode(('0123456789 中文 output\r\n').repeat(160000));
    async function measure(target) {
      const latencies = []; let writes = 0;
      const originalWrite = target.term.write.bind(target.term);
      target.term.write = (bytes, done) => { writes++; originalWrite(bytes, done); };
      let lastBeat = performance.now(), maxEventLoopGap = 0;
      const beat = setInterval(() => { const now = performance.now(); maxEventLoopGap = Math.max(maxEventLoopGap, now - lastBeat); lastBeat = now; }, 4);
      const start = performance.now(); const tasks = [];
      for (let i = 0; i < payload.length; i += 4096) {
        const sent = performance.now();
        const bytes = payload.subarray(i, i + 4096);
        tasks.push(new Promise(resolve => {
          const done = () => { latencies.push(performance.now() - sent); resolve(); };
          if (adapted) target.output.write(bytes, done); else target.term.write(bytes, done);
        }));
      }
      await Promise.all(tasks);
      const ms = performance.now() - start;
      await new Promise(resolve => setTimeout(resolve, 8));
      clearInterval(beat); target.term.write = originalWrite;
      latencies.sort((a,b) => a-b);
      return { bytes: payload.length, writes, ms, p50: latencies[Math.floor(latencies.length * .5)], p95: latencies[Math.floor(latencies.length * .95)], maxEventLoopGap };
    }
    metadata.foreground = await measure(foreground);
    const background = terminal(true);
    metadata.twoTerminals = await Promise.all([measure(foreground), measure(background)]);
    // Real GPU context-loss extension; buffer and parser must survive fallback.
    const gl = [...foreground.term.element.querySelectorAll('.xterm-screen canvas')].map(canvas => canvas.getContext('webgl2')).find(Boolean);
    const loss = gl?.getExtension('WEBGL_lose_context');
    if (loss) {
      // addon-webgl 0.19 allows 3 seconds for native context restoration.
      loss.loseContext(); await new Promise(resolve => setTimeout(resolve, 3300));
      await new Promise(resolve => foreground.term.write('\r\nRECOVERY中文😀', resolve));
      metadata.contextLoss = { domRenderer: !!foreground.term.element.querySelector('.xterm-rows'), lastLine: foreground.term.buffer.active.getLine(foreground.term.buffer.active.baseY + foreground.term.buffer.active.cursorY)?.translateToString(true) };
    } else { metadata.contextLoss = { unsupported: true }; }
    if (adapted) {
      const replies = [];
      foreground.term.onData(data => replies.push(data));
      cleanups.push(observeTerminalTheme(foreground.term, () => {}, true));
      await new Promise(resolve => foreground.term.write('\x1b]10;rgb:12/34/56\x07\x1b]11;rgb:22/33/44\x07', resolve));
      document.documentElement.dataset.theme = savedTheme === 'dark' ? 'light' : 'dark';
      await new Promise(resolve => setTimeout(resolve, 0));
      await new Promise(resolve => foreground.term.write('\x1b]10;?\x07\x1b]11;?\x07', resolve));
      metadata.agentColorReplies = replies;
    }
    return metadata;
  } finally {
    if (savedTheme === undefined) delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = savedTheme;
    cleanups.reverse().forEach(stop => stop()); terms.forEach(term => term.dispose()); hosts.forEach(host => host.remove());
  }
}})(${JSON.stringify(mode)})`;
try {
  const result = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('WebView2 probe timed out')), 45000);
    ws.onmessage = event => { const value = JSON.parse(event.data); if (value.id === 1) { clearTimeout(timer); resolve(value); } };
    ws.send(JSON.stringify({id: 1, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true }}));
  });
  if (result.error || result.result.exceptionDetails) throw new Error(JSON.stringify(result));
  console.log(JSON.stringify(result.result.result.value, null, 2));
} finally { ws.close(); }
