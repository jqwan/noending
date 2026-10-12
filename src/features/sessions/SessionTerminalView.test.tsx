import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import SessionTerminalView, { resetLiveTerminalsForTests } from "./SessionTerminalView";
import { disposeTerminal, liveTerminals } from "./terminalCache";
import { api } from "../../api";
import { sessionPages, sessionTerminals } from "./sessionNavigation";
import type { TerminalBound, TerminalSnapshot } from "../../types";
import { listen } from "@tauri-apps/api/event";

// 终端子页的交互面全部在后端；这里覆盖进入即用的状态机：
// 直接 attach / 失败可重试 / 身份切换与 pending / 退出横幅。
vi.mock("../../api", () => ({
  api: {
    terminalList: vi.fn().mockResolvedValue([]),
    getAgentStatus: vi.fn().mockResolvedValue({ codex: { terminal_cli: true } }),
    getSessionDetail: vi.fn().mockResolvedValue({
      session: { agent: "codex", source_kind: "codex", archived_at: null, title: "会话" },
      can_resume: true,
    }),
    terminalForSession: vi.fn(),
    terminalAttach: vi.fn(),
    terminalInput: vi.fn().mockResolvedValue(undefined),
    terminalReconnect: vi.fn(),
    terminalClose: vi.fn(),
    terminalResize: vi.fn().mockResolvedValue(undefined),
    launchEmbeddedResume: vi.fn(),
    readClipboardForTerminal: vi.fn(),
  },
}));
const { eventHandlers } = vi.hoisted(() => ({
  eventHandlers: new Map<string, (e: unknown) => void>(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((event: string, handler: (e: unknown) => void) => {
    eventHandlers.set(event, handler);
    return Promise.resolve(() => eventHandlers.delete(event));
  }),
}));
const { readTextMock, copyMock } = vi.hoisted(() => ({ readTextMock: vi.fn(), copyMock: vi.fn() }));
vi.mock("../../components/common", () => ({ copyToClipboard: copyMock }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  readText: readTextMock,
  writeText: vi.fn().mockResolvedValue(undefined),
}));

const { FakeTerminal } = vi.hoisted(() => {
  class FakeTerminal {
    static last: FakeTerminal | null = null;
    written: (string | Uint8Array)[] = [];
    handlers: ((data: string) => void)[] = [];
    keyHandler: ((e: unknown) => boolean) | null = null;
    pasted: string[] = [];
    disposed = false;
    cols = 80;
    rows = 24;
    modes = { mouseTrackingMode: "none" };
    buffer = { active: { type: "normal" } };
    options: import("@xterm/xterm").ITerminalOptions = {};
    parser = {
      registerCsiHandler: () => ({ dispose() {} }),
      registerEscHandler: () => ({ dispose() {} }),
    };
    constructor(_opts?: unknown) {
      FakeTerminal.last = this;
    }
    loadAddon() {}
    open(container: HTMLElement) {
      container.appendChild(this.element);
    }
    write(data: string | Uint8Array) {
      this.written.push(data);
    }
    onData(handler: (data: string) => void) {
      this.handlers.push(handler);
      return { dispose: () => {} };
    }
    dataHandler(data: string) {
      this.handlers.forEach((h) => h(data));
    }
    attachCustomKeyEventHandler(handler: (e: unknown) => boolean) {
      FakeTerminal.last!.keyHandler = handler;
    }
    paste(text: string) {
      this.pasted.push(text);
    }
    hasSelection() {
      return false;
    }
    getSelection() {
      return "";
    }
    inputHandler: ((e: unknown) => void) | null = null;
    compositionStartHandler: (() => void) | null = null;
    compositionEndHandler: (() => void) | null = null;
    pasteHandler: ((e: unknown) => void) | null = null;
    textarea = {
      value: "",
      addEventListener: (type: string, handler: (e: unknown) => void) => {
        if (type === "paste") FakeTerminal.last!.pasteHandler = handler;
        if (type === "input") FakeTerminal.last!.inputHandler = handler;
        if (type === "compositionstart") FakeTerminal.last!.compositionStartHandler = handler as () => void;
        if (type === "compositionend") FakeTerminal.last!.compositionEndHandler = handler as () => void;
      },
    };
    capturePasteHandler: ((e: unknown) => void) | null = null;
    captureInputHandler: ((e: unknown) => void) | null = null;
    element: HTMLElement = (() => {
      const el = document.createElement("div");
      const original = el.addEventListener.bind(el);
      el.addEventListener = ((type: string, handler: EventListenerOrEventListenerObject, opts?: boolean | AddEventListenerOptions) => {
        if (type === "paste") FakeTerminal.last!.capturePasteHandler = handler as (e: unknown) => void;
        if (type === "input") FakeTerminal.last!.captureInputHandler = handler as (e: unknown) => void;
        return original(type, handler, opts);
      }) as typeof el.addEventListener;
      return el;
    })();
    dispose() {
      this.disposed = true;
    }
  }
  return { FakeTerminal };
});
vi.mock("@xterm/xterm", () => ({ Terminal: FakeTerminal }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: class {} }));

// jsdom 没有 ResizeObserver。
class FakeResizeObserver {
  static observed: { target: HTMLElement; notify: () => void }[] = [];
  constructor(private notify: () => void) {}
  observe(target: HTMLElement) { FakeResizeObserver.observed.push({ target, notify: this.notify }); }
  disconnect() {}
  unobserve() {}
}
(globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeResizeObserver;

const { toastMock } = vi.hoisted(() => ({ toastMock: vi.fn() }));
vi.mock("../../components/Toast", () => ({ showToast: toastMock }));

const reconnected = { terminal_id: "t-new", launched_via: "embedded", command_line: "codex resume", note: "", };
beforeEach(() => {
  sessionPages.clear(); sessionTerminals.clear();
  vi.mocked(api.terminalList).mockReset().mockResolvedValue([]);
  vi.mocked(api.terminalClose).mockReset();
  vi.mocked(api.terminalReconnect).mockReset().mockResolvedValue(reconnected);
  vi.mocked(api.getSessionDetail).mockReset().mockResolvedValue(detail("会话"));
});

afterEach(() => {
  cleanup();
  resetLiveTerminalsForTests();
  vi.clearAllMocks();
  FakeTerminal.last = null;
  FakeResizeObserver.observed = [];
  delete document.documentElement.dataset.theme;
});

function snapshot(over: Partial<TerminalSnapshot>): TerminalSnapshot {
  return {
    terminal_id: "t-1",
    session_id: "s1",
    initial_workstream_ids: [],
    identity_revision: 1,
    agent: "codex",
    cwd: "/tmp/work",
    created_at: "2026-10-06T00:00:00Z",
    live: true,
    exit_code: null,
    session_title: null,
    scrollback: "",
    cols: 80,
    rows: 24,
    ...over,
  };
}

function detail(title: string): Awaited<ReturnType<typeof api.getSessionDetail>> {
  return { session: { agent: "codex", title }, can_resume: true } as never;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

function pushIdentity(sessionId: string | null, revision: number, terminalId = "t-1") {
  const payload: TerminalBound = { terminal_id: terminalId, session_id: sessionId, identity_revision: revision };
  act(() => { eventHandlers.get("terminal-bound")?.({ payload }); });
}

function renderView(navigate?: ReturnType<typeof vi.fn>) {
  return render(
    <SessionTerminalView terminalId="t-1" navigate={navigate ?? vi.fn()} />,
  );
}

it("preserves PTY dimensions while hidden and resizes again when shown", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
  renderView();
  await waitFor(() => expect(FakeResizeObserver.observed).toHaveLength(1));
  const { target, notify } = FakeResizeObserver.observed[0];
  let visible = false;
  Object.defineProperties(target, {
    clientWidth: { get: () => visible ? 800 : 0 },
    clientHeight: { get: () => visible ? 600 : 0 },
  });
  act(notify);
  expect(api.terminalResize).not.toHaveBeenCalled();
  visible = true;
  act(notify);
  expect(api.terminalResize).toHaveBeenCalledWith("t-1", 80, 24);
});

it("updates a detached cached terminal's theme without restarting or replaying it", async () => {
  document.documentElement.dataset.theme = "light";
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ scrollback: btoa("saved output") }));
  const view = renderView();
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  const terminal = FakeTerminal.last!;
  expect(terminal.options.theme?.background).toBe("#ffffff");
  view.unmount();
  document.documentElement.dataset.theme = "dark";
  await waitFor(() => expect(terminal.options.theme?.background).toBe("#141413"));
  renderView();
  await screen.findByLabelText("重新连接");
  expect(FakeTerminal.last).toBe(terminal);
  expect(terminal.written).toHaveLength(1);
  expect(terminal.disposed).toBe(false);
  expect(api.terminalInput).not.toHaveBeenCalled();
  expect(api.terminalReconnect).not.toHaveBeenCalled();
});

it("attaches by terminal ID, replays scrollback and forwards input", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ scrollback: btoa("hello-pty") }));

  renderView();
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  expect(
    new TextDecoder().decode(FakeTerminal.last!.written[0] as Uint8Array),
  ).toBe("hello-pty");

  FakeTerminal.last!.handlers.forEach((h) => h("q"));
  await waitFor(() => expect(api.terminalInput).toHaveBeenCalledWith("t-1", "q"));
});

it("an attach failure shows the message with a retry", async () => {
  vi.mocked(api.terminalAttach).mockReset().mockRejectedValueOnce(new Error("终端不存在或已随应用重启失效")).mockResolvedValue(snapshot({ scrollback: btoa("retried output") }));

  renderView();
  await screen.findByText(/内嵌终端不可用/);
  expect(screen.getByText(/终端不存在或已随应用重启失效/)).toBeTruthy();
  fireEvent.click(screen.getByText("重试"));
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  expect(api.terminalAttach).toHaveBeenCalledTimes(2);
  expect(new TextDecoder().decode(FakeTerminal.last!.written[0] as Uint8Array)).toBe("retried output");
  expect(screen.queryByText(/内嵌终端不可用/)).toBeNull();
});

it("an unbound terminal keeps the session-detail entry disabled; binding lights it up", async () => {
  const navigate = vi.fn();
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ session_id: null }));
  vi.mocked(api.getSessionDetail).mockResolvedValue({
    session: { agent: "codex", source_kind: "codex", archived_at: null, title: "页面匹配的会话" },
    messages: [],
    tracked_workstreams: [],
    workspace_path: null,
    source_status: "available",
    can_resume: true,
  } as never);

  renderView(navigate);
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  const entry = screen.getByRole("button", { name: "会话详情" }) as HTMLButtonElement;
  expect(entry.disabled).toBe(true);
  expect(screen.getByRole("heading", { name: "新会话" })).toBeTruthy();

  // 注册表 bind() 的广播：事件一到入口即亮，标题换成会话本名（无轮询）。
  pushIdentity("s9", 2);
  await waitFor(() => expect(entry.disabled).toBe(false));
  await waitFor(() => expect(screen.getByText("页面匹配的会话")).toBeTruthy());
  fireEvent.click(entry);
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "session", sessionId: "s9", initialTitle: "页面匹配的会话", initialAgent: "codex" }));
});

it("reconnects an exited terminal and navigates to the new PTY", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ live: false, exit_code: 0 }));
  const navigate = vi.fn();
  renderView(navigate);
  await screen.findByText(/Agent 已退出/);
  const old = FakeTerminal.last!;
  fireEvent.click(screen.getByRole("button", { name: "重新连接" }));
  await waitFor(() => expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "terminal", terminalId: "t-new", initialSessionId: "s1", initialAgent: "codex" })));
  expect(api.terminalReconnect).toHaveBeenCalledWith("t-1");
  expect(old.disposed).toBe(true);
});

it("retains an exited terminal across page changes until manual removal disposes its cache", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ live: false, exit_code: 0 }));
  const view = renderView();
  await screen.findByText(/Agent 已退出/);
  const old = FakeTerminal.last!;
  view.unmount();
  expect(old.disposed).toBe(false);
  renderView();
  await screen.findByText(/Agent 已退出/);
  expect(FakeTerminal.last).toBe(old);
  disposeTerminal("t-1");
  await act(async () => {});
  expect(old.disposed).toBe(true);
  expect(liveTerminals.has("t-1")).toBe(false);
  expect(eventHandlers.has("terminal-output://t-1")).toBe(false);
  expect(eventHandlers.has("terminal-exit://t-1")).toBe(false);
});

it("handles reconnect lifecycle: prevents double clicks, recovers on failure, and navigates on success", async () => {
  const pending = deferred<Awaited<ReturnType<typeof api.terminalReconnect>>>();
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
  vi.mocked(api.terminalReconnect).mockReturnValueOnce(pending.promise).mockResolvedValueOnce(reconnected);
  const navigate = vi.fn();
  renderView(navigate);
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  const old = FakeTerminal.last!;

  // 1. Initial click enters busy state and prevents double clicks
  fireEvent.click(screen.getByRole("button", { name: "重新连接" }));
  const busy = screen.getByRole("button", { name: "正在重新连接" }) as HTMLButtonElement;
  expect(busy.disabled).toBe(true);
  fireEvent.click(busy);
  expect(api.terminalReconnect).toHaveBeenCalledTimes(1);

  // 2. Failure retains terminal output and keeps button retryable
  await act(async () => pending.reject(new Error("spawn failed")));
  await waitFor(() => expect(toastMock).toHaveBeenCalledWith(expect.stringContaining("spawn failed")));
  expect(navigate).not.toHaveBeenCalled();
  expect(old.disposed).toBe(false);

  // 3. Retry succeeds and navigates
  const retryBtn = screen.getByRole("button", { name: "重新连接" }) as HTMLButtonElement;
  expect(retryBtn.disabled).toBe(false);
  fireEvent.click(retryBtn);
  await waitFor(() => expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "terminal", terminalId: "t-new", initialSessionId: "s1", initialAgent: "codex" })));
});

it("disables reconnect until the terminal has a verified Session", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ session_id: null }));
  renderView();
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  const reconnect = screen.getByRole("button", { name: "重新连接" }) as HTMLButtonElement;
  expect(reconnect.disabled).toBe(true);
  fireEvent.click(reconnect);
  expect(api.terminalReconnect).not.toHaveBeenCalled();
  await act(async () => pushIdentity("session-b", 2));
  expect(reconnect.disabled).toBe(false);
});

it("does not navigate away from another page if reconnection finishes after unmount", async () => {
  const pending = deferred<Awaited<ReturnType<typeof api.terminalReconnect>>>();
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
  vi.mocked(api.terminalReconnect).mockReturnValue(pending.promise);
  const navigate = vi.fn();
  const view = renderView(navigate);
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  fireEvent.click(screen.getByRole("button", { name: "重新连接" }));
  view.unmount();
  await act(async () => pending.resolve(reconnected));
  expect(navigate).not.toHaveBeenCalled();
});

it("the route seed renders the final header on the first frame — no 新会话 flash", async () => {
  // attach 故意挂起不返回：种子必须独立于它成立（会话页跳转的防闪契约）。
  const attach = deferred<TerminalSnapshot>();
  vi.mocked(api.terminalAttach).mockReturnValue(attach.promise);
  vi.mocked(api.getSessionDetail).mockReturnValue(new Promise(() => {}));
  const navigate = vi.fn();

  render(
    <SessionTerminalView
      terminalId="t-1"
      initialTitle="修复布局"
      initialAgent="codex"
      initialSessionId="s1"
      navigate={navigate}
    />,
  );
  // 同步首帧：标题就是会话本名，入口已可点。
  expect(screen.getByText("修复布局")).toBeTruthy();
  const entry = screen.getByRole("button", { name: "会话详情" }) as HTMLButtonElement;
  expect(entry.disabled).toBe(false);
  expect(screen.queryByText("新会话")).toBeNull();
  await act(async () => attach.resolve(snapshot({ session_id: "s1", session_title: null })));
  expect(screen.getByRole("heading").textContent).toBe("修复布局");
});

it("a bound snapshot shows the session-detail entry enabled from the start", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ session_id: "s1" }));
  vi.mocked(api.getSessionDetail).mockResolvedValue({
    session: { agent: "codex", source_kind: "codex", archived_at: null, title: "已有身份" },
    messages: [],
    tracked_workstreams: [],
    workspace_path: null,
    source_status: "available",
    can_resume: true,
  } as never);

  renderView();
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  const entry = screen.getByRole("button", { name: "会话详情" }) as HTMLButtonElement;
  expect(entry.disabled).toBe(false);
  await waitFor(() => expect(screen.getByText("已有身份")).toBeTruthy());
});

it("waits for identity subscription before attaching", async () => {
  const subscription = deferred<() => void>();
  vi.mocked(listen).mockReturnValueOnce(subscription.promise);
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ session_title: "当前会话" }));
  renderView();
  expect(listen).toHaveBeenCalledWith("terminal-bound", expect.any(Function));
  expect(api.terminalAttach).not.toHaveBeenCalled();

  await act(async () => { subscription.resolve(() => {}); });
  await waitFor(() => expect(api.terminalAttach).toHaveBeenCalledWith("t-1"));
});

it("follows A to B without rebuilding the terminal and ignores old or duplicate events", async () => {
  const navigate = vi.fn();
  const pendingB = deferred<Awaited<ReturnType<typeof api.getSessionDetail>>>();
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ session_id: "a", session_title: "任务 A" }));
  vi.mocked(api.getSessionDetail).mockReturnValue(pendingB.promise);
  renderView(navigate);
  await screen.findByText("任务 A");
  const term = FakeTerminal.last!;
  const writes = term.written.length;

  pushIdentity("b", 2);
  expect(screen.queryByText("任务 A")).toBeNull();
  expect(screen.getByRole("heading").textContent).toBe("");
  fireEvent.click(screen.getByRole("button", { name: "会话详情" }));
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "session", sessionId: "b", initialAgent: "codex" }));
  await act(async () => { pendingB.resolve(detail("任务 B")); });
  expect(screen.getByText("任务 B")).toBeTruthy();

  pushIdentity("a", 1);
  pushIdentity("b", 2);
  pushIdentity("other", 99, "another-terminal");
  expect(screen.getByText("任务 B")).toBeTruthy();
  expect(FakeTerminal.last).toBe(term);
  expect(term.written.length).toBe(writes);
  expect(api.terminalAttach).toHaveBeenCalledTimes(1);
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
});

it("clears the old title and detail target while the new native session is pending", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ session_id: "a", session_title: "旧会话" }));
  const navigate = vi.fn();
  renderView(navigate);
  await screen.findByText("旧会话");
  const term = FakeTerminal.last!;

  pushIdentity(null, 2);
  expect(screen.queryByText("旧会话")).toBeNull();
  expect(screen.getByRole("heading", { name: "新会话" })).toBeTruthy();
  const entry = screen.getByRole("button", { name: "会话详情" }) as HTMLButtonElement;
  expect(entry.disabled).toBe(true);
  fireEvent.click(entry);
  expect(navigate).not.toHaveBeenCalled();
  pushIdentity("a", 1);
  expect(entry.disabled).toBe(true);
  expect(FakeTerminal.last).toBe(term);

  pushIdentity("b", 3);
  await waitFor(() => expect(entry.disabled).toBe(false));
  fireEvent.click(entry);
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "session", sessionId: "b", initialAgent: "codex" }));
});

it("does not let a late attach snapshot replace a newer pending identity", async () => {
  const attach = deferred<TerminalSnapshot>();
  vi.mocked(api.terminalAttach).mockReturnValue(attach.promise);
  render(<SessionTerminalView terminalId="t-1" initialTitle="路由旧标题" initialSessionId="a" navigate={vi.fn()} />);
  await waitFor(() => expect(api.terminalAttach).toHaveBeenCalled());
  pushIdentity(null, 3);
  expect(screen.queryByText("路由旧标题")).toBeNull();

  await act(async () => { attach.resolve(snapshot({ session_id: "a", identity_revision: 2, session_title: "旧快照" })); });
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  expect(screen.queryByText("旧快照")).toBeNull();
  expect((screen.getByRole("button", { name: "会话详情" }) as HTMLButtonElement).disabled).toBe(true);
  expect(api.getSessionDetail).not.toHaveBeenCalled();
});

it("authoritative unbound snapshots replace route seeds", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ session_id: null, identity_revision: 0 }));
  render(<SessionTerminalView terminalId="t-1" initialTitle="路由旧标题" initialSessionId="a" navigate={vi.fn()} />);
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  expect(screen.queryByText("路由旧标题")).toBeNull();
  expect((screen.getByRole("button", { name: "会话详情" }) as HTMLButtonElement).disabled).toBe(true);
});

it("ignores stale detail responses even after A to B to A", async () => {
  const oldA = deferred<Awaited<ReturnType<typeof api.getSessionDetail>>>();
  const oldB = deferred<Awaited<ReturnType<typeof api.getSessionDetail>>>();
  const currentA = deferred<Awaited<ReturnType<typeof api.getSessionDetail>>>();
  vi.mocked(api.getSessionDetail).mockReturnValueOnce(oldA.promise).mockReturnValueOnce(oldB.promise).mockReturnValueOnce(currentA.promise);
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ session_id: "a" }));
  renderView();
  await waitFor(() => expect(api.getSessionDetail).toHaveBeenCalledWith("a"));
  pushIdentity("b", 2);
  await waitFor(() => expect(api.getSessionDetail).toHaveBeenCalledWith("b"));
  pushIdentity("a", 3);
  await waitFor(() => expect(api.getSessionDetail).toHaveBeenCalledTimes(3));
  await act(async () => { currentA.resolve(detail("当前 A")); });
  expect(screen.getByText("当前 A")).toBeTruthy();
  await act(async () => { oldA.resolve(detail("过时 A")); oldB.resolve(detail("过时 B")); });
  expect(screen.getByText("当前 A")).toBeTruthy();
  expect(screen.queryByText("过时 A")).toBeNull();
  expect(screen.queryByText("过时 B")).toBeNull();
});

it("keeps details disabled when an old title request resolves during pending", async () => {
  const oldDetail = deferred<Awaited<ReturnType<typeof api.getSessionDetail>>>();
  vi.mocked(api.getSessionDetail).mockReturnValue(oldDetail.promise);
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ session_id: "a" }));
  renderView();
  await waitFor(() => expect(api.getSessionDetail).toHaveBeenCalledWith("a"));
  pushIdentity(null, 2);
  await act(async () => { oldDetail.resolve(detail("迟到的旧标题")); });
  expect(screen.queryByText("迟到的旧标题")).toBeNull();
  expect(screen.getByRole("heading", { name: "新会话" })).toBeTruthy();
  expect((screen.getByRole("button", { name: "会话详情" }) as HTMLButtonElement).disabled).toBe(true);
});
it("delivers clipboard text once on supported paste shortcuts", async () => {
  const pasteModifiers = [
    { metaKey: true, ctrlKey: false, shiftKey: false },
    { metaKey: false, ctrlKey: true, shiftKey: true },
  ];

  for (const modifiers of pasteModifiers) {
    vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
    vi.mocked(api.readClipboardForTerminal).mockResolvedValue("pasted text");
    vi.mocked(api.terminalInput).mockClear();

    const view = renderView();
    await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
    const term = FakeTerminal.last!;
    expect(term.keyHandler!({ type: "keydown", ...modifiers, key: "v", preventDefault: () => {} })).toBe(false);
    await waitFor(() => expect(api.terminalInput).toHaveBeenCalledWith("t-1", "pasted text"));
    expect(api.terminalInput).toHaveBeenCalledTimes(1);
    expect(term.pasted).toEqual([]);
    view.unmount();
  }
});

it("does not receive input when Cmd+V finds no clipboard text", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ agent: "codex" }));
  vi.mocked(api.readClipboardForTerminal).mockResolvedValue(null);

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  await act(async () => {
    expect(FakeTerminal.last!.keyHandler!({
      type: "keydown", metaKey: true, key: "v", preventDefault: () => {},
    })).toBe(false);
  });
  expect(api.readClipboardForTerminal).toHaveBeenCalledTimes(1);
  expect(api.terminalInput).not.toHaveBeenCalled();
});

it("the browser default paste is cancelled and event pastes die at capture", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
  vi.mocked(api.readClipboardForTerminal).mockResolvedValue("x");

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  const prevented = vi.fn();
  // 返回 false 之外必须真的 cancelDefault，否则原生粘贴照跑（双投递的根）。
  expect(
    FakeTerminal.last!.keyHandler!({ type: "keydown", metaKey: true, key: "v", preventDefault: prevented }),
  ).toBe(false);
  expect(prevented).toHaveBeenCalledTimes(1);

  // 捕获阶段终结一切事件粘贴：菜单/右键/默认动作残余都到不了 xterm。
  expect(FakeTerminal.last!.capturePasteHandler).not.toBeNull();
  const stopped = vi.fn();
  FakeTerminal.last!.capturePasteHandler!({
    preventDefault: stopped,
    stopPropagation: () => {},
  });
  expect(stopped).toHaveBeenCalledTimes(1);
});

it("passes native TUI paste shortcuts through to xterm", async () => {
  const shortcutModifiers = [
    { ctrlKey: true, altKey: false },
    { ctrlKey: false, altKey: true },
  ];

  for (const modifiers of shortcutModifiers) {
    vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
    vi.mocked(api.readClipboardForTerminal).mockClear();

    const view = renderView();
    await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
    const prevented = vi.fn();
    expect(FakeTerminal.last!.keyHandler!({
      type: "keydown", metaKey: false, shiftKey: false, key: "v",
      ...modifiers, preventDefault: prevented,
    })).toBe(true);
    expect(prevented).not.toHaveBeenCalled();
    expect(api.readClipboardForTerminal).not.toHaveBeenCalled();
    view.unmount();
  }
});

it("copy with a selection goes through the shared clipboard helper", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));

  copyMock.mockResolvedValue(true);
  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  const term = FakeTerminal.last!;
  term.hasSelection = () => true;
  term.getSelection = () => "selected";
  term.keyHandler!({ type: "keydown", metaKey: true, key: "c" });
  await waitFor(() => expect(copyMock).toHaveBeenCalledWith("selected"));
});

it("remounting reuses the live instance instead of rebuilding and replaying", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ scrollback: btoa("first") }));

  const view = renderView();
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  const first = FakeTerminal.last!;
  const firstWrites = first.written.length;
  const element = first.element;
  expect(element.isConnected).toBe(true);

  // 切走（unmount）→ 切回（remount）：实例与 DOM 节点存活。
  view.unmount();
  expect(element.isConnected).toBe(false);
  const second = renderView();
  await waitFor(() => expect(element.isConnected).toBe(true));
  // 同一个实例：没有第二个 Terminal 被构造。
  expect(FakeTerminal.last).toBe(first);
  // 不重放 scrollback：回放正是跨尺寸渲染错乱的根源。
  expect(first.written.length).toBe(firstWrites);
  second.unmount();
});

const tick = () => new Promise((r) => setTimeout(r, 5));

it("IME direct-commit punctuation is delivered from the input payload", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  const term = FakeTerminal.last!;
  // 229 keydown（IME 接管）必须被挡在 xterm 之外（防它的 preventDefault 吞键）。
  expect(term.keyHandler!({ type: "keydown", keyCode: 229, preventDefault: () => {} })).toBe(false);
  // 首键插入——载荷是权威文本；即使 textarea 被清空/不落盘（WebKit 首次
  // 直提交的怪癖，差分永远读空），载荷路径也立即交付。
  term.keyHandler!({ type: "keyup", keyCode: 229 });
  term.captureInputHandler!({ inputType: "insertText", data: "？", stopImmediatePropagation: () => {} });
  expect(api.terminalInput).toHaveBeenCalledWith("t-1", "？");
});

it("first direct-commit arms on the bare Shift keydown (probe log sequence)", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  const term = FakeTerminal.last!;
  // 探针 A/B 实测的首键序列：WebKit 吞掉字符键的 keydown，229 到第二键才
  // 出现，唯一前置是裸 Shift keydown——窗口必须由它武装，否则首键必丢。
  expect(
    term.keyHandler!({ type: "keydown", keyCode: 16, key: "Shift", preventDefault: () => {} }),
  ).toBe(true);
  term.captureInputHandler!({ inputType: "insertText", data: "？", stopImmediatePropagation: () => {} });
  await tick();
  await tick();
  const delivered = vi.mocked(api.terminalInput).mock.calls.filter((c) => c[1] === "？");
  expect(delivered).toHaveLength(1);
});

it("armed input events die at capture so xterm's input path never fires", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  const term = FakeTerminal.last!;
  // _keyDownSeen 被上一键 keyup 清零后 xterm 的 _inputEvent 会同步交付同一份
  // ev.data——武装窗口内必须在捕获层终结事件，杜绝它的那份。
  const stopped = vi.fn();
  term.keyHandler!({ type: "keydown", keyCode: 229, preventDefault: () => {} });
  term.captureInputHandler!({ inputType: "insertText", data: "？", stopImmediatePropagation: stopped });
  expect(stopped).toHaveBeenCalledTimes(1);
  // 窗口外放行：普通输入不经捕获终结（xterm 原路径负责）。
  const untouched = vi.fn();
  term.keyHandler!({ type: "keydown", keyCode: 65, preventDefault: () => {} });
  term.captureInputHandler!({ inputType: "insertText", data: "a", stopImmediatePropagation: untouched });
  expect(untouched).not.toHaveBeenCalled();
});

it("a re-dispatched identical payload for one insertion is delivered once", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  const term = FakeTerminal.last!;
  term.keyHandler!({ type: "keydown", keyCode: 229, preventDefault: () => {} });
  // 第一次派发：插入落地（textarea 变为 "？"），交付一次。
  term.textarea.value = "？";
  term.captureInputHandler!({ inputType: "insertText", data: "？", stopImmediatePropagation: () => {} });
  expect(api.terminalInput).toHaveBeenCalledWith("t-1", "？");
  // 按住修饰键连打时 WebKit 会对同一次插入重复派发：载荷相同且 textarea
  // 未再变化 → 不再交付。
  term.captureInputHandler!({ inputType: "insertText", data: "？", stopImmediatePropagation: () => {} });
  // 两份之间隔着 229 keydown 也不放行：值未落新内容，鉴别基线跨窗口保留。
  term.keyHandler!({ type: "keydown", keyCode: 229, preventDefault: () => {} });
  term.captureInputHandler!({ inputType: "insertText", data: "？", stopImmediatePropagation: () => {} });
  await tick();
  await tick();
  expect(
    vi.mocked(api.terminalInput).mock.calls.filter((c) => c[1] === "？"),
  ).toHaveLength(1);
  // 落上新内容后的同载荷是下一次真实按键：正常交付。
  term.textarea.value = "？？";
  term.captureInputHandler!({ inputType: "insertText", data: "？", stopImmediatePropagation: () => {} });
  await tick();
  expect(
    vi.mocked(api.terminalInput).mock.calls.filter((c) => c[1] === "？"),
  ).toHaveLength(2);
});

it("the diff shim deducts what xterm already delivered — no double send", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  const term = FakeTerminal.last!;
  term.keyHandler!({ type: "keydown", keyCode: 229, preventDefault: () => {} });
  term.compositionStartHandler!();
  term.compositionEndHandler!();
  // 组合提交的字符 xterm 已经发过（onData 记账；经输入通道进 PTY 是合法路径）：
  term.dataHandler("你");
  term.textarea.value = "你";
  term.captureInputHandler!({ inputType: "insertText", data: "你", stopImmediatePropagation: () => {} });
  await tick();
  await tick();
  // 差分层必须扣掉 xterm 已发的部分——「你」恰好交付一次，不能双发。
  const delivered = vi.mocked(api.terminalInput).mock.calls.filter((c) => c[1] === "你");
  expect(delivered).toHaveLength(1);
});

it("the diff shim ignores non-IME typing and mid-composition states", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  const term = FakeTerminal.last!;
  // 普通按键（非 229）不武装：输入事件归 xterm 的 keypress 通路。
  term.keyHandler!({ type: "keydown", keyCode: 65, preventDefault: () => {} });
  term.textarea.value = "a";
  term.captureInputHandler!({ inputType: "insertText", data: "a", stopImmediatePropagation: () => {} });
  await tick();
  expect(api.terminalInput).not.toHaveBeenCalledWith("t-1", "a");
  // 候选窗中间态（composition 未结束）不投递。
  term.keyHandler!({ type: "keydown", keyCode: 229, preventDefault: () => {} });
  term.compositionStartHandler!();
  term.textarea.value = "ni";
  term.captureInputHandler!({ inputType: "insertCompositionText", data: "ni", stopImmediatePropagation: () => {} });
  await tick();
  expect(api.terminalInput).not.toHaveBeenCalledWith("t-1", "ni");
});

it("shows the exit banner with the exit code for an exited terminal", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(
    snapshot({ live: false, exit_code: 7, scrollback: btoa("done") }),
  );
  renderView();
  await screen.findByText(/Agent 已退出/);
  expect(screen.getByText(/退出码 7/)).toBeTruthy();
});

it("attaching by terminal id never launches and never navigates away on bind", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ terminal_id: "t-free", session_id: null }));
  const navigate = vi.fn();

  render(<SessionTerminalView terminalId="t-free" navigate={navigate} />);
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  // 本视图只 attach：launch 是会话页终端入口的职责。
  expect(api.launchEmbeddedResume).not.toHaveBeenCalled();
  expect(api.terminalForSession).not.toHaveBeenCalled();

  // 绑定事实到达：视图留在原地，只点亮入口（旧的"replace 跳子页"已退场）。
  pushIdentity("s-bound", 2, "t-free");
  await act(async () => { await tick(); });
  expect(navigate).not.toHaveBeenCalled();
});

it("closes only the selected terminal and switches to another live instance", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
  vi.mocked(api.terminalClose).mockResolvedValue(snapshot({ session_id: "s1" }));
  vi.mocked(api.terminalList).mockResolvedValue([
    snapshot({ terminal_id: "t-dead", live: false }), snapshot({ terminal_id: "t-live" }),
  ]);
  const navigate = vi.fn();
  renderView(navigate);
  const button = await screen.findByRole("button", { name: "关闭终端" });
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  const current = FakeTerminal.last!;
  fireEvent.click(button);
  await waitFor(() => expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "terminal", terminalId: "t-live" })));
  expect(api.terminalClose).toHaveBeenCalledTimes(1);
  expect(api.terminalClose).toHaveBeenCalledWith("t-1");
  expect(current.disposed).toBe(true);
});

it("uses the close result's verified identity for the final destination", async () => {
  const cases = [
    { sessionId: "new-native-session", expected: { view: "session", sessionId: "new-native-session" } },
    { sessionId: null, expected: { view: "sessions" } },
  ];

  for (const { sessionId, expected } of cases) {
    vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ live: false }));
    vi.mocked(api.terminalClose).mockResolvedValue(snapshot({ session_id: sessionId }));
    const navigate = vi.fn();
    const view = renderView(navigate);
    fireEvent.click(await screen.findByRole("button", { name: "移除终端" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith(expected));
    view.unmount();
  }
});

it("does not interrupt the new page when an old terminal close finishes after leaving", async () => {
  const close = deferred<TerminalSnapshot>();
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
  vi.mocked(api.terminalClose).mockReturnValue(close.promise);
  const navigate = vi.fn();
  const view = renderView(navigate);
  const button = await screen.findByRole("button", { name: "关闭终端" });
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(button);
  view.unmount();
  await act(async () => { close.resolve(snapshot({})); });
  expect(navigate).not.toHaveBeenCalled();
  expect(liveTerminals.has("t-1")).toBe(false);
});

it("keeps a terminal usable when closing fails", async () => {
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
  vi.mocked(api.terminalClose).mockRejectedValue(new Error("close failed"));
  const navigate = vi.fn();
  renderView(navigate);
  const button = await screen.findByRole("button", { name: "关闭终端" });
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  const current = FakeTerminal.last!;
  fireEvent.click(button);
  await waitFor(() => expect(toastMock).toHaveBeenCalledWith(expect.stringContaining("close failed")));
  expect(current.disposed).toBe(false);
  expect(navigate).not.toHaveBeenCalled();
});
