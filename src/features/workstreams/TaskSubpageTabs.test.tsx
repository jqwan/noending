import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import TaskSubpageTabs from "./TaskSubpageTabs";

afterEach(cleanup);

it("switches the task page between detail and collaboration without dropping the entry default", () => {
  const navigate = vi.fn();
  const view = render(
    <TaskSubpageTabs workstreamId="task-1" initialTitle="跨会话聊天室开发" navigate={navigate} />,
  );

  const detail = screen.getByRole("button", { name: "任务详情" });
  expect(detail.getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByRole("button", { name: "协作室" }).getAttribute("aria-pressed")).toBe("false");

  fireEvent.click(screen.getByRole("button", { name: "协作室" }));
  expect(navigate).toHaveBeenLastCalledWith({
    view: "workstream", workstreamId: "task-1", initialTitle: "跨会话聊天室开发", entry: "collaboration",
  });

  view.rerender(
    <TaskSubpageTabs workstreamId="task-1" entry="collaboration" initialTitle="跨会话聊天室开发" navigate={navigate} />,
  );
  expect(screen.getByRole("button", { name: "协作室" }).getAttribute("aria-pressed")).toBe("true");

  // 回到详情时把 entry 清掉，路由上不留默认值。
  fireEvent.click(screen.getByRole("button", { name: "任务详情" }));
  expect(navigate).toHaveBeenLastCalledWith({
    view: "workstream", workstreamId: "task-1", initialTitle: "跨会话聊天室开发", entry: undefined,
  });
});
