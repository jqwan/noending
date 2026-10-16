import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import CommandPalette from "./CommandPalette";
import { api } from "../api";
import type { NavigationItem, NavigationResults } from "../types";
import type { Route } from "../app/routes";

vi.mock("../api", () => ({ api: { lookupNavigation: vi.fn() } }));
beforeEach(() => vi.mocked(api.lookupNavigation).mockReset().mockResolvedValue({ items: [], has_more: false }));
afterEach(cleanup);

function item(over: Partial<NavigationItem> = {}): NavigationItem {
  return { kind: "agent", ref_id: "codex", title: "Codex", hint: "Agent 详情", agent: "codex", archived: false, ...over };
}
const cases: { entry: NavigationItem; route: Route }[] = [
  { entry: item(), route: { view: "agents", agent: "codex" } },
  { entry: item({ kind: "project", ref_id: "p", title: "Project", hint: "/repo", agent: null }), route: { view: "project", projectId: "p", initialTitle: "Project" } },
  { entry: item({ kind: "workstream", ref_id: "w", title: "Task", hint: "", agent: null, archived: true }), route: { view: "workstream", workstreamId: "w", initialTitle: "Task" } },
  { entry: item({ kind: "session", ref_id: "s", title: "Conversation", hint: "/repo" }), route: { view: "session", sessionId: "s", initialTitle: "Conversation", initialAgent: "codex" } },
];
it.each(cases)("opens the corresponding detail for $entry.kind", async ({ entry, route }) => {
  vi.mocked(api.lookupNavigation).mockResolvedValue({ items: [entry], has_more: false });
  const navigate = vi.fn();
  const onClose = vi.fn();
  render(<CommandPalette navigate={navigate} onClose={onClose} />);
  const option = await screen.findByRole("option", { name: new RegExp(entry.title) });
  expect(screen.queryByText("新建会话")).toBeNull();
  if (entry.archived) expect(screen.getByText("已归档")).toBeTruthy();
  fireEvent.click(option);
  expect(navigate).toHaveBeenCalledWith(route);
  expect(onClose).toHaveBeenCalledTimes(1);
});

it("discards superseded lookup responses and never opens a stale result while typing", async () => {
  let resolveOld!: (value: NavigationResults) => void;
  vi.mocked(api.lookupNavigation).mockImplementation(query => query === "old"
    ? new Promise(resolve => { resolveOld = resolve; })
    : Promise.resolve({ items: [item({ title: query === "new" ? "New Agent" : "Recent Agent" })], has_more: false }));
  const navigate = vi.fn();
  render(<CommandPalette navigate={navigate} onClose={vi.fn()} />);
  await screen.findByRole("option", { name: /Recent Agent/ });
  const input = screen.getByRole("combobox");
  fireEvent.change(input, { target: { value: "old" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(navigate).not.toHaveBeenCalled();
  await waitFor(() => expect(api.lookupNavigation).toHaveBeenCalledWith("old"));
  fireEvent.change(input, { target: { value: "new" } });
  await screen.findByRole("option", { name: /New Agent/ });
  await act(async () => { resolveOld({ items: [item({ title: "Old Agent" })], has_more: false }); });
  expect(screen.queryByText("Old Agent")).toBeNull();
  expect(screen.queryByText("Recent Agent")).toBeNull();
  fireEvent.keyDown(input, { key: "Enter" });
  expect(navigate).toHaveBeenCalledWith({ view: "agents", agent: "codex" });
});

it("supports keyboard selection without treating IME confirmation as navigation", async () => {
  const project = cases[1].entry;
  vi.mocked(api.lookupNavigation).mockResolvedValue({ items: [item(), project], has_more: false });
  const navigate = vi.fn();
  render(<CommandPalette navigate={navigate} onClose={vi.fn()} />);
  await screen.findByRole("option", { name: /Project/ });
  const input = screen.getByRole("combobox");
  fireEvent.keyDown(input, { key: "Enter", keyCode: 229 });
  expect(navigate).not.toHaveBeenCalled();
  fireEvent.keyDown(input, { key: "ArrowDown" });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(navigate).toHaveBeenCalledWith(cases[1].route);
});

it("allows retry after a failed lookup and explains bounded results", async () => {
  vi.mocked(api.lookupNavigation).mockRejectedValueOnce(new Error("unavailable"))
    .mockResolvedValue({ items: [item({ agent: "qoder", ref_id: "qoder", title: "Qoder" })], has_more: true });
  render(<CommandPalette navigate={vi.fn()} onClose={vi.fn()} />);
  await screen.findByRole("alert");
  fireEvent.click(screen.getByRole("button", { name: "重试" }));
  await screen.findByRole("option", { name: /Qoder/ });
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.getByText(/补充名称或目录/)).toBeTruthy();
});
