import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import WorkstreamPathList from "./WorkspacePaths";
import type { WorkstreamPathRow } from "../../types";

afterEach(cleanup);

function row(id: string, position: number): WorkstreamPathRow {
  return {
    id,
    workstream_id: "w",
    workspace_path_id: id,
    position,
    created_at: "",
    canonical_path: "/repo/" + id,
    project_id: "p",
    project_name: "Project",
    exists: true,
  };
}

it("shows the ordered paths and marks only the first as primary", () => {
  render(<WorkstreamPathList paths={[row("main", 0), row("docs", 1)]} />);
  screen.getByText("工作目录");
  screen.getByText("/repo/main");
  screen.getByText("/repo/docs");
  // 顺序就是角色（§1.5）：第 1 条即主工作目录，第 2 条没有徽标。
  expect(screen.getAllByText("主目录")).toHaveLength(1);
});

it("leaves no write affordance behind — editing lives in the 编辑任务 dialog", () => {
  render(<WorkstreamPathList paths={[row("main", 0), row("docs", 1)]} />);
  for (const name of ["新增目录", "选择已有目录", "移除", "设为主要"]) {
    expect(screen.queryByRole("button", { name })).toBeNull();
  }
});

it("distinguishes 还没读到 from 真的没有工作目录", () => {
  const { unmount } = render(<WorkstreamPathList paths={null} />);
  screen.getByText("读取工作目录…");
  unmount();

  render(<WorkstreamPathList paths={[]} />);
  screen.getByText("未设置工作目录，新会话使用默认目录。");
});

it("surfaces a read failure instead of pretending there are no paths", () => {
  render(<WorkstreamPathList paths={null} error="读取工作目录失败：boom" />);
  screen.getByText("读取工作目录失败：boom");
});
