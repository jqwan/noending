import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { clearRuntimeCatalog } from "../agents/useAgentRuntime";
import { runtimeFixture } from "../agents/runtimeFixtures";
import { viewState } from "../../hooks/useViewState";
import SettingsView from "./SettingsView";
import { api } from "../../api";

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue("/custom/storage/dir"),
}));

vi.mock("../../api", () => ({
  api: {
    getWorkspaceSettings: vi.fn().mockResolvedValue({
      noending_home: "/tmp/noending",
      default_workspace: "/tmp/noending/workspace",
      pending_home: null,
      restart_required: false,
      db_path: "/tmp/noending/data/noending.db",
      home_source: "default_home",
    }),
    setNoendingHome: vi.fn().mockResolvedValue({
      noending_home: "/tmp/noending",
      default_workspace: "/tmp/noending/workspace",
      pending_home: "/custom/storage/dir",
      restart_required: true,
      db_path: "/tmp/noending/data/noending.db",
      home_source: "bootstrap",
    }),
    openPath: vi.fn().mockResolvedValue(true),
    listSkills: vi.fn().mockResolvedValue({ resources: [], locations: [], warnings: [] }),
    listSkillCatalog: vi.fn().mockResolvedValue({ resources: [], locations: [], warnings: [] }),
    enableSkillAll: vi.fn(),
    disableSkillAll: vi.fn(),
    getExtractionSettings: vi.fn(),
    setExtractionSettings: vi.fn(),
    getAgentRuntimeSettings: vi.fn(),
    refreshAgentRuntimeOptions: vi.fn(),
  },
}));

afterEach(() => {
  cleanup();
  localStorage.clear();
  delete document.documentElement.dataset.theme;
});

it("applies and persists themes, then restores system appearance", async () => {
  render(<SettingsView section="appearance" navigate={vi.fn()} />);
  await screen.findByText("/tmp/noending/data/noending.db");
  fireEvent.click(screen.getByRole("button", { name: "深色" }));
  expect(document.documentElement.dataset.theme).toBe("dark");
  expect(screen.getByRole("button", { name: "深色" }).getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "跟随系统" }));
  expect(document.documentElement.dataset.theme).toBeUndefined();
});

it("opens modal to change data storage directory and can submit new path", async () => {
  render(<SettingsView navigate={vi.fn()} />);
  await screen.findByText("/tmp/noending/data/noending.db");

  fireEvent.click(screen.getByRole("button", { name: "更改位置…" }));
  expect(screen.getByText("更改数据存储目录")).toBeTruthy();
  expect(screen.getByRole("button", { name: "浏览…" })).toBeTruthy();

  fireEvent.change(screen.getByPlaceholderText("/tmp/noending"), {
    target: { value: "/custom/storage/dir" },
  });
  fireEvent.click(screen.getByRole("button", { name: "登记并在下次启动迁移" }));

  await screen.findByText("待重启生效");
  expect(api.setNoendingHome).toHaveBeenCalledTimes(1);
  expect(api.setNoendingHome).toHaveBeenCalledWith("/custom/storage/dir");
});

beforeEach(() => {
  viewState.clear(); localStorage.clear(); clearRuntimeCatalog();
  vi.mocked(api.getExtractionSettings).mockReset().mockResolvedValue({ agent: "codex", runtime: { model: null, provider: null, effort: null } });
  vi.mocked(api.setExtractionSettings).mockReset();
  vi.mocked(api.getAgentRuntimeSettings).mockReset().mockImplementation(async agent => runtimeFixture(agent));
  vi.mocked(api.refreshAgentRuntimeOptions).mockReset().mockRejectedValue(new Error("catalog unavailable"));
});

it("saves extraction settings immediately on selection and handles disable extraction", async () => {
  render(<SettingsView navigate={vi.fn()} />);
  const agent = await screen.findByLabelText("提取 Agent");

  const piExpected = { agent: "pi" as const, runtime: { model: null, provider: null, effort: null } };
  vi.mocked(api.setExtractionSettings).mockResolvedValueOnce(piExpected);
  fireEvent.change(agent, { target: { value: "pi" } });
  await waitFor(() => expect(api.setExtractionSettings).toHaveBeenCalledWith(piExpected));

  const runtimeBtn = screen.getByRole("button", { name: "模型与推理设置" });
  fireEvent.click(runtimeBtn);
  fireEvent.click(screen.getByText("推理等级"));
  const highEffortExpected = { agent: "pi" as const, runtime: { model: null, provider: null, effort: "high" } };
  vi.mocked(api.setExtractionSettings).mockResolvedValueOnce(highEffortExpected);
  fireEvent.click(screen.getByRole("button", { name: "high" }));
  await waitFor(() => expect(api.setExtractionSettings).toHaveBeenCalledWith(highEffortExpected));

  const noneExpected = { agent: "none" as const, runtime: { model: null, provider: null, effort: null } };
  vi.mocked(api.setExtractionSettings).mockResolvedValueOnce(noneExpected);
  fireEvent.change(screen.getByLabelText("提取 Agent"), { target: { value: "none" } });
  await waitFor(() => expect(api.setExtractionSettings).toHaveBeenCalledWith(noneExpected));
  expect(screen.queryByRole("button", { name: "模型与推理设置" })).toBeNull();
  expect(screen.getByText(/已停用后台提取/)).toBeTruthy();
});
