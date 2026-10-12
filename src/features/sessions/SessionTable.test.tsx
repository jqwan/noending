import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import SessionCards from "./SessionTable";
import type { Session, TerminalSummary } from "../../types";
import { viewState } from "../../hooks/useViewState";
vi.mock("../../api", () => ({
  api: {
    getAgentStatus: vi.fn().mockResolvedValue({ codex: { desktop_app: "ChatGPT", desktop_app_present: true } }),
    continueSessionDesktop: vi.fn().mockResolvedValue({ uri: "x://y", note: "已打开" }),
  },
}));
beforeEach(() => viewState.clear());
afterEach(() => {
  cleanup();
  localStorage.removeItem("noending.continue_mode");
});
async function renderCards(element: Parameters<typeof render>[0]) {
  let view!: ReturnType<typeof render>;
  await act(async () => { view = render(element); });
  return view;
}
const session = { id: "s1", title: "修复布局", agent: "codex", cwd: "/test/project", tracked_workstream_ids: [], started_at: "2026-09-21T00:00:00Z" } as unknown as Session;
it("opens details separately from resume and trash actions", async () => {
  const open = vi.fn(); const resume = vi.fn(); const trash = vi.fn();
  const terminalResume = vi.fn();
  await renderCards(<SessionCards sessions={[session]} workstreamTitleById={new Map()} projectNameById={new Map()} onOpen={open} onResume={resume} onTerminalResume={terminalResume} onArchive={trash} />);
  // Codex supports both routes; the single action prefers terminal.
  const continueBtn = screen.getByLabelText("在终端中继续修复布局");
  fireEvent.click(continueBtn);
  expect(terminalResume).toHaveBeenCalledWith(session);
  expect(resume).not.toHaveBeenCalled();
  fireEvent.click(screen.getByLabelText("将修复布局归档"));
  expect(trash).toHaveBeenCalledWith("s1");
  expect(open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByText("修复布局"));
  expect(open).toHaveBeenCalledWith("s1");
});
it("limits initial rows and reveals more without losing sessions", async () => {
  await renderCards(<SessionCards sessions={Array.from({ length: 101 }, (_, i) => ({ ...session, id: String(i), title: `会话${i}` }))} workstreamTitleById={new Map()} projectNameById={new Map()} onOpen={() => {}} onResume={() => {}} onArchive={() => {}} />);
  expect(screen.queryByText("会话100")).toBeNull();
  fireEvent.click(screen.getByText("显示更多（剩余 1）"));
  expect(screen.getByText("会话100")).toBeTruthy();
});
it("shows the single owner workstream and omits unowned workstream row", async () => {
  await renderCards(<SessionCards
    sessions={[
      { ...session, id: "owned", title: "有归属", tracked_workstream_ids: ["w1"] },
      { ...session, id: "free", title: "没归属", tracked_workstream_ids: [] },
    ]}
    workstreamTitleById={new Map([["w1", "会话重构"]])}
    projectNameById={new Map()}
    onOpen={() => {}} onResume={() => {}} onArchive={() => {}} />);

  // 有归属会话显示任务标题，没归属会话不显示未归属任务
  screen.getByText("会话重构");
  expect(screen.queryByText("未归属任务")).toBeNull();
  expect(screen.queryByText(/\+\d/)).toBeNull();
});

it("displays full workstream title without truncating to 16 characters in JS", async () => {
  const longTitle = "这是一个超过十六个字符的非常长的任务标题名称";
  await renderCards(<SessionCards
    sessions={[
      { ...session, id: "s-long-ws", title: "长任务会话", tracked_workstream_ids: ["w-long"] },
    ]}
    workstreamTitleById={new Map([["w-long", longTitle]])}
    projectNameById={new Map()}
    onOpen={() => {}} onResume={() => {}} onArchive={() => {}} />);

  expect(screen.getByText(longTitle)).toBeTruthy();
});


it("prefers terminal capability regardless of old preferences, and uses desktop for desktop-only formats", async () => {
  const onTerminalResume = vi.fn();
  localStorage.setItem("noending.continue_mode", "desktop");
  const onResume = vi.fn();
  const { rerender } = await renderCards(
    <SessionCards
      sessions={[session]}
      workstreamTitleById={new Map()}
      projectNameById={new Map()}
      onOpen={() => {}}
      onResume={() => {}}
      onArchive={() => {}}
      onTerminalResume={onTerminalResume}
    />
  );
  // A previously saved desktop preference does not hide the terminal route.
  const terminalBtn = screen.getByLabelText("在终端中继续修复布局");
  fireEvent.click(terminalBtn);
  expect(onTerminalResume).toHaveBeenCalledWith(session);

  // Terminal-only formats follow the same rule.
  localStorage.setItem("noending.continue_mode", "desktop");
  const claudeSession = { ...session, id: "claude-1", agent: "claude_code" } as unknown as Session;
  rerender(
    <SessionCards
      sessions={[claudeSession]}
      workstreamTitleById={new Map()}
      projectNameById={new Map()}
      onOpen={() => {}}
      onResume={() => {}}
      onArchive={() => {}}
      onTerminalResume={onTerminalResume}
    />
  );
  expect(screen.getByLabelText("在终端中继续修复布局")).toBeTruthy();
  const desktopSession = { ...session, id: "desktop", agent: "workbuddy" } as unknown as Session;
  rerender(<SessionCards sessions={[desktopSession]} workstreamTitleById={new Map()} projectNameById={new Map()}
    onOpen={() => {}} onResume={onResume} onArchive={() => {}} onTerminalResume={onTerminalResume} />);
  fireEvent.click(screen.getByRole("button", { name: "在桌面应用中继续修复布局" }));
  expect(onResume).toHaveBeenCalledWith("desktop");
  expect(screen.queryByRole("button", { name: "在终端中继续修复布局" })).toBeNull();
  localStorage.removeItem("noending.continue_mode");
});

it("disables archived session continuation in both card and list terminal modes", async () => {
  localStorage.setItem("noending.continue_mode", "terminal");
  const resume = vi.fn();
  const terminalResume = vi.fn();
  const archived = { ...session, archived_at: "2026-10-08T00:00:00Z" };
  const props = {
    sessions: [archived], workstreamTitleById: new Map(), projectNameById: new Map(),
    onOpen: vi.fn(), onResume: resume, onArchive: vi.fn(), onTerminalResume: terminalResume,
  };
  let rerender!: ReturnType<typeof render>["rerender"];
  await act(async () => { ({ rerender } = render(<SessionCards {...props} />)); });
  const assertBlocked = () => {
    const button = screen.getByRole("button", { name: "在终端中继续修复布局" });
    expect((button as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(button);
    expect(terminalResume).not.toHaveBeenCalled();
    expect(resume).not.toHaveBeenCalled();
  };
  assertBlocked();
  rerender(<SessionCards {...props} viewMode="list" />);
  assertBlocked();
  localStorage.removeItem("noending.continue_mode");
});

it("displays running terminal state on the continue button when session has a live terminal", async () => {
  const sLive = { ...session, id: "s-live", title: "活跃终端会话" };
  const sExited = { ...session, id: "s-exited", title: "退出终端会话" };
  const terminals: TerminalSummary[] = [
    { terminal_id: "t1", session_id: "s-live", live: true, agent: "codex", cwd: "/test", created_at: "", exit_code: null, session_title: "活跃终端会话", initial_workstream_ids: [], identity_revision: 0 },
    { terminal_id: "t2", session_id: "s-exited", live: false, agent: "codex", cwd: "/test", created_at: "", exit_code: 0, session_title: "退出终端会话", initial_workstream_ids: [], identity_revision: 0 },
  ];
  await renderCards(
    <SessionCards
      sessions={[sLive, sExited]}
      terminals={terminals}
      workstreamTitleById={new Map()}
      projectNameById={new Map()}
      onOpen={vi.fn()}
      onResume={vi.fn()}
      onArchive={vi.fn()}
      onTerminalResume={vi.fn()}
    />
  );
  const liveBtn = screen.getByRole("button", { name: "在终端中继续活跃终端会话" });
  expect(liveBtn.className).toContain("terminal-running");
  expect(liveBtn.title).toContain("切回终端 · 运行中");

  const exitedBtn = screen.getByRole("button", { name: "在终端中继续退出终端会话" });
  expect(exitedBtn.className).toContain("terminal-exited");
  expect(exitedBtn.title).toBe("唤起终端");
});
