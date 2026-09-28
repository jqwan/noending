import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionDetailView from "./SessionDetailView";
import { api } from "../../api";
import { viewState } from "../../hooks/useViewState";
import type {
  Session,
  SessionAggregateStats,
  SessionContextView,
  LocalDeletePreview,
  SessionDetail,
  SessionMember,
  SessionMessage,
  SessionMemberStats,
  Workstream,
} from "../../types";

// 只覆盖重构后的详情页：执行成员与执行统计、源会话状态、fork 链接、
// 回收站横幅的删除入口、「所属任务」单 Owner 入口，以及 Context 面板。
vi.mock("../../api", () => ({
  api: {
    getSessionDetail: vi.fn(),
    revealSessionSource: vi.fn(),
    getSessionContext: vi.fn().mockResolvedValue({
      session_id: "",
      fields: null,
      revision: 0,
      ingest_generation: 0,
      processed_through_seq: 0,
      latest_message_seq: 0,
      updated_at: null,
      pending: false,
    }),
    updateSessionContext: vi.fn().mockResolvedValue({
      session_id: "",
      status: "updated",
      revision: 1,
    }),
    listProjects: vi.fn().mockResolvedValue([]),
    listWorkstreams: vi.fn().mockResolvedValue([]),
    trashSession: vi.fn(),
    restoreSession: vi.fn(),
    setSessionOwnerWorkstream: vi.fn(),
    getSessionLocalDeletePreview: vi.fn(),
    permanentlyDeleteSession: vi.fn(),
  },
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  // 「显示统计」这类开关活在模块级 view state 里，会跨用例残留。
  viewState.clear();
});

function session(id: string, over: Partial<Session> = {}): Session {
  return {
    id,
    agent: "codex",
    root_agent_session_id: `${id}-agent`,
    title: null,
    cwd: null,
    project_id: null,
    workspace_path_id: null,
    owner_workstream_id: null,
    forked_from_session_id: null,
    started_at: null,
    last_activity_at: null,
    last_conversation_at: null,
    trashed_at: null,
    ...over,
  };
}

function workstream(id: string, title: string): Workstream {
  return {
    id,
    title,
    description: "",
    lifecycle: "active",
    visibility: "normal",
    created_at: "2026-09-21T00:00:00+00:00",
    updated_at: "2026-09-21T00:00:00+00:00",
  };
}

function stats(over: Partial<SessionAggregateStats> = {}): SessionAggregateStats {
  return {
    member_count: 1,
    child_count: 0,
    side_count: 0,
    max_depth: 0,
    tool_call_count: 0,
    user_message_count: 0,
    assistant_message_count: 0,
    compaction_count: 0,
    side_activity_count: 0,
    input_tokens: null,
    output_tokens: null,
    cached_tokens: null,
    reasoning_tokens: null,
    cost: null,
    ...over,
  };
}

function member(
  sessionId: string,
  sourceMemberId: string,
  relation: SessionMember["relation"],
  parentSourceMemberId: string | null,
  over: Partial<SessionMember> & { stats?: SessionMemberStats | null } = {},
): SessionMember & { stats: SessionMemberStats | null } {
  const { stats: memberStats = null, ...rest } = over;
  return {
    id: `${sessionId}-${sourceMemberId}`,
    session_id: sessionId,
    agent: "codex",
    source_member_id: sourceMemberId,
    relation,
    parent_source_member_id: parentSourceMemberId,
    source_kind: "codex_thread",
    source_path: `/sources/${sourceMemberId}.jsonl`,
    cwd: null,
    started_at: null,
    last_activity_at: null,
    metadata: {},
    stats: memberStats,
    ...rest,
  };
}

function message(
  sessionId: string,
  sequence: number,
  role: SessionMessage["role"],
  content: string,
): SessionMessage {
  return {
    id: `m${sequence}`,
    session_id: sessionId,
    member_id: "root",
    sequence,
    role,
    content,
    ts: null,
    source_message_id: null,
    source_generation: 0,
    source_position: "",
    source_identity_hash: "",
    provider: null,
    model: null,
    raw_ref: "",
  };
}

function detail(me: Session, over: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session: me,
    messages: [],
    owner_workstream: null,
    workspace_path: null,
    members: [member(me.id, `${me.id}-root`, "root", null)],
    stats: stats(),
    cost_unit: null,
    ingested_message_sequence: 0,
    processed_message_sequence: 0,
    root_source_status: "present",
    can_resume: true,
    forked_from: null,
    ...over,
  };
}

async function renderDetail(d: SessionDetail) {
  vi.mocked(api.getSessionDetail).mockResolvedValue(d);
  const navigate = vi.fn();
  const view = render(<SessionDetailView sessionId={d.session.id} navigate={navigate} goBack={vi.fn()} />);
  await screen.findByText("会话信息");
  return { navigate, container: document.body, rerender: view.rerender };
}

/** 「执行成员」里某个成员那一行的文本；汇总组与成员行有同样的标签，必须按行核对。 */
function memberRow(sourceMemberId: string): string {
  const row = [...document.querySelectorAll(".member-row")]
    .find((r) => r.querySelector(".member-id")?.textContent === sourceMemberId);
  if (!row) throw new Error(`成员行不存在：${sourceMemberId}`);
  return row.textContent ?? "";
}

// 执行信息

it("shows aggregate execution stats and the always-visible member tree", async () => {
  const me = session("me");
  await renderDetail(detail(me, {
    members: [
      member(me.id, `${me.id}-root`, "root", null, {
        stats: {
          member_id: "root", tool_call_count: 12, user_message_count: 3, assistant_message_count: 5,
          compaction_count: 1, input_tokens: 1200, output_tokens: 340, cost: 0.25,
        } as SessionMemberStats,
        source_path: "/sources/root.jsonl",
      }),
      member(me.id, "child-src-1", "child", `${me.id}-root`, {
        stats: {
          member_id: "child", tool_call_count: 2, input_tokens: 800, cost: 0.1,
        } as SessionMemberStats,
        source_path: "/sources/child-src-1.jsonl",
      }),
      // 边成员：源什么都不报，只留身份与源文件——不拿 0 冒称观测。
      member(me.id, "side-src-1", "side", `${me.id}-root`),
    ],
    stats: stats({
      member_count: 3,
      child_count: 1,
      side_count: 1,
      max_depth: 1,
      tool_call_count: 12,
      user_message_count: 3,
      assistant_message_count: 5,
      compaction_count: 1,
    }),
    cost_unit: "USD",
  }));

  // 执行统计常驻：每个数各占一格，不再串成一行。整组是各成员份额的和，
  // 与单个成员自己的数分开核对——同一批标签在汇总和成员行里都会出现。
  const metrics = document.querySelector(".exec-metrics")!;
  for (const cell of ["规模", "3 个成员", "1 子", "1 边", "深度 1", "消息构成", "用户 3", "工具 12"]) {
    expect(metrics.textContent).toContain(cell);
  }
  screen.getByText("执行统计");

  // 成员不再展开/收起：子会话、边会话各占一节「执行成员」，是这页的一等事实。
  expect(screen.queryByRole("button", { name: /展开成员|收起成员/ })).toBeNull();
  screen.getByText("执行成员");
  screen.getByText("child-src-1");
  screen.getByText("side-src-1");
  // 关系标签：根 / 子 / 边。
  expect(screen.getAllByText("根")).toHaveLength(1);
  screen.getByText("子");
  screen.getByText("边");

  // 成员各自的数字默认不画：先给结构与来源。汇总组不受这个开关影响。
  expect(memberRow("me-root")).not.toContain("用户");
  expect(memberRow("me-root")).not.toContain("成本");
  expect(memberRow("child-src-1")).toContain("源");
  expect(metrics.textContent).toContain("用户 3");

  fireEvent.click(screen.getByRole("button", { name: "显示统计" }));

  // 每个成员各自的份额：计数、tokens、成本都是它自己的数。
  expect(memberRow("me-root")).toContain("用户 3");
  expect(memberRow("me-root")).toContain("工具 12");
  expect(memberRow("me-root")).toContain("输入 1,200");
  expect(memberRow("me-root")).toContain("成本 0.25 USD");
  expect(memberRow("child-src-1")).toContain("工具 2");
  expect(memberRow("child-src-1")).toContain("输入 800");
  expect(memberRow("child-src-1")).toContain("成本 0.1 USD");
  // 边成员源什么都没报：一行数字都不画，不拿 0 冒称观测。
  expect(memberRow("side-src-1")).not.toContain("工具");
  expect(memberRow("side-src-1")).not.toContain("成本");
  // 子/边的源可复制（原值在 title 里，显示用中段省略）；root 的源只在「会话信息 · 源会话」出现。
  screen.getByTitle(/\/sources\/child-src-1\.jsonl · 这个成员在 Agent 侧的源/);
  expect(memberRow("me-root")).not.toContain("源");

  // 成员是执行信息，不是可进入的其他会话页面。
  expect(screen.queryByRole("button", { name: /child-src-1/ })).toBeNull();
  expect(screen.queryByRole("button", { name: /side-src-1/ })).toBeNull();
});

it("keeps 执行成员 off a session that is only its root", async () => {
  // 只有 root 时既没有成员清单也没有开关：汇总里的「规模」已经把话说完了。
  await renderDetail(detail(session("me")));
  screen.getByText("规模");
  screen.getByText("1 个成员");
  screen.getByText("执行统计");
  expect(screen.queryByText("me-root")).toBeNull();
  expect(screen.queryByText("执行成员")).toBeNull();
  // 没有成员行可藏，就不摆一个按不动的开关。
  expect(screen.queryByRole("button", { name: /统计/ })).toBeNull();
});

it("shows message / token / cost rows only when the data is present", async () => {
  const me = session("me");
  await renderDetail(detail(me, { stats: stats() }));

  // 执行图的规模永远在；消息构成 / Tokens / 成本这些组只在数据真的在场上时出现。
  screen.getByText("规模");
  expect(screen.queryByText("消息构成")).toBeNull();
  expect(screen.queryByText("Tokens")).toBeNull();
  expect(screen.queryByText("成本")).toBeNull();

  cleanup();
  await renderDetail(detail(me, {
    stats: stats({
      user_message_count: 2,
      assistant_message_count: 3,
      tool_call_count: 4,
      side_activity_count: 3,
      input_tokens: 1000,
      output_tokens: 200,
      cost: 0.5,
    }),
    cost_unit: "USD",
  }));

  screen.getByText("消息构成");
  screen.getByText("用户 2");
  screen.getByText("助手 3");
  screen.getByText("工具 4");
  screen.getByText("协同 3");
  screen.getByText("Tokens");
  screen.getByText("输入 1,000");
  screen.getByText("输出 200");
  screen.getByText("成本");
  // 数字带着 Agent 自己的单位，不能被当成别的单位读。
  screen.getByText("0.5 USD");
});

it("labels a non-currency cost with the Agent's own unit", async () => {
  const me = session("me");
  await renderDetail(detail(me, {
    stats: stats({ cost: 749.353 }),
    cost_unit: "credits",
  }));

  screen.getByText("成本");
  screen.getByText("749.353 credits");
});

// 源会话

it("shows the root member's source as 源会话 without status noise when present", async () => {
  await renderDetail(detail(session("me")));

  screen.getByText("源会话");
  screen.getByText(/sources\/me-root\.jsonl/);
  expect(screen.queryByText("源会话已不存在")).toBeNull();
  expect(screen.queryByText("无法确认源会话状态")).toBeNull();
});

it("reveals the root source in the file manager", async () => {
  await renderDetail(detail(session("me")));

  fireEvent.click(screen.getByRole("button", { name: "在文件管理器中显示" }));
  await waitFor(() => expect(api.revealSessionSource).toHaveBeenCalledWith("me"));
});

it("warns and disables resume when the root source is missing", async () => {
  await renderDetail(detail(session("me"), { root_source_status: "missing", can_resume: false }));

  screen.getByText("源会话已不存在");
  const resume = screen.getByRole("button", { name: "继续" }) as HTMLButtonElement;
  expect(resume.disabled).toBe(true);
  expect(resume.title).toContain("源会话已不存在");
  expect((screen.getByRole("button", { name: "在文件管理器中显示" }) as HTMLButtonElement).disabled).toBe(true);
});

it("warns when the root source status is unavailable", async () => {
  await renderDetail(detail(session("me"), { root_source_status: "unavailable", can_resume: false }));

  screen.getByText("无法确认源会话状态");
  const resume = screen.getByRole("button", { name: "继续" }) as HTMLButtonElement;
  expect(resume.disabled).toBe(true);
});

it("treats a missing root member as unavailable even when the verdict says present", async () => {
  await renderDetail(detail(session("me"), { members: [], root_source_status: "present" }));

  screen.getByText(/没有 Root 成员记录/);
  screen.getByText("无法确认源会话状态");
});

// 消息

it("renders only the user/assistant conversation", async () => {
  await renderDetail(detail(session("me"), {
    messages: [
      message("me", 1, "user", "帮我看看这个报错"),
      message("me", 2, "assistant", "好的，我在看"),
    ],
  }));

  screen.getByText("帮我看看这个报错");
  screen.getByText("好的，我在看");
});

it("previews the newest messages and links to the whole conversation", async () => {
  const me = session("me");
  const { navigate } = await renderDetail(detail(me, {
    messages: [message("me", 419, "user", "最近的一句话")],
    ingested_message_sequence: 420,
  }));

  screen.getByText("最近的一句话");
  // 预览只是最后 10 条：说清还剩多少没显示，入口用当前会话总数。
  screen.getByText("以上是最近 1 条。");
  fireEvent.click(screen.getByRole("button", { name: "查看全部会话（共 420 条）" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "me", entry: "conversation" });
});

it("offers no conversation entry for a session that has no messages", async () => {
  await renderDetail(detail(session("me")));

  expect(screen.queryByRole("button", { name: /查看全部会话/ })).toBeNull();
  screen.getByText(/还没有摄入消息/);
});

// Fork

it("links to the session it forked from", async () => {
  const me = session("me", { forked_from_session_id: "orig" });
  const { navigate } = await renderDetail(detail(me, {
    forked_from: session("orig", { title: "原始会话" }),
  }));

  fireEvent.click(screen.getByRole("button", { name: "分叉自：原始会话" }));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "orig" });
});

it("names the missing fork source instead of hiding the provenance", async () => {
  const me = session("me", { forked_from_session_id: "ghost" });
  await renderDetail(detail(me));

  screen.getByText(/来源会话不在 NoEnding 库里/);
  screen.getByText(/ghost/);
});

it("shows no fork field for a session that is not a fork", async () => {
  await renderDetail(detail(session("me")));

  expect(screen.queryByText(/分叉自/)).toBeNull();
});

// 回收站横幅

function preview(over: Partial<LocalDeletePreview> = {}): LocalDeletePreview {
  return {
    session_id: "me",
    session_title: "会话一",
    agent: "codex",
    root_agent_session_id: "me-agent",
    root_source_status: "missing",
    message_count: 3,
    member_count: 1,
    session_context_count: 0,
    launch_intent_count: 0,
    context_revision_redaction_count: 0,
    ...over,
  };
}

function trashedDetail(over: Partial<SessionDetail> = {}): SessionDetail {
  return detail(session("me", { trashed_at: "2026-09-24T00:00:00+00:00" }), {
    can_resume: false,
    ...over,
  });
}

it("offers the same 删除 entry whatever the root source state", async () => {
  // Trash is the only gate: the source verdict never renames or hides the entry.
  for (const root_source_status of ["missing", "present"] as const) {
    cleanup();
    await renderDetail(trashedDetail({ root_source_status }));
    screen.getByRole("button", { name: "删除…" });
    expect(screen.queryByText(/重新入库…/)).toBeNull();
    expect(screen.queryByText(/删除不可用/)).toBeNull();
  }
});

// 点删除之后：先查源状态，告知这次是彻底删除还是会被重新入库，用户再确认。

it("checks the root source before confirming and says the copy will be rebuilt", async () => {
  vi.mocked(api.getSessionLocalDeletePreview).mockResolvedValue(
    preview({ root_source_status: "present" }),
  );
  await renderDetail(trashedDetail({ root_source_status: "present" }));

  fireEvent.click(screen.getByRole("button", { name: "删除…" }));

  await screen.findByText(/Root 源会话仍然存在/);
  screen.getByText(/下一次同步会从它重新摄入/);
  // 每一条「将删除」都渲染出数字：后端改名而前端漏改时，这里会当场炸掉，
  // 而不是在生产里把整棵树渲染崩成黑屏。
  screen.getByText("3 条会话消息");
  screen.getByText("1 个执行成员");
  screen.getByText("0 条 Context 摘要记录");
  screen.getByText("0 条启动记录");
  screen.getByText("上下文来源改写 0 条");
  screen.getByRole("button", { name: "删除" });
});

it("says the local data is unrecoverable when the root source is gone", async () => {
  vi.mocked(api.getSessionLocalDeletePreview).mockResolvedValue(
    preview({ root_source_status: "missing" }),
  );
  await renderDetail(trashedDetail({ root_source_status: "missing" }));

  fireEvent.click(screen.getByRole("button", { name: "删除…" }));

  await screen.findByText(/本地数据删除后无法找回/);
  expect(screen.queryByText(/Root 源会话仍然存在/)).toBeNull();
});

// 所属任务（单 Owner）

it("shows the one owner workstream and links to it", async () => {
  const owner = workstream("w1", "会话与 Workstream 重构");
  const { navigate } = await renderDetail(detail(session("me", { owner_workstream_id: "w1" }), {
    owner_workstream: owner,
  }));

  screen.getByText("所属任务");
  const link = screen.getByRole("button", { name: owner.title });
  fireEvent.click(link);
  expect(navigate).toHaveBeenCalledWith({ view: "workstream", workstreamId: "w1" });
  // 只有一个所属任务：没有「添加」多任务入口。
  expect(screen.queryByRole("button", { name: /添加/ })).toBeNull();
});

it("marks an unowned session and offers 选择 instead of 添加", async () => {
  await renderDetail(detail(session("me")));

  screen.getByText("所属任务");
  screen.getByText("未归属任务");
  screen.getByRole("button", { name: "选择" });
  expect(screen.queryByRole("button", { name: "更改" })).toBeNull();
});

it("replaces the owner through a single-choice modal", async () => {
  const owner = workstream("w1", "旧任务");
  vi.mocked(api.listWorkstreams).mockResolvedValue([owner, workstream("w2", "新任务")]);
  vi.mocked(api.setSessionOwnerWorkstream).mockResolvedValue(session("me"));
  await renderDetail(detail(session("me", { owner_workstream_id: "w1" }), { owner_workstream: owner }));

  fireEvent.click(screen.getByRole("button", { name: "更改" }));
  const dialog = await screen.findByRole("dialog", { name: "选择所属任务" });
  // 单选：一次只能选一个。
  const radios = dialog.querySelectorAll('input[type="radio"]');
  expect(radios).toHaveLength(3);
  fireEvent.click(screen.getByText("新任务"));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));

  await waitFor(() => expect(api.setSessionOwnerWorkstream).toHaveBeenCalledWith("me", "w2"));
});

it("clears the owner by choosing 未归属", async () => {
  const owner = workstream("w1", "旧任务");
  vi.mocked(api.listWorkstreams).mockResolvedValue([owner]);
  vi.mocked(api.setSessionOwnerWorkstream).mockResolvedValue(session("me"));
  await renderDetail(detail(session("me", { owner_workstream_id: "w1" }), { owner_workstream: owner }));

  fireEvent.click(screen.getByRole("button", { name: "更改" }));
  await screen.findByRole("dialog", { name: "选择所属任务" });
  fireEvent.click(screen.getByText("未归属"));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));

  await waitFor(() => expect(api.setSessionOwnerWorkstream).toHaveBeenCalledWith("me", null));
});

// Context 面板

function contextView(over: Partial<SessionContextView> = {}): SessionContextView {
  return {
    session_id: "me",
    fields: null,
    revision: 0,
    ingest_generation: 0,
    processed_through_seq: 0,
    latest_message_seq: 0,
    updated_at: null,
    pending: false,
    ...over,
  };
}

it("offers 生成摘要 only when there are messages and no summary yet", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "开始吧")],
  }));

  screen.getByText("Context");
  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  await waitFor(() => expect(api.updateSessionContext).toHaveBeenCalledWith("me"));
  // 更新成功后重新拉取 Context 与详情。
  await waitFor(() => expect(api.getSessionContext).toHaveBeenCalledTimes(2));
});

it("keeps the summary read-only and shows 更新摘要 while there is a pending increment", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({
    fields: {
      summary_current_state: "正在重构前端入口",
      decisions: ["删掉 Context Intelligence 开关"],
      open_questions: [],
      next_steps: ["补测试"],
    },
    pending: true,
    revision: 3,
  }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "继续")],
  }));

  screen.getByText(/正在重构前端入口/);
  screen.getByText(/删掉 Context Intelligence 开关/);
  screen.getByText(/补测试/);
  fireEvent.click(screen.getByRole("button", { name: "更新摘要" }));
  await waitFor(() => expect(api.updateSessionContext).toHaveBeenCalledWith("me"));
});

it("hides the update button when there is nothing pending", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({
    fields: {
      summary_current_state: "已完成",
      decisions: [],
      open_questions: [],
      next_steps: [],
    },
    pending: false,
    revision: 1,
  }));
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "好了")],
  }));

  expect(screen.queryByRole("button", { name: "更新摘要" })).toBeNull();
  expect(screen.queryByRole("button", { name: "生成摘要" })).toBeNull();
});

it("shows structured Context failure details and copies the operation id", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  vi.mocked(api.updateSessionContext).mockRejectedValue(
    { code: "stale_snapshot", message: "内容已变化，请重新更新", operation_id: "123e4567-e89b-12d3-a456-426614174000" },
  );
  const originalClipboard = navigator.clipboard;
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "开始吧")],
  }));

  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  await screen.findByText("内容已变化，请重新更新 · 操作 ID 123e4567-e89b-12d3-a456-426614174000");
  fireEvent.click(screen.getByRole("button", { name: "复制错误详情" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(
    "内容已变化，请重新更新\n错误代码：stale_snapshot\n操作 ID：123e4567-e89b-12d3-a456-426614174000",
  ));
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: originalClipboard });
});

it("keeps an update error visible across background sync refreshes", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  vi.mocked(api.updateSessionContext).mockRejectedValue({
    code: "stale_snapshot",
    message: "内容已变化，请重新更新",
    operation_id: "123e4567-e89b-12d3-a456-426614174000",
  });
  await renderDetail(detail(session("me"), {
    messages: [message("me", 1, "user", "开始吧")],
  }));

  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  const failure = "内容已变化，请重新更新 · 操作 ID 123e4567-e89b-12d3-a456-426614174000";
  await screen.findByText(failure);

  window.dispatchEvent(new Event("noending:sync"));
  await waitFor(() => expect(api.getSessionContext).toHaveBeenCalledTimes(2));
  expect(screen.getByText(failure)).toBeTruthy();
});

it("clears a session's update error when the same detail view navigates to another session", async () => {
  vi.mocked(api.getSessionContext).mockResolvedValue(contextView({ pending: true }));
  vi.mocked(api.updateSessionContext).mockRejectedValue({
    code: "stale_snapshot",
    message: "内容已变化，请重新更新",
    operation_id: "123e4567-e89b-12d3-a456-426614174000",
  });
  const { navigate, rerender } = await renderDetail(detail(session("session-a"), {
    messages: [message("session-a", 1, "user", "开始吧")],
  }));

  fireEvent.click(screen.getByRole("button", { name: "生成摘要" }));
  const failure = "内容已变化，请重新更新 · 操作 ID 123e4567-e89b-12d3-a456-426614174000";
  await screen.findByText(failure);

  rerender(<SessionDetailView sessionId="session-b" navigate={navigate} goBack={vi.fn()} />);
  await waitFor(() => expect(screen.queryByText(failure)).toBeNull());
});
