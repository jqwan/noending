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
it("shows tracked tasks inline and follows the containing sidebar visibility", async () => {
  const navigate = vi.fn();
  const layout = (visible = true) => <aside hidden={!visible}><SessionTaskTracking sessionId="s" navigate={navigate} /></aside>;
  const view = render(layout());
  fireEvent.click(await screen.findByRole("button", { name: "existing" }));
  expect(navigate).toHaveBeenCalledWith({ view: "workstream", workstreamId: "existing", initialTitle: "existing" });
  view.rerender(layout(false));
  expect(screen.queryByRole("region", { name: "跟踪任务" })).toBeNull();
  view.rerender(layout());
  expect(screen.getByRole("button", { name: "existing" })).toBeTruthy();
  view.unmount();
  render(layout());
  expect(await screen.findByRole("button", { name: "existing" })).toBeTruthy();
});

it("opens task selection modal, supports multi-select checkboxes and quick untracking", async () => {
  render(<SessionTaskTracking sessionId="s" navigate={vi.fn()} />);
  await screen.findByRole("button", { name: "existing" });

  // Open modal via plus button
  fireEvent.click(screen.getByRole("button", { name: "添加跟踪任务" }));
  expect(await screen.findByRole("dialog", { name: "选择跟踪任务" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "新建任务" })).toBeTruthy();
  expect(screen.getAllByRole("checkbox").length).toBe(3);

  // Directly check untracked task in modal
  fireEvent.click(screen.getByRole("checkbox", { name: "second" }));
  await waitFor(() => expect(api.setSessionTaskTracking).toHaveBeenCalledWith("s", "second", true));

  // Directly uncheck tracked task or use remove button in panel
  fireEvent.click(screen.getByRole("button", { name: "取消跟踪：existing" }));
  await waitFor(() => expect(api.setSessionTaskTracking).toHaveBeenCalledWith("s", "existing", false));
});

it("refreshes external changes and preserves a failed mutation's error in modal", async () => {
  render(<SessionTaskTracking sessionId="s" navigate={vi.fn()} />);
  await screen.findByRole("button", { name: "existing" });
  vi.mocked(api.getSessionTracking).mockResolvedValue({ ...data(), tracked_workstreams: [task("changed")] });
  act(() => { window.dispatchEvent(new Event(EVT_SYNCED)); });
  await screen.findByRole("button", { name: "changed" });

  // Open modal and toggle with error
  fireEvent.click(screen.getByRole("button", { name: "添加跟踪任务" }));
  await screen.findByRole("dialog", { name: "选择跟踪任务" });
  vi.mocked(api.setSessionTaskTracking).mockRejectedValue(new Error("任务已归档"));
  fireEvent.click(screen.getByRole("checkbox", { name: "first" }));
  expect((await screen.findByRole("alert")).textContent).toContain("任务已归档");
});
