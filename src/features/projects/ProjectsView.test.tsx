import { invalidateTargets, removedTargets } from "../../app/removedTargets";
import { viewState } from "../../hooks/useViewState";

import { act, cleanup, fireEvent, render, screen, within, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectsView, { clearProjectCardsCache } from "./ProjectsView";
import { api } from "../../api";
import { open } from "@tauri-apps/plugin-dialog";
import type { ProjectCardData } from "../../types";

vi.mock("../../api", () => ({
  api: { matchObjects: vi.fn().mockResolvedValue([]),
    listProjectCards: vi.fn(),
    getProjectDetail: vi.fn(),
    addProjectPath: vi.fn(),
    archiveProject: vi.fn(),
    restoreProject: vi.fn(),
    deleteProjectPermanently: vi.fn(),
    createAssistantProfile: vi.fn(),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
}));

const navigate = vi.fn();

beforeEach(() => {
  Object.values(removedTargets).forEach(ids => ids.clear());
  navigate.mockReset();
  vi.mocked(open).mockReset().mockResolvedValue(null);
  vi.mocked(api.addProjectPath).mockReset();
  vi.mocked(api.archiveProject).mockReset();
  vi.mocked(api.restoreProject).mockReset();
  vi.mocked(api.deleteProjectPermanently).mockReset();
  vi.mocked(api.createAssistantProfile).mockReset();
  vi.mocked(api.matchObjects).mockReset().mockResolvedValue([]);
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
  it("opens project and assistant details for the same project from cards and lists", async () => {
    const role = card({ id: "role-project", name: "架构评审", is_assistant: true });
    vi.mocked(api.listProjectCards).mockResolvedValue([role, card()]);
    const unfiltered = render(<ProjectsView navigate={navigate} />);
    expect(await screen.findByText("NoEnding")).toBeTruthy();
    expect(api.getProjectDetail).not.toHaveBeenCalled();
    for (const view of ["卡片视图", "列表视图"]) {
      fireEvent.click(screen.getByRole("button", { name: view }));
      const project = screen.getByRole("button", { name: /^架构评审/ });
      expect(within(project).getByText("智能体").tagName).toBe("SPAN");
      fireEvent.click(project);
      expect(navigate).toHaveBeenLastCalledWith({ view: "project", projectId: role.id, initialTitle: role.name });
    }
    unfiltered.unmount();

    render(<ProjectsView navigate={navigate} assistantOnly />);
    expect(await screen.findByRole("heading", { name: "智能体" })).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: /^架构评审/ }));
    expect(screen.queryByText("NoEnding")).toBeNull();
    expect(navigate).toHaveBeenLastCalledWith({ view: "project", projectId: role.id, initialTitle: role.name, entry: "assistant", fromAssistants: true });
    fireEvent.click(screen.getByRole("button", { name: "新建会话" }));
    expect(navigate).toHaveBeenLastCalledWith({ view: "new-session", projectId: role.id });
  });

  it("opens a newly created assistant by its project ID", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([]);
    const stamp = "2026-11-19T00:00:00Z";
    const project = { id: "role-project", name: "评审员", visibility: "normal" as const, git_id: null, name_customized: true, created_at: stamp, updated_at: stamp };
    vi.mocked(api.createAssistantProfile).mockResolvedValue(project);
    render(<ProjectsView navigate={navigate} assistantOnly />);
    fireEvent.click(screen.getByRole("button", { name: "新建智能体" }));
    fireEvent.change(screen.getByRole("textbox", { name: /名称/ }), { target: { value: project.name } });
    fireEvent.click(screen.getByRole("button", { name: "创建智能体" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith({ view: "project", projectId: project.id, initialTitle: project.name, entry: "assistant", fromAssistants: true }));
  });

  it("bulk-deletes only eligible assistant projects regardless of the board search", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "eligible", name: "可删除智能体", visibility: "archived", is_assistant: true, session_count: 0 }),
      card({ id: "occupied", name: "有会话的智能体", visibility: "archived", is_assistant: true, session_count: 1 }),
      card({ id: "regular", name: "普通归档项目", visibility: "archived", session_count: 0 }),
    ]);
    vi.mocked(api.deleteProjectPermanently).mockResolvedValue({ warnings: [] });
    vi.mocked(api.matchObjects).mockResolvedValue(["occupied"]);
    render(<ProjectsView navigate={navigate} assistantOnly scope="archived" />);
    expect(await screen.findByText("可删除智能体")).toBeTruthy();
    expect(screen.queryByText("普通归档项目")).toBeNull();
    fireEvent.change(screen.getByRole("textbox", { name: "搜索智能体" }), { target: { value: "有会话" } });
    await waitFor(() => expect(screen.queryByText("可删除智能体")).toBeNull());
    fireEvent.click(screen.getByRole("button", { name: "删除全部" }));
    expect(screen.getByText(/档案目录及其中的文件也会永久删除/)).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "永久删除" })); });
    expect(vi.mocked(api.deleteProjectPermanently).mock.calls).toEqual([["eligible", { view: "projects", scope: "archived", assistantOnly: true }]]);
  });
  it("projects_search_matches_name_and_workspace_path", async () => {
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

    vi.mocked(api.matchObjects).mockResolvedValue(["p-1"]);
    const searchInput = screen.getByPlaceholderText(/搜索项目/);

    // Matches by name
    fireEvent.change(searchInput, { target: { value: "noending" } });
    expect(await screen.findByText("NoEnding")).toBeTruthy();
    expect(screen.queryByText("My App")).toBeNull();

    expect(api.matchObjects).toHaveBeenLastCalledWith("project", "noending", ["p-1", "p-2"]);

    // Matches by hidden search path
    fireEvent.change(searchInput, { target: { value: "worktrees/noending-ui" } });
    expect(await screen.findByText("NoEnding")).toBeTruthy();
    expect(screen.queryByText("My App")).toBeNull();
    expect(api.matchObjects).toHaveBeenLastCalledWith("project", "worktrees/noending-ui", ["p-1", "p-2"]);
  });

  it("warns only for total directory loss in either layout and filters all missing projects", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "p-1", name: "Healthy", missing_path_count: 0 }),
      card({ id: "p-2", name: "Broken", missing_path_count: 1 }),
      card({ id: "p-3", name: "Unavailable", missing_path_count: 2 }),
      card({ id: "p-4", name: "Missing Single Directory", kind: "single_directory", path_count: 1, missing_path_count: 1 }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("Healthy");
    for (const view of ["卡片视图", "列表视图"]) {
      fireEvent.click(screen.getByRole("button", { name: view }));
      const project = (name: string) => within(screen.getByText(name).closest("article")!);
      expect(project("Healthy").queryByText(/目录缺失/)).toBeNull();
      expect(project("Broken").queryByText(/目录缺失/)).toBeNull();
      expect(project("Unavailable").getByText("全部目录缺失")).toBeTruthy();
      expect(project("Missing Single Directory").getByText("全部目录缺失")).toBeTruthy();
    }

    fireEvent.click(screen.getByRole("button", { name: /^筛选：/ }));
    fireEvent.click(screen.getByRole("option", { name: "有目录缺失" }));

    expect(screen.getByText("Broken")).toBeTruthy();
    expect(screen.getByText("Unavailable")).toBeTruthy();
    expect(screen.getByText("Missing Single Directory")).toBeTruthy();
    expect(screen.queryByText("Healthy")).toBeNull();
  });

  it("kind_filter_separates_git_single_directories_and_default_chats", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([
      card({ id: "p-1", name: "Git Project", kind: "git", purpose: "chat" }),
      card({ id: "p-2", name: "Normal Directory", kind: "single_directory", is_assistant: true }),
      card({ id: "p-3", name: "Renamed Chats", kind: "directory_group", purpose: "chat" }),
    ]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("Normal Directory");

    fireEvent.click(screen.getByRole("button", { name: /^项目类型：/ }));
    fireEvent.click(screen.getByRole("option", { name: "Git 项目" }));

    expect(screen.getByText("Git Project")).toBeTruthy();
    expect(screen.queryByText("Normal Directory")).toBeNull();
    expect(screen.queryByText("Renamed Chats")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /^项目类型：/ }));
    fireEvent.click(screen.getByRole("option", { name: "单目录项目" }));
    expect(screen.getByText("Normal Directory")).toBeTruthy();
    expect(screen.queryByText("Git Project")).toBeNull();
    expect(screen.queryByText("Renamed Chats")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /^项目类型：/ }));
    fireEvent.click(screen.getByRole("option", { name: "目录组项目" }));
    expect(screen.getByText("Renamed Chats")).toBeTruthy();
    expect(screen.queryByText("Git Project")).toBeNull();
    expect(screen.queryByText("Normal Directory")).toBeNull();
  });

  it("toggles between card and list view and restores the choice after a restart", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([card()]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");

    expect(screen.getByRole("button", { name: "卡片视图" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "列表视图" }).getAttribute("aria-pressed")).toBe("false");

    fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
    expect(screen.getByRole("button", { name: "列表视图" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "卡片视图" }).getAttribute("aria-pressed")).toBe("false");

    cleanup();
    viewState.clear();
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");
    expect(screen.getByRole("button", { name: "列表视图" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "卡片视图" }).getAttribute("aria-pressed")).toBe("false");

    fireEvent.click(screen.getByRole("button", { name: "卡片视图" }));
    expect(screen.getByRole("button", { name: "卡片视图" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "列表视图" }).getAttribute("aria-pressed")).toBe("false");
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

  it("navigates to new-session with projectId in card and list view", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValue([card({ id: "p-123" })]);
    render(<ProjectsView navigate={navigate} />);
    await screen.findByText("NoEnding");

    // Card view click
    fireEvent.click(screen.getByRole("button", { name: "新建会话" }));
    expect(navigate).toHaveBeenCalledWith({ view: "new-session", projectId: "p-123" });
    expect(navigate).not.toHaveBeenCalledWith({ view: "project", projectId: "p-123" });

    // Switch to list view
    fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
    fireEvent.click(screen.getByRole("button", { name: "新建会话" }));
    expect(navigate).toHaveBeenCalledWith({ view: "new-session", projectId: "p-123" });
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

  it("archive view restricts delete and bulk deletion to eligible projects", async () => {
    const projects = [
      card({ id: "normal", name: "Normal", visibility: "normal", session_count: 0 }),
      card({ id: "empty", name: "Empty", visibility: "archived", session_count: 0 }),
      card({ id: "occupied", name: "Occupied", visibility: "archived", session_count: 1 }),
      card({ id: "chat", name: "Chat", visibility: "archived", session_count: 0, purpose: "chat" }),
      card({ id: "assistant", name: "Assistant", visibility: "archived", session_count: 0, is_assistant: true }),
    ];
    vi.mocked(api.listProjectCards).mockResolvedValue(projects);
    vi.mocked(api.deleteProjectPermanently).mockResolvedValue({ warnings: [] });
    render(<ProjectsView navigate={navigate} scope="archived" />);
    await screen.findByText("Empty");
    expect(screen.queryByText("Normal")).toBeNull();
    expect(screen.queryByRole("button", { name: "新增项目" })).toBeNull();
    const row = (name: string) => within(screen.getByText(name).closest("article")!);
    expect((row("Empty").getByRole("button", { name: "永久删除项目" }) as HTMLButtonElement).disabled).toBe(false);
    expect((row("Occupied").getByRole("button", { name: "永久删除项目" }) as HTMLButtonElement).disabled).toBe(true);
    expect(row("Chat").queryByRole("button", { name: "永久删除项目" })).toBeNull();
    expect((row("Assistant").getByRole("button", { name: "永久删除项目" }) as HTMLButtonElement).disabled).toBe(false);
    expect(row("Assistant").getByText("智能体")).toBeTruthy();
    // Confirmation is required for single-item delete; delegates to root modal.
    fireEvent.click(row("Empty").getByRole("button", { name: "永久删除项目" }));
    expect(screen.getByText("确定永久删除 「Empty」？")).toBeTruthy();
    expect(api.deleteProjectPermanently).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.queryByText("确定永久删除 「Empty」？")).toBeNull();

    // Confirmation is required; bulk deletion excludes occupied and built-in projects.
    fireEvent.click(screen.getByRole("button", { name: "删除全部" }));
    expect(api.deleteProjectPermanently).not.toHaveBeenCalled();
    expect(screen.getByText(/档案目录及其中的文件也会永久删除/)).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: /^永久删除$/ })); });
    expect(api.deleteProjectPermanently).toHaveBeenCalledTimes(2);
    expect(api.deleteProjectPermanently).toHaveBeenCalledWith("empty");
    expect(api.deleteProjectPermanently).toHaveBeenCalledWith("assistant");
  });

});

it("removes a deleted project immediately and keeps it out of a late refresh", async () => {
  const gone = card({ id: "gone", name: "删除项目" });
  const kept = card({ id: "kept", name: "保留项目" });
  let finishRefresh!: (cards: ProjectCardData[]) => void;
  const refresh = new Promise<ProjectCardData[]>(resolve => { finishRefresh = resolve; });
  vi.mocked(api.listProjectCards).mockResolvedValueOnce([gone, kept]).mockReturnValueOnce(refresh);
  render(<ProjectsView navigate={navigate} />);
  await screen.findByText("删除项目");

  act(() => invalidateTargets({ projectIds: ["gone"] }));
  expect(screen.queryByText("删除项目")).toBeNull();
  expect(screen.getByText("保留项目")).toBeTruthy();

  await act(async () => finishRefresh([gone, { ...kept, name: "更新后的项目" }]));
  expect(screen.getByText("更新后的项目")).toBeTruthy();
  expect(screen.queryByText("删除项目")).toBeNull();
});

it("intersects complete lookup matches with project filters and preserves sorting", async () => {
  vi.mocked(api.listProjectCards).mockResolvedValue([
    card({ id: "late", name: "Late result", missing_path_count: 1 }),
    card({ id: "healthy", name: "Healthy result" }),
    card({ id: "archived", name: "Archived result", visibility: "archived", missing_path_count: 1 }),
  ]);
  vi.mocked(api.matchObjects).mockResolvedValue([...Array.from({length:30},(_,n)=>`outside-${n}`),"late","healthy","archived"]);
  render(<ProjectsView navigate={navigate} />);
  await screen.findByText("Healthy result");
  fireEvent.click(screen.getByRole("button", { name: /^筛选：/ }));
  fireEvent.click(screen.getByRole("option", { name: "有目录缺失" }));
  fireEvent.change(screen.getByRole("textbox", {name:"搜索项目"}), {target:{value:"result"}});
  await screen.findByText("Late result");
  expect(screen.queryByText("Healthy result")).toBeNull();
  expect(screen.queryByText("Archived result")).toBeNull();
  await waitFor(()=>expect(api.matchObjects).toHaveBeenCalledWith("project","result", ["late"]));
});
