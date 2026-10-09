import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import WorkstreamCard from "./WorkstreamCard";
import type { WorkstreamCardData } from "../../types";
afterEach(cleanup);
const card = { id: "w1", projects: [], description: "", created_at: "", updated_at: "", current_state: null, goal: null, last_activity_at: null, path_count: 0, title: "优化看板", visibility: "normal", session_count: 1, latest_session: { id: "s1", agent: "codex" } } as WorkstreamCardData;
it("keeps card navigation separate from the new-session action", () => {
  const navigate = vi.fn();
  render(<WorkstreamCard card={card} mode="full" navigate={navigate} defaultAgent="codex" />);
  fireEvent.click(screen.getByRole("button", { name: "新建会话" }));
  expect(navigate).toHaveBeenCalledWith({ view: "new-session", workstreamId: "w1" });
  fireEvent.click(screen.getByRole("button", { name: "优化看板" }));
  expect(navigate).toHaveBeenCalledTimes(2);
  expect(navigate).toHaveBeenCalledWith({ view: "workstream", workstreamId: "w1" });
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
