import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import WorkstreamCard from "./WorkstreamCard";
import type { WorkstreamCardData } from "../../types";
afterEach(cleanup);
const card = { id: "w1", title: "优化看板", lifecycle: "active", visibility: "normal", session_count: 1, latest_session: { id: "s1", agent: "codex" } } as WorkstreamCardData;
it("keeps card navigation separate from the new-session action", () => {
  const navigate = vi.fn();
  render(<WorkstreamCard card={card} mode="full" navigate={navigate} defaultAgent="codex" />);
  fireEvent.click(screen.getByRole("button", { name: "新建会话" }));
  expect(navigate).toHaveBeenCalledWith({ view: "new-session", workstreamId: "w1" });
  fireEvent.click(screen.getByRole("button", { name: "优化看板" }));
  expect(navigate).toHaveBeenCalledTimes(2);
  expect(navigate).toHaveBeenCalledWith({ view: "workstream", workstreamId: "w1" });
});
it("offers no continue button on the card — continue lives in the session header", () => {
  render(<WorkstreamCard card={card} mode="full" navigate={() => {}} defaultAgent="codex" />);
  expect(screen.queryByRole("button", { name: /继续/ })).toBeNull();
});
it("omits launch actions for archived tasks", () => {
  render(<WorkstreamCard card={{ ...card, visibility: "archived" }} mode="full" navigate={() => {}} defaultAgent="codex" />);
  expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
  expect(screen.getByText("回收站")).toBeTruthy();
});
