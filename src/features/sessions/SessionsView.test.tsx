// Sessions 页的 Workstream 筛选口径：筛选直接看
// `session.owner_workstream_id`，未归属看 null。不存在"关联任务"这一层。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SessionsView, { clearSessionsCache } from "./SessionsView";
import { api } from "../../api";
import { listen } from "@tauri-apps/api/event";
import { viewState } from "../../hooks/useViewState";
import type { Session } from "../../types";

vi.mock("../../api", () => ({
  api: {
    listSessions: vi.fn(),
    listProjects: vi.fn(),
    listWorkstreams: vi.fn(),
    listIngestSources: vi.fn(),
    trashSession: vi.fn(),
    restoreSession: vi.fn(),
    getSessionLocalDeletePreview: vi.fn(),
    permanentlyDeleteSession: vi.fn(),
    getAgentStatus: vi.fn().mockResolvedValue({}),
    continueSessionDesktop: vi.fn().mockResolvedValue({ uri: "x://y", note: "已打开" }),
    terminalList: vi.fn().mockResolvedValue([]),
  },
}));

// 组件挂载时订阅刷新事件；测试环境没有 Tauri IPC。
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const navigate = vi.fn();

function session(over: Partial<Session> = {}): Session {
  return {
    id: "s1",
    agent: "codex",
    root_agent_session_id: "a1",
    title: "会话",
    cwd: "/repo",
    workspace_path_id: null,
    project_id: null,
    owner_workstream_id: null,
    forked_from_session_id: null,
    started_at: "2026-09-01T00:00:00Z",
    last_activity_at: "2026-09-02T00:00:00Z",
    last_conversation_at: "2026-09-02T00:00:00Z",
    trashed_at: null,
        source_kind: "codex_rollout",
        source_path: "/tmp/rollout.jsonl",
        metadata: {},
        source_file_identity: "identity",
        source_generation: 1,
        source_byte_offset: 100,
        source_last_seen_size: 100,
        source_mtime: null,
        source_prefix_hash: "",
        source_tail_hash: "",
        fact_generation: 1,
        latest_message_seq: 2,
    
    ...over,
  };
}

/** The 任务 filter select, found by its own options rather than by index. */
function taskFilter(): HTMLSelectElement {
  const select = screen.getByText("全部任务").closest("select");
  if (!select) throw new Error("任务筛选下拉框未渲染");
  return select as HTMLSelectElement;
}

beforeEach(() => {
  navigate.mockClear();
  clearSessionsCache();
  viewState.clear();
  vi.mocked(api.listSessions).mockReset();
  vi.mocked(api.listProjects).mockReset().mockResolvedValue([]);
  vi.mocked(api.listWorkstreams).mockReset().mockResolvedValue([
    { id: "w1", title: "会话与 Workstream 重构", description: "", lifecycle: "active", visibility: "normal", created_at: "", updated_at: "" },
  ] as never);
  vi.mocked(api.listIngestSources).mockReset().mockResolvedValue([{ id: "src", agent: "codex", path: "/p", enabled: true, origin: "default", created_at: "" }] as never);
  vi.mocked(listen).mockReset().mockResolvedValue(() => {});
});

afterEach(cleanup);

describe("Sessions 页按所属任务筛选", () => {
  it("筛选选项来自各 Session 的 owner，未归属是 null 而不是某个任务", async () => {
    vi.mocked(api.listSessions).mockResolvedValue([
      session({ id: "owned", title: "有归属", owner_workstream_id: "w1" }),
      session({ id: "free", title: "没归属", owner_workstream_id: null }),
    ]);
    render(<SessionsView navigate={navigate} />);

    await screen.findByText("全部任务");
    // 选中一个真实任务后，只剩 owner_workstream_id 等于它的那一条。
    fireEvent.change(taskFilter(), { target: { value: "w1" } });
    screen.getByText("有归属");
    expect(screen.queryByText("没归属")).toBeNull();

    // 「未归属任务」是 `owner_workstream_id === null`，与任何任务 id 都不相等。
    fireEvent.change(taskFilter(), { target: { value: "unassigned" } });
    screen.getByText("没归属");
    expect(screen.queryByText("有归属")).toBeNull();
  });
});

it("opens the new-session page from the toolbar and empty-state action", async () => {
  vi.mocked(api.listSessions).mockResolvedValue([]);
  render(<SessionsView navigate={navigate} />);

  await screen.findByRole("button", { name: "配置会话来源" });
  const buttons = screen.getAllByRole("button", { name: "新建会话" });
  expect(buttons).toHaveLength(2);
  buttons.forEach((button) => fireEvent.click(button));
  expect(navigate).toHaveBeenCalledTimes(2);
  expect(navigate).toHaveBeenCalledWith({ view: "new-session" });
  expect(screen.queryByRole("dialog")).toBeNull();
});
