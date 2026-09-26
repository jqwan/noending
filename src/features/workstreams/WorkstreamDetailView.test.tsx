import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import WorkstreamDetailView from "./WorkstreamDetailView";
import { api } from "../../api";
import type { WorkstreamContext, WorkstreamContextView, WorkstreamPathRow } from "../../types";

// 只覆盖「页面把编辑入口收敛到弹窗」；命令怎么走由 WorkstreamFormModal.test.tsx 覆盖。
vi.mock("../../api", () => ({
  api: {
    getWorkstreamContext: vi.fn(),
    getWorkstreamContextState: vi.fn().mockResolvedValue(null),
    updateWorkstreamContext: vi.fn().mockResolvedValue({
      workstream_id: "",
      status: "updated",
      context_revision: 1,
      updated_sessions: [],
      mutations_applied: 0,
      remaining_pending: 0,
    }),
    listWorkstreamPaths: vi.fn(),
    getDefaultAgent: vi.fn().mockResolvedValue(null),
    updateWorkstream: vi.fn(),
    addWorkstreamPath: vi.fn(),
    removeWorkstreamPath: vi.fn(),
    reorderWorkstreamPaths: vi.fn(),
    probeWorkspacePath: vi.fn().mockResolvedValue(null),
    listRecentWorkspacePaths: vi.fn().mockResolvedValue([]),
    // 挂载时读取的 Review 面板（SinceLastReview）默认空窗。
    getWorkstreamReviewWindow: vi.fn().mockResolvedValue({
      state: { workstream_id: "w1", frontier: { through_at: "", boundary_change_ids: [] }, reviewed_at: "" },
      unseen_changes: [],
      mark_through: { through_at: "", boundary_change_ids: [] },
    }),
    getWorkstreamReviewSummary: vi.fn().mockResolvedValue({
      workstream_id: "w1",
      unseen_change_count: 0,
      open_conflict_count: 0,
      new_facts: 0,
      updated_facts: 0,
      resolved_items: 0,
      superseded_items: 0,
      last_unseen_change_at: null,
      reviewed_at: "",
      has_updates: false,
      needs_attention: false,
    }),
    markWorkstreamReviewed: vi.fn(),
  },
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const PATHS: WorkstreamPathRow[] = [
  {
    id: "wp-main",
    workstream_id: "w1",
    workspace_path_id: "p-main",
    position: 0,
    created_at: "",
    canonical_path: "/repo/main",
    project_id: "pr",
    project_name: "Main",
    exists: true,
  },
];

function context(): WorkstreamContext {
  return {
    workstream: {
      id: "w1",
      title: "接口设计",
      description: "整理 API 设计",
      lifecycle: "active",
      visibility: "normal",
      created_at: "2026-09-21T00:00:00+00:00",
      updated_at: "2026-09-21T00:00:00+00:00",
    },
    project_name: null,
    core: [],
    items: [],
    sessions: [],
    conflicts: [],
    relations: [],
    recent_changes: [],
  };
}

function contextState(over: Partial<WorkstreamContextView> = {}): WorkstreamContextView {
  return {
    workstream_id: "w1",
    title: "接口设计",
    description: "整理 API 设计",
    lifecycle: "active",
    sections: [],
    context_revision: 1,
    input_revision: 1,
    consumed_input_revision: 1,
    pending: false,
    pending_sessions: 0,
    ...over,
  };
}

async function renderDetail() {
  vi.mocked(api.getWorkstreamContext).mockResolvedValue(context());
  vi.mocked(api.listWorkstreamPaths).mockResolvedValue(PATHS);
  // 逐个测试独立：默认「已是最新」，pending 用例自己覆盖。
  vi.mocked(api.getWorkstreamContextState).mockResolvedValue(contextState());
  render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);
  await screen.findByText("任务概览");
}

it("keeps the title band while loading", async () => {
  // 读取永不返回：证明加载态仍然渲染 PageHeader。少了它，吸在标题栏上的那条 band
  // 会先整条消失、数据到了再补回来，看着就像闪了一个"加载中的页面"。
  vi.mocked(api.getWorkstreamContext).mockReturnValue(new Promise(() => {}));
  vi.mocked(api.listWorkstreamPaths).mockReturnValue(new Promise(() => {}));
  render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);

  screen.getByRole("heading", { name: "加载中…" });
  expect(screen.queryByText("任务概览")).toBeNull();
});

it("keeps exactly one edit entry: the 编辑任务 icon button", async () => {
  await renderDetail();

  screen.getByRole("button", { name: "编辑任务" });
  expect(screen.queryByRole("button", { name: "重命名…" })).toBeNull();
  expect(screen.queryByRole("button", { name: "编辑描述…" })).toBeNull();
  // 移入回收站也是独立图标按钮；未归档时没有 ••• 菜单。
  screen.getByRole("button", { name: "移入回收站" });
  expect(screen.queryByTitle("更多操作")).toBeNull();
});

it("leaves no edit affordance on the page itself", async () => {
  await renderDetail();

  expect(screen.queryByRole("button", { name: "编辑描述" })).toBeNull();
  for (const name of ["新增目录", "选择已有目录", "移除", "设为主要"]) {
    expect(screen.queryByRole("button", { name })).toBeNull();
  }
  // 路径只出现在「工作目录」列表里（标题下那行元信息已去掉）。
  screen.getByText("/repo/main");
  screen.getByText("主目录");
  // 状态切换不是编辑，保留
  screen.getByRole("button", { name: "已完成" });
});

it("lists the sessions owned by this task and nothing else", async () => {
  const ctx = context();
  ctx.sessions = [{
    id: "s1",
    agent: "codex",
    root_agent_session_id: "s1-agent",
    title: "归属于本任务的会话",
    cwd: "/repo/main",
    project_id: "pr",
    workspace_path_id: "p-main",
    owner_workstream_id: "w1",
    forked_from_session_id: null,
    started_at: "2026-09-21T00:00:00+00:00",
    last_activity_at: "2026-09-21T00:00:00+00:00",
    last_conversation_at: null,
    trashed_at: null,
  }];
  vi.mocked(api.getWorkstreamContext).mockResolvedValue(ctx);
  vi.mocked(api.listWorkstreamPaths).mockResolvedValue(PATHS);
  render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);

  await screen.findByText("任务概览");
  // 只有 owner Sessions 会出现在这里（后端按 owner_workstream_id 过滤）。
  screen.getByText("会话");
  screen.getByText("归属于本任务的会话");
  expect(screen.queryByText("还没有会话归属到这项任务。")).toBeNull();
});

it("opens the 新建任务 form, prefilled with the current task", async () => {
  await renderDetail();
  fireEvent.click(screen.getByRole("button", { name: "编辑任务" }));

  const dialog = screen.getByRole("dialog", { name: "编辑任务" });
  screen.getByDisplayValue("接口设计");
  screen.getByDisplayValue("整理 API 设计");
  within(dialog).getByText("/repo/main");
  screen.getByRole("button", { name: "保存" });
  screen.getByRole("button", { name: "新增目录" });
});

it("shows 更新状态 with the pending-session count and calls the explicit update", async () => {
  vi.mocked(api.getWorkstreamContext).mockResolvedValue(context());
  vi.mocked(api.listWorkstreamPaths).mockResolvedValue(PATHS);
  const state: WorkstreamContextView = contextState({
    context_revision: 2,
    input_revision: 5,
    consumed_input_revision: 3,
    pending: true,
    pending_sessions: 2,
  });
  vi.mocked(api.getWorkstreamContextState).mockResolvedValue(state);
  vi.mocked(api.updateWorkstreamContext).mockResolvedValue({
    workstream_id: "w1",
    status: "updated",
    context_revision: 3,
    updated_sessions: ["s1", "s2"],
    mutations_applied: 2,
    remaining_pending: 0,
  });
  render(<WorkstreamDetailView workstreamId="w1" navigate={vi.fn()} goBack={vi.fn()} />);
  await screen.findByText("任务概览");

  screen.getByText(/有 2 个相关 Session 有新内容/);
  fireEvent.click(screen.getByRole("button", { name: "更新状态" }));
  await waitFor(() => expect(api.updateWorkstreamContext).toHaveBeenCalledWith("w1"));
});

it("hides 更新状态 when the Context is already up to date", async () => {
  await renderDetail();
  expect(screen.queryByRole("button", { name: "更新状态" })).toBeNull();
});
