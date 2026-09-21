import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import NewWorkstreamModal from "./NewWorkstreamModal";
import { api } from "../../api";
import type { CreateWorkstreamReport, Workstream } from "../../types";

vi.mock("../../api", () => ({
  api: {
    createWorkstream: vi.fn(),
    // PathListEditor / WorkspacePathField 的探测与快选；这里不关心结果，
    // 只要调用安全落地。
    probeWorkspacePath: vi.fn().mockResolvedValue(null),
    listRecentWorkspacePaths: vi.fn().mockResolvedValue([]),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: vi.fn(() => ({
    onDragDropEvent: vi.fn(() => Promise.resolve(() => {})),
  })),
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
  const input = screen.getByPlaceholderText(ADD_PLACEHOLDER);
  await act(async () => {
    fireEvent.change(input, { target: { value } });
  });
  await act(async () => {
    fireEvent.keyDown(input, { key: "Enter" });
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

describe("NewWorkstreamModal path list", () => {
  it("collects several paths in order, first one flagged as primary", async () => {
    render(<NewWorkstreamModal onClose={vi.fn()} />);
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
    render(<NewWorkstreamModal onClose={vi.fn()} onCreated={onCreated} />);
    await addPath("/repo/main");
    await addPath("/repo/docs");
    await submit();

    await waitFor(() =>
      expect(api.createWorkstream).toHaveBeenCalledWith("新流", "", ["/repo/main", "/repo/docs"])
    );
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(expect.objectContaining({ id: "w1" })));
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
    render(<NewWorkstreamModal onClose={onClose} onCreated={onCreated} />);
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
    render(<NewWorkstreamModal onClose={vi.fn()} onCreated={onCreated} />);
    await addPath("/repo/ghost");
    await submit();

    await screen.findByText("任务已创建（没有工作路径）");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    });
    expect(onCreated).toHaveBeenCalled();
  });

  it("refuses a duplicate path in the list instead of adding it twice", async () => {
    render(<NewWorkstreamModal onClose={vi.fn()} />);
    await addPath("/repo/main");
    await addPath("/repo/main");
    expect(await screen.findAllByText("/repo/main")).toHaveLength(1);
    screen.getByText("这条路径已经在列表里了。");
  });
});
