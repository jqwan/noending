// UsageView 契约：
// 1. Token 用量与模型请求统一由用量事件提供；
// 2. null 的 token 轴显示「—」，不冒充 0；
// 3. 账本为空时给出补全指引（从头重扫/删库重建），不装作有数据；
// 4. 排行通过会话名称打开详情。

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import UsageView from "./UsageView";
import { api } from "../../api";
import type { Route } from "../../app/routes";
import type { UsageOverview } from "../../types";

vi.mock("../../api", () => ({
  api: {
    getUsageOverview: vi.fn(),
  },
}));

afterEach(cleanup);

/** 分组标题已移除，每张表用第一列的列标题定位它所在的 section。 */
function sectionOf(firstColumnLabel: string): HTMLElement {
  const el = screen.getByRole("columnheader", { name: firstColumnLabel }).closest("section");
  if (!(el instanceof HTMLElement)) throw new Error(`找不到 ${firstColumnLabel} 所在的表格分组`);
  return el;
}

function sectionOfButton(buttonName: string): HTMLElement {
  const el = screen.getByRole("button", { name: buttonName }).closest("section");
  if (!(el instanceof HTMLElement)) throw new Error(`找不到 ${buttonName} 所在的表格分组`);
  return el;
}

const sliceCounts = {
  members: 3, root_members: 2, user_messages: 410, agent_replies: 12_746,
  tool_calls: 96, side_activities: 7, requests: 1234, cache_hit_rate: null,
};

function overviewFixture(): UsageOverview {
  return {
    sessions: 2,
    cache_hit_rate: null,
    requests: 1234,
    members: 3,
    root_members: 2,
    user_messages: 410,
    agent_replies: 12_746,
    tool_calls: 96,
    side_activities: 7,
    assistant_messages: 12_746,
    input_tokens: 12_746_123,
    output_tokens: 9_800,
    cached_tokens: null,
    reasoning_tokens: null,
    by_agent: [
      {
        ...sliceCounts,
        agent: "codex",
        sessions: 1,
        members: 2,
        assistant_messages: 12_000,
        input_tokens: 12_746_123,
        output_tokens: 9_000,
        cached_tokens: null,
        reasoning_tokens: null,


      },
      {
        ...sliceCounts,
        agent: "qoder",
        sessions: 1,
        members: 1,
        assistant_messages: 746,
        input_tokens: 500,
        output_tokens: 800,
        cached_tokens: 300,
        reasoning_tokens: null,


      },
    ],
    by_relation: [
      { ...sliceCounts, relation: "root", input_tokens: 12_745_623, output_tokens: 9_700, cached_tokens: null, reasoning_tokens: null },
      { ...sliceCounts, relation: "child", input_tokens: 500, output_tokens: 100, cached_tokens: null, reasoning_tokens: null },
    ],
    by_project: [
      { ...sliceCounts, id: "p-1", name: "NoEnding", sessions: 2, input_tokens: 12_746_123, output_tokens: 9_800, cached_tokens: null, reasoning_tokens: null },
    ],
    by_workstream: [],
    top_sessions: [
      {
        ...sliceCounts,
        session_id: "s-1",
        title: "重构 ingestion",
        agent: "codex",
        last_activity_at: "2026-09-28T10:00:00Z",
        assistant_messages: 50,
        input_tokens: 9_000_000,
        output_tokens: 1_000,
        cached_tokens: null,
        reasoning_tokens: null,
      },
    ],
    ledger_events: 3,
    attributed_events: 2,
    unattributed_events: 1,
    by_model: [
      {
        members: 3, root_members: 2,
        user_messages: null, agent_replies: null, tool_calls: null, side_activities: null,
        cache_hit_rate: 8 / 10,
        model: "deepseek-v4.1-flash",
        display: "DeepSeek-V4.1-Flash",
        events: 2,
        requests: 15,
        input_tokens: 2_000_000,
        output_tokens: 1_000_000,
        cached_tokens: 8_000_000,
        reasoning_tokens: 0,

        agents: ["qoder", "workbuddy"],
      },
    ],
    by_category: [
      { category: "conversation", events: 3, requests: 15, input_tokens: 2_000_000, output_tokens: 1_000_000, cached_tokens: 8_000_000, reasoning_tokens: 0 },
    ],
    series: [
      { day: "2026-10-01", events: 3, input_tokens: 2_000_000, output_tokens: 1_000_000, cached_tokens: 0, reasoning_tokens: 0 },
    ],
  };
}

beforeEach(() => {
  vi.mocked(api.getUsageOverview).mockReset().mockResolvedValue(overviewFixture());
});

describe("UsageView", () => {
  it("renders the billed axes and keeps an unrecorded axis at —", async () => {
    render(<UsageView navigate={vi.fn()} />);

    // 12,746,123 → 12.75M；9,800 → 9,800（低于 10k 缩写阈值）。
    expect((await screen.findByTitle("12,746,123")).textContent).toContain("12.75M");
    const requestStat = screen.getByText("模型请求", { selector: ".lbl" }).closest(".usage-stat")!;
    expect(requestStat.querySelector(".num")?.textContent).toBe("1,234");
    expect(screen.getByTitle("9,800").textContent).toContain("9,800");

    // 没人记录的轴不冒充 0。
    expect(screen.getAllByText("—").length).toBeGreaterThanOrEqual(2);

    // 统计条与 token 大数卡同一套卡片 UI：数值在 .num（title 带全量数字）。
    expect((await screen.findByTitle("3")).textContent).toBe("3");
    expect(screen.getAllByText("会话成员").length).toBeGreaterThan(1);
    expect(screen.getByTitle("12,746").textContent).toBe("12,746");
    await waitFor(() => expect(api.getUsageOverview).toHaveBeenCalled());
    expect(screen.queryByText("估算成本")).toBeNull();
    expect(screen.queryByRole("button", { name: "刷新价格表" })).toBeNull();
    expect(screen.queryByTitle(/计价来源/)).toBeNull();
    expect(document.querySelectorAll(".usage-totals .usage-total")).toHaveLength(4);
  });

  it("shows the ledger's per-model tokens and cache hit rate", async () => {
    render(<UsageView navigate={vi.fn()} />);

    const cell = await screen.findByText("DeepSeek-V4.1-Flash");
    expect(cell.getAttribute("title")).toBe("deepseek-v4.1-flash");
    expect(screen.queryByText("Qoder、WorkBuddy")).toBeNull();
    // 未归因桶单独一个诚实计数。
    expect(screen.getByText(/另有 1 次计费调用没有模型出处/).textContent).toBeTruthy();
    // 命中率按 token 加权、保留两位小数：8M / (2M + 8M) = 80.00%。
    expect(within(cell.closest("tr")!).getByTitle(/Token 加权/).textContent).toBe("80.00%");
  });

  it("renders the daily series, relation and project slices", async () => {
    render(<UsageView navigate={vi.fn()} />);

    expect(await screen.findByText("最近 1 天")).toBeTruthy();
    expect(document.querySelector(".usage-series")?.querySelectorAll(".usage-series-bar").length).toBe(1);
    expect(screen.getAllByText("会话").length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("根会话").textContent).toBeTruthy(); // 关系表的行标签保留「根会话」词汇
    expect(screen.getByText("子会话").textContent).toBeTruthy();
    expect(screen.getByText("NoEnding").textContent).toBeTruthy();
    // 按任务板块常驻：还没有任务时板块照在，说明怎么让这里长出数据。
    expect(sectionOf("任务").textContent).toContain("还没有任务");
  });

  it("shows overview axes for other dimensions and only applicable model columns", async () => {
    const data = overviewFixture();
    data.by_workstream = [{ ...data.by_project[0], id: "w-1", name: "用量面板" }];
    vi.mocked(api.getUsageOverview).mockResolvedValue(data);
    render(<UsageView navigate={vi.fn()} />);
    await screen.findByText("NoEnding");

    const countLabels = ["会话", "会话成员", "用户消息", "代理回复", "工具调用", "代理协同", "模型请求"];
    for (const firstColumn of ["会话格式", "成员类别", "项目", "任务"]) {
      const section = sectionOf(firstColumn);
      const columns = firstColumn === "成员类别" ? countLabels.filter((label) => label !== "会话") : countLabels;
      for (const label of ["输入", "输出", "缓存命中", ...columns]) {
        expect(within(section).getByRole("columnheader", { name: label })).toBeTruthy();
      }
      const headers = within(section).getAllByRole("columnheader").map((cell) => cell.textContent);
      const countStart = 1;
      expect(headers.slice(countStart, countStart + columns.length + 4)).toEqual([
        ...columns, "输入", "输出", "缓存命中", "总量",
      ]);
      expect(section.querySelector(".usage-table-scroll")).toBeTruthy();
    }
    const project = screen.getByText("NoEnding").closest("tr")!;
    expect([...project.querySelectorAll("td")].map((cell) => cell.textContent)).toEqual([
      "NoEnding", "2", "3", "410", "12,746", "96", "7", "1,234", "12.75M", "9,800", "—", "12.76M",
    ]);
    const modelSection = sectionOf("模型");
    expect(within(modelSection).getAllByRole("columnheader").map((cell) => cell.textContent)).toEqual([
      "模型", "会话", "会话成员", "模型请求", "输入", "输出", "缓存命中", "总量",
    ]);
    const model = screen.getByText("DeepSeek-V4.1-Flash").closest("tr")!;
    expect(model.querySelectorAll("td")).toHaveLength(8);
    expect([...model.querySelectorAll("td")].slice(1, 4).map((cell) => cell.textContent)).toEqual([
      String(data.by_model[0].root_members), String(data.by_model[0].members), String(data.by_model[0].requests),
    ]);
    expect(screen.getByText(/不能跨模型相加/)).toBeTruthy();
  });

  it("merges cached input in every dimension, the overview and daily series", async () => {
    const data = overviewFixture();
    const tokens = { input_tokens: 100, cached_tokens: 50, output_tokens: 20 };
    Object.assign(data, tokens);
    for (const row of [...data.by_agent, ...data.by_model, ...data.by_relation, ...data.by_project, ...data.top_sessions]) {
      Object.assign(row, tokens);
    }
    data.by_workstream = [{ ...data.by_project[0], id: "w-1", name: "用量面板" }];
    data.series = [
      { ...data.series[0], ...tokens },
      { ...data.series[0], day: "2026-09-30", input_tokens: 85, cached_tokens: 0, output_tokens: 0 },
    ];
    vi.mocked(api.getUsageOverview).mockResolvedValue(data);
    render(<UsageView navigate={vi.fn()} />);
    await screen.findByText("任务");

    const totalCards = [...document.querySelectorAll(".usage-totals .usage-total")];
    expect(totalCards.map((card) => card.querySelector(".lbl")?.textContent)).toEqual([
      "总量", "输入", "输出", "缓存命中",
    ]);
    expect(totalCards.map((card) => card.querySelector(".num")?.textContent)).toEqual([
      "170", "150", "20", "—",
    ]);
    expect(totalCards[0].querySelector(".num")?.getAttribute("title")).toBe("170");
    for (const table of screen.getAllByRole("table")) {
      const headers = within(table).getAllByRole("columnheader");
      const inputColumn = headers.findIndex((header) => header.textContent === "输入");
      for (const row of table.querySelectorAll("tbody tr")) {
        expect(row.querySelectorAll("td")[inputColumn].textContent).toBe("150");
      }
    }
    expect(screen.queryByText("输入缓存")).toBeNull();
    const bars = [...document.querySelectorAll(".usage-series-bar")];
    expect(bars.map((bar) => bar.getAttribute("title"))).toEqual([
      "2026-09-30：输入 85 · 输出 0", "2026-10-01：输入 150 · 输出 20",
    ]);
    expect((bars[0].firstElementChild as HTMLElement).style.height).toBe("50%");
    expect((bars[1].firstElementChild as HTMLElement).style.height).toBe("100%");
    expect(screen.getByText("80.00%")).toBeTruthy();
  });

  it("keeps unknown merged totals at — and includes cache-only input", async () => {
    const data = overviewFixture();
    data.input_tokens = null;
    data.cached_tokens = 50;
    data.output_tokens = null;
    data.by_agent[0].input_tokens = null;
    data.by_agent[0].cached_tokens = null;
    data.by_agent[1].input_tokens = null;
    data.by_agent[1].cached_tokens = 300;
    vi.mocked(api.getUsageOverview).mockResolvedValue(data);
    render(<UsageView navigate={vi.fn()} />);
    await screen.findByText("会话格式");
    const overview = document.querySelector(".usage-totals")!;
    expect([...overview.querySelectorAll(".num")].map((cell) => cell.textContent)).toEqual([
      "50", "50", "—", "—",
    ]);
    const agentSection = sectionOf("会话格式");
    const rows = agentSection.querySelectorAll("tbody tr");
    expect(rows[0].querySelectorAll("td")[8].textContent).toBe("—");
    expect(rows[1].querySelectorAll("td")[8].textContent).toBe("300");
  });

  it("distinguishes missing overview totals from recorded zero", async () => {
    for (const [cached, expected] of [[null, "—"], [0, "0"]] as const) {
      const data = overviewFixture();
      data.input_tokens = null;
      data.cached_tokens = cached;
      data.output_tokens = null;
      vi.mocked(api.getUsageOverview).mockResolvedValue(data);
      render(<UsageView navigate={vi.fn()} />);
      await screen.findByText("会话格式");
      const overview = document.querySelector(".usage-totals")!;
      expect([...overview.querySelectorAll(".num")].map((cell) => cell.textContent)).toEqual([
        expected, expected, "—", "—",
      ]);
      cleanup();
    }
  });

  it("opens a project from its name", async () => {
    const navigate = vi.fn();
    render(<UsageView navigate={navigate} />);
    fireEvent.click(await screen.findByRole("button", { name: "NoEnding" }));
    expect(navigate).toHaveBeenCalledWith({ view: "project", projectId: "p-1" } satisfies Route);
  });

  it("shows consistent cache-hit formatting across all dimensions and the overview", async () => {
    const data = overviewFixture();
    data.cache_hit_rate = 0.6;
    data.by_agent[0].cache_hit_rate = 0;
    data.by_agent[1].cache_hit_rate = 1;
    data.by_relation[0].cache_hit_rate = 0.25;
    data.by_project[0].cache_hit_rate = 0.4;
    data.by_workstream = [{ ...data.by_project[0], id: "w-1", name: "用量面板", cache_hit_rate: 0.3 }];
    data.top_sessions[0].cache_hit_rate = 0.5;
    vi.mocked(api.getUsageOverview).mockResolvedValue(data);
    render(<UsageView navigate={vi.fn()} />);
    await screen.findByText("任务");
    const overview = screen.getByText("缓存命中", { selector: ".lbl" }).closest(".usage-total")!;
    expect(overview.querySelector(".num")?.textContent).toBe("60.00%");
    for (const [firstColumn, values] of [
      ["会话格式", ["0.00%", "100.00%"]],
      ["模型", ["80.00%"]],
      ["成员类别", ["25.00%", "—"]],
      ["项目", ["40.00%"]],
      ["任务", ["30.00%"]],
    ] as const) {
      const section = sectionOf(firstColumn);
      expect(within(section).getAllByTitle(/Token 加权/).map((cell) => cell.textContent)).toEqual([...values]);
    }
    const ranking = sectionOfButton("重构 ingestion");
    expect(within(ranking).getAllByTitle(/Token 加权/).map((cell) => cell.textContent)).toEqual(["50.00%"]);
    const relation = sectionOf("成员类别");
    expect(within(relation).queryByRole("columnheader", { name: "根会话" })).toBeNull();
    expect(within(relation).getByText("根会话")).toBeTruthy();
    expect(screen.queryByRole("columnheader", { name: "命中率" })).toBeNull();
  });

  it("guides the reader to a rebuild when the ledger is empty", async () => {
    vi.mocked(api.getUsageOverview).mockReset().mockResolvedValue({
      ...overviewFixture(),
      requests: 0,
      ledger_events: 0,
      attributed_events: 0,
      unattributed_events: 0,
      by_model: [],
      by_category: [],
      series: [],
    });
    render(<UsageView navigate={vi.fn()} />);

    expect((await screen.findByText(/账本还没有数据/)).textContent).toBeTruthy();
    expect(screen.getByText(/从头重扫/).textContent).toBeTruthy();
  });

  it("navigates to the session detail from the ranking's session title", async () => {
    const navigate = vi.fn();
    render(<UsageView navigate={navigate} />);

    const open = await screen.findByRole("button", { name: "重构 ingestion" });
    const section = sectionOfButton("重构 ingestion");
    expect(within(section).getAllByRole("columnheader").map((cell) => cell.getAttribute("aria-label") ?? cell.textContent)).toEqual([
      "会话", "会话成员", "用户消息", "代理回复", "工具调用", "代理协同", "模型请求", "输入", "输出", "缓存命中", "总量",
    ]);
    expect([...open.closest("tr")!.querySelectorAll("td")].map((cell) => cell.textContent)).toEqual([
      "重构 ingestion", "3", "410", "12,746", "96", "7", "1,234", "9.00M", "1,000", "—", "9.00M",
    ]);
    expect(within(section).queryByRole("button", { name: "打开" })).toBeNull();
    fireEvent.click(open);

    expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s-1" } satisfies Route);
  });

  it("clicking a column header sorts by it and toggles direction", async () => {
    const data = overviewFixture();
    data.by_agent[0].cache_hit_rate = null; // codex：没有输入用量 → —
    data.by_agent[1].cache_hit_rate = 0.5;
    vi.mocked(api.getUsageOverview).mockResolvedValue(data);
    render(<UsageView navigate={vi.fn()} />);
    await screen.findByText("会话格式");
    const table = document.querySelector<HTMLElement>(".usage-table")!;
    const rows = () => [...table.querySelectorAll("tbody tr")].map((r) => r.querySelector("td")!.textContent);
    // 默认按总量降序，且“总量”列头从加载起就带箭头提示。
    expect(rows()).toEqual(["Codex", "Qoder"]);
    expect(within(table).getByRole("columnheader", { name: "总量" }).getAttribute("aria-sort")).toBe("descending");

    // 数值列第一次点击 = 降序（大的在前），再点同一列反向。
    const headerButton = (label: string) => within(table).getByRole("button", { name: label });
    fireEvent.click(headerButton("会话成员"));
    expect(rows()).toEqual(["Codex", "Qoder"]); // 2 > 1，降序
    expect(within(table).getByRole("columnheader", { name: "会话成员" }).getAttribute("aria-sort")).toBe("descending");

    fireEvent.click(headerButton("会话成员"));
    expect(rows()).toEqual(["Qoder", "Codex"]); // 反向：升序
    expect(within(table).getByRole("columnheader", { name: "会话成员" }).getAttribute("aria-sort")).toBe("ascending");

    // 换一列从默认方向重新开始；null（—）恒排最后，与方向无关。
    fireEvent.click(headerButton("缓存命中"));
    expect(within(table).getByRole("columnheader", { name: "缓存命中" }).getAttribute("aria-sort")).toBe("descending");
    expect(rows()).toEqual(["Qoder", "Codex"]); // qoder 有命中率，codex 的 null 殿后
  });

  it("pages every table to 10 and sorts across ALL rows, not just the shown page", async () => {
    const data = overviewFixture();
    // 12 个会话：默认按总量（输入+输出）降序；s-11/s-12 总量最小，默认不在前 10。
    data.top_sessions = Array.from({ length: 12 }, (_, i) => ({
      ...data.top_sessions[0],
      session_id: `s-${i + 1}`,
      title: `会话 ${i + 1}`,
      input_tokens: 1_000_000 - i * 1000,
      output_tokens: 100,
      cached_tokens: null,
    }));
    // s-11 的用户消息全场最多——点“用户消息”降序时它必须从第 11 行冒到第一行。
    data.top_sessions[10].user_messages = 1_000_000;
    vi.mocked(api.getUsageOverview).mockResolvedValue(data);
    render(<UsageView navigate={vi.fn()} />);

    const ranking = await screen.findByText("会话 1");
    const section = ranking.closest("section")!;
    const visibleTitles = () =>
      [...section.querySelectorAll("tbody tr")].map((r) => r.querySelector("td")!.textContent);
    expect(visibleTitles()).toHaveLength(10);
    expect(visibleTitles()).not.toContain("会话 11");

    // 排序作用于全部 12 行：新列的第一名从页面外进来。
    fireEvent.click(within(section).getByRole("button", { name: "用户消息" }));
    expect(visibleTitles()[0]).toBe("会话 11");
    expect(visibleTitles()).toHaveLength(10);

    // 全部展开后 12 行可见；收起回到前 10。
    fireEvent.click(within(section).getByRole("button", { name: "显示全部" }));
    expect(visibleTitles()).toHaveLength(12);
    fireEvent.click(within(section).getByRole("button", { name: "收起，仅显示前 10" }));
    expect(visibleTitles()).toHaveLength(10);
  });

  it("offers the empty state before any session exists", async () => {
    vi.mocked(api.getUsageOverview).mockReset().mockResolvedValue({
      ...overviewFixture(),
      sessions: 0,
      requests: 0,
      root_members: 0,
      user_messages: 0,
      agent_replies: 0,
      tool_calls: 0,
      side_activities: 0,
      by_agent: [],
      by_relation: [],
      by_project: [],
      by_model: [],
      by_category: [],
      series: [],
      top_sessions: [],
    });
    render(<UsageView navigate={vi.fn()} />);

    expect(await screen.findByText("还没有会话数据")).toBeTruthy();
  });
});
