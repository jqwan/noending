// Projects Experience v0.2 §27 — Projects Board 契约：
// 一次卡片查询（§8）、名称/路径搜索（§5）、缺失筛选（§6）、
// 以及刷新期间卡片保持可见（§13）。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectsView from "./ProjectsView";
import { api } from "../../api";
import { listen } from "@tauri-apps/api/event";
import type { ProjectCardData } from "../../types";

vi.mock("../../api", () => ({
  api: {
    listProjectCards: vi.fn(),
    getProjectDetail: vi.fn(),
    refreshWorkspaceProjects: vi.fn(),
    refreshProjectWorkspace: vi.fn(),
  },
}));

// 组件挂载时订阅 workspace-reconcile-* 事件；测试环境没有 Tauri IPC。
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const navigate = vi.fn();

beforeEach(() => {
  vi.mocked(api.listProjectCards).mockReset();
  vi.mocked(api.getProjectDetail).mockReset();
  vi.mocked(api.refreshWorkspaceProjects).mockReset().mockResolvedValue({ started: true });
  vi.mocked(listen).mockReset().mockResolvedValue(() => {});
});

afterEach(cleanup);

function card(over: Partial<ProjectCardData> = {}): ProjectCardData {
  return {
    id: "p-1",
    name: "NoEnding",
    name_customized: false,
    has_git_identity: true,
    path_count: 2,
    missing_path_count: 0,
    primary_workstream_count: 2,
    related_workstream_count: 1,
    session_count: 18,
    representative_paths: ["/Users/me/code/noending"],
    last_activity_at: "2026-09-20T10:00:00Z",
    updated_at: "2026-09-20T09:00:00Z",
    ...over,
  };
}

describe("Projects Board", () => {
  it("projects_board_uses_single_card_query", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([card()]);
    render(<ProjectsView navigate={navigate} />);

    await screen.findByText("NoEnding");
    expect(api.listProjectCards).toHaveBeenCalledTimes(1);
    expect(api.getProjectDetail).not.toHaveBeenCalled();
  });

  it("projects_search_matches_name", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "p-1", name: "NoEnding" }),
      card({
        id: "p-2",
        name: "My App",
        has_git_identity: false,
        representative_paths: ["/Users/me/dev/my-app"],
      }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("My App");

    fireEvent.change(screen.getByPlaceholderText(/搜索 Projects/), {
      target: { value: "noending" },
    });

    expect(screen.getByText("NoEnding")).toBeTruthy();
    expect(screen.queryByText("My App")).toBeNull();
  });

  it("projects_search_matches_workspace_path", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "p-1", name: "NoEnding", representative_paths: ["/Users/me/code/noending"] }),
      card({ id: "p-2", name: "My App", representative_paths: ["/Users/me/dev/my-app"] }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("My App");

    // 输入目录名的一部分：按 canonical path 命中，而不是 Project 名。
    fireEvent.change(screen.getByPlaceholderText(/搜索 Projects/), {
      target: { value: "my-app" },
    });

    expect(screen.getByText("My App")).toBeTruthy();
    expect(screen.queryByText("NoEnding")).toBeNull();
  });

  it("missing_filter_only_shows_projects_with_missing_paths", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "p-1", name: "Healthy", missing_path_count: 0 }),
      card({ id: "p-2", name: "Broken", missing_path_count: 1 }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("Healthy");

    fireEvent.change(screen.getByDisplayValue("全部"), {
      target: { value: "missing" },
    });

    expect(screen.getByText("Broken")).toBeTruthy();
    expect(screen.queryByText("Healthy")).toBeNull();
  });

  it("project_refresh_keeps_existing_cards_while_running", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([card()]);
    // 刷新永不返回：证明等待期间页面内容纹丝不动。
    vi.mocked(api.refreshWorkspaceProjects).mockReturnValue(new Promise(() => {}));
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");

    fireEvent.click(screen.getByText("刷新工作区状态"));

    expect(screen.getByText("正在刷新工作区状态…")).toBeTruthy();
    expect(screen.getByText("NoEnding")).toBeTruthy();
    // 卡片没有被清空重建：Board 数据仍在。
    expect(screen.queryByText("加载中…")).toBeNull();
  });
});
