import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import Router from "./Router";
import { api } from "../api";
import type { Route } from "./routes";
import type { Agent } from "../types";

vi.mock("../features/sessions/NewSessionView", () => ({
  default: ({ workstreamId, agent }: { workstreamId?: string; agent?: Agent }) => (
    <div>
      <output aria-label="新会话预置">{`${workstreamId ?? ""}:${agent ?? ""}`}</output>
      <input aria-label="首条消息" defaultValue="" />
    </div>
  ),
}));

// Router 的会话分支渲染级覆盖：概览 / 对话各归其位，终端是一等路由（另测）。
vi.mock("../api", () => ({
  api: {
    getAgentStatus: vi.fn().mockResolvedValue({}),
    terminalForSession: vi.fn().mockResolvedValue(null),
    launchEmbeddedResume: vi.fn().mockRejectedValue(new Error("no terminal")),
    getSessionMessages: vi.fn().mockResolvedValue({ messages: [], total: 0, tail_ordinal: 0, remaining: 0 }),
    getSessionDetail: vi.fn().mockResolvedValue({
      session: { agent: "codex", source_kind: "codex", archived_at: null, title: "T" },
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
    archiveSession: vi.fn(),
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

it("routes new-session presets and resets the draft when they change", () => {
  const navigate = vi.fn();
  const goBack = vi.fn();
  const { rerender } = render(<Router route={{ view: "new-session", workstreamId: "w1", agent: "codex" }} navigate={navigate} goBack={goBack} actionSeq={0} />);
  expect(screen.getByLabelText("新会话预置").textContent).toBe("w1:codex");
  fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "任务一的消息" } });

  rerender(<Router route={{ view: "new-session", workstreamId: "w2", agent: "codex" }} navigate={navigate} goBack={goBack} actionSeq={0} />);
  expect(screen.getByLabelText("新会话预置").textContent).toBe("w2:codex");
  expect((screen.getByLabelText("首条消息") as HTMLInputElement).value).toBe("");

  fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "Codex 的消息" } });
  rerender(<Router route={{ view: "new-session", workstreamId: "w2", agent: "claude_code" }} navigate={navigate} goBack={goBack} actionSeq={0} />);
  expect(screen.getByLabelText("新会话预置").textContent).toBe("w2:claude_code");
  expect((screen.getByLabelText("首条消息") as HTMLInputElement).value).toBe("");
});

it("renders the detail page when entry is absent", () => {
  vi.mocked(api.getSessionDetail).mockReturnValue(new Promise(() => {}) as never);
  const route: Route = { view: "session", sessionId: "s1", initialTitle: "T", initialAgent: "codex" };
  const { container } = render(<Router route={route} navigate={vi.fn()} goBack={vi.fn()} actionSeq={0} />);
  // 详情页的骨架（ rail-section / 所属任务 区块标签）在加载中也会出现 section-label。
  expect(container.querySelector(".session-detail")).toBeTruthy();
});
