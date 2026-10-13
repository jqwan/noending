import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import { EVT_SYNCED } from "../../app/routes";
import { viewState } from "../../hooks/useViewState";
import type { SessionTracking, Workstream } from "../../types";
import SessionTaskTracking from "./SessionTaskTracking";
vi.mock("../../api", () => ({ api: { getSessionTracking: vi.fn(), setSessionTaskTracking: vi.fn() } }));
const task = (id: string, visibility: Workstream["visibility"] = "normal"): Workstream => ({ id, title: id, description: "", visibility, created_at: "", updated_at: "" });
const data = (): SessionTracking => ({ session: { id: "s", project_id: "p", tracked_workstream_ids: ["existing"] } as SessionTracking["session"], tracked_workstreams: [task("existing", "archived")], available_workstreams: [task("first"), task("second")] });
beforeEach(() => { vi.resetAllMocks(); viewState.clear(); vi.mocked(api.getSessionTracking).mockResolvedValue(data()); });
afterEach(cleanup);
it("hides with the session page and remembers open and closed state across navigation", async () => {
  const navigate = vi.fn();
  const layout = (visible = true) => <div hidden={!visible}><SessionTaskTracking sessionId="s" navigate={navigate} /></div>;
  const view = render(layout());
  expect(api.getSessionTracking).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "跟踪任务" }));
  fireEvent.click(await screen.findByRole("button", { name: "existing" }));
  expect(navigate).toHaveBeenCalledWith({ view: "workstream", workstreamId: "existing", initialTitle: "existing" });
  expect(screen.getAllByRole("checkbox").length).toBe(3);
  fireEvent.mouseDown(document.body);
  expect(screen.getByRole("complementary")).toBeTruthy();
  // AppShell keeps the session page mounted but hidden in other sections.
  view.rerender(layout(false));
  expect(screen.queryByRole("complementary")).toBeNull();
  view.rerender(layout());
  expect(screen.getByRole("complementary")).toBeTruthy();
  view.unmount();
  const restored = render(layout());
  expect(screen.getByRole("complementary")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "跟踪任务" }));
  expect(screen.queryByRole("complementary")).toBeNull();
  restored.unmount();
  render(layout());
  expect(screen.queryByRole("complementary")).toBeNull();
});

it("directly toggles multi-select checkboxes, supports collapse/expand and hide untracked filter", async () => {
  render(<SessionTaskTracking sessionId="s" navigate={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "跟踪任务" }));
  await screen.findByRole("button", { name: "existing" });

  // Filter: toggle hide untracked
  expect(screen.getByRole("checkbox", { name: "first" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "隐藏未跟踪任务" }));
  expect(screen.queryByRole("checkbox", { name: "first" })).toBeNull();
  expect(screen.getByRole("checkbox", { name: "existing" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "显示未跟踪任务" }));
  expect(screen.getByRole("checkbox", { name: "first" })).toBeTruthy();

  // Directly check untracked task
  fireEvent.click(screen.getByRole("checkbox", { name: "second" }));
  await waitFor(() => expect(api.setSessionTaskTracking).toHaveBeenCalledWith("s", "second", true));

  // Directly uncheck tracked task or use remove button
  fireEvent.click(screen.getByRole("button", { name: "取消跟踪：existing" }));
  await waitFor(() => expect(api.setSessionTaskTracking).toHaveBeenCalledWith("s", "existing", false));

  // Collapse and expand panel
  fireEvent.click(screen.getByRole("button", { name: "收起跟踪任务面板" }));
  expect(screen.queryByRole("checkbox")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "展开跟踪任务面板" }));
  expect(screen.getAllByRole("checkbox").length).toBe(3);
});

it("refreshes external changes and preserves a failed mutation's error after reloading", async () => {
  render(<SessionTaskTracking sessionId="s" navigate={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "跟踪任务" }));
  await screen.findByRole("button", { name: "existing" });
  vi.mocked(api.getSessionTracking).mockResolvedValue({ ...data(), tracked_workstreams: [task("changed")] });
  act(() => { window.dispatchEvent(new Event(EVT_SYNCED)); });
  await screen.findByRole("button", { name: "changed" });
  vi.mocked(api.setSessionTaskTracking).mockRejectedValue(new Error("任务已归档"));
  fireEvent.click(screen.getByRole("checkbox", { name: "first" }));
  expect((await screen.findByRole("alert")).textContent).toContain("任务已归档");
  await waitFor(() => expect(api.getSessionTracking).toHaveBeenCalledTimes(3));
  expect(screen.getByRole("alert").textContent).toContain("任务已归档");
});
