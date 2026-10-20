import { invalidateTargets, removedTargets } from "./removedTargets";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import AppShell from "./AppShell";
import { sessionPages, sessionTerminals } from "../features/sessions/sessionNavigation";
import type { Route } from "./routes";
import { api } from "../api";
import { sessionDetailCache, sessionContextCache } from "../features/sessions/sessionDetailCache";
import { useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useRefreshSignal } from "../components/common";

const focusListener = vi.hoisted(() => ({
  callback: undefined as undefined | ((event: { payload: boolean }) => void),
}));
const appForeground = vi.hoisted(() => vi.fn().mockResolvedValue({ queued: true }));

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onFocusChanged: (callback: (event: { payload: boolean }) => void) => {
      focusListener.callback = callback;
      return Promise.resolve(() => {});
    },
  }),
}));
vi.mock("../api", () => ({ api: { appForeground, getSessionDetail: vi.fn(), getSessionContext: vi.fn() } }));
vi.mock("../layout/Sidebar", () => ({ default: ({ collapsed, hidden }: { collapsed?: boolean; hidden?: boolean }) => <aside aria-label="会话侧边栏" hidden={collapsed || hidden} /> }));
const pageNavigation = vi.hoisted(() => ({ navigate: undefined as undefined | ((route: Route) => void) }));
const sessionNavigation = vi.hoisted(() => ({ navigate: undefined as undefined | ((route: Route) => void) }));
vi.mock("./Router", () => ({ default: ({ route, navigate }: { route: Route; navigate: (route: Route) => void }) => {
  const [draft, setDraft] = useState("");
  pageNavigation.navigate = navigate;
  if (["new-session", "sessions", "session", "terminal"].includes(route.view)) sessionNavigation.navigate = navigate;
  return <>
    <output aria-label="当前页面">{route.view}</output>
    <output aria-label="页面参数">{JSON.stringify(route)}</output>
    {route.view === "new-session" && <input aria-label="会话草稿" value={draft} onChange={e => setDraft(e.target.value)} />}
  </>;
} }));
vi.mock("../components/Toast", () => ({ default: () => null }));
vi.mock("../components/CommandPalette", () => ({ default: ({ navigate, onClose }: { navigate: (route: Route) => void; onClose: () => void }) => <div role="dialog" aria-label="查找">
  <button onClick={() => { navigate({ view: "agents", agent: "codex" }); onClose(); }}>进入 Agent 详情</button>
</div> }));
vi.mock("../features/launcher/LaunchResultModal", () => ({ LaunchDetailsHost: () => null }));

beforeEach(() => {
  Object.values(removedTargets).forEach(ids => ids.clear());
  vi.mocked(listen).mockClear();
  sessionDetailCache.clear();
  sessionContextCache.clear();
  vi.mocked(api.getSessionDetail).mockReset();
  vi.mocked(api.getSessionContext).mockReset().mockImplementation(async session_id => ({
    session_id, fields: null, revision: 0, ingest_generation: 0,
    processed_through_seq: 0, latest_message_seq: 0, updated_at: null, pending: false,
  }));
  sessionPages.clear(); sessionTerminals.clear();
  localStorage.clear();
  appForeground.mockClear();
  sessionNavigation.navigate = undefined;
});
afterEach(cleanup);

it("refreshes visible data when a CLI business operation completes", async () => {
  function RefreshConsumer() {
    const [count, setCount] = useState(0);
    useRefreshSignal(() => setCount(value => value + 1));
    return <output aria-label="数据刷新次数">{count}</output>;
  }
  render(<><AppShell /><RefreshConsumer /></>);
  const listener = vi.mocked(listen).mock.calls.find(([name]) => name === "cli-data-changed")?.[1];
  expect(listener).toBeDefined();
  await act(async () => { listener!({ event: "cli-data-changed", id: 1, payload: null }); });
  expect(screen.getByRole("status", { name: "数据刷新次数" }).textContent).toBe("1");
});

it("keeps icon navigation fixed and allows toggling the session sidebar in any section", () => {
  render(<AppShell />);
  const rail = screen.getByRole("navigation", { name: "工作区导航" });
  expect(screen.getByRole("complementary", { name: "会话侧边栏" })).toBeTruthy();
  expect(screen.getByRole("separator")).toBeTruthy();
  expect(screen.getByRole("button", { name: "收起侧边栏" })).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "项目" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("projects");
  expect(screen.getByRole("navigation", { name: "工作区导航" })).toBe(rail);

  // In projects section, session sidebar is still available and can be collapsed
  expect(screen.getByRole("complementary", { name: "会话侧边栏" })).toBeTruthy();
  expect(screen.getByRole("separator")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "收起侧边栏" }));
  expect(screen.queryByRole("complementary", { name: "会话侧边栏" })).toBeNull();
  expect(screen.queryByRole("separator")).toBeNull();
  expect(screen.getByRole("button", { name: "展开侧边栏" })).toBeTruthy();

  // Re-expand in projects section
  fireEvent.click(screen.getByRole("button", { name: "展开侧边栏" }));
  expect(screen.getByRole("complementary", { name: "会话侧边栏" })).toBeTruthy();
  expect(screen.getByRole("separator")).toBeTruthy();

  // Clicking 会话 restores the previous session page (new-session); clicking again navigates to sessions board
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("new-session");
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("sessions");
  expect(screen.getByRole("complementary", { name: "会话侧边栏" })).toBeTruthy();
});

it("preserves the session page's unsent state while opening Agent details from lookup", () => {
  render(<AppShell />);
  fireEvent.change(screen.getByRole("textbox", { name: "会话草稿" }), { target: { value: "未发送的消息" } });
  fireEvent.click(screen.getByRole("button", { name: "任务" }));
  fireEvent.click(screen.getByRole("button", { name: "查找" }));
  expect(screen.getByRole("dialog", { name: "查找" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "进入 Agent 详情" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("agents");
  fireEvent.click(screen.getByRole("button", { name: "返回上一页" }));
  fireEvent.click(screen.getByRole("button", { name: "返回上一页" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("new-session");
  expect((screen.getByRole("textbox", { name: "会话草稿" }) as HTMLInputElement).value).toBe("未发送的消息");
});

it("restores the last session page and parameters across history navigation", () => {
  const route: Route = { view: "session", sessionId: "s-4", entry: "conversation", initialTitle: "Existing", initialAgent: "codex" };
  render(<AppShell />);
  act(() => sessionNavigation.navigate!(route));
  fireEvent.click(screen.getByRole("button", { name: "项目" }));
  fireEvent.click(screen.getByRole("button", { name: "返回上一页" }));
  expect(JSON.parse(screen.getByRole("status", { name: "页面参数" }).textContent!)).toEqual(route);
  fireEvent.click(screen.getByRole("button", { name: "前进到下一页" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("projects");
  fireEvent.click(screen.getByRole("button", { name: "返回上一页" }));
  expect(JSON.parse(screen.getByRole("status", { name: "页面参数" }).textContent!)).toEqual(route);
});

it("remembers a background launch result without changing the current section", () => {
  render(<AppShell />);
  const finishLaunch = sessionNavigation.navigate!;
  fireEvent.click(screen.getByRole("button", { name: "Agent" }));
  act(() => finishLaunch({ view: "terminal", terminalId: "t-created" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("agents");
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("new-session");
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("sessions");
});

it("remembers keyboard resizing, clamps width and resets on double click", () => {
  const view = render(<AppShell />);
  const divider = screen.getByRole("separator");
  fireEvent.keyDown(divider, { key: "ArrowRight" });
  view.unmount();
  render(<AppShell />);
  const restored = screen.getByRole("separator");
  expect(restored.getAttribute("aria-valuenow")).toBe("256");
  fireEvent.keyDown(restored, { key: "End" });
  fireEvent.keyDown(restored, { key: "ArrowRight" });
  expect(restored.getAttribute("aria-valuenow")).toBe("360");
  fireEvent.keyDown(restored, { key: "Home" });
  fireEvent.keyDown(restored, { key: "ArrowLeft" });
  expect(restored.getAttribute("aria-valuenow")).toBe("200");
  fireEvent.doubleClick(restored);
  expect(restored.getAttribute("aria-valuenow")).toBe("248");
});

it("queues ingestion only after the window returns from the background", async () => {
  render(<AppShell />);
  const changed = focusListener.callback!;
  changed({ payload: true });
  expect(appForeground).not.toHaveBeenCalled();
  changed({ payload: false });
  changed({ payload: true });
  changed({ payload: true });

  await waitFor(() => expect(appForeground).toHaveBeenCalledTimes(1));
});

it("normalizes legacy settings sections to agents view", async () => {
  const { normalizeRoute } = await import("./AppShell");
  expect(normalizeRoute({ view: "settings", section: "agents" })).toEqual({ view: "agents" });
  expect(normalizeRoute({ view: "settings", section: "sources" })).toEqual({ view: "agents" });
  expect(normalizeRoute({ view: "settings", section: "general" })).toEqual({ view: "settings", section: "general" });
  expect(normalizeRoute({ view: "workstreams" })).toEqual({ view: "workstreams" });
  expect(normalizeRoute({ view: "sessions", action: "new" })).toEqual({ view: "new-session" });
});

it("resolves an ID-only destination before changing pages, and cancels a late result after newer navigation", async () => {
  let resolve!: (value: Awaited<ReturnType<typeof api.getSessionDetail>>) => void;
  vi.mocked(api.getSessionDetail).mockImplementation(() => new Promise(done => { resolve = done; }));
  render(<AppShell />);
  act(() => sessionNavigation.navigate!({ view: "session", sessionId: "pending" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("new-session");
  await act(async () => resolve({ session: { id: "pending", title: "Ready", agent: "codex" } } as never));
  expect(JSON.parse(screen.getByRole("status", { name: "页面参数" }).textContent!)).toMatchObject({ view: "session", initialTitle: "Ready", initialAgent: "codex" });
  act(() => sessionNavigation.navigate!({ view: "session", sessionId: "late" }));
  fireEvent.click(screen.getByRole("button", { name: "项目" }));
  await act(async () => resolve({ session: { id: "late", title: "Late", agent: "codex" } } as never));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("projects");
});

it("keeps assistant project launches and terminals in the session section", async () => {
  render(<AppShell />);
  await act(async () => { sessionNavigation.navigate!({ view: "new-session", projectId: "assistant-project" }); });
  expect(JSON.parse(screen.getByRole("status", { name: "页面参数" }).textContent!)).toEqual({ view: "new-session", projectId: "assistant-project" });
  await act(async () => { sessionNavigation.navigate!({ view: "terminal", terminalId: "assistant-terminal" }); });
  expect(screen.getByRole("button", { name: "会话" }).getAttribute("aria-current")).toBe("page");
  fireEvent.click(screen.getByRole("button", { name: "项目" }));
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(JSON.parse(screen.getByRole("status", { name: "页面参数" }).textContent!)).toEqual({ view: "terminal", terminalId: "assistant-terminal" });
});

it.each([
  { route: { view: "session", sessionId: "deleted", initialTitle: "S", initialAgent: "codex" }, targets: { sessionIds: ["deleted"] }, replacement: { view: "sessions", scope: "archived" }, section: "会话" },
  { route: { view: "workstream", workstreamId: "deleted", initialTitle: "T" }, targets: { workstreamIds: ["deleted"] }, replacement: { view: "workstreams", scope: "archived" }, section: "任务" },
  { route: { view: "project", projectId: "deleted", initialTitle: "P" }, targets: { projectIds: ["deleted"] }, replacement: { view: "projects", scope: "archived" }, section: "项目" },
  { route: { view: "skill", resourceId: "skill1", projectId: "deleted", workspacePathId: "w1" }, targets: { projectIds: ["deleted"] }, replacement: { view: "skills" }, section: "技能" },
  { route: { view: "terminal", terminalId: "deleted", initialSessionId: "s" }, targets: { terminalIds: ["deleted"] }, replacement: { view: "session", sessionId: "s", initialTitle: "S", initialAgent: "codex" }, section: "会话" },
] as const)("removes invalid $route.view routes from back/forward and section memory", ({ route, targets, replacement, section }) => {
  render(<AppShell />);
  act(() => pageNavigation.navigate!(route));
  if (route.view === "session") act(() => pageNavigation.navigate!({ ...route, entry: "conversation" }));
  act(() => invalidateTargets({ ...targets, replacement }));
  expect(JSON.parse(screen.getByRole("status", { name: "页面参数" }).textContent!)).toEqual(replacement);
  fireEvent.click(screen.getByRole("button", { name: "返回上一页" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("new-session");
  fireEvent.click(screen.getByRole("button", { name: "前进到下一页" }));
  expect(JSON.parse(screen.getByRole("status", { name: "页面参数" }).textContent!)).toEqual(replacement);
  fireEvent.click(screen.getByRole("button", { name: "Agent" }));
  fireEvent.click(screen.getByRole("button", { name: section }));
  expect(JSON.parse(screen.getByRole("status", { name: "页面参数" }).textContent!)).toEqual(replacement);
});

it("prunes deleted targets while keeping an unrelated current page and rejects late title loads", async () => {
  let resolve!: (value: Awaited<ReturnType<typeof api.getSessionDetail>>) => void;
  vi.mocked(api.getSessionDetail).mockImplementation(() => new Promise(done => { resolve = done; }));
  render(<AppShell />);
  act(() => pageNavigation.navigate!({ view: "session", sessionId: "pending-delete" }));
  fireEvent.click(screen.getByRole("button", { name: "项目" }));
  act(() => invalidateTargets({ sessionIds: ["pending-delete"] }));
  await act(async () => resolve({ session: { id: "pending-delete", title: "Old", agent: "codex" } } as never));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("projects");
});

it("removes a replaced terminal from earlier history while retaining the new terminal", () => {
  render(<AppShell />);
  act(() => pageNavigation.navigate!({ view: "terminal", terminalId: "old" }));
  const replacement: Route = { view: "terminal", terminalId: "new" };
  act(() => { pageNavigation.navigate!(replacement); invalidateTargets({ terminalIds: ["old"], replacement }); });
  fireEvent.click(screen.getByRole("button", { name: "返回上一页" }));
  expect(screen.getByRole("status", { name: "当前页面" }).textContent).toBe("new-session");
  fireEvent.click(screen.getByRole("button", { name: "前进到下一页" }));
  expect(JSON.parse(screen.getByRole("status", { name: "页面参数" }).textContent!)).toEqual(replacement);
});
