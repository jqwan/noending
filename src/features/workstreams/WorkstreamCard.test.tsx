import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import WorkstreamCard from "./WorkstreamCard";
import type { WorkstreamCardData } from "../../types";
afterEach(cleanup);
const card = { id: "w1", projects: [{ id: "p1", name: "主项目" }], description: "", created_at: "", updated_at: "",  last_activity_at: null,  title: "优化看板", visibility: "normal" } as WorkstreamCardData;
it("archives without navigating, and offers unarchive/delete only on archived cards", () => {
  const navigate = vi.fn(); const archive = vi.fn(); const restore = vi.fn(); const remove = vi.fn();
  const props = { mode: "full" as const, navigate, onArchive: archive, onRestore: restore, onDelete: remove };
  const { rerender } = render(<WorkstreamCard card={card} {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "归档优化看板" }));
  expect(archive).toHaveBeenCalledWith(card);
  expect(navigate).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: /永久删除/ })).toBeNull();
  const archived = { ...card, visibility: "archived" as const };
  rerender(<WorkstreamCard card={archived} {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "取消归档优化看板" }));
  fireEvent.click(screen.getByRole("button", { name: "永久删除优化看板" }));
  expect(restore).toHaveBeenCalledWith(archived);
  expect(remove).toHaveBeenCalledWith(archived);
  expect(navigate).not.toHaveBeenCalled();
});

it("displays task overview and project count without individual project names", () => {
  const cardData = {
    ...card,
    description: "这是任务描述内容",
    projects: [
      { id: "p1", name: "前端项目" },
      { id: "p2", name: "后端服务" },
    ],
  };
  render(<WorkstreamCard card={cardData} mode="full" navigate={vi.fn()} />);
  expect(screen.getByText("这是任务描述内容")).toBeTruthy();
  expect(screen.getByText("2 个项目")).toBeTruthy();
  expect(screen.queryByText("前端项目")).toBeNull();
  expect(screen.queryByText("后端服务")).toBeNull();
});
