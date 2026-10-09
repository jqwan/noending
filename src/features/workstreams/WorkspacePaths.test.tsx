import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, it } from "vitest";
import WorkstreamPathList from "./WorkspacePaths";

afterEach(cleanup);

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
