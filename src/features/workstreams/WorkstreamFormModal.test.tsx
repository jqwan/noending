import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import WorkstreamFormModal from "./WorkstreamFormModal";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import type {
  CreateWorkstreamReport,
  PathProbe,
  ProjectCardData,
  Workstream,
  WorkstreamPath,
  WorkstreamPathRow,
} from "../../types";

vi.mock("../../api", () => ({
  api: {
    createWorkstream: vi.fn(),
    // 编辑模式的落盘命令。
    updateWorkstream: vi.fn(),
    addWorkstreamPath: vi.fn(),
    removeWorkstreamPath: vi.fn(),
    reorderWorkstreamPaths: vi.fn(),
    // PathListEditor / WorkspacePathField 的探测与快选；这里不关心结果，
    // 只要调用安全落地。
    probeWorkspacePath: vi.fn().mockResolvedValue(null as unknown as PathProbe),
    listRecentWorkspacePaths: vi.fn().mockResolvedValue([]),
    listProjectCards: vi.fn().mockResolvedValue([]),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
}));

beforeEach(() => {
  vi.mocked(api.listProjectCards).mockResolvedValue([]);
  vi.mocked(api.probeWorkspacePath).mockResolvedValue(null as unknown as PathProbe);
  vi.mocked(open).mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function workstream(id: string): Workstream {
  return {
    id,
    title: "新流",
    description: "",
    lifecycle: "active",
    visibility: "normal",
    created_at: "2026-09-21T00:00:00+00:00",
    updated_at: "2026-09-21T00:00:00+00:00",
  };
}

function report(id: string, paths: CreateWorkstreamReport["paths"]): CreateWorkstreamReport {
  return { workstream: workstream(id), paths };
}

function projectCard(id: string, name: string, paths: string[]): ProjectCardData {
  return {
    id,
    name,
    name_customized: false,
    has_git_identity: false,
    path_count: paths.length,
    missing_path_count: 0,
    primary_workstream_count: 0,
    related_workstream_count: 0,
    session_count: 0,
    representative_paths: paths.slice(0, 2),
    search_paths: paths,
    last_activity_at: null,
    updated_at: "2026-09-21T00:00:00+00:00",
  };
}

async function submit() {
  await act(async () => {
    fireEvent.change(screen.getByPlaceholderText("例如：接口设计 / 行程规划 / 预算整理"), {
      target: { value: "新流" },
    });
  });
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "创建" }));
  });
}

describe("WorkstreamFormModal project selection", () => {
  it("renders project selection with 'NoEnding Workspace' and no directory picker by default", async () => {
    render(<WorkstreamFormModal onClose={vi.fn()} />);
    expect(screen.getByText("NoEnding Workspace")).toBeTruthy();
    expect(screen.queryByText("主项目")).toBeNull();
    expect(screen.getByRole("combobox", { name: "关联项目选择" })).toBeTruthy();
    expect(screen.queryByText("工作目录（可选，可多条）")).toBeNull();
    expect(screen.queryByRole("button", { name: "新增目录" })).toBeNull();
    expect(screen.queryByText(/默认关联该项目下的所有工作目录/)).toBeNull();
  });

  it("submits empty paths when created with default 'NoEnding Workspace' without paths", async () => {
    vi.mocked(api.createWorkstream).mockResolvedValue(report("w1", []));
    const onCreated = vi.fn();
    render(<WorkstreamFormModal onClose={vi.fn()} onCreated={onCreated} />);
    await submit();

    await waitFor(() => expect(api.createWorkstream).toHaveBeenCalledWith("新流", "", []));
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "w1" })));
  });

  it("selects an existing project and submits all directories under it without showing prompt or directory picker", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValueOnce([
      projectCard("p1", "项目 A", ["/repo/main", "/repo/worktree-1"]),
    ]);
    vi.mocked(api.createWorkstream).mockResolvedValue(report("w2", []));

    render(<WorkstreamFormModal onClose={vi.fn()} />);
    await screen.findByRole("option", { name: "项目 A" });

    // 移除默认项目，添加项目 A
    fireEvent.click(screen.getByRole("button", { name: "移除项目 NoEnding Workspace" }));
    const select = screen.getByRole("combobox", { name: "关联项目选择" });
    fireEvent.change(select, { target: { value: "p1" } });

    expect(screen.queryByText(/默认关联该项目下的所有工作目录/)).toBeNull();
    expect(screen.queryByText("工作目录（可选，可多条）")).toBeNull();

    await submit();
    await waitFor(() =>
      expect(api.createWorkstream).toHaveBeenCalledWith("新流", "", ["/repo/main", "/repo/worktree-1"]),
    );
  });

  it("supports associating multiple projects", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValueOnce([
      projectCard("p1", "项目 A", ["/repo/a1", "/repo/a2"]),
      projectCard("p2", "项目 B", ["/repo/b1"]),
    ]);
    vi.mocked(api.createWorkstream).mockResolvedValue(report("w-multi", []));

    render(<WorkstreamFormModal onClose={vi.fn()} />);
    await screen.findByRole("option", { name: "项目 A" });

    // 移除默认项目，添加项目 A 和项目 B
    fireEvent.click(screen.getByRole("button", { name: "移除项目 NoEnding Workspace" }));
    const select = screen.getByRole("combobox", { name: "关联项目选择" });
    fireEvent.change(select, { target: { value: "p1" } });
    fireEvent.change(select, { target: { value: "p2" } });

    expect(screen.getByText("项目 A")).toBeTruthy();
    expect(screen.getByText("项目 B")).toBeTruthy();

    await submit();
    await waitFor(() =>
      expect(api.createWorkstream).toHaveBeenCalledWith("新流", "", ["/repo/a1", "/repo/a2", "/repo/b1"]),
    );
  });

  it("automatically selects the existing project when picked directory belongs to it", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValueOnce([
      projectCard("p1", "项目 A", ["/repo/main", "/repo/worktree-1"]),
    ]);
    vi.mocked(open).mockResolvedValueOnce("/repo/main/sub");
    vi.mocked(api.probeWorkspacePath).mockResolvedValueOnce({
      raw: "/repo/main/sub",
      status: "ok",
      canonical_path: "/repo/main/sub",
      exists: true,
      git_state: "detected",
      git_kind: "worktree",
      project: { id: "p1", name: "项目 A", known: true },
    });
    vi.mocked(api.createWorkstream).mockResolvedValue(report("w3", []));

    render(<WorkstreamFormModal onClose={vi.fn()} />);
    await screen.findByRole("option", { name: "项目 A" });

    fireEvent.click(screen.getByRole("button", { name: "移除项目 NoEnding Workspace" }));

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "新增项目" }));
    });

    expect(screen.getByText("项目 A")).toBeTruthy();
    expect(screen.queryByText(/默认关联该项目下的所有工作目录/)).toBeNull();

    await submit();
    await waitFor(() =>
      expect(api.createWorkstream).toHaveBeenCalledWith("新流", "", ["/repo/main", "/repo/worktree-1"]),
    );
  });

  it("adds a new project option and selects it when picked directory is new", async () => {
    vi.mocked(open).mockResolvedValueOnce("/new/custom/repo");
    vi.mocked(api.probeWorkspacePath).mockResolvedValueOnce({
      raw: "/new/custom/repo",
      status: "ok",
      canonical_path: "/canonical/custom/repo",
      exists: true,
      git_state: "detected",
      git_kind: "repo",
      project: { id: null, name: "repo", known: false },
    });
    vi.mocked(api.createWorkstream).mockResolvedValue(report("w4", []));

    render(<WorkstreamFormModal onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "移除项目 NoEnding Workspace" }));

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "新增项目" }));
    });

    expect(screen.getByText("repo")).toBeTruthy();
    expect(screen.queryByText(/默认关联该项目下的所有工作目录/)).toBeNull();

    await submit();
    await waitFor(() =>
      expect(api.createWorkstream).toHaveBeenCalledWith("新流", "", ["/canonical/custom/repo"]),
    );
  });

  it("does not create while the title is being composed by an IME", async () => {
    const onCreated = vi.fn();
    render(<WorkstreamFormModal onClose={vi.fn()} onCreated={onCreated} />);
    const title = screen.getByPlaceholderText("例如：接口设计 / 行程规划 / 预算整理");
    await act(async () => {
      fireEvent.change(title, { target: { value: "回车" } });
      fireEvent.keyDown(title, { key: "Enter", keyCode: 229 });
    });
    expect(api.createWorkstream).not.toHaveBeenCalled();
    expect(onCreated).not.toHaveBeenCalled();
  });

  it("says per path what did not land instead of silently dropping it", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValueOnce([
      projectCard("p1", "Main", ["/repo/main", "/repo/ghost"]),
    ]);
    vi.mocked(api.createWorkstream).mockResolvedValue(
      report("w5", [
        { raw: "/repo/main", accepted: true, canonical_path: "/repo/main", position: 0, project_name: "Main", reason: null },
        {
          raw: "/repo/ghost",
          accepted: false,
          canonical_path: null,
          position: null,
          project_name: null,
          reason: "该目录不能作为工作路径：需要一个可解析的绝对路径，且不能是 NoEnding 自留目录",
        },
      ]),
    );
    const onCreated = vi.fn();
    const onClose = vi.fn();
    render(<WorkstreamFormModal onClose={onClose} onCreated={onCreated} />);
    await screen.findByRole("option", { name: "Main" });

    fireEvent.click(screen.getByRole("button", { name: "移除项目 NoEnding Workspace" }));
    const select = screen.getByRole("combobox", { name: "关联项目选择" });
    fireEvent.change(select, { target: { value: "p1" } });

    await submit();

    // 结果面板逐条说破，而不是静默跳走。
    await screen.findByText("任务已创建（部分路径未接受）");
    screen.getByText(/NoEnding 自留目录/);
    screen.getByText(/第 1 条 · 项目 Main/);

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    });
    expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "w5" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("keeps the zero-path outcome honest when every path was refused", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValueOnce([
      projectCard("p1", "Ghost", ["/repo/ghost"]),
    ]);
    vi.mocked(api.createWorkstream).mockResolvedValue(
      report("w6", [
        {
          raw: "/repo/ghost",
          accepted: false,
          canonical_path: null,
          position: null,
          project_name: null,
          reason: "该目录不能作为工作路径：需要一个可解析的绝对路径，且不能是 NoEnding 自留目录",
        },
      ]),
    );
    const onCreated = vi.fn();
    render(<WorkstreamFormModal onClose={vi.fn()} onCreated={onCreated} />);
    await screen.findByRole("option", { name: "Ghost" });

    fireEvent.click(screen.getByRole("button", { name: "移除项目 NoEnding Workspace" }));
    const select = screen.getByRole("combobox", { name: "关联项目选择" });
    fireEvent.change(select, { target: { value: "p1" } });

    await submit();

    await screen.findByText("任务已创建（没有工作目录）");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    });
    expect(onCreated).toHaveBeenCalled();
  });
});

describe("WorkstreamFormModal edit mode", () => {
  function current(): Workstream {
    return {
      id: "w1",
      title: "接口设计",
      description: "整理 API 设计",
      lifecycle: "active",
      visibility: "normal",
      created_at: "2026-09-21T00:00:00+00:00",
      updated_at: "2026-09-21T00:00:00+00:00",
    };
  }

  function row(
    id: string,
    position: number,
    canonicalPath: string,
    projectId = "p1",
    projectName = "Project 1",
  ): WorkstreamPathRow {
    return {
      id,
      workstream_id: "w1",
      workspace_path_id: `p-${id}`,
      position,
      created_at: "",
      canonical_path: canonicalPath,
      project_id: projectId,
      project_name: projectName,
      exists: true,
    };
  }

  const rows = [
    row("main", 0, "/repo/main", "p1", "Project 1"),
    row("docs", 1, "/repo/docs", "p2", "Project 2"),
  ];

  /** 后端解析新加路径的 identity：只有用户新加的草稿才会走到这里。 */
  function resolveByPath(map: Record<string, string>) {
    vi.mocked(api.addWorkstreamPath).mockImplementation(
      async (_workstreamId: string, path: string): Promise<WorkstreamPath> => ({
        id: `wp-${map[path]}`,
        workstream_id: "w1",
        workspace_path_id: map[path],
        position: 0,
        created_at: "",
      }),
    );
  }

  function renderEdit() {
    const onSaved = vi.fn();
    const onClose = vi.fn();
    render(<WorkstreamFormModal workstream={current()} paths={rows} onSaved={onSaved} onClose={onClose} />);
    return { onSaved, onClose };
  }

  const saveButton = () => screen.getByRole("button", { name: "保存" }) as HTMLButtonElement;

  it("prefills the current task and keeps 保存 disabled until something changes", async () => {
    renderEdit();
    screen.getByDisplayValue("接口设计");
    screen.getByDisplayValue("整理 API 设计");
    screen.getByText("Project 1");
    screen.getByText("Project 2");
    expect(saveButton().disabled).toBe(true);

    await act(async () => {
      fireEvent.change(screen.getByDisplayValue("接口设计"), { target: { value: "接口设计 v2" } });
    });
    expect(saveButton().disabled).toBe(false);
  });

  it("saves 标题/描述 as a whole object and leaves an untouched path list alone", async () => {
    const { onSaved, onClose } = renderEdit();
    await act(async () => {
      fireEvent.change(screen.getByDisplayValue("接口设计"), { target: { value: "接口设计 v2" } });
    });
    await act(async () => {
      fireEvent.click(saveButton());
    });

    expect(api.updateWorkstream).toHaveBeenCalledWith(
      expect.objectContaining({ id: "w1", title: "接口设计 v2", description: "整理 API 设计" }),
    );
    expect(api.addWorkstreamPath).not.toHaveBeenCalled();
    expect(api.removeWorkstreamPath).not.toHaveBeenCalled();
    expect(api.reorderWorkstreamPaths).not.toHaveBeenCalled();
    expect(onSaved).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("says what a dropped project means, then removes its paths and reorders the rest", async () => {
    const { onSaved } = renderEdit();
    expect(screen.queryByText(/保存后会从当前任务移除/)).toBeNull();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "移除项目 Project 2" }));
    });
    // 删项目下路径不再牵连 Session —— 只说移除目录，并说明不改会话归属。
    screen.getByText(/保存后会从当前任务移除 1 条工作目录/);
    screen.getByText(/不会删除会话，也不会修改已有会话的所属任务/);

    await act(async () => {
      fireEvent.click(saveButton());
    });
    await waitFor(() => expect(api.removeWorkstreamPath).toHaveBeenCalledWith("w1", "docs"));
    expect(api.reorderWorkstreamPaths).toHaveBeenCalledWith("w1", ["p-main"]);
    expect(api.addWorkstreamPath).not.toHaveBeenCalled();
    expect(api.updateWorkstream).not.toHaveBeenCalled();
    expect(onSaved).toHaveBeenCalledOnce();
  });

  it("only sends the newly added project paths to the backend and keeps the draft order", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValueOnce([
      projectCard("p3", "Project 3", ["/repo/api"]),
    ]);
    resolveByPath({ "/repo/api": "p-new" });
    const { onSaved } = renderEdit();

    await screen.findByRole("option", { name: "Project 3" });
    fireEvent.change(screen.getByRole("combobox", { name: "关联项目选择" }), {
      target: { value: "p3" },
    });
    screen.getByText("Project 3");

    await act(async () => {
      fireEvent.click(saveButton());
    });

    expect(api.addWorkstreamPath).toHaveBeenCalledTimes(1);
    expect(api.addWorkstreamPath).toHaveBeenCalledWith("w1", "/repo/api");
    expect(api.reorderWorkstreamPaths).toHaveBeenCalledWith("w1", ["p-main", "p-docs", "p-new"]);
    expect(api.removeWorkstreamPath).not.toHaveBeenCalled();
    expect(onSaved).toHaveBeenCalledOnce();
  });

  it("reports a path the backend refused instead of silently dropping it", async () => {
    vi.mocked(api.listProjectCards).mockResolvedValueOnce([
      projectCard("p3", "Project 3", ["/reserved/nope"]),
    ]);
    vi.mocked(api.addWorkstreamPath).mockImplementation(
      async (_workstreamId: string, path: string): Promise<WorkstreamPath> => {
        if (path === "/reserved/nope") throw new Error("需要一个可解析的绝对路径");
        return {
          id: `wp-${path}`,
          workstream_id: "w1",
          workspace_path_id: `p${path}`,
          position: 0,
          created_at: "",
        };
      },
    );
    const { onSaved } = renderEdit();

    await screen.findByRole("option", { name: "Project 3" });
    fireEvent.change(screen.getByRole("combobox", { name: "关联项目选择" }), {
      target: { value: "p3" },
    });

    await act(async () => {
      fireEvent.click(saveButton());
    });

    await screen.findByText("任务已保存（部分路径未生效）");
    screen.getByText(/需要一个可解析的绝对路径/);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    });
    expect(onSaved).toHaveBeenCalledOnce();
  });

  it("reports two spellings of one directory instead of keeping a duplicate", async () => {
    // 后端把 "/repo/main/" 解析回已有的 p-main：前端不替它决定这是不是同一条。
    vi.mocked(api.listProjectCards).mockResolvedValueOnce([
      projectCard("p3", "Project 3", ["/repo/main/"]),
    ]);
    resolveByPath({ "/repo/main/": "p-main" });
    const { onSaved } = renderEdit();

    await screen.findByRole("option", { name: "Project 3" });
    fireEvent.change(screen.getByRole("combobox", { name: "关联项目选择" }), {
      target: { value: "p3" },
    });

    await act(async () => {
      fireEvent.click(saveButton());
    });

    await screen.findByText("任务已保存（部分路径未生效）");
    screen.getByText(/与前面一条指向同一目录/);
    expect(api.reorderWorkstreamPaths).toHaveBeenCalledWith("w1", ["p-main", "p-docs"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    });
    expect(onSaved).toHaveBeenCalledOnce();
  });
});
