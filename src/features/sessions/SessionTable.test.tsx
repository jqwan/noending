import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import SessionCards from "./SessionTable";
import type { Session } from "../../types";
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
const session = { id: "s1", title: "修复布局", agent: "codex", cwd: "/test/project", started_at: "2026-09-21T00:00:00Z" } as Session;
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
it("shows the single owner workstream and marks the unowned case", async () => {
  await renderCards(<SessionCards
    sessions={[
      { ...session, id: "owned", title: "有归属", owner_workstream_id: "w1" },
      { ...session, id: "free", title: "没归属", owner_workstream_id: null },
    ]}
    workstreamTitleById={new Map([["w1", "会话重构"]])}
    projectNameById={new Map()}
    onOpen={() => {}} onResume={() => {}} onArchive={() => {}} />);

  // 一行最多一个任务：标题只有一个，没有「+N」这种多任务计数。
  screen.getByText("会话重构");
  screen.getByText("未归属任务");
  expect(screen.queryByText(/\+\d/)).toBeNull();
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
  const claudeSession = { ...session, id: "claude-1", agent: "claude_code" } as Session;
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
  const desktopSession = { ...session, id: "desktop", agent: "workbuddy" } as Session;
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
