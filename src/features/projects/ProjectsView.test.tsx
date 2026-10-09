import { viewState } from "../../hooks/useViewState";
// Projects Board 契约：一次卡片查询、名称/路径搜索、缺失筛选。

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectsView, { clearProjectCardsCache } from "./ProjectsView";
import { api } from "../../api";
import { open } from "@tauri-apps/plugin-dialog";
import type { ProjectCardData } from "../../types";

vi.mock("../../api", () => ({
  api: {
    listProjectCards: vi.fn(),
    getProjectDetail: vi.fn(),
    addProjectPath: vi.fn(),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
}));

const navigate = vi.fn();

beforeEach(() => {
  navigate.mockReset();
  vi.mocked(open).mockReset().mockResolvedValue(null);
  vi.mocked(api.addProjectPath).mockReset();
  clearProjectCardsCache();
  viewState.clear();
  vi.mocked(api.listProjectCards).mockReset();
  vi.mocked(api.getProjectDetail).mockReset();
});

afterEach(cleanup);

function card(over: Partial<ProjectCardData> = {}): ProjectCardData {
  return {
    id: "p-1",
    name: "NoEnding",
    name_customized: false,
    kind: "git",
    path_count: 2,
    missing_path_count: 0,
    workstream_count: 3,
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
        kind: "directory",
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

    // 第三个 worktree 不在展示用的 representative_paths 里，但路径搜索覆盖全部 search_paths。
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

  it("kind_filter_separates_git_single_directories_and_default_chats", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "p-1", name: "Git Project", kind: "git" }),
      card({ id: "p-2", name: "Normal Directory", kind: "directory" }),
      card({ id: "p-3", name: "Renamed Chats", kind: "chat_directory" }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("Normal Directory");

    fireEvent.change(screen.getByLabelText("项目类型"), {
      target: { value: "git" },
    });

    expect(screen.getByText("Git Project")).toBeTruthy();
    expect(screen.queryByText("Normal Directory")).toBeNull();
    expect(screen.queryByText("Renamed Chats")).toBeNull();

    fireEvent.change(screen.getByLabelText("项目类型"), { target: { value: "directory" } });
    const normal = screen.getByRole("button", { name: /Normal Directory/ });
    expect(normal.textContent).toContain("普通项目");
    expect(screen.queryByText("Git Project")).toBeNull();
    expect(screen.queryByText("Renamed Chats")).toBeNull();

    fireEvent.change(screen.getByLabelText("项目类型"), { target: { value: "chat_directory" } });
    const chat = screen.getByRole("button", { name: /Renamed Chats/ });
    expect(chat.textContent).toContain("默认项目");
    expect(screen.queryByText("Git Project")).toBeNull();
    expect(screen.queryByText("Normal Directory")).toBeNull();
  });

  it("toggles between card and list view", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([card()]);
    const { container } = render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");

    expect(container.querySelector(".board-grid")).toBeTruthy();
    expect(container.querySelector(".project-list")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
    expect(container.querySelector(".project-list")).toBeTruthy();
    expect(container.querySelector(".board-grid")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "卡片视图" }));
    expect(container.querySelector(".board-grid")).toBeTruthy();
    expect(container.querySelector(".project-list")).toBeNull();
  });

  it("add_project_button_opens_picker_and_calls_add_project_path", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([card()]);
    vi.mocked(open).mockResolvedValueOnce("/Users/me/code/new-project");
    vi.mocked(api.addProjectPath).mockResolvedValueOnce({
      path: {
        id: "p-new",
        canonical_path: "/Users/me/code/new-project",
        project_id: "proj-new",
        git_state: "detected",
        git_kind: "repo",
        exists: true,
        first_seen_at: "",
        last_seen_at: "",
      },
      project_id: "proj-new",
      project_name: "new-project",
    });

    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");

    const addBtn = screen.getByRole("button", { name: "新增项目" });
    await act(async () => {
      fireEvent.click(addBtn);
    });

    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "选择项目目录",
    });

    await vi.waitFor(() => {
      expect(api.addProjectPath).toHaveBeenCalledWith("/Users/me/code/new-project");
    });
  });

  it("navigates to new-session with projectId when clicking plus button on card", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([card({ id: "p-123" })]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");

    const newSessionBtn = screen.getByRole("button", { name: "新建会话" });
    fireEvent.click(newSessionBtn);

    expect(navigate).toHaveBeenCalledWith({ view: "new-session", projectId: "p-123" });
    expect(navigate).not.toHaveBeenCalledWith({ view: "project", projectId: "p-123" });
  });

  it("navigates to new-session with projectId when clicking plus button in list view", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([card({ id: "p-456" })]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");

    fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
    const newSessionBtn = screen.getByRole("button", { name: "新建会话" });
    fireEvent.click(newSessionBtn);

    expect(navigate).toHaveBeenCalledWith({ view: "new-session", projectId: "p-456" });
  });

  it("does not render plus button for other agent default projects in card and list view", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "p-agent", name: "Codex", kind: "chat_directory" }),
      card({ id: "p-noending", name: "NoEnding Workspace", kind: "chat_directory" }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("Codex");

    // In card view: only NoEnding Workspace has the button
    const buttons = screen.getAllByRole("button", { name: "新建会话" });
    expect(buttons).toHaveLength(1);

    // In list view: only NoEnding Workspace has the button
    fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
    const listButtons = screen.getAllByRole("button", { name: "新建会话" });
    expect(listButtons).toHaveLength(1);
  });
});
