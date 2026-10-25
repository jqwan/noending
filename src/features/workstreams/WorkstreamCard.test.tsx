import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import WorkstreamCard from "./WorkstreamCard";
import type { WorkstreamCardData } from "../../types";
afterEach(cleanup);
const card = { id: "w1", projects: [{ id: "p1", name: "主项目" }], description: "", created_at: "", updated_at: "",  last_activity_at: null,  title: "优化看板", visibility: "normal", session_count: 1, latest_session: { id: "s1", agent: "codex" } } as WorkstreamCardData;
it("keeps card navigation separate from the new-session action", () => {
  const navigate = vi.fn();
  render(<WorkstreamCard card={card} mode="full" navigate={navigate} defaultAgent="codex" />);
  fireEvent.click(screen.getByRole("button", { name: "新建会话" }));
  expect(navigate).toHaveBeenCalledWith({ view: "new-session", workstreamId: "w1" });
  fireEvent.click(screen.getByRole("button", { name: "优化看板" }));
  expect(navigate).toHaveBeenCalledTimes(2);
  expect(navigate).toHaveBeenCalledWith({ view: "workstream", workstreamId: "w1", initialTitle: "优化看板" });
});


it("archives without navigating, and offers unarchive/delete only on archived cards", () => {
  const navigate = vi.fn(); const archive = vi.fn(); const restore = vi.fn(); const remove = vi.fn();
  const props = { mode: "full" as const, navigate, defaultAgent: "codex" as const, onArchive: archive, onRestore: restore, onDelete: remove };
  const { rerender } = render(<WorkstreamCard card={card} {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "归档优化看板" }));
  expect(archive).toHaveBeenCalledWith(card);
  expect(navigate).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: /永久删除/ })).toBeNull();
  const archived = { ...card, visibility: "archived" as const };
  rerender(<WorkstreamCard card={archived} {...props} />);
  expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "取消归档优化看板" }));
  fireEvent.click(screen.getByRole("button", { name: "永久删除优化看板" }));
  expect(restore).toHaveBeenCalledWith(archived);
  expect(remove).toHaveBeenCalledWith(archived);
  expect(navigate).not.toHaveBeenCalled();
});

it("hides the new-session action when the task has no projects", () => {
  render(<WorkstreamCard card={{ ...card, projects: [] }} mode="full" navigate={vi.fn()} defaultAgent="codex" />);
  expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
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
  render(<WorkstreamCard card={cardData} mode="full" navigate={vi.fn()} defaultAgent="codex" />);
  expect(screen.getByText("这是任务描述内容")).toBeTruthy();
  expect(screen.getByText("2 个项目")).toBeTruthy();
  expect(screen.queryByText("前端项目")).toBeNull();
  expect(screen.queryByText("后端服务")).toBeNull();
});
