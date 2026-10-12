import { viewState } from "../../hooks/useViewState";
// Projects Board 契约：一次卡片查询、名称/路径搜索、缺失筛选。

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
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
    archiveProject: vi.fn(),
    restoreProject: vi.fn(),
    deleteProjectPermanently: vi.fn(),
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
  vi.mocked(api.archiveProject).mockReset();
  vi.mocked(api.restoreProject).mockReset();
  vi.mocked(api.deleteProjectPermanently).mockReset();
  clearProjectCardsCache();
  viewState.clear(); localStorage.clear();
  vi.mocked(api.listProjectCards).mockReset();
  vi.mocked(api.getProjectDetail).mockReset();
});

afterEach(cleanup);

function card(over: Partial<ProjectCardData> = {}): ProjectCardData {
  return {
    id: "p-1",
    name: "NoEnding",
    name_customized: false, visibility: "normal",
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
    expect(screen.queryByText("/Users/me/code/noending")).toBeNull();
    expect(api.listProjectCards).toHaveBeenCalledTimes(1);
    expect(api.getProjectDetail).not.toHaveBeenCalled();
  });

  it("projects_search_matches_name", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "p-1", name: "NoEnding" }),
      card({
        id: "p-2",
        name: "My App",
        kind: "single_directory",
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
      card({ id: "p-1", name: "Git Project", kind: "git", purpose: "chat" }),
      card({ id: "p-2", name: "Normal Directory", kind: "single_directory", purpose: "assistant" }),
      card({ id: "p-3", name: "Renamed Chats", kind: "directory_group", purpose: "chat" }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("Normal Directory");

    fireEvent.change(screen.getByLabelText("项目类型"), {
      target: { value: "git" },
    });

    expect(screen.getByText("Git Project")).toBeTruthy();
    expect(screen.queryByText("Normal Directory")).toBeNull();
    expect(screen.queryByText("Renamed Chats")).toBeNull();

    fireEvent.change(screen.getByLabelText("项目类型"), { target: { value: "single_directory" } });
    const normal = screen.getByRole("button", { name: /Normal Directory/ });
    expect(normal.textContent).toContain("单目录项目");
    expect(normal.textContent).toContain("助手");
    expect(screen.queryByText("Git Project")).toBeNull();
    expect(screen.queryByText("Renamed Chats")).toBeNull();

    fireEvent.change(screen.getByLabelText("项目类型"), { target: { value: "directory_group" } });
    const chat = screen.getByRole("button", { name: /Renamed Chats/ });
    expect(chat.textContent).toContain("目录组项目");
    expect(chat.textContent).toContain("聊天");
    expect(screen.queryByText("Git Project")).toBeNull();
    expect(screen.queryByText("Normal Directory")).toBeNull();
  });

  it("toggles between card and list view and restores the choice after a restart", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([card()]);
    const { container } = render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");

    expect(container.querySelector(".board-grid")).toBeTruthy();
    expect(container.querySelector(".project-list")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
    expect(container.querySelector(".project-list")).toBeTruthy();
    expect(container.querySelector(".board-grid")).toBeNull();

    cleanup();
    viewState.clear();
    const reopened = render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");
    expect(reopened.container.querySelector(".project-list")).toBeTruthy();
    expect(reopened.container.querySelector(".board-grid")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "卡片视图" }));
    expect(reopened.container.querySelector(".board-grid")).toBeTruthy();
    expect(reopened.container.querySelector(".project-list")).toBeNull();
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

  it("does not render new-session actions for any chat-directory project in card and list view", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "p-agent", name: "Codex Chat", kind: "directory_group" }),
      card({ id: "p-noending", name: "NoEnding Chat", kind: "directory_group" }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("Codex Chat");

    expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
    expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
  });
  it("archive moves the project to the archive scope and restoration brings it back", async () => {
    const active = card({ id: "p-active", name: "Active" });
    const archived = card({ id: "p-archived", name: "Archived", visibility: "archived" });
    vi.mocked(api.listProjectCards).mockResolvedValue([active, archived]);
    vi.mocked(api.archiveProject).mockResolvedValue({ id: active.id, name: active.name, git_id: null, visibility: "archived", name_customized: false, created_at: "", updated_at: "" });
    vi.mocked(api.restoreProject).mockResolvedValue({ id: archived.id, name: archived.name, git_id: null, visibility: "normal", name_customized: false, created_at: "", updated_at: "" });
    const { rerender } = render(<ProjectsView navigate={navigate} />);
    await screen.findByText("Active");
    expect(screen.queryByText("Archived")).toBeNull();
    vi.mocked(api.listProjectCards).mockResolvedValue([{ ...active, visibility: "archived" }, archived]);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "归档项目" })); });
    expect(api.archiveProject).toHaveBeenCalledWith(active.id);
    await vi.waitFor(() => expect(screen.queryByText("Active")).toBeNull());
    fireEvent.click(screen.getByRole("button", { name: "已归档" }));
    expect(navigate).toHaveBeenCalledWith({ view: "projects", scope: "archived" });
    rerender(<ProjectsView navigate={navigate} scope="archived" />);
    await screen.findByText("Archived");
    const row = screen.getByText("Archived").closest("article")!;
    vi.mocked(api.listProjectCards).mockResolvedValue([{ ...active, visibility: "archived" }, { ...archived, visibility: "normal" }]);
    await act(async () => { fireEvent.click(within(row).getByRole("button", { name: "恢复项目" })); });
    expect(api.restoreProject).toHaveBeenCalledWith(archived.id);
    await vi.waitFor(() => expect(screen.queryByText("Archived")).toBeNull());
  });

  it.each(["cards", "list"] as const)("archive %s view restricts delete and bulk deletion to eligible projects", async (mode) => {
    viewState.set("projects.viewMode", mode);
    const projects = [
      card({ id: "normal", name: "Normal", visibility: "normal", session_count: 0 }),
      card({ id: "empty", name: "Empty", visibility: "archived", session_count: 0 }),
      card({ id: "occupied", name: "Occupied", visibility: "archived", session_count: 1 }),
      card({ id: "chat", name: "Chat", visibility: "archived", session_count: 0, purpose: "chat" }),
      card({ id: "assistant", name: "Assistant", visibility: "archived", session_count: 0, purpose: "assistant" }),
    ];
    vi.mocked(api.listProjectCards).mockResolvedValue(projects);
    vi.mocked(api.deleteProjectPermanently).mockResolvedValue(undefined);
    render(<ProjectsView navigate={navigate} scope="archived" />);
    await screen.findByText("Empty");
    expect(screen.queryByText("Normal")).toBeNull();
    expect(screen.queryByRole("button", { name: "新增项目" })).toBeNull();
    const row = (name: string) => within(screen.getByText(name).closest("article")!);
    expect((row("Empty").getByRole("button", { name: "永久删除项目" }) as HTMLButtonElement).disabled).toBe(false);
    expect((row("Occupied").getByRole("button", { name: "永久删除项目" }) as HTMLButtonElement).disabled).toBe(true);
    expect(row("Chat").queryByRole("button", { name: "永久删除项目" })).toBeNull();
    expect(row("Assistant").queryByRole("button", { name: "永久删除项目" })).toBeNull();
    // Confirmation is required; bulk deletion excludes occupied and built-in projects.
    fireEvent.click(screen.getByRole("button", { name: "删除全部" }));
    expect(api.deleteProjectPermanently).not.toHaveBeenCalled();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: /^永久删除$/ })); });
    expect(api.deleteProjectPermanently).toHaveBeenCalledTimes(1);
    expect(api.deleteProjectPermanently).toHaveBeenCalledWith("empty");
  });

});
