import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionConversationView from "./SessionConversationView";
import { api } from "../../api";
import type {
  Session,
  SessionDetail,
  SessionMessage,
  SessionMessageMark,
  SessionMessageWindow,
  SessionWindowMessage,
} from "../../types";

// 只覆盖阅读界面自己的规则：从最新一页打开、按 ordinal 拼页不重不乱、会话被改写时
// 回到最新、导航条跳转（含未加载的目标）、跳转后向下续读、读失败不当成空会话。
vi.mock("../../api", () => ({
  api: {
    getSessionDetail: vi.fn(),
    getSessionMessages: vi.fn(),
    getSessionUserMessageMarks: vi.fn(),
  },
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function session(id: string): Session {
  return {
    id,
    agent: "codex",
    root_agent_session_id: `${id}-agent`,
    title: "重构会话消息",
    cwd: null,
    project_id: null,
    workspace_path_id: null,
    owner_workstream_id: null,
    forked_from_session_id: null,
    started_at: null,
    last_activity_at: null,
    last_conversation_at: null,
    trashed_at: null,
  };
}

function detail(me: Session): SessionDetail {
  return {
    session: me,
    messages: [],
    owner_workstream: null,
    workspace_path: null,
    members: [],
    stats: {
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
    },
    ingested_message_sequence: 0,
    processed_message_sequence: 0,
    root_source_status: "present",
    can_resume: true,
    forked_from: null,
    cost_unit: null,
  };
}

function message(ordinal: number, role: SessionMessage["role"], content: string): SessionWindowMessage {
  return {
    id: `m${ordinal}`,
    session_id: "me",
    member_id: "root",
    sequence: ordinal,
    ordinal,
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

function page(over: Partial<SessionMessageWindow> = {}): SessionMessageWindow {
  return { messages: [], generation: 1, total: 0, next_before_ordinal: null, ...over };
}

function mark(ordinal: number, preview: string): SessionMessageMark {
  return { ordinal, preview };
}

async function renderConversation(marks: SessionMessageMark[] = []) {
  vi.mocked(api.getSessionDetail).mockResolvedValue(detail(session("me")));
  vi.mocked(api.getSessionUserMessageMarks).mockResolvedValue(marks);
  const view = render(<SessionConversationView sessionId="me" goBack={vi.fn()} />);
  return view.container;
}

/** 给滚动列装上 jsdom 不会算的几何量，才能分别命中「向上/向下加载」的判定。 */
function sizeScroller(container: HTMLElement, scrollHeight: number, clientHeight: number, scrollTop: number) {
  const node = container.querySelector(".conversation-scroll") as HTMLDivElement;
  Object.defineProperty(node, "scrollHeight", { configurable: true, value: scrollHeight });
  Object.defineProperty(node, "clientHeight", { configurable: true, value: clientHeight });
  node.scrollTop = scrollTop;
  return node;
}

/**
 * 给滚动列和每一行装上随 scrollTop 变化的几何：第 i 行占 100px。jsdom 不排版，
 * 不装这套就量不出「那一行还在原来的位置」——行的位置是锚定的唯一依据。
 */
function fakeRows(container: HTMLElement, rowHeight = 100, clientHeight = 150) {
  const scroller = container.querySelector(".conversation-scroll") as HTMLDivElement;
  const rows = () => [...scroller.querySelectorAll<HTMLElement>("[data-seq]")];
  const box = (top: number, height: number) => ({
    top, bottom: top + height, left: 0, right: 400, width: 400, height, x: 0, y: top,
    toJSON: () => ({}),
  }) as DOMRect;
  Object.defineProperty(scroller, "scrollHeight", { configurable: true, get: () => rows().length * rowHeight });
  Object.defineProperty(scroller, "clientHeight", { configurable: true, value: clientHeight });
  scroller.getBoundingClientRect = () => box(0, clientHeight);
  for (const row of rows()) {
    // 位置按当次的行序现算：插入更早的消息后，同一行的下标会变。
    row.getBoundingClientRect = () => box(rows().indexOf(row) * rowHeight - scroller.scrollTop, rowHeight);
  }
  return scroller;
}

it("opens on the newest page and counts the older messages left above it", async () => {
  vi.mocked(api.getSessionMessages).mockResolvedValue(page({
    messages: [message(4, "user", "第四句"), message(5, "assistant", "第五句")],
    total: 5,
    next_before_ordinal: 3,
  }));

  await renderConversation([mark(1, "第一次提问"), mark(4, "第四句")]);

  await screen.findByText("第四句");
  screen.getByText("第五句");
  expect(api.getSessionMessages).toHaveBeenCalledWith("me", { limit: expect.any(Number) });
  screen.getByRole("button", { name: "加载更早的消息（还有 3 条）" });
  screen.getByText(/共 5 条消息/);
  // 导航条按用户消息给出 tick，悬停文案是消息首行。
  const ticks = document.querySelectorAll(".conversation-nav .nav-tick");
  expect(ticks).toHaveLength(2);
  expect(ticks[0].getAttribute("title")).toBe("第一次提问");
});

it("prepends the older page the cursor points at", async () => {
  vi.mocked(api.getSessionMessages)
    .mockResolvedValueOnce(page({
      messages: [message(4, "user", "第四句")],
      total: 4,
      next_before_ordinal: 3,
    }))
    .mockResolvedValueOnce(page({
      messages: [message(1, "user", "第一句"), message(2, "assistant", "第二句")],
      total: 4,
      next_before_ordinal: null,
    }));

  await renderConversation();

  fireEvent.click(await screen.findByRole("button", { name: "加载更早的消息（还有 3 条）" }));

  await screen.findByText("第一句");
  expect(api.getSessionMessages).toHaveBeenLastCalledWith("me", {
    beforeOrdinal: 3,
    limit: expect.any(Number),
  });
  const body = document.body.textContent ?? "";
  expect(body.indexOf("第一句")).toBeLessThan(body.indexOf("第四句"));
  expect(screen.queryByRole("button", { name: /加载更早的消息/ })).toBeNull();
});

it("keeps the conversation ordered by ordinal even when a page repeats or arrives scrambled", async () => {
  // 第二页与已加载的内容重叠（序号 3 又来一次）、并且自己就是乱序的：
  // 同一 ordinal 只留一条，最终顺序仍严格按 ordinal——不会出现「563 下一条是 504」。
  vi.mocked(api.getSessionMessages)
    .mockResolvedValueOnce(page({
      messages: [message(3, "user", "第三句"), message(4, "assistant", "第四句")],
      total: 4,
      next_before_ordinal: 3,
    }))
    .mockResolvedValueOnce(page({
      messages: [message(3, "user", "第三句"), message(1, "user", "第一句"), message(2, "assistant", "第二句")],
      total: 4,
      next_before_ordinal: null,
    }));

  await renderConversation();

  fireEvent.click(await screen.findByRole("button", { name: "加载更早的消息（还有 3 条）" }));
  await screen.findByText("第一句");

  const rendered = [...document.querySelectorAll(".conversation-scroll .event")].map((el) => el.getAttribute("data-seq"));
  expect(rendered).toEqual(["1", "2", "3", "4"]);
});

it("jumps to an unloaded user message by loading a window around it", async () => {
  vi.mocked(api.getSessionMessages)
    .mockResolvedValueOnce(page({
      messages: [message(20, "user", "较近的一条"), message(21, "assistant", "回答")],
      total: 30,
      next_before_ordinal: 19,
    }))
    .mockResolvedValueOnce(page({
      messages: [message(1, "user", "很久以前的提问"), message(2, "assistant", "当时的回答")],
      total: 30,
      next_before_ordinal: null,
    }));

  await renderConversation([mark(1, "很久以前的提问")]);

  const tick = await screen.findByRole("button", { name: "很久以前的提问" });
  fireEvent.click(tick);

  await screen.findByText("很久以前的提问");
  // 跳转取的是一个以目标为中心的窗口，不是一页；落点上方留了 JUMP_LEAD 条。
  expect(api.getSessionMessages).toHaveBeenLastCalledWith("me", {
    beforeOrdinal: 42,
    limit: expect.any(Number),
  });
  expect(screen.queryByText("较近的一条")).toBeNull();
});

it("keeps reading forward after a jump", async () => {
  vi.mocked(api.getSessionMessages)
    .mockResolvedValueOnce(page({
      messages: [message(20, "user", "较近的一条")],
      total: 30,
      next_before_ordinal: 19,
    }))
    .mockResolvedValueOnce(page({
      messages: [message(1, "user", "很久以前的提问"), message(2, "assistant", "当时的回答")],
      total: 30,
      next_before_ordinal: null,
    }))
    .mockResolvedValueOnce(page({
      messages: [message(3, "user", "接着往下")],
      total: 30,
      next_before_ordinal: 2,
    }));

  const container = await renderConversation([mark(1, "很久以前的提问")]);
  fireEvent.click(await screen.findByRole("button", { name: "很久以前的提问" }));
  await screen.findByText("当时的回答");

  // 跳进中段之后，往下来到已加载范围的末尾就该接上更新的一页。
  const scroller = sizeScroller(container, 2000, 400, 1560);
  fireEvent.scroll(scroller);

  await screen.findByText("接着往下");
  expect(api.getSessionMessages).toHaveBeenLastCalledWith("me", {
    afterOrdinal: 2,
    limit: expect.any(Number),
  });
});

it("keeps the row the reader is on when older messages are inserted above", async () => {
  vi.mocked(api.getSessionMessages)
    .mockResolvedValueOnce(page({
      messages: [message(3, "user", "第三句"), message(4, "assistant", "第四句")],
      total: 4,
      next_before_ordinal: 3,
    }))
    .mockResolvedValueOnce(page({
      messages: [message(1, "user", "第一句"), message(2, "assistant", "第二句")],
      total: 4,
      next_before_ordinal: null,
    }));

  const container = await renderConversation();
  await screen.findByText("第三句");
  const scroller = fakeRows(container);
  const topRowSeq = () => {
    const box = scroller.getBoundingClientRect();
    return [...scroller.querySelectorAll<HTMLElement>("[data-seq]")]
      .find((row) => row.getBoundingClientRect().bottom > box.top)?.dataset.seq;
  };
  expect(topRowSeq()).toBe("3");

  // 读者停在第三条上：向上翻页时它是视口最上方那条。
  scroller.scrollTop = 50;
  const offsetBefore = scroller.querySelector<HTMLElement>('[data-seq="3"]')!
    .getBoundingClientRect().top - scroller.getBoundingClientRect().top;

  fireEvent.click(screen.getByRole("button", { name: "加载更早的消息（还有 3 条）" }));
  await screen.findByText("第一句");
  fakeRows(container);

  // 上方的两条把内容撑高了 200px，视口跟着下移 200px：第三条仍在原来的偏移上，
  // 读数留在原地，而不是被顶到新内容的开头（那看起来就像「跳到更早的消息」）。
  const offsetAfter = scroller.querySelector<HTMLElement>('[data-seq="3"]')!
    .getBoundingClientRect().top - scroller.getBoundingClientRect().top;
  expect(scroller.scrollTop).toBe(250);
  expect(offsetAfter).toBe(offsetBefore);
  expect(topRowSeq()).toBe("3");
});

it("does not move the viewport when a newer page is appended below", async () => {
  vi.mocked(api.getSessionMessages)
    .mockResolvedValueOnce(page({
      messages: [4, 5, 6, 7, 8, 9].map((n) => message(n, "assistant", `第${n}句`)),
      total: 10,
      next_before_ordinal: 3,
    }))
    .mockResolvedValueOnce(page({
      messages: [message(10, "assistant", "第十句")],
      total: 10,
      next_before_ordinal: 3,
    }));

  const container = await renderConversation();
  await screen.findByText("第4句");
  const scroller = fakeRows(container);
  // 已经到了已加载范围的末尾（离底 50px），但离顶部还很远：这是「向下续读」。
  scroller.scrollTop = 400;
  fireEvent.scroll(scroller);

  await screen.findByText("第十句");
  expect(api.getSessionMessages).toHaveBeenLastCalledWith("me", {
    afterOrdinal: 9,
    limit: expect.any(Number),
  });
  // 追加在下方：首页没变，锚点那一关不会过，视口原地不动。
  expect(scroller.scrollTop).toBe(400);
});

it("waits for the scroll to settle before pulling an older page", async () => {
  vi.mocked(api.getSessionMessages).mockResolvedValue(page({
    messages: [message(4, "user", "第四句")],
    total: 4,
    next_before_ordinal: 3,
  }));

  const container = await renderConversation();
  await screen.findByText("第四句");
  const scroller = fakeRows(container);
  vi.mocked(api.getSessionMessages).mockClear();

  // 手势还在进行：连着几个滚动事件，期间一次都不该发请求——滑动中滚动位置归手势，
  // 这一帧写进去的位置补偿下一帧就被覆盖，正是「加载完跳到更早消息」的来路。
  for (const top of [200, 150, 100, 50, 0]) {
    scroller.scrollTop = top;
    fireEvent.scroll(scroller);
  }
  await new Promise((resolve) => setTimeout(resolve, 80));
  expect(api.getSessionMessages).not.toHaveBeenCalled();

  // 停稳之后才取那一页。
  await waitFor(() => expect(api.getSessionMessages).toHaveBeenCalledTimes(1), { timeout: 2000 });
  expect(api.getSessionMessages).toHaveBeenCalledWith("me", { beforeOrdinal: 3, limit: expect.any(Number) });
});

it("lays the user-message ticks on a fixed pitch, not on their document position", async () => {
  vi.mocked(api.getSessionMessages).mockResolvedValue(page({
    messages: [message(10, "user", "很早的提问")],
    total: 1000,
    next_before_ordinal: null,
  }));

  // 三条用户消息分别在第 10、500、999 条：刻度只按它们是第几条提问排，与它们在会话里
  // 的位置无关，否则 1000 条会话里前三根会被压成一根。
  await renderConversation([
    mark(10, "很早的提问"),
    mark(500, "中间那问"),
    mark(999, "最后一问"),
  ]);
  await screen.findByText("很早的提问");

  const tops = [...document.querySelectorAll<HTMLElement>(".conversation-nav .nav-mark")]
    .map((el) => el.style.top);
  expect(tops).toEqual(["6px", "16px", "26px"]);
  expect(document.querySelectorAll(".conversation-nav .nav-tick")).toHaveLength(3);
});

it("reloads from the tail when the conversation was rewritten", async () => {
  vi.mocked(api.getSessionMessages)
    .mockResolvedValueOnce(page({
      messages: [message(4, "user", "第四句")],
      total: 4,
      next_before_ordinal: 3,
    }))
    .mockResolvedValueOnce(page({
      messages: [message(1, "user", "旧会话的消息")],
      generation: 2,
      total: 1,
      next_before_ordinal: null,
    }))
    .mockResolvedValueOnce(page({
      messages: [message(9, "user", "改写后的唯一一条")],
      generation: 2,
      total: 1,
      next_before_ordinal: null,
    }));

  await renderConversation();

  fireEvent.click(await screen.findByRole("button", { name: "加载更早的消息（还有 3 条）" }));

  await screen.findByText("改写后的唯一一条");
  expect(api.getSessionMessages).toHaveBeenLastCalledWith("me", { limit: expect.any(Number) });
  expect(screen.queryByText("旧会话的消息")).toBeNull();
});

it("offers 跳到最新 only after the viewport left the newest message", async () => {
  vi.mocked(api.getSessionMessages).mockResolvedValue(page({
    messages: [message(1, "user", "唯一一条")],
    total: 1,
  }));

  const container = await renderConversation();
  await screen.findByText("唯一一条");
  const scroller = sizeScroller(container, 1000, 400, 0);
  const scrollTo = vi.fn();
  scroller.scrollTo = scrollTo;

  expect(screen.queryByRole("button", { name: /跳到最新/ })).toBeNull();

  scroller.scrollTop = 100;
  fireEvent.scroll(scroller);

  const jump = await screen.findByRole("button", { name: /跳到最新/ });
  fireEvent.click(jump);
  expect(scrollTo).toHaveBeenCalledWith({ top: 1000, behavior: "smooth" });
});

it("reports a failed read instead of showing an empty conversation", async () => {
  vi.mocked(api.getSessionMessages).mockRejectedValue(new Error("db locked"));

  await renderConversation();

  await screen.findByText(/读不到这个会话的消息/);
  expect(screen.queryByText(/还没有摄入消息/)).toBeNull();
  await waitFor(() => expect(api.getSessionMessages).toHaveBeenCalledTimes(1));
});
