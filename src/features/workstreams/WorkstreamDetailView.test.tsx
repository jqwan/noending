import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import WorkstreamDetailView from "./WorkstreamDetailView";
import { api } from "../../api";
import type { WorkstreamContext, WorkstreamPathRow } from "../../types";

// 只覆盖「页面把编辑入口收敛到弹窗」；命令怎么走由 WorkstreamFormModal.test.tsx 覆盖。
vi.mock("../../api", () => ({
  api: {
    getWorkstreamContext: vi.fn(),
    listWorkstreamPaths: vi.fn(),
    getDefaultAgent: vi.fn().mockResolvedValue(null),
    updateWorkstream: vi.fn(),
    addWorkstreamPath: vi.fn(),
    removeWorkstreamPath: vi.fn(),
    reorderWorkstreamPaths: vi.fn(),
    probeWorkspacePath: vi.fn().mockResolvedValue(null),
    listRecentWorkspacePaths: vi.fn().mockResolvedValue([]),
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
    source: "user",
    created_at: "",
    canonical_path: "/repo/main",
    project_id: "pr",
    project_name: "Main",
    exists: true,
    bound_session_count: 0,
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
    related_sessions: [],
    conflicts: [],
    relations: [],
    recent_changes: [],
  };
}

async function renderDetail() {
  vi.mocked(api.getWorkstreamContext).mockResolvedValue(context());
  vi.mocked(api.listWorkstreamPaths).mockResolvedValue(PATHS);
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
