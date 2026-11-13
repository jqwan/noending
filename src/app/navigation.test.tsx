import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../api";
import { prepareNavigation } from "./navigation";
import { clearReadResources } from "../hooks/readResource";
import { viewState } from "../hooks/useViewState";
import { clearProjectCardsCache } from "../features/projects/projectCardsCache";
import { clearWorkstreamDetailCache } from "../features/workstreams/workstreamDetailCache";
import { clearProjectSummaryCache } from "../features/projects/projectSummaryCache";
import ProjectsView from "../features/projects/ProjectsView";
import WorkstreamDetailView from "../features/workstreams/WorkstreamDetailView";
import SettingsView from "../features/settings/SettingsView";
import PackagesView from "../features/packages/PackagesView";
import TerminalMailbox from "../features/sessions/TerminalMailbox";
import { setPackageCatalogCache } from "../features/packages/packageDetailCache";

vi.mock("./Router", () => ({ prepareRouteModule: vi.fn() }));
vi.mock("../api", () => ({ api: {
  listProjectCards: vi.fn(), getDefaultAgent: vi.fn(), getWorkstreamContext: vi.fn(), listTaskItems: vi.fn(), getWorkstreamContextState: vi.fn(),
  collaborationRoomGet: vi.fn(), collaborationCandidates: vi.fn(), getOperationStatus: vi.fn(), getExtractionSettings: vi.fn(), getWorkspaceSettings: vi.fn(),
  listPackages: vi.fn(), mailboxList: vi.fn(), mailboxNotificationHint: vi.fn(),
} }));
const pending = () => new Promise<never>(() => {});
const task = { id: "task", title: "统一首屏加载", description: "目标：稳定首屏。\n现状：已接入公共导航。", visibility: "normal" as const, created_at: "2026-11-08", updated_at: "2026-11-08" };

beforeEach(() => {
  cleanup(); vi.resetAllMocks(); clearReadResources(); clearProjectCardsCache(); clearWorkstreamDetailCache(); clearProjectSummaryCache(); viewState.clear(); localStorage.clear();
  setPackageCatalogCache(null);
  vi.mocked(api.getDefaultAgent).mockResolvedValue(null);
  vi.mocked(api.getOperationStatus).mockResolvedValue([]);
  vi.mocked(api.getWorkstreamContext).mockResolvedValue({ workstream: task, projects: [], sessions: [] });
  vi.mocked(api.collaborationRoomGet).mockResolvedValue({ task_id: "task", title: task.title, room: null, run: null, members: [] });
  vi.mocked(api.collaborationCandidates).mockResolvedValue({ items: [], next_cursor: null });
  vi.mocked(api.listTaskItems).mockResolvedValue([{ id: "work", task_id: "task", revision: 1, title: "接入公共加载流程", description: "", details: [], created_at: "2026-11-08", updated_at: "2026-11-08" }]);
  vi.mocked(api.getWorkstreamContextState).mockResolvedValue({ summary: { items: [{ id: "summary", title: "首屏约定", description: "先预备，再切换。", details: [] }] } } as never);
});

it("opens the package frame without waiting for directory discovery and shows a delayed loading state", async () => {
  let finish!: (inventory: Awaited<ReturnType<typeof api.listPackages>>) => void;
  vi.mocked(api.listPackages).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  expect(prepareNavigation({ view: "packages" })).toEqual({ view: "packages" });
  render(<PackagesView navigate={vi.fn()} />);
  expect(screen.getByRole("heading", { name: "插件包" })).toBeTruthy();
  expect(screen.queryByText("没有可管理的插件包。")).toBeNull();
  await screen.findByRole("status", { name: "正在读取列表" });
  await act(async () => { finish({ packages: [], warnings: [] }); });
  expect(screen.getByText("没有可管理的插件包。")).toBeTruthy();
  expect(screen.queryByRole("status", { name: "正在读取列表" })).toBeNull();
});
afterEach(cleanup);

it("renders a cold project board with real content on its first commit while background reads remain pending", async () => {
  vi.mocked(api.listProjectCards).mockResolvedValue([{ id: "p", name: "首帧可见项目", name_customized: false, visibility: "normal", kind: "single_directory", path_count: 1, missing_path_count: 0, workstream_count: 0, session_count: 0, representative_paths: [], search_paths: [], last_activity_at: null, updated_at: "2026-11-08" }]);
  await prepareNavigation({ view: "projects" });
  vi.mocked(api.listProjectCards).mockImplementation(pending);
  render(<ProjectsView navigate={vi.fn()} />);
  expect(screen.getByText("首帧可见项目")).toBeTruthy();
  expect(screen.getByText("共 1 项")).toBeTruthy();
  expect(screen.getByRole("button", { name: "新增项目" })).toHaveProperty("disabled", false);
  expect(screen.queryByRole("status", { name: "正在读取列表" })).toBeNull();
});

it("prepares task items, summary controls and member selection for the first task detail frame", async () => {
  await prepareNavigation({ view: "workstream", workstreamId: "task" });
  vi.mocked(api.getWorkstreamContext).mockImplementation(pending);
  vi.mocked(api.listTaskItems).mockImplementation(pending);
  vi.mocked(api.getWorkstreamContextState).mockImplementation(pending);
  vi.mocked(api.collaborationRoomGet).mockImplementation(pending);
  vi.mocked(api.collaborationCandidates).mockImplementation(pending);
  render(<WorkstreamDetailView workstreamId="task" navigate={vi.fn()} goBack={vi.fn()} />);
  expect(screen.getByRole("heading", { name: "接入公共加载流程" })).toBeTruthy();
  expect(screen.getByRole("heading", { name: "首屏约定" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "复制摘要" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "新增事项" })).toHaveProperty("disabled", false);
  expect(screen.getByRole("group", { name: "本次协作" })).toBeTruthy();
});

it("keeps auxiliary read failures recoverable inside the destination rather than preventing navigation", async () => {
  vi.mocked(api.getWorkstreamContextState).mockRejectedValue(new Error("摘要暂不可读"));
  await expect(prepareNavigation({ view: "workstream", workstreamId: "task" })).resolves.toMatchObject({ view: "workstream", workstreamId: "task" });
  render(<WorkstreamDetailView workstreamId="task" navigate={vi.fn()} goBack={vi.fn()} />);
  expect(screen.getByRole("heading", { name: "任务描述" })).toBeTruthy();
  await screen.findByRole("alert");
  expect(screen.getByRole("heading", { name: "接入公共加载流程" })).toBeTruthy();
  vi.mocked(api.getWorkstreamContextState).mockResolvedValue({ summary: { items: [{ id: "summary", title: "恢复读取的摘要", description: "", details: [] }] } } as never);
  fireEvent.click(screen.getByRole("button", { name: "重试读取" }));
  await screen.findByRole("heading", { name: "恢复读取的摘要" });
});

it("renders prepared settings immediately and retains their controls during revalidation", async () => {
  vi.mocked(api.getExtractionSettings).mockResolvedValue({ agent: "none", runtime: { model: null, provider: null, effort: null } });
  vi.mocked(api.getWorkspaceSettings).mockResolvedValue({ noending_home: "/tmp/isolated", default_workspace: "/tmp/isolated/workspace", pending_home: null, restart_required: false, db_path: "/tmp/isolated/noending.db", home_source: "default_home" });
  await prepareNavigation({ view: "settings" });
  vi.mocked(api.getExtractionSettings).mockImplementation(pending);
  vi.mocked(api.getWorkspaceSettings).mockImplementation(pending);
  render(<SettingsView />);
  expect(screen.getByText("/tmp/isolated/noending.db")).toBeTruthy();
  expect(screen.getByRole("button", { name: "更改位置…" })).toBeTruthy();
  expect(screen.getByRole("button", { name: /停用后台提取/ })).toBeTruthy();
  expect(screen.queryByText("正在读取存储配置…")).toBeNull();
});

it("prepares the unread count before opening a bound terminal", async () => {
  vi.mocked(api.mailboxList).mockResolvedValue({ items: [], total: 0, unread: 0, next_cursor: null });
  vi.mocked(api.mailboxNotificationHint).mockResolvedValue(false);
  await prepareNavigation({ view: "terminal", terminalId: "t", initialSessionId: "s" });
  vi.mocked(api.mailboxList).mockImplementation(pending);
  await act(async () => {
    render(<TerminalMailbox terminalId="t" sessionId="s" notification={null} error="" apply={vi.fn()} refresh={vi.fn()} navigate={vi.fn()} />);
  });
  expect(screen.getByRole("heading", { name: "信箱（0）" })).toBeTruthy();
  expect(screen.getByText("暂无未读消息")).toBeTruthy();
});
