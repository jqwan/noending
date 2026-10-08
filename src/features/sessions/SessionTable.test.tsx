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
afterEach(cleanup);
const session = { id: "s1", title: "修复布局", agent: "codex", cwd: "/test/project", started_at: "2026-09-21T00:00:00Z" } as Session;
it("opens details separately from resume and trash actions", () => {
  const open = vi.fn(); const resume = vi.fn(); const trash = vi.fn();
  render(<SessionCards sessions={[session]} workstreamTitleById={new Map()} projectNameById={new Map()} onOpen={open} onResume={resume} onArchive={trash} />);
  // 行内「继续」= 图标按钮（与聚合按钮默认设置一致）；统一显示桌面图标，可用性由格式能力 + 桌面端在场决定。
  const continueBtn = screen.getByLabelText("在桌面应用中继续修复布局");
  expect(continueBtn.querySelector(".ui-icon")).toBeTruthy();
  expect(continueBtn.querySelector(".agent-icon")).toBeNull();
  fireEvent.click(continueBtn);
  expect(resume).toHaveBeenCalledWith("s1");
  fireEvent.click(screen.getByLabelText("将修复布局归档"));
  expect(trash).toHaveBeenCalledWith("s1");
  expect(open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByText("修复布局"));
  expect(open).toHaveBeenCalledWith("s1");
});
it("limits initial rows and reveals more without losing sessions", () => {
  render(<SessionCards sessions={Array.from({ length: 101 }, (_, i) => ({ ...session, id: String(i), title: `会话${i}` }))} workstreamTitleById={new Map()} projectNameById={new Map()} onOpen={() => {}} onResume={() => {}} onArchive={() => {}} />);
  expect(screen.queryByText("会话100")).toBeNull();
  fireEvent.click(screen.getByText("显示更多（剩余 1）"));
  expect(screen.getByText("会话100")).toBeTruthy();
});
it("shows the single owner workstream and marks the unowned case", () => {
  render(<SessionCards
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

it("renders agent icon before session title and plain agent label in meta", () => {
  render(
    <SessionCards
      sessions={[session]}
      workstreamTitleById={new Map()}
      projectNameById={new Map()}
      onOpen={() => {}}
      onResume={() => {}}
      onArchive={() => {}}
    />
  );

  const titleRow = document.querySelector(".session-list-title");
  expect(titleRow).toBeTruthy();
  expect(titleRow?.querySelector(".agent-icon")).toBeTruthy();
  expect(titleRow?.textContent).toContain("修复布局");

  const metaRow = document.querySelector(".session-list-meta");
  expect(metaRow).toBeTruthy();
  expect(metaRow?.textContent).toContain("Codex");
  expect(metaRow?.querySelector(".agent-icon")).toBeNull();
});

it("renders card view by default and list view when viewMode is list", () => {
  const { rerender } = render(
    <SessionCards
      sessions={[session]}
      workstreamTitleById={new Map()}
      projectNameById={new Map()}
      onOpen={() => {}}
      onResume={() => {}}
      onArchive={() => {}}
    />
  );

  // 默认是卡片模式
  expect(document.querySelector(".session-card")).toBeTruthy();
  expect(document.querySelector(".session-list-row")).toBeNull();

  // 显式指定列表模式
  rerender(
    <SessionCards
      sessions={[session]}
      workstreamTitleById={new Map()}
      projectNameById={new Map()}
      viewMode="list"
      onOpen={() => {}}
      onResume={() => {}}
      onArchive={() => {}}
    />
  );
  expect(document.querySelector(".session-card")).toBeNull();
  expect(document.querySelector(".session-list-row")).toBeTruthy();
});

it("shows terminal continue button when continue mode is terminal or format is terminal only", () => {
  const onTerminalResume = vi.fn();
  localStorage.setItem("noending.continue_mode", "terminal");
  const { rerender } = render(
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
  // codex 在 terminal 模式下展示终端继续按钮
  const terminalBtn = screen.getByLabelText("在终端中继续修复布局");
  expect(terminalBtn.querySelector(".ui-icon")).toBeTruthy();
  fireEvent.click(terminalBtn);
  expect(onTerminalResume).toHaveBeenCalledWith(session);

  // 切回 desktop 模式，但 claude_code 属于仅终端格式，仍然展示终端继续按钮
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
