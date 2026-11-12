import { clearReadResources } from "../../hooks/readResource";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import TerminalMailbox from "./TerminalMailbox";
import { MailboxHistory } from "./MailboxMessages";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import { viewState } from "../../hooks/useViewState";
import { copyToClipboard } from "../../components/common";
import type { MailboxMessage, TerminalNotifications } from "../../types";
const { handlers } = vi.hoisted(() => ({ handlers: new Map<string, Set<() => void>>() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name: string, callback: () => void) => {
  const set = handlers.get(name) ?? new Set(); set.add(callback); handlers.set(name, set); return () => set.delete(callback);
}) }));
vi.mock("../../api", () => ({ api: {
  mailboxList: vi.fn(), mailboxSend: vi.fn(), mailboxEdit: vi.fn(), mailboxRemind: vi.fn(),
  mailboxNotificationHint: vi.fn(), terminalAutoNotifications: vi.fn(), terminalCacheImage: vi.fn(),
} }));
vi.mock("../../components/common", async importOriginal => ({ ...await importOriginal<typeof import("../../components/common")>(), copyToClipboard: vi.fn() }));
const message: MailboxMessage = { notify_immediately: false, id: "mail-1", recipient_session_id: "self", sender_session_id: "other", sender: { title: "发件会话", agent: "codex" }, body: "external instructions", created_at: "2026-10-24T01:00:00Z", read_at: null, revision: 1 };
const notification: TerminalNotifications = { terminal_id: "t", session_id: "self", enabled: false, writing: false, input_revision: 1, identity_revision: 2, revision: 1, error: null };
const navigate = vi.fn();
function mount(n = notification) { return render(<TerminalMailbox navigate={navigate} terminalId="t" sessionId={n.session_id} notification={n} error="" apply={vi.fn()} refresh={vi.fn().mockResolvedValue(undefined)} />); }
beforeEach(() => {
  clearReadResources();
  vi.resetAllMocks(); handlers.clear(); viewState.clear();
  vi.mocked(copyToClipboard).mockResolvedValue(true);
  vi.mocked(listen).mockImplementation(async (name,callback) => {
    const fn=callback as unknown as () => void; const set=handlers.get(name) ?? new Set<() => void>();set.add(fn);handlers.set(name,set);return () => {set.delete(fn);};
  });
  vi.mocked(api.mailboxList).mockResolvedValue({ items: [{ ...message }], total: 1, unread: 1, next_cursor: null });
  vi.mocked(api.mailboxNotificationHint).mockResolvedValue(false);
  vi.mocked(api.mailboxEdit).mockResolvedValue({ ...message, body: "edited", revision: 2 });
  vi.mocked(api.mailboxRemind).mockResolvedValue(undefined);
  vi.mocked(api.terminalAutoNotifications).mockResolvedValue({ ...notification, enabled: true, revision: 2 });
});
afterEach(cleanup);
it.each(["history", "terminal"])("opens and copies a sender independently from previewing mail in the %s", async kind => {
  if (kind === "history") render(<MailboxHistory sessionId="self" navigate={navigate} />);
  else mount();
  const sender = await screen.findByRole("button", { name: "打开发件会话：发件会话" });
  expect(sender.title).toContain("NoEnding ID：other");
  fireEvent.click(screen.getByRole("button", { name: "复制发件会话 ID" }));
  expect(copyToClipboard).toHaveBeenCalledWith("NoEnding ID：other");
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(navigate).not.toHaveBeenCalled();
  fireEvent.click(sender);
  expect(navigate).toHaveBeenLastCalledWith({ view: "session", sessionId: "other", initialTitle: "发件会话", initialAgent: "codex" });
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: `查看消息：${message.body}` }));
  const dialog = screen.getByRole("dialog", { name: "信箱消息" });
  expect(within(dialog).getByText("other")).toBeTruthy();
  fireEvent.click(within(dialog).getByRole("button", { name: "打开发件会话：发件会话" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(navigate).toHaveBeenCalledTimes(2);
});

it("keeps deleted sender provenance copyable and distinguishes user mail", async () => {
  vi.mocked(api.mailboxList).mockResolvedValue({ items: [{ ...message, sender: undefined }, { ...message, id: "user-mail", sender_session_id: null, sender: undefined }], total: 2, unread: 2, next_cursor: null });
  mount();
  expect(await screen.findByText("已删除会话 other")).toBeTruthy();
  expect(screen.getByText("用户")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /^打开发件会话：/ })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "复制发件会话 ID" }));
  expect(copyToClipboard).toHaveBeenCalledWith("NoEnding ID：other");
});
it("keeps latest unread arrivals first when the terminal sidebar loads older messages", async () => {
  vi.mocked(api.mailboxList).mockImplementation(async (_session, _filter, cursor) => cursor == null
    ? { items: [{ ...message, body: "最新未读消息", created_at: "2026-10-24T03:00:00Z" }], total: 2, unread: 2, next_cursor: 2 }
    : { items: [{ ...message, id: "older", body: "较早未读消息" }], total: 2, unread: 2, next_cursor: null });
  mount();
  await screen.findByRole("button", { name: "查看消息：最新未读消息" });
  fireEvent.click(screen.getByRole("button", { name: "加载更多" }));
  await screen.findByRole("button", { name: "查看消息：较早未读消息" });
  expect(screen.getAllByRole("button", { name: /^查看消息：/ }).map(button => button.getAttribute("aria-label")))
    .toEqual(["查看消息：最新未读消息", "查看消息：较早未读消息"]);
  expect(api.mailboxList).toHaveBeenCalledWith("self", "unread", 2);
});

it("restores mailbox history on return while a fresh read is pending", async () => {
  const first = render(<MailboxHistory navigate={navigate} sessionId="self" />);
  await screen.findByRole("button", { name: `查看消息：${message.body}` });
  first.unmount();
  vi.mocked(api.mailboxList).mockImplementation(() => new Promise(() => {}));
  render(<MailboxHistory navigate={navigate} sessionId="self" />);
  expect(screen.getByRole("button", { name: `查看消息：${message.body}` })).toBeTruthy();
  expect(screen.getByRole("heading", { name: "信箱（1）" })).toBeTruthy();
  expect(screen.queryByText("正在读取信箱…")).toBeNull();
});
it("stores the complete long user message without submitting its body to a terminal", async () => {
  mount(); const input = await screen.findByRole("textbox", { name: "发送消息" }); await waitFor(() => expect(input).toHaveProperty("disabled", false));
  const text = "长消息".repeat(1000); fireEvent.change(input, { target: { value: text } }); fireEvent.click(screen.getByRole("button", { name: "发送到信箱" }));
  await waitFor(() => expect(api.mailboxSend).toHaveBeenCalledWith("self", text)); expect(api.mailboxRemind).not.toHaveBeenCalled();
});
it("sends the mailbox draft with Enter and leaves newlines and IME confirmation to the input", async () => {
  mount();
  const input = await screen.findByRole("textbox", { name: "发送消息" });
  fireEvent.change(input, { target: { value: "请查看报告" } });

  expect(fireEvent.keyDown(input, { key: "Enter", shiftKey: true })).toBe(true);
  expect(fireEvent.keyDown(input, { key: "Enter", isComposing: true })).toBe(true);
  expect(fireEvent.keyDown(input, { key: "Enter", keyCode: 229 })).toBe(true);
  expect(fireEvent.keyDown(input, { key: "Enter", repeat: true })).toBe(false);
  expect(api.mailboxSend).not.toHaveBeenCalled();

  const body = "请查看报告\n文件在工作区";
  fireEvent.change(input, { target: { value: body } });
  expect(fireEvent.keyDown(input, { key: "Enter" })).toBe(false);
  await waitFor(() => expect(input).toHaveProperty("value", ""));
  expect(api.mailboxSend).toHaveBeenCalledTimes(1);
  expect(api.mailboxSend).toHaveBeenCalledWith("self", body);
});
it("allows editing received unread content and requires a second inline delete action", async () => {
  mount(); await screen.findByText(message.body); fireEvent.click(screen.getByRole("button", { name: "编辑消息" }));
  fireEvent.change(screen.getByRole("textbox", { name: "编辑信箱消息" }), { target: { value: "edited" } }); fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.mailboxEdit).toHaveBeenCalledWith("t", 2, "mail-1", 1, "edited"));
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "编辑信箱消息" })).toBeNull());
  vi.mocked(api.mailboxEdit).mockClear(); fireEvent.click(screen.getByRole("button", { name: "删除消息" }));
  expect(api.mailboxEdit).not.toHaveBeenCalled(); expect(screen.queryByRole("dialog")).toBeNull(); fireEvent.click(screen.getByRole("button", { name: "确认永久删除" }));
  await waitFor(() => expect(api.mailboxEdit).toHaveBeenCalledWith("t", 2, "mail-1", 1, null));
});
it("removes mail from the sidebar after an external read-state update", async () => {
  mount(); fireEvent.click(await screen.findByRole("button", { name: `查看消息：${message.body}` }));
  const dialog = screen.getByRole("dialog", { name: "信箱消息" });
  expect(within(dialog).getByText(message.body)).toBeTruthy();
  expect(within(dialog).queryByRole("button", { name: "编辑消息" })).toBeNull();
  vi.mocked(api.mailboxList).mockResolvedValue({ items: [], total: 0, unread: 0, next_cursor: null });
  act(() => handlers.get("cli-data-changed")?.forEach(fn => fn())); await screen.findByText("暂无未读消息");
  expect(screen.queryByText(message.body)).toBeNull(); expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.getByRole("button", { name: "提醒查收" })).toHaveProperty("disabled", true);
  expect(screen.getByRole("button", { name: "开启自动通知" })).toHaveProperty("disabled", false);
});
it("offers manual unread reminders when auto notifications are off but hides edits when on", async () => {
  vi.mocked(api.mailboxList).mockResolvedValue({ items: [{ ...message, notify_immediately: true }], total: 1, unread: 1, next_cursor: null });
  const view = mount(); await screen.findByText(message.body);
  expect(screen.getByRole("button", { name: "开启自动通知" }).getAttribute("aria-pressed")).toBe("false");
  expect(screen.getByRole("button", { name: "提醒查收" }).title).toContain("立即插队");
  fireEvent.click(screen.getByRole("button", { name: "提醒查收" }));
  await waitFor(() => expect(api.mailboxRemind).toHaveBeenCalledWith("t"));
  view.rerender(<TerminalMailbox navigate={navigate} terminalId="t" sessionId="self" notification={{ ...notification, enabled: true }} error="" apply={vi.fn()} refresh={vi.fn().mockResolvedValue(undefined)} />);
  expect(screen.getByRole("button", { name: "关闭自动通知" }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByText(/自动通知已开启，终端只读/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "编辑消息" })).toBeNull(); expect(screen.queryByRole("button", { name: "删除消息" })).toBeNull();
  await waitFor(() => expect(screen.getByRole("button", { name: "提醒查收" })).toHaveProperty("disabled", false));
  fireEvent.click(screen.getByRole("button", { name: "关闭自动通知" }));
  await waitFor(() => expect(api.terminalAutoNotifications).toHaveBeenCalledWith("t", 2, false));
});
it("persists skip-hint choice while requiring an explicit enable action on a later mount", async () => {
  const first = mount(); await screen.findByText(message.body); fireEvent.click(screen.getByRole("button", { name: "开启自动通知" }));
  const dialog = await screen.findByRole("dialog", { name: "开启自动通知" }); fireEvent.click(within(dialog).getByRole("checkbox", { name: "不再提示" })); fireEvent.click(within(dialog).getByRole("button", { name: "开启自动通知" }));
  await waitFor(() => expect(api.mailboxNotificationHint).toHaveBeenCalledWith(true)); await waitFor(() => expect(api.terminalAutoNotifications).toHaveBeenCalledWith("t", 2, true)); first.unmount();
  vi.mocked(api.mailboxNotificationHint).mockResolvedValue(true); vi.mocked(api.terminalAutoNotifications).mockClear(); mount(); await screen.findByText(message.body);
  expect(api.terminalAutoNotifications).not.toHaveBeenCalled(); fireEvent.click(screen.getByRole("button", { name: "开启自动通知" }));
  await waitFor(() => expect(api.terminalAutoNotifications).toHaveBeenCalledWith("t", 2, true)); expect(screen.queryByRole("dialog")).toBeNull();
});
it("previews read mail and opens its complete content in a read-only dialog without changing state", async () => {
  const body = "请核对技能配置与任务记录。".repeat(30) + "\n最后一段保留原始换行和路径 /workspace/report.md";
  vi.mocked(api.mailboxList).mockResolvedValue({ items: [{ ...message, body, notify_immediately: true, read_at: "2026-10-24T02:00:00Z" }], total: 1, unread: 0, next_cursor: null });
  render(<MailboxHistory navigate={navigate} sessionId="self" />);
  const preview = await screen.findByRole("button", { name: /^查看消息：请核对/ });
  expect(screen.queryByText(/最后一段/)).toBeNull(); expect(screen.getByText("已读")).toBeTruthy();
  preview.focus(); fireEvent.click(preview);
  const dialog = screen.getByRole("dialog", { name: "信箱消息" });
  expect(within(dialog).getByText(/最后一段/).textContent).toBe(body);
  expect(within(dialog).getByText(message.sender_session_id!)).toBeTruthy();
  expect(within(dialog).getByText(message.id)).toBeTruthy();
  fireEvent.keyDown(dialog, { key: "Escape" });
  expect(screen.queryByRole("dialog")).toBeNull(); expect(document.activeElement).toBe(preview);
  expect(screen.getByText("已读")).toBeTruthy();
  expect(api.mailboxEdit).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: "删除消息" })).toBeNull(); expect(screen.queryByRole("button", { name: "提醒查收" })).toBeNull(); expect(api.mailboxList).toHaveBeenCalledWith("self", "all");
});

it("shows the entire inbox count across pages while keeping every message accessible", async () => {
  vi.mocked(api.mailboxList).mockImplementation(async (_session, _filter, cursor) => cursor == null
    ? { items: [{ ...message, body: "最新消息", created_at: "2026-10-24T03:00:00Z" }], total: 3, unread: 1, next_cursor: 1 }
    : { items: [
      { ...message, id: "mail-2", body: "第二条完整消息", created_at: "2026-10-24T02:00:00Z", read_at: "2026-10-24T02:00:00Z" },
      { ...message, id: "mail-3", body: "最后一条完整消息", read_at: "2026-10-24T02:00:00Z" },
    ], total: 3, unread: 1, next_cursor: null });
  render(<MailboxHistory navigate={navigate} sessionId="self" />);
  await screen.findByRole("heading", { name: "信箱（3）" });
  expect(screen.getAllByRole("button", { name: /^查看消息：/ })).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "加载更多" }));
  const last = await screen.findByRole("button", { name: "查看消息：最后一条完整消息" });
  expect(screen.getAllByRole("button", { name: /^查看消息：/ })).toHaveLength(3);
  expect(screen.getAllByRole("button", { name: /^查看消息：/ }).map(button => button.getAttribute("aria-label")))
    .toEqual(["查看消息：最新消息", "查看消息：第二条完整消息", "查看消息：最后一条完整消息"]);
  expect(screen.getByRole("heading", { name: "信箱（3）" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "加载更多" })).toBeNull();
  fireEvent.click(last);
  expect(within(screen.getByRole("dialog", { name: "信箱消息" })).getByText("最后一条完整消息")).toBeTruthy();
});

it("sends user mail from session details without a running terminal and refreshes the history", async () => {
  const body = "完整正文\n" + "长消息".repeat(1000);
  const sent = { ...message, id: "user-mail", sender_session_id: null, body };
  let finish!: (value: MailboxMessage) => void;
  vi.mocked(api.mailboxSend).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  render(<MailboxHistory navigate={navigate} sessionId="self" />);
  fireEvent.click(screen.getByRole("button", { name: "发送消息" }));
  const dialog = screen.getByRole("dialog", { name: "发送信箱消息" });
  const submit = within(dialog).getByRole("button", { name: "发送到信箱" });
  expect(submit).toHaveProperty("disabled", true);
  fireEvent.change(within(dialog).getByRole("textbox", { name: "消息内容" }), { target: { value: body } });
  fireEvent.click(submit); fireEvent.click(submit);
  expect(api.mailboxSend).toHaveBeenCalledTimes(1);
  expect(api.mailboxSend).toHaveBeenCalledWith("self", body);
  expect(within(dialog).getByRole("button", { name: "发送中…" })).toHaveProperty("disabled", true);
  vi.mocked(api.mailboxList).mockResolvedValue({ items: [message, sent], total: 2, unread: 2, next_cursor: null });
  await act(async () => { finish(sent); });
  await screen.findByRole("heading", { name: "信箱（2）" });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.getByText("用户")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /^查看消息：完整正文/ }));
  expect(within(screen.getByRole("dialog", { name: "信箱消息" })).getByText(/完整正文/).textContent).toBe(body);
  expect(api.mailboxRemind).not.toHaveBeenCalled();
  expect(api.mailboxEdit).not.toHaveBeenCalled();
});

it("keeps a failed draft available for retry and clears it when switching recipient sessions", async () => {
  vi.mocked(api.mailboxSend).mockRejectedValueOnce("保存失败").mockResolvedValueOnce({ ...message, sender_session_id: null, body: "重试正文" });
  const view = render(<MailboxHistory navigate={navigate} sessionId="self" />);
  fireEvent.click(screen.getByRole("button", { name: "发送消息" }));
  fireEvent.change(screen.getByRole("textbox", { name: "消息内容" }), { target: { value: "重试正文" } });
  fireEvent.click(screen.getByRole("button", { name: "发送到信箱" }));
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "保存失败");
  expect(screen.getByRole("textbox", { name: "消息内容" })).toHaveProperty("value", "重试正文");
  fireEvent.click(screen.getByRole("button", { name: "发送到信箱" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(api.mailboxSend).toHaveBeenLastCalledWith("self", "重试正文");
  fireEvent.click(screen.getByRole("button", { name: "发送消息" }));
  fireEvent.change(screen.getByRole("textbox", { name: "消息内容" }), { target: { value: "只给原会话的草稿" } });
  view.rerender(<MailboxHistory navigate={navigate} sessionId="other" />);
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "发送消息" }));
  expect(screen.getByRole("textbox", { name: "消息内容" })).toHaveProperty("value", "");
  fireEvent.change(screen.getByRole("textbox", { name: "消息内容" }), { target: { value: "新会话消息" } });
  fireEvent.click(screen.getByRole("button", { name: "发送到信箱" }));
  await waitFor(() => expect(api.mailboxSend).toHaveBeenLastCalledWith("other", "新会话消息"));
});
