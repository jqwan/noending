import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionTerminalView, { resetLiveTerminalsForTests } from "./SessionTerminalView";
import { api } from "../../api";
import type { TerminalSnapshot } from "../../types";

// 终端子页的交互面全部在后端；这里覆盖进入即用的状态机：
// 已有终端直接 attach / 没有则自动直启 / 失败可重试 / 退出横幅。
vi.mock("../../api", () => ({
  api: {
    getAgentStatus: vi.fn().mockResolvedValue({ codex: { terminal_cli: true } }),
    terminalForSession: vi.fn(),
    terminalAttach: vi.fn(),
    terminalInput: vi.fn().mockResolvedValue(undefined),
    terminalResize: vi.fn().mockResolvedValue(undefined),
    launchEmbeddedResume: vi.fn(),
    readClipboardForTerminal: vi.fn(),
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
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
  observe() {}
  disconnect() {}
  unobserve() {}
}
(globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeResizeObserver;

afterEach(() => {
  cleanup();
  resetLiveTerminalsForTests();
  vi.clearAllMocks();
  FakeTerminal.last = null;
});

function snapshot(over: Partial<TerminalSnapshot>): TerminalSnapshot {
  return {
    terminal_id: "t-1",
    session_id: "s1",
    agent: "codex",
    cwd: "/tmp/work",
    created_at: "2026-10-06T00:00:00Z",
    live: true,
    exit_code: null,
    scrollback: "",
    cols: 80,
    rows: 24,
    ...over,
  };
}

function renderView() {
  return render(
    <SessionTerminalView
      sessionId="s1"
      initialTitle="会话"
      initialAgent="codex"
      navigate={vi.fn()}
    />,
  );
}

it("attaches directly when the session already has an embedded terminal", async () => {
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
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

it("auto-launches an embedded resume when none exists, then attaches", async () => {
  vi.mocked(api.terminalForSession)
    .mockResolvedValueOnce(null)
    .mockResolvedValueOnce(snapshot({ terminal_id: "t-2" }));
  vi.mocked(api.launchEmbeddedResume).mockResolvedValue({
    launched_via: "内嵌终端",
    command_line: "pi --session x",
    note: "已在内嵌终端恢复该会话。",
    launch_intent_id: null,
    terminal_id: "t-2",
  });
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ terminal_id: "t-2" }));

  renderView();
  await waitFor(() => expect(api.launchEmbeddedResume).toHaveBeenCalledWith("s1"));
  await waitFor(() => expect(FakeTerminal.last).not.toBeNull());
  expect(api.terminalAttach).toHaveBeenCalledWith("t-2");
});

it("shows the stated reason and a retry when the direct launch fails", async () => {
  vi.mocked(api.terminalForSession).mockResolvedValue(null);
  vi.mocked(api.launchEmbeddedResume).mockRejectedValue(new Error("源会话已不存在，无法继续"));

  renderView();
  await screen.findByText(/内嵌终端不可用/);
  expect(screen.getByText(/源会话已不存在/)).toBeTruthy();
  expect(screen.getByText("重试")).toBeTruthy();
});

it("paste keys intercept native event and deliver text as typed keystrokes", async () => {
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
  vi.mocked(api.readClipboardForTerminal).mockResolvedValue({ text: "pasted text", image_path: null });

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  const term = FakeTerminal.last!;
  // 返回 false：拦截键盘事件，交给 readClipboardForTerminal；文本按打字语义
  // 经 terminalInput 交付（四家 TUI 渲染的最小公约数，term.paste 是 pi 双份
  // 的嫌疑残留层，已整体弃用）。
  expect(term.keyHandler!({ type: "keydown", metaKey: true, key: "v", preventDefault: () => {} })).toBe(false);
  await waitFor(() => expect(api.terminalInput).toHaveBeenCalledWith("t-1", "pasted text"));
  expect(term.pasted).toEqual([]);
});

it("an image paste forwards ^V to claude code instead of a path", async () => {
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
  // agent 取自快照（权威）：即使组件的 agent props 为空也能正确分流。
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({ agent: "claude_code" }));
  vi.mocked(api.readClipboardForTerminal).mockResolvedValue({
    text: null,
    image_path: "/tmp/noending/paste/clipboard-1.png",
  });

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  FakeTerminal.last!.keyHandler!({ type: "keydown", metaKey: true, key: "v", preventDefault: () => {} });
  await waitFor(() => expect(api.terminalInput).toHaveBeenCalledWith("t-1", "\x16"));
  // claude 自己读剪贴板出单 chip；再交付路径文本会双显。
  expect(api.terminalInput).not.toHaveBeenCalledWith(
    "t-1",
    "/tmp/noending/paste/clipboard-1.png",
  );
});

it("the browser default paste is cancelled and event pastes die at capture", async () => {
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
  vi.mocked(api.readClipboardForTerminal).mockResolvedValue({ text: "x", image_path: null });

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

it("an image-only paste path is delivered as typed text", async () => {
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
  vi.mocked(api.terminalAttach).mockResolvedValue(snapshot({}));
  vi.mocked(api.readClipboardForTerminal).mockResolvedValue({
    text: null,
    image_path: "/tmp/noending/paste/clipboard-1.png",
  });

  renderView();
  await waitFor(() => expect(FakeTerminal.last!.keyHandler).not.toBeNull());
  const term = FakeTerminal.last!;

  expect(term.keyHandler!({ type: "keydown", metaKey: true, key: "v", preventDefault: () => {} })).toBe(false);
  await waitFor(() =>
    expect(api.terminalInput).toHaveBeenCalledWith("t-1", "/tmp/noending/paste/clipboard-1.png"),
  );
  expect(term.pasted).toEqual([]);
});

it("copy with a selection goes through the shared clipboard helper", async () => {
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
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
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
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
  view.rerender = undefined as never; // noop guard (rerender not used)
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
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
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
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
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
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
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
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
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
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
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
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
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
  vi.mocked(api.terminalForSession).mockResolvedValue(snapshot({}));
  vi.mocked(api.terminalAttach).mockResolvedValue(
    snapshot({ live: false, exit_code: 7, scrollback: btoa("done") }),
  );
  renderView();
  await screen.findByText(/Agent 已退出/);
  expect(screen.getByText(/退出码 7/)).toBeTruthy();
  expect(screen.getByText("再次启动")).toBeTruthy();
});
