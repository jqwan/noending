import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import WorkstreamDetailView, { clearWorkstreamDetailCache } from "./WorkstreamDetailView";
import { api } from "../../api";
import type { WorkstreamContext, WorkstreamContextView } from "../../types";

// 只覆盖「页面把编辑入口收敛到弹窗」；命令怎么走由 WorkstreamFormModal.test.tsx 覆盖。
vi.mock("../../api", () => ({
  api: {
    saveSummary: vi.fn(),
    getSessionTerminalStates: vi.fn(async (ids: string[]) => Object.fromEntries(ids.map(id => [id, { available: true, reason: null }]))),
    getAgentStatus: vi.fn().mockResolvedValue({}),
    continueSessionDesktop: vi.fn().mockResolvedValue({ uri: "x://y", note: "已打开" }),
    getWorkstreamContext: vi.fn(),
    getWorkstreamContextState: vi.fn().mockResolvedValue(null),
    regenerateWorkstreamContext: vi.fn(),
    updateWorkstreamContext: vi.fn().mockResolvedValue({
      workstream_id: "",
      status: "updated",
      context_revision: 1,
      updated_sessions: [],
      skipped_session_ids: [],
      failed_sessions: [],
      remaining_pending: 0,
    }),
    getDefaultAgent: vi.fn().mockResolvedValue(null),
    updateWorkstream: vi.fn(),
    probeWorkspacePath: vi.fn().mockResolvedValue(null),
    listRecentWorkspacePaths: vi.fn().mockResolvedValue([]),
    listProjectCards: vi.fn().mockResolvedValue([]),
    setWorkstreamProjects: vi.fn().mockResolvedValue(undefined),
    restoreWorkstream: vi.fn().mockResolvedValue({ id: "w1", visibility: "normal" }),
    deleteWorkstreamPermanently: vi.fn().mockResolvedValue(undefined),
  },
}));

beforeEach(() => {
  vi.mocked(api.getWorkstreamContext).mockReset().mockResolvedValue(context());
  vi.mocked(api.getWorkstreamContextState).mockReset().mockResolvedValue(contextState());
});
afterEach(() => {
  clearWorkstreamDetailCache();
  cleanup();
  vi.clearAllMocks();
});

function context(): WorkstreamContext {
  return {
    workstream: {
      id: "w1",
      title: "接口设计",
      description: "整理 API 设计",
      visibility: "normal",
      created_at: "2026-09-21T00:00:00+00:00",
      updated_at: "2026-09-21T00:00:00+00:00",
    },
    projects: [{id:"pr",name:"Main"}],
    sessions: [],
  };
}

function contextState(over: Partial<WorkstreamContextView> = {}): WorkstreamContextView {
  return {
    workstream_id: "w1",
    summary: { items: [{ id: "WorkstreamDetailView.test-item-1", title: "任务状态", description: "**当前进展**：接口设计已确定。", title_locked: false, details: [] }] },
    updated_at: null,
    context_revision: 1,
    pending: false,
    pending_sessions: 0, freshness: { refreshed_at: null, pending_messages: 0, history_rewritten_sessions: 0, unknown_coverage_sessions: 0, summary_changed_sessions: 0, scope_changed: false, pending: false },
    ...over,
  };
}

async function renderDetail(navigate = vi.fn()) {
  vi.mocked(api.getWorkstreamContext).mockResolvedValue(context());
  // 逐个测试独立：默认「已是最新」，pending 用例自己覆盖。
  vi.mocked(api.getWorkstreamContextState).mockResolvedValue(contextState());
  render(<WorkstreamDetailView workstreamId="w1" navigate={navigate} goBack={vi.fn()} />);
  await screen.findByText("任务概览");
  return navigate;
}

it("opens the new-session page with the current task selected", async () => {
  const navigate = await renderDetail();
  fireEvent.click(screen.getByRole("button", { name: "新建会话" }));
  expect(navigate).toHaveBeenCalledWith({ view: "new-session", workstreamId: "w1" });
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("hides the new-session entry when the task has no associated projects", async () => {
  vi.mocked(api.getWorkstreamContext).mockResolvedValue({ ...context(), projects: [] });
  render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);
  await screen.findByText("任务概览");
  expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
});

it("renders the returned task sessions and navigates to the selected session", async () => {
  const ctx = context();
  ctx.sessions = [{
    id: "s1",
    agent: "codex",
    root_agent_session_id: "s1-agent",
    title: "归属于本任务的会话",
    cwd: "/repo/main",
    project_id: "pr",
    workspace_path_id: "p-main",
    tracked_workstream_ids: ["w1"],
    started_at: "2026-09-21T00:00:00+00:00",
    last_activity_at: "2026-09-21T00:00:00+00:00",
    last_conversation_at: null,
    archived_at: null,
        source_kind: "codex_rollout",
        latest_message_seq: 2,

  }];
  vi.mocked(api.getWorkstreamContext).mockResolvedValue(ctx);
  const navigate = vi.fn();
  render(<WorkstreamDetailView workstreamId="w1" navigate={navigate} goBack={vi.fn()} />);

  await screen.findByText("任务概览");
  // 只有跟踪此任务的会话会出现在这里。
  screen.getByRole("heading", { name: "关联会话（1）" });
  fireEvent.click(screen.getByText("归属于本任务的会话"));
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ view: "session", sessionId: "s1" }));
  expect(screen.queryByText("还没有会话归属到这项任务。")).toBeNull();
});

it("opens the 新建任务 form, prefilled with the current task", async () => {
  await renderDetail();
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "编辑任务" })); });

  const dialog = screen.getByRole("dialog", { name: "编辑任务" });
  screen.getByDisplayValue("接口设计");
  screen.getByDisplayValue("整理 API 设计");
  within(dialog).getByText("Main");
  screen.getByRole("button", { name: "保存" });
  screen.getByRole("button", { name: "添加关联项目" });
});

it("shows 更新摘要 with the pending-session count and calls the explicit update", async () => {
  vi.mocked(api.getWorkstreamContext).mockResolvedValue(context());
  const state: WorkstreamContextView = contextState({
    context_revision: 2,
    pending: true,
    pending_sessions: 2, freshness: { refreshed_at: null, pending_messages: 0, history_rewritten_sessions: 0, unknown_coverage_sessions: 0, summary_changed_sessions: 0, scope_changed: false, pending: false },
  });
  vi.mocked(api.getWorkstreamContextState).mockResolvedValue(state);
  vi.mocked(api.updateWorkstreamContext).mockResolvedValue({
    workstream_id: "w1",
    status: "updated",
    context_revision: 3,
    updated_sessions: ["s1", "s2"],
    skipped_session_ids: [],
    failed_sessions: [],
    remaining_pending: 0,
  });
  render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);
  await screen.findByText("任务概览");

  screen.getByText("有待更新内容，涉及 2 个会话");
  fireEvent.click(screen.getByRole("button", { name: "更新摘要" }));
  await waitFor(() => expect(api.updateWorkstreamContext).toHaveBeenCalledWith("w1"));
});

it("hides 更新摘要 when the Context is already up to date", async () => {
  await renderDetail();
  expect(screen.queryByRole("button", { name: "更新摘要" })).toBeNull();
});

it("offers a copyable operation id after a structured Context update failure", async () => {
  vi.mocked(api.getWorkstreamContext).mockResolvedValue(context());
  vi.mocked(api.getWorkstreamContextState).mockResolvedValue(contextState({ pending: true, pending_sessions: 1, freshness: { refreshed_at: null, pending_messages: 0, history_rewritten_sessions: 0, unknown_coverage_sessions: 0, summary_changed_sessions: 0, scope_changed: false, pending: false }, }));
  vi.mocked(api.updateWorkstreamContext).mockRejectedValue({
    code: "cli_timeout",
    message: "Agent CLI 超时，请重试或检查 CLI 状态。",
    operation_id: "123e4567-e89b-12d3-a456-426614174001",
  });
  const originalClipboard = navigator.clipboard;
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);
  await screen.findByText("任务概览");

  fireEvent.click(screen.getByRole("button", { name: "更新摘要" }));
  await screen.findByText("Agent CLI 超时，请重试或检查 CLI 状态。");
  fireEvent.click(screen.getByRole("button", { name: "复制错误详情" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(
    "Agent CLI 超时，请重试或检查 CLI 状态。\n错误代码：cli_timeout\n操作 ID：123e4567-e89b-12d3-a456-426614174001",
  ));
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: originalClipboard });
});

it("renders structured topics and regenerates through the separate action even when up to date", async () => {
  vi.mocked(api.regenerateWorkstreamContext).mockResolvedValue({ workstream_id: "w1", status: "updated", context_revision: 2, updated_sessions: [], skipped_session_ids: [], failed_sessions: [], remaining_pending: 0 });
  await renderDetail();
  expect(screen.getByText("当前进展").tagName).toBe("STRONG");
  screen.getByRole("button",{name:"锁定标题：任务状态"});
  fireEvent.click(screen.getByRole("button", { name: "重新生成" }));
  await waitFor(() => expect(api.regenerateWorkstreamContext).toHaveBeenCalledWith("w1"));
  expect(api.updateWorkstreamContext).not.toHaveBeenCalled();
});

it("keeps the task summary visible and reports failed sessions after a partial update", async () => {
  vi.mocked(api.getWorkstreamContextState).mockResolvedValue(contextState({ pending: true, pending_sessions: 1, freshness: { refreshed_at: null, pending_messages: 0, history_rewritten_sessions: 0, unknown_coverage_sessions: 0, summary_changed_sessions: 0, scope_changed: false, pending: false }, }));
  vi.mocked(api.updateWorkstreamContext).mockResolvedValue({ workstream_id: "w1", status: "partial", context_revision: 2, updated_sessions: [], skipped_session_ids: [], failed_sessions: [{ session_id: "s-failed", error: "无法完整读取消息" }], remaining_pending: 1 });
  render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);
  await screen.findByRole("button", { name: "更新摘要" });
  fireEvent.click(screen.getByRole("button", { name: "更新摘要" }));
  await screen.findByText("s-failed：无法完整读取消息");
  screen.getByRole("heading", { name: "任务摘要（1）" });
  expect(screen.getByText("当前进展")).toBeTruthy();
  expect((screen.getByRole("button", { name: "更新摘要" }) as HTMLButtonElement).disabled).toBe(false);
});

it("renders separate unarchive and permanent delete buttons when workstream is archived", async () => {
  const ctx = context();
  ctx.workstream.visibility = "archived";
  vi.mocked(api.getWorkstreamContext).mockResolvedValue(ctx);
  vi.mocked(api.getWorkstreamContextState).mockResolvedValue(contextState());
  render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);
  await screen.findByText("任务概览");

  expect(screen.queryByRole("button", { name: "更多操作" })).toBeNull();
  const unarchiveBtn = screen.getByRole("button", { name: "取消归档" });
  const deleteBtn = screen.getByRole("button", { name: "永久删除" });
  expect(unarchiveBtn).toBeDefined();
  expect(deleteBtn).toBeDefined();

  // Clicking delete opens permanent delete modal without text confirmation input
  fireEvent.click(deleteBtn);
  const dialog = screen.getByRole("dialog", { name: "永久删除这项任务？" });
  expect(dialog).toBeDefined();
  expect(within(dialog).queryByRole("textbox")).toBeNull();
  within(dialog).getByText(/确定要永久删除/);

  // Directly clickable without typing title
  const confirmBtn = within(dialog).getByRole("button", { name: "永久删除" });
  expect((confirmBtn as HTMLButtonElement).disabled).toBe(false);
  fireEvent.click(confirmBtn);
  await waitFor(() => expect(api.deleteWorkstreamPermanently).toHaveBeenCalledWith("w1"));
});

it("allows associating projects directly from task detail page", async () => {
  const p1 = { id: "pr", name: "Main" } as any;
  const p2 = { id: "p2", name: "后端服务" } as any;
  vi.mocked(api.listProjectCards).mockResolvedValue([p1, p2]);
  await renderDetail();
  fireEvent.click(screen.getByRole("button", { name: "关联项目" }));
  const dialog = screen.getByRole("dialog", { name: "选择关联项目" });
  const checkboxMain = await within(dialog).findByRole("checkbox", { name: "Main" });
  const checkboxBackend = within(dialog).getByRole("checkbox", { name: "后端服务" });
  expect(checkboxMain).toHaveProperty("checked", true);
  expect(checkboxBackend).toHaveProperty("checked", false);
  fireEvent.click(checkboxBackend);
  expect(checkboxBackend).toHaveProperty("checked", true);
  fireEvent.click(within(dialog).getByRole("button", { name: "确定" }));
  await waitFor(() => expect(api.setWorkstreamProjects).toHaveBeenCalledWith("w1", ["pr", "p2"]));
  expect(screen.queryByRole("dialog", { name: "选择关联项目" })).toBeNull();
});

it("edits the task summary without changing the task and restores it on remount", async () => {
  let persisted = contextState();
  vi.mocked(api.getWorkstreamContext).mockResolvedValue(context());
  vi.mocked(api.getWorkstreamContextState).mockImplementation(async () => persisted);
  vi.mocked(api.saveSummary).mockImplementation(async (kind,id,revision,summary) => {
    expect([kind,id,revision]).toEqual(["task","w1",1]);
    persisted = {...persisted,context_revision:2,summary};
    return {revision:2,summary};
  });
  const first = render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button",{name:"编辑事项：任务状态"}));
  fireEvent.change(screen.getByRole("textbox",{name:"事项描述"}),{target:{value:"接口实现已完成，等待验收。"}});
  fireEvent.click(screen.getByRole("button",{name:"保存"}));
  await screen.findByText("接口实现已完成，等待验收。");
  first.unmount();
  render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);
  await screen.findByText("接口实现已完成，等待验收。");
  screen.getByText("整理 API 设计");
});
