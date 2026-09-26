import { viewState } from "../../hooks/useViewState";
// Projects Experience v0.2 Projects Board 契约：
// 一次卡片查询、名称/路径搜索、缺失筛选、
// 以及刷新期间卡片保持可见。

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
  viewState.clear();
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
    search_paths: ["/Users/me/code/noending"],
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
        search_paths: ["/Users/me/dev/my-app"],
      }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("My App");

    fireEvent.change(screen.getByPlaceholderText(/搜索项目/), {
      target: { value: "noending" },
    });

    expect(screen.getByText("NoEnding")).toBeTruthy();
    expect(screen.queryByText("My App")).toBeNull();
  });

  it("projects_search_matches_workspace_path", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({
        id: "p-1",
        name: "NoEnding",
        representative_paths: ["/Users/me/code/noending"],
        search_paths: [
          "/Users/me/code/noending",
          "/Users/me/code/noending-docs",
          "/Users/me/worktrees/noending-ui",
        ],
      }),
      card({ id: "p-2", name: "My App", search_paths: ["/Users/me/dev/my-app"] }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("My App");

    // Review P2-1 — 第三个 worktree 不在展示用的 representative_paths 里，
    // 但路径搜索覆盖全部 search_paths。
    fireEvent.change(screen.getByPlaceholderText(/搜索项目/), {
      target: { value: "worktrees/noending-ui" },
    });

    expect(screen.getByText("NoEnding")).toBeTruthy();
    expect(screen.queryByText("My App")).toBeNull();
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

  it("kind_filter_separates_git_projects_from_normal_directories", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "p-1", name: "Git Project", has_git_identity: true }),
      card({ id: "p-2", name: "Normal Directory", has_git_identity: false }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("Normal Directory");

    fireEvent.change(screen.getByLabelText("项目类型"), {
      target: { value: "git" },
    });

    expect(screen.getByText("Git Project")).toBeTruthy();
    expect(screen.queryByText("Normal Directory")).toBeNull();
  });

  it("project_refresh_keeps_existing_cards_while_running", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([card()]);
    // 刷新永不返回：证明等待期间页面内容纹丝不动。
    vi.mocked(api.refreshWorkspaceProjects).mockReturnValue(new Promise(() => {}));
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");

    fireEvent.click(screen.getByRole("button", { name: "刷新工作区状态" }));

    // 页头的是图标按钮：进度只能靠 aria-label 与 disabled 如实呈现。
    expect(
      (screen.getByRole("button", { name: "正在刷新工作区状态" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(screen.getByText("NoEnding")).toBeTruthy();
    // 卡片没有被清空重建：Board 数据仍在。
    expect(screen.queryByText("加载中…")).toBeNull();
  });
});
