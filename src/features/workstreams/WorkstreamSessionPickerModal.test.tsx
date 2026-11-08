import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import WorkstreamSessionPickerModal from "./WorkstreamSessionPickerModal";
import type { SessionListItem } from "../../types";

vi.mock("../../api", () => ({ api: { listSessions: vi.fn() } }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });
const session = (id: string, title: string, archived = false): SessionListItem => ({ id, title, agent: "codex", cwd: `/projects/${id}`, project_id: "p1", archived_at: archived ? "2026-11-08" : null, last_activity_at: "2026-11-08T01:00:00Z" } as SessionListItem);

it("limits candidates to linked projects, deduplicates sessions and keeps existing links selected", async () => {
  const linked = session("linked", "已有关联");
  vi.mocked(api.listSessions).mockImplementation(async project => project === "p1" ? [linked, session("new", "新候选", true)] : [linked]);
  const confirm = vi.fn().mockResolvedValue(undefined), close = vi.fn();
  render(<WorkstreamSessionPickerModal projectIds={["p1", "p2"]} linkedSessions={[linked]} onConfirm={confirm} onClose={close} />);
  const checkbox = await screen.findByRole("checkbox", { name: "新候选" });
  const existing = screen.getByRole("checkbox", { name: "已有关联" });
  expect(existing).toHaveProperty("checked", true);
  expect(existing).toHaveProperty("disabled", false);
  expect(screen.getAllByRole("checkbox")).toHaveLength(2);
  expect(vi.mocked(api.listSessions).mock.calls.map(args => args[0])).toEqual(["p1", "p2"]);
  expect(screen.getByText("已归档")).toBeTruthy();
  fireEvent.change(screen.getByRole("textbox", { name: "搜索已有会话" }), { target: { value: "新候选" } });
  expect(screen.queryByRole("checkbox", { name: "已有关联" })).toBeNull();
  fireEvent.click(checkbox);
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(confirm).toHaveBeenCalledWith([{ sessionId: "new", tracked: true }]);
});

it("keeps failed additions selected and excludes successful links from the retry", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([session("a", "会话 A"), session("b", "会话 B")]);
  const confirm = vi.fn().mockRejectedValueOnce(new Error("B 添加失败")).mockResolvedValueOnce(undefined);
  const close = vi.fn();
  const props = { projectIds: ["p1"], onConfirm: confirm, onClose: close };
  const view = render(<WorkstreamSessionPickerModal {...props} linkedSessions={[]} />);
  fireEvent.click(await screen.findByRole("checkbox", { name: "会话 A" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "会话 B" }));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await screen.findByRole("alert");
  expect(close).not.toHaveBeenCalled();
  view.rerender(<WorkstreamSessionPickerModal {...props} linkedSessions={[session("a", "会话 A")]} />);
  expect(screen.getByRole("checkbox", { name: "会话 A" })).toHaveProperty("checked", true);
  expect(screen.getByRole("checkbox", { name: "会话 B" })).toHaveProperty("checked", true);
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(confirm).toHaveBeenLastCalledWith([{ sessionId: "b", tracked: true }]);
});

it("does not offer a partial project list after a source read fails", async () => {
  vi.mocked(api.listSessions).mockImplementation(async project => { if (project === "p2") throw new Error("项目读取失败"); return [session("a", "会话 A")]; });
  render(<WorkstreamSessionPickerModal projectIds={["p1", "p2"]} linkedSessions={[]} onConfirm={vi.fn()} onClose={vi.fn()} />);
  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("项目读取失败");
  expect(screen.queryByRole("checkbox")).toBeNull();
  vi.mocked(api.listSessions).mockResolvedValue([session("a", "会话 A")]);
  fireEvent.click(within(alert).getByRole("button", { name: "重试" }));
  await screen.findByRole("checkbox", { name: "会话 A" });
});

it("saves explicit additions and removals without removing links added elsewhere", async () => {
  const old = session("old", "原有关联"), added = session("added", "新增候选"), external = session("external", "外部新增");
  vi.mocked(api.listSessions).mockResolvedValue([added, external]);
  const confirm = vi.fn().mockResolvedValue(undefined), close = vi.fn();
  const props = { projectIds: ["p1"], linkedSessions: [old], onConfirm: confirm, onClose: close };
  const view = render(<WorkstreamSessionPickerModal {...props} />);
  await screen.findByRole("checkbox", { name: "新增候选" });
  expect(screen.getByRole("button", { name: "保存" })).toHaveProperty("disabled", true);
  fireEvent.click(screen.getByRole("checkbox", { name: "原有关联" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "新增候选" }));
  view.rerender(<WorkstreamSessionPickerModal {...props} linkedSessions={[old, external]} />);
  expect(screen.getByRole("checkbox", { name: "外部新增" })).toHaveProperty("checked", true);
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(confirm).toHaveBeenCalledWith([{ sessionId: "old", tracked: false }, { sessionId: "added", tracked: true }]);
});

it("prevents unlinking members when collaboration starts while the dialog is open", async () => {
  const a = session("a", "成员 A"), b = session("b", "成员 B");
  vi.mocked(api.listSessions).mockResolvedValue([a, b]);
  const confirm = vi.fn().mockResolvedValue(undefined), close = vi.fn();
  const props = { projectIds: ["p1"], linkedSessions: [a, b], onConfirm: confirm, onClose: close };
  const view = render(<WorkstreamSessionPickerModal {...props} />);
  await screen.findByRole("checkbox", { name: "成员 A" });
  fireEvent.click(screen.getByRole("checkbox", { name: "成员 A" }));
  view.rerender(<WorkstreamSessionPickerModal {...props} lockedIds={["a"]} />);
  expect(screen.getByRole("checkbox", { name: "成员 A" })).toHaveProperty("checked", true);
  expect(screen.getByRole("checkbox", { name: "成员 A" })).toHaveProperty("disabled", true);
  expect(screen.getByRole("button", { name: "保存" })).toHaveProperty("disabled", true);
  fireEvent.click(screen.getByRole("checkbox", { name: "成员 B" }));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(confirm).toHaveBeenCalledWith([{ sessionId: "b", tracked: false }]);
});
