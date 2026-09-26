import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import WorkstreamFormModal from "./WorkstreamFormModal";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import type { CreateWorkstreamReport, Workstream, WorkstreamPath, WorkstreamPathRow } from "../../types";

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
    probeWorkspacePath: vi.fn().mockResolvedValue(null),
    listRecentWorkspacePaths: vi.fn().mockResolvedValue([]),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
}));

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

const ADD_PLACEHOLDER = "/path/to/目录 — 回车或点「添加」加入列表";

async function addPath(value: string) {
  vi.mocked(open).mockResolvedValueOnce(value);
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "新增目录" }));
  });
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

describe("WorkstreamFormModal path list", () => {
  it("collects several paths in order, first one flagged as primary", async () => {
    render(<WorkstreamFormModal onClose={vi.fn()} />);
    await addPath("/repo/main");
    await addPath("/repo/docs");

    screen.getByText("主路径");
    screen.getByText("第 2 条");
    screen.getByText("/repo/main");
    screen.getByText("/repo/docs");
  });

  it("submits every entry and reports the full list to the backend", async () => {
    vi.mocked(api.createWorkstream).mockResolvedValue(
      report("w1", [
        { raw: "/repo/main", accepted: true, canonical_path: "/repo/main", position: 0, project_name: "Main", reason: null },
        { raw: "/repo/docs", accepted: true, canonical_path: "/repo/docs", position: 1, project_name: "Docs", reason: null },
      ])
    );
    const onCreated = vi.fn();
    render(<WorkstreamFormModal onClose={vi.fn()} onCreated={onCreated} />);
    await addPath("/repo/main");
    await addPath("/repo/docs");
    await submit();

    await waitFor(() =>
      expect(api.createWorkstream).toHaveBeenCalledWith("新流", "", ["/repo/main", "/repo/docs"])
    );
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "w1" })));
  });

  it("does not create while the title is being composed by an IME", () => {
    const onCreated = vi.fn();
    render(<WorkstreamFormModal onClose={vi.fn()} onCreated={onCreated} />);
    const title = screen.getByPlaceholderText("例如：接口设计 / 行程规划 / 预算整理");
    fireEvent.change(title, { target: { value: "回车" } });
    fireEvent.keyDown(title, { key: "Enter", keyCode: 229 });
    expect(api.createWorkstream).not.toHaveBeenCalled();
    expect(onCreated).not.toHaveBeenCalled();
  });

  it("says per path what did not land instead of silently dropping it", async () => {
    vi.mocked(api.createWorkstream).mockResolvedValue(
      report("w2", [
        { raw: "/repo/main", accepted: true, canonical_path: "/repo/main", position: 0, project_name: "Main", reason: null },
        {
          raw: "/repo/ghost",
          accepted: false,
          canonical_path: null,
          position: null,
          project_name: null,
          reason: "该目录不能作为工作路径：需要一个可解析的绝对路径，且不能是 NoEnding 自留目录",
        },
      ])
    );
    const onCreated = vi.fn();
    const onClose = vi.fn();
    render(<WorkstreamFormModal onClose={onClose} onCreated={onCreated} />);
    await addPath("/repo/main");
    await addPath("/repo/ghost");
    await submit();

    // 结果面板逐条说破，而不是静默跳走。
    await screen.findByText("任务已创建（部分路径未接受）");
    screen.getByText(/NoEnding 自留目录/);
    screen.getByText(/第 1 条 · 项目 Main/);

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    });
    expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "w2" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("keeps the zero-path outcome honest when every path was refused", async () => {
    vi.mocked(api.createWorkstream).mockResolvedValue(
      report("w3", [
        {
          raw: "/repo/ghost",
          accepted: false,
          canonical_path: null,
          position: null,
          project_name: null,
          reason: "该目录不能作为工作路径：需要一个可解析的绝对路径，且不能是 NoEnding 自留目录",
        },
      ])
    );
    const onCreated = vi.fn();
    render(<WorkstreamFormModal onClose={vi.fn()} onCreated={onCreated} />);
    await addPath("/repo/ghost");
    await submit();

    await screen.findByText("任务已创建（没有工作目录）");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    });
    expect(onCreated).toHaveBeenCalled();
  });

  it("refuses a duplicate path in the list instead of adding it twice", async () => {
    render(<WorkstreamFormModal onClose={vi.fn()} />);
    await addPath("/repo/main");
    await addPath("/repo/main");
    expect(await screen.findAllByText("/repo/main")).toHaveLength(1);
    screen.getByText("这条路径已经在列表里了。");
  });
});

it("adds picked folders immediately and submits the chosen primary first", async () => {
  vi.mocked(open).mockResolvedValueOnce(["/repo/main", "/repo/docs"]);
  vi.mocked(api.createWorkstream).mockResolvedValue(report("picked", []));
  render(<WorkstreamFormModal onClose={vi.fn()} />);
  expect(screen.queryByPlaceholderText(ADD_PLACEHOLDER)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "新增目录" }));
  await screen.findByText("/repo/docs");
  fireEvent.click(screen.getAllByRole("button", { name: "设为主要" })[0]);
  await submit();
  expect(api.createWorkstream).toHaveBeenCalledWith("新流", "", ["/repo/docs", "/repo/main"]);
});

it("removes a picked path without opening its menu", async () => {
  vi.mocked(open).mockResolvedValueOnce("/repo/main");
  render(<WorkstreamFormModal onClose={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "新增目录" }));
  await screen.findByText("/repo/main");
  fireEvent.click(screen.getByRole("button", { name: "移除 /repo/main" }));
  expect(screen.queryByText("/repo/main")).toBeNull();
});

it("shows only directory paths and adds the selection without closing the task", async () => {
  vi.mocked(api.listRecentWorkspacePaths).mockResolvedValueOnce([
    { path: "/repo/docs", known: true, exists: true, project_name: "Docs", git_state: null, git_kind: null, last_used_at: null },
    { path: "/repo/main", known: true, exists: true, project_name: "Main", git_state: null, git_kind: null, last_used_at: null },
  ]);
  const close = vi.fn();
  render(<WorkstreamFormModal onClose={close} />);
  fireEvent.click(screen.getByRole("button", { name: "选择已有目录" }));
  await screen.findByText("/repo/docs");
  expect(screen.queryByRole("textbox", { name: "搜索已有目录" })).toBeNull();
  expect(screen.queryByText("Docs")).toBeNull();
  fireEvent.click(screen.getByRole("checkbox", { name: "/repo/docs" }));
  fireEvent.click(screen.getByRole("button", { name: "添加 (1)" }));
  expect(screen.queryByRole("dialog", { name: "选择已有目录" })).toBeNull();
  screen.getByText("/repo/docs");
  expect(close).not.toHaveBeenCalled();
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

  function row(id: string, position: number, canonicalPath: string): WorkstreamPathRow {
    return {
      id,
      workstream_id: "w1",
      workspace_path_id: `p-${id}`,
      position,
      created_at: "",
      canonical_path: canonicalPath,
      project_id: "p",
      project_name: "Project",
      exists: true,
    };
  }

  const rows = [row("main", 0, "/repo/main"), row("docs", 1, "/repo/docs")];

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

  it("prefills the current task and keeps 保存 disabled until something changes", () => {
    renderEdit();
    screen.getByDisplayValue("接口设计");
    screen.getByDisplayValue("整理 API 设计");
    screen.getByText("/repo/main");
    screen.getByText("/repo/docs");
    expect(saveButton().disabled).toBe(true);

    fireEvent.change(screen.getByDisplayValue("接口设计"), { target: { value: "接口设计 v2" } });
    expect(saveButton().disabled).toBe(false);
  });

  it("saves 标题/描述 as a whole object and leaves an untouched path list alone", async () => {
    const { onSaved, onClose } = renderEdit();
    fireEvent.change(screen.getByDisplayValue("接口设计"), { target: { value: "接口设计 v2" } });
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

  it("says what a dropped path means, then removes it and reorders the rest", async () => {
    const { onSaved } = renderEdit();
    expect(screen.queryByText(/保存后会从当前任务移除/)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "移除 /repo/docs" }));
    // 删路径不再牵连 Session —— 只说移除目录，并说明不改会话归属。
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

  it("only sends the newly added path to the backend and keeps the draft order", async () => {
    vi.mocked(open).mockResolvedValueOnce("/repo/api");
    resolveByPath({ "/repo/api": "p-new" });
    const { onSaved } = renderEdit();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "新增目录" }));
    });
    await screen.findByText("/repo/api");
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
    vi.mocked(open).mockResolvedValueOnce("/reserved/nope");
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

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "新增目录" }));
    });
    await screen.findByText("/reserved/nope");
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
    vi.mocked(open).mockResolvedValueOnce("/repo/main/");
    resolveByPath({ "/repo/main/": "p-main" });
    const { onSaved } = renderEdit();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "新增目录" }));
    });
    await screen.findByText("/repo/main/");
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

