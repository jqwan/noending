import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ErrorBoundary from "./ErrorBoundary";

let boom = true;
function Boom() {
  if (boom) throw new Error("渲染炸了");
  return <div>内容回来了</div>;
}

// React 自己会把被捕获的错误打进 console.error；这里只想听我们自己的断言。
beforeEach(() => {
  boom = true;
  vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("keeps a render error inside the content area instead of taking the shell with it", () => {
  render(
    <div>
      <div>侧栏还在</div>
      <ErrorBoundary><Boom /></ErrorBoundary>
    </div>,
  );

  screen.getByText("侧栏还在");
  screen.getByText("这一页出错了");
  // 原始错误必须可见：黑屏之所以是黑屏，就是因为此前没有一句可报障的话。
  screen.getByText(/渲染炸了/);
});

it("retries the subtree once the failure clears", () => {
  render(<ErrorBoundary><Boom /></ErrorBoundary>);
  screen.getByText("这一页出错了");

  boom = false;
  fireEvent.click(screen.getByRole("button", { name: "重试" }));

  screen.getByText("内容回来了");
  expect(screen.queryByText("这一页出错了")).toBeNull();
});

it("clears the error when the route changes", () => {
  const view = render(<ErrorBoundary resetKey="a"><Boom /></ErrorBoundary>);
  screen.getByText("这一页出错了");

  // 同一个边界实例换到另一页：兜底要让位给新页面。
  view.rerender(<ErrorBoundary resetKey="b"><div>下一页</div></ErrorBoundary>);

  screen.getByText("下一页");
  expect(screen.queryByText("这一页出错了")).toBeNull();
});
