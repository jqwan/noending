import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import Router from "./Router";
import { api } from "../api";
import type { Route } from "./routes";

// Router 的三分支（概览 / 对话 / 终端）此前没有渲染级覆盖：切换器测试只断言
// navigate 被调用，这里证明 entry=terminal 真的渲染出终端子页而不是回落到详情页。
vi.mock("../api", () => ({
  api: {
    getAgentStatus: vi.fn().mockResolvedValue({}),
    terminalForSession: vi.fn().mockResolvedValue(null),
    launchEmbeddedResume: vi.fn().mockRejectedValue(new Error("no terminal")),
    getSessionMessages: vi.fn().mockResolvedValue({ messages: [], total: 0, tail_ordinal: 0, remaining: 0 }),
    getSessionDetail: vi.fn().mockResolvedValue({
      session: { agent: "codex", source_kind: "codex", trashed_at: null, title: "T" },
      messages: [],
      owner_workstream: null,
      workspace_path: null,
      source_status: "available",
      can_resume: true,
    }),
    getSessionUserMessageMarks: vi.fn().mockResolvedValue([]),
    getSessionContext: vi.fn().mockResolvedValue({ session_id: "", fields: null, revision: 0, ingest_generation: 0, processed_through_seq: 0, latest_message_seq: 0, updated_at: null, pending: false }),
    updateSessionContext: vi.fn(),
    listProjects: vi.fn().mockResolvedValue([]),
    listWorkstreams: vi.fn().mockResolvedValue([]),
    getWorkspaceSettings: vi.fn().mockResolvedValue({ default_workspace: "" }),
    revealSessionSource: vi.fn(),
    trashSession: vi.fn(),
    restoreSession: vi.fn(),
    getSessionLocalDeletePreview: vi.fn(),
    permanentlyDeleteSession: vi.fn(),
    refreshSession: vi.fn().mockResolvedValue({ queued: true }),
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@xterm/xterm", () => ({ Terminal: class { loadAddon() {} open() {} write() {} onData() { return { dispose() {} }; } attachCustomKeyEventHandler() {} dispose() {} } }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: class {} }));

afterEach(() => cleanup());

it("renders the terminal subpage for entry=terminal, not the detail page", async () => {
  const route: Route = { view: "session", sessionId: "s1", entry: "terminal", initialTitle: "T", initialAgent: "codex" };
  render(<Router route={route} navigate={vi.fn()} goBack={vi.fn()} actionSeq={0} />);
  // 终端子页的标志性文案（直启失败的错误态带重试），详情页没有这段；
  // 「进入即直启」由 SessionTerminalView 的专属测试覆盖。
  expect(await screen.findByText(/内嵌终端不可用/)).toBeTruthy();
  expect(screen.getByText("重试")).toBeTruthy();
});

it("renders the detail page when entry is absent", () => {
  vi.mocked(api.getSessionDetail).mockReturnValue(new Promise(() => {}) as never);
  const route: Route = { view: "session", sessionId: "s1", initialTitle: "T", initialAgent: "codex" };
  const { container } = render(<Router route={route} navigate={vi.fn()} goBack={vi.fn()} actionSeq={0} />);
  // 详情页的骨架（ rail-section / 所属任务 区块标签）在加载中也会出现 section-label。
  expect(container.querySelector(".session-detail")).toBeTruthy();
});
