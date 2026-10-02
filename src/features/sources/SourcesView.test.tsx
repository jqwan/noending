import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { IngestSource } from "../../types";

vi.mock("../../api", () => ({
  api: {
    listIngestSources: vi.fn().mockResolvedValue([]),
    getIngestionStatus: vi.fn().mockResolvedValue(null),
    getAgentStatus: vi.fn().mockResolvedValue({}),
    setAllIngestSourcesEnabled: vi.fn().mockResolvedValue(2),
    setIngestSourceEnabled: vi.fn().mockResolvedValue(undefined),
    setResumeOpenMethod: vi.fn().mockResolvedValue(undefined),
  },
}));

// jsdom 没有 Tauri IPC：组件挂载时会订阅 ingestion-completed，桩掉让 promise 正常落定。
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

// jsdom 不带 matchMedia（AgentIcon/其余依赖若用得到也安全），当前无需额外桩。
import SourcesView from "./SourcesView";
import { api } from "../../api";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const source = (id: string, enabled: boolean): IngestSource => ({
  id,
  agent: "codex",
  path: `/tmp/${id}`,
  enabled,
  origin: "default",
  created_at: "2026-09-29T00:00:00Z",
  exists: true,
});

/** 全选 checkbox 三态 + 点击行为：混合态 indeterminate、点击后批量命令 +
 *  reload。 */
it("the select-all checkbox toggles every source", async () => {
  vi.mocked(api.listIngestSources).mockResolvedValue([
    source("s1", true),
    source("s2", false),
  ]);
  render(<SourcesView />);
  const box = (await screen.findByTitle("选中全部来源")) as HTMLInputElement;
  await waitFor(() => expect(box.indeterminate).toBe(true), {
    timeout: 2000,
  });
  expect(box.checked).toBe(false);

  fireEvent.click(box);
  await waitFor(() =>
    expect(api.setAllIngestSourcesEnabled).toHaveBeenCalledWith(true)
  );
  await waitFor(() => expect(api.listIngestSources).toHaveBeenCalledTimes(2));
  expect(screen.getByText(/已启用 2 个来源/)).toBeTruthy();
});

it("a fully enabled list checks the box and unchecking disables all", async () => {
  vi.mocked(api.listIngestSources).mockResolvedValue([
    source("s1", true),
    source("s2", true),
  ]);
  vi.mocked(api.setAllIngestSourcesEnabled).mockResolvedValue(2);
  render(<SourcesView />);
  const box = (await screen.findByTitle("取消选中全部来源")) as HTMLInputElement;
  await waitFor(() => expect(box.checked).toBe(true));
  expect(box.indeterminate).toBe(false);

  fireEvent.click(box);
  await waitFor(() =>
    expect(api.setAllIngestSourcesEnabled).toHaveBeenCalledWith(false)
  );
  expect(screen.getByText(/已停用 2 个来源/)).toBeTruthy();
});

it("a no-op bulk toggle says so instead of pretending work happened", async () => {
  vi.mocked(api.listIngestSources).mockResolvedValue([
    source("s1", true),
    source("s2", true),
  ]);
  vi.mocked(api.setAllIngestSourcesEnabled).mockResolvedValue(0);
  render(<SourcesView />);
  const box = (await screen.findByTitle("取消选中全部来源")) as HTMLInputElement;
  // Clicking the checked box unchecks it → the command is "disable all",
  // and 0 changed rows means they were already off.
  fireEvent.click(box);
  await waitFor(
    () => expect(screen.getByText(/本来就已停用/)).toBeTruthy(),
    { timeout: 2000 }
  );
});
