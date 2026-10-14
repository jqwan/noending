// Read-only diagnostic for a focused Pi/Agent terminal in an isolated real
// Windows NoEnding dev WebView2. No Agent input, clipboard or DB writes.
// Requires WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9224.
const port = Number(process.argv[2] ?? 9224);
const targets = await (await fetch(`http://localhost:${port}/json/list`)).json();
const target = targets.find(t => t.type === 'page' && t.url.startsWith('http://localhost:1420/'));
if (!target) throw new Error('Requires the local NoEnding development WebView2');
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
const expression = `(${async function () {
  if (!window.__TAURI_INTERNALS__ || !/Windows/.test(navigator.userAgent)) throw new Error('Requires real Windows Tauri WebView2');
  const { liveTerminals } = await import('/src/features/sessions/terminalCache.ts');
  const { api } = await import('/src/api.ts');
  const entry = [...liveTerminals].find(([, e]) => e.term.textarea === document.activeElement && !e.exited);
  if (!entry) throw new Error('Focus a live test Agent terminal first');
  const [id, { term }] = entry;
  let native;
  const queryMs = [];
  for (let i = 0; i < 5; i++) { const start = performance.now(); native = await api.terminalInputCursor(id); queryMs.push(performance.now() - start); }
  if (!native) throw new Error('Native cursor unavailable');
  const textarea = term.textarea.getBoundingClientRect();
  const screen = term.element.querySelector('.xterm-screen').getBoundingClientRect();
  const scroll = [];
  for (let e = term.element; e; e = e.parentElement) if (e.scrollLeft || e.scrollTop) scroll.push({ element: e.className, left: e.scrollLeft, top: e.scrollTop });
  return { native, queryMs, xterm: { x: term.buffer.active.cursorX, y: term.buffer.active.cursorY, cols: term.cols, rows: term.rows },
    buffer: term.buffer.active.type, mouse: term.modes.mouseTrackingMode, composing: !!term.element.querySelector('.composition-view.active'),
    textarea: { left: textarea.left - screen.left, top: textarea.top - screen.top, height: textarea.height }, scroll };
}.toString()})()`;
try {
  const result = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('Cursor probe timed out')), 10000);
    ws.onmessage = e => { const data = JSON.parse(e.data); if (data.id === 1) { clearTimeout(timeout); resolve(data); } };
    ws.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true } }));
  });
  if (result.error || result.result.exceptionDetails) throw new Error(JSON.stringify(result));
  console.log(JSON.stringify(result.result.result.value, null, 2));
} finally { ws.close(); }
