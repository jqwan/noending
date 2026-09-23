import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionDetailView from "./SessionDetailView";
import { api } from "../../api";
import type { Session, SessionDetail } from "../../types";

// 只覆盖「会话信息」里那棵父子会话树（§37.20）：父会话可能不在库里、子会话可点进
// 详情页、回收站里的孩子照样列出。命令怎么走由后端测试覆盖。
vi.mock("../../api", () => ({
  api: {
    getSessionDetail: vi.fn(),
    listProjects: vi.fn().mockResolvedValue([]),
    syncSession: vi.fn(),
    trashSession: vi.fn(),
    restoreSession: vi.fn(),
    replaceSessionBindings: vi.fn(),
  },
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function session(id: string, over: Partial<Session> = {}): Session {
  return {
    id,
    agent: "codex",
    agent_session_id: `${id}-agent`,
    title: null,
    cwd: null,
    project_id: null,
    workspace_path_id: null,
    raw_path: `/${id}.jsonl`,
    parent_agent_session_id: null,
    started_at: null,
    last_activity_at: null,
    trashed_at: null,
    ...over,
  };
}

function detail(me: Session, over: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session: me,
    events: [],
    bindings: [],
    cursor: 0,
    processed_cursor: 0,
    classification: "",
    raw_path_status: "present",
    workspace_path: null,
    parent: null,
    children: [],
    ...over,
  };
}

async function renderDetail(d: SessionDetail) {
  vi.mocked(api.getSessionDetail).mockResolvedValue(d);
  const navigate = vi.fn();
  render(<SessionDetailView sessionId={d.session.id} navigate={navigate} goBack={vi.fn()} />);
  await screen.findByText("会话信息");
  return navigate;
}

it("links to the parent session and to each child", async () => {
  const me = session("me", { parent_agent_session_id: "p-agent" });
  const navigate = await renderDetail(detail(me, {
    parent: session("p", { title: "父会话标题", agent_session_id: "p-agent" }),
    children: [
      session("c1", { title: "子会话一" }),
      session("c2", { title: "子会话二" }),
    ],
  }));

  fireEvent.click(screen.getByRole("button", { name: "父会话标题" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "p" });

  fireEvent.click(screen.getByRole("button", { name: "子会话一" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "c1" });

  fireEvent.click(screen.getByRole("button", { name: "子会话二" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "c2" });
});

it("says the parent is missing instead of pretending there is none", async () => {
  const me = session("me", { parent_agent_session_id: "ghost-agent" });
  await renderDetail(detail(me, { parent: null }));

  // 转录记了父会话、但我们从没发现过那一条：字段在，链接不在。
  screen.getByText("父会话");
  screen.getByText(/不在 NoEnding 库里/);
  screen.getByText(/ghost-agent/);
  expect(screen.queryByRole("button", { name: /未命名会话/ })).toBeNull();
});

it("marks a trashed child but still links to it", async () => {
  const me = session("me");
  const navigate = await renderDetail(detail(me, {
    children: [session("c1", { title: "子会话一", trashed_at: "2026-09-24T00:00:00+00:00" })],
  }));

  const link = screen.getByRole("button", { name: /子会话一/ });
  expect(link.textContent).toContain("回收站");
  fireEvent.click(link);
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "c1" });
});

it("shows no session-tree fields for a session that has neither", async () => {
  await renderDetail(detail(session("me"), { children: [] }));

  expect(screen.queryByText("父会话")).toBeNull();
  expect(screen.queryByText(/^子会话/)).toBeNull();
});
