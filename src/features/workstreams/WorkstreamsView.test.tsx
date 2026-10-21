import { invalidateTargets, removedTargets } from "../../app/removedTargets";
import { act, renderHook, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import WorkstreamsView from "./WorkstreamsView";
import { api } from "../../api";
import { viewState } from "../../hooks/useViewState";
import type { WorkstreamCardData } from "../../types";

const state = vi.hoisted(() => ({ cards: [] as WorkstreamCardData[], refresh: vi.fn() }));
vi.mock("./useWorkstreamCards", () => ({ useWorkstreamCards: () => ({ cards: state.cards, defaultAgent: "codex", refresh: state.refresh, loadError: "" }) }));
vi.mock("../../api", () => ({ api: { matchObjects: vi.fn().mockResolvedValue([]), listWorkstreamCards: vi.fn(), getDefaultAgent: vi.fn(), archiveWorkstream: vi.fn(), restoreWorkstream: vi.fn(), deleteWorkstreamPermanently: vi.fn() } }));
vi.mock("../../components/Toast", () => ({ showToast: vi.fn() }));
const card = (id: string, visibility: WorkstreamCardData["visibility"] = "normal"): WorkstreamCardData => ({ id, projects: [], title: id, description: "", visibility, session_count: 1,  created_at: "", updated_at: "",  last_activity_at: null, latest_session: null });
beforeEach(() => { Object.values(removedTargets).forEach(ids => ids.clear()); vi.clearAllMocks(); viewState.clear(); localStorage.clear(); state.cards = [card("普通任务"), card("归档甲", "archived"), card("归档乙", "archived")]; });
afterEach(cleanup);

it("uses the same searchable cards for both scopes with the correct toolbar actions", async () => {
  vi.mocked(api.matchObjects).mockResolvedValue(["归档甲"]);
  const navigate = vi.fn();
  const { rerender } = render(<WorkstreamsView navigate={navigate} actionSeq={0} />);
  screen.getByRole("button", { name: "新建任务" });
  expect(screen.queryByRole("button", { name: "删除全部" })).toBeNull();
  screen.getByRole("button", { name: "普通任务" });
  rerender(<WorkstreamsView navigate={navigate} scope="archived" actionSeq={0} />);
  expect(screen.queryByRole("button", { name: "新建任务" })).toBeNull();
  expect(document.querySelectorAll(".task-card")).toHaveLength(2);
  fireEvent.change(screen.getByRole("textbox", { name: "搜索任务" }), { target: { value: "甲" } });
  await screen.findByRole("button", { name: "永久删除归档甲" });
  expect(document.querySelectorAll(".task-card")).toHaveLength(1);
});

it("delete all covers archived tasks even when the board is filtered", async () => {
  vi.mocked(api.deleteWorkstreamPermanently).mockResolvedValue(undefined);
  render(<WorkstreamsView navigate={() => {}} scope="archived" actionSeq={0} />);
  fireEvent.change(screen.getByRole("textbox", { name: "搜索任务" }), { target: { value: "甲" } });
  fireEvent.click(screen.getByRole("button", { name: "删除全部" }));
  const dialog = screen.getByRole("dialog");
  expect(dialog.textContent).toContain("2 个任务");
  fireEvent.click(within(dialog).getByRole("button", { name: "删除全部" }));
  await waitFor(() => expect(api.deleteWorkstreamPermanently).toHaveBeenCalledTimes(2));
  expect(api.deleteWorkstreamPermanently).toHaveBeenCalledWith("归档甲");
  expect(api.deleteWorkstreamPermanently).toHaveBeenCalledWith("归档乙");
  expect(api.deleteWorkstreamPermanently).not.toHaveBeenCalledWith("普通任务");
});

it("keeps card and list views available in both archive scopes", async () => {
  const navigate = vi.fn();
  const { rerender } = render(<WorkstreamsView navigate={navigate} actionSeq={0} />);
  expect(screen.getByRole("button", { name: "卡片视图" }).getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
  expect(screen.getByRole("button", { name: "列表视图" }).getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "归档普通任务" }));
  await waitFor(() => expect(api.archiveWorkstream).toHaveBeenCalledWith("普通任务"));
  rerender(<WorkstreamsView navigate={navigate} scope="archived" actionSeq={0} />);
  expect(screen.getByRole("button", { name: "列表视图" }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
  screen.getByRole("button", { name: "取消归档归档甲" });
  screen.getByRole("button", { name: "永久删除归档甲" });
  fireEvent.click(screen.getByRole("button", { name: "卡片视图" }));
  expect(screen.getByRole("button", { name: "卡片视图" }).getAttribute("aria-pressed")).toBe("true");
});

it("shows and filters every related project in both board views", () => {
  const multiple = { ...card("跨项目任务"), projects: [{ id: "p-a", name: "项目甲" }, { id: "p-b", name: "项目乙" }] };
  state.cards = [multiple, card("无项目任务")];
  render(<WorkstreamsView navigate={() => {}} actionSeq={0} />);
  const projectFilter = screen.getByText("全部项目").closest("select")!;
  fireEvent.change(projectFilter, { target: { value: "p-b" } });
  screen.getByRole("button", { name: "跨项目任务" });
  screen.getByRole("button", { name: "新建会话" });
  expect(screen.queryByRole("button", { name: "无项目任务" })).toBeNull();
  expect(screen.getByText("2 个项目")).toBeTruthy();
  expect(screen.getByTitle("项目甲、项目乙")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
  const listTask = screen.getByRole("button", { name: /跨项目任务.*项目甲.*项目乙/ });
  expect(within(listTask).getByText("项目甲")).toBeTruthy();
  expect(within(listTask).getByText("项目乙")).toBeTruthy();
  fireEvent.change(projectFilter, { target: { value: "none" } });
  screen.getByText("无项目任务");
  expect(screen.queryByRole("button", { name: "新建会话" })).toBeNull();
  expect(screen.queryByText("跨项目任务")).toBeNull();
});

it("removes deleted tasks from the real board data source even when an old request returns", async () => {
  const { useWorkstreamCards, clearWorkstreamCardsCache } = await vi.importActual<typeof import("./useWorkstreamCards")>("./useWorkstreamCards");
  clearWorkstreamCardsCache();
  vi.mocked(api.listWorkstreamCards).mockResolvedValue([card("gone"), card("kept")]);
  vi.mocked(api.getDefaultAgent).mockResolvedValue("codex");
  const { result } = renderHook(() => useWorkstreamCards());
  await waitFor(() => expect(result.current.cards?.map(w => w.id)).toEqual(["gone", "kept"]));
  act(() => invalidateTargets({ workstreamIds: ["gone"] }));
  expect(result.current.cards?.map(w => w.id)).toEqual(["kept"]);
  await act(async () => {});
  expect(result.current.cards?.map(w => w.id)).toEqual(["kept"]);
});
