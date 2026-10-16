import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { clearRuntimeCatalog } from "../agents/useAgentRuntime";
import { runtimeFixture } from "../agents/runtimeFixtures";
import { viewState } from "../../hooks/useViewState";
import SettingsView from "./SettingsView";
import { api } from "../../api";
import type { Preference } from "../../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue("/custom/storage/dir"),
}));

vi.mock("../../api", () => ({
  api: {
    listPreferences: vi.fn().mockResolvedValue([]),
    savePreference: vi.fn(),
    deletePreference: vi.fn(),
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
    getExtractionSettings: vi.fn(),
    setExtractionSettings: vi.fn(),
    getAgentRuntimeSettings: vi.fn(),
    refreshAgentRuntimeOptions: vi.fn(),
    getSummaryPromptSettings: vi.fn(),
    setSummaryPromptTemplate: vi.fn(),
    previewSummaryPrompt: vi.fn(),
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
  expect(localStorage.getItem("noending.theme")).toBe("dark");
  expect(screen.getByRole("button", { name: "深色" }).getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "跟随系统" }));
  expect(document.documentElement.dataset.theme).toBeUndefined();
  expect(localStorage.getItem("noending.theme")).toBeNull();
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

const templates = {session: "默认会话要求", task: "默认任务要求", project: "默认项目要求", preferences: "默认偏好要求"};
beforeEach(() => {
  viewState.clear(); localStorage.clear(); clearRuntimeCatalog();
  vi.mocked(api.getExtractionSettings).mockReset().mockResolvedValue({ agent: "codex", runtime: { model: null, provider: null, effort: null } });
  vi.mocked(api.setExtractionSettings).mockReset();
  vi.mocked(api.getAgentRuntimeSettings).mockReset().mockImplementation(async agent => runtimeFixture(agent));
  vi.mocked(api.refreshAgentRuntimeOptions).mockReset().mockRejectedValue(new Error("catalog unavailable"));
  vi.mocked(api.listPreferences).mockReset().mockResolvedValue([]);
  vi.mocked(api.savePreference).mockReset();
  vi.mocked(api.deletePreference).mockReset();
  vi.mocked(api.getSummaryPromptSettings).mockReset().mockResolvedValue({templates: {...templates}, defaults: {...templates}});
  vi.mocked(api.setSummaryPromptTemplate).mockReset();
  vi.mocked(api.previewSummaryPrompt).mockReset().mockResolvedValue("完整提示词预览与文件协议");
});

it("keeps drafts across summary types, previews the draft and saves only the selected template", async () => {
  render(<SettingsView navigate={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", {name: "查看与编辑"}));
  const session = await screen.findByRole("textbox", {name: "会话摘要要求模板"});
  fireEvent.change(session, {target: {value: "会话草稿"}});
  fireEvent.click(screen.getByRole("tab", {name: "任务摘要"}));
  fireEvent.change(screen.getByRole("textbox", {name: "任务摘要要求模板"}), {target: {value: "任务草稿"}});
  fireEvent.click(screen.getByRole("button", {name: "预览完整提示词"}));
  await screen.findByText("完整提示词预览与文件协议");
  expect(api.previewSummaryPrompt).toHaveBeenCalledWith("task", {...templates, task: "任务草稿"});
  expect(api.setSummaryPromptTemplate).not.toHaveBeenCalled();
  vi.mocked(api.setSummaryPromptTemplate).mockResolvedValue({templates: {...templates, task: "任务草稿"}, defaults: {...templates}});
  fireEvent.click(screen.getByRole("button", {name: "保存模板"}));
  await waitFor(() => expect(screen.getByRole("button", {name: "保存模板"})).toHaveProperty("disabled", true));
  expect(api.setSummaryPromptTemplate).toHaveBeenCalledWith("task", "任务草稿");
  fireEvent.click(screen.getByRole("tab", {name: "会话摘要"}));
  fireEvent.click(screen.getByRole("button", {name: "编辑模板"}));
  expect(screen.getByRole("textbox", {name: "会话摘要要求模板"})).toHaveProperty("value", "会话草稿");
  fireEvent.click(screen.getByRole("button", {name: "撤销修改"}));
  expect(screen.getByRole("textbox", {name: "会话摘要要求模板"})).toHaveProperty("value", templates.session);
  fireEvent.click(screen.getByRole("tab", {name: "偏好提取"}));
  fireEvent.change(screen.getByRole("textbox", {name: "偏好提取要求模板"}), {target: {value: "只提取明确的长期约定"}});
  vi.mocked(api.setSummaryPromptTemplate).mockResolvedValue({templates: {...templates, preferences: "只提取明确的长期约定"}, defaults: {...templates}});
  fireEvent.click(screen.getByRole("button", {name: "保存模板"}));
  await waitFor(() => expect(screen.getByRole("button", {name: "保存模板"})).toHaveProperty("disabled", true));
  expect(api.setSummaryPromptTemplate).toHaveBeenCalledWith("preferences", "只提取明确的长期约定");
});

it("restores defaults explicitly, retains edits after save failure and rejects an empty draft", async () => {
  vi.mocked(api.getSummaryPromptSettings).mockResolvedValue({templates: {...templates, session: "自定义模板"}, defaults: {...templates}});
  render(<SettingsView navigate={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", {name: "查看与编辑"}));
  const editor = await screen.findByRole("textbox", {name: "会话摘要要求模板"});
  fireEvent.click(screen.getByRole("button", {name: "恢复默认"}));
  expect(editor).toHaveProperty("value", templates.session);
  expect(api.setSummaryPromptTemplate).not.toHaveBeenCalled();
  vi.mocked(api.setSummaryPromptTemplate).mockRejectedValueOnce("无法保存");
  fireEvent.click(screen.getByRole("button", {name: "保存模板"}));
  await screen.findByRole("alert");
  expect(editor).toHaveProperty("value", templates.session);
  vi.mocked(api.setSummaryPromptTemplate).mockResolvedValue({templates: {...templates}, defaults: {...templates}});
  fireEvent.click(screen.getByRole("button", {name: "保存模板"}));
  await waitFor(() => expect(screen.getByRole("button", {name: "保存模板"})).toHaveProperty("disabled", true));
  expect(api.setSummaryPromptTemplate).toHaveBeenLastCalledWith("session", templates.session);
  fireEvent.change(editor, {target: {value: "  "}});
  expect(screen.getByRole("button", {name: "保存模板"})).toHaveProperty("disabled", true);
});

it("lets users edit and delete locked preferences and create new locked items", async () => {
  let items: Preference[] = [{ id: "locked", project_id: null, content: "原有锁定偏好", locked: true }];
  vi.mocked(api.listPreferences).mockImplementation(async () => [...items]);
  vi.mocked(api.savePreference).mockImplementation(async (id, projectId, content, locked) => {
    const item = { id: id ?? "new", project_id: projectId, content, locked };
    items = [...items.filter(p => p.id !== item.id), item];
    return item;
  });
  vi.mocked(api.deletePreference).mockImplementation(async id => { items = items.filter(p => p.id !== id); });
  render(<SettingsView navigate={vi.fn()} />);
  await screen.findByText("原有锁定偏好");
  fireEvent.click(screen.getByRole("button", {name: "编辑偏好：原有锁定偏好"}));
  fireEvent.change(screen.getByRole("textbox", {name: "偏好"}), {target: {value: "用户修改的锁定偏好"}});
  fireEvent.click(screen.getByRole("button", {name: "保存"}));
  await screen.findByText("用户修改的锁定偏好");
  expect(api.savePreference).toHaveBeenCalledWith("locked", null, "用户修改的锁定偏好", true);
  fireEvent.click(screen.getByRole("button", {name: "删除偏好：用户修改的锁定偏好"}));
  await waitFor(() => expect(screen.queryByText("用户修改的锁定偏好")).toBeNull());
  fireEvent.click(screen.getByRole("button", {name: "新增"}));
  const save = screen.getByRole("button", {name: "保存"});
  expect(save).toHaveProperty("disabled", true);
  fireEvent.change(screen.getByRole("textbox", {name: "偏好"}), {target: {value: "新偏好"}});
  fireEvent.click(screen.getByRole("checkbox", {name: "锁定，禁止 Agent 修改或删除"}));
  fireEvent.click(save);
  await screen.findByText("新偏好");
  expect(api.savePreference).toHaveBeenLastCalledWith(null, null, "新偏好", true);
  fireEvent.click(screen.getByRole("button", {name: "解锁"}));
  await screen.findByRole("button", {name: "锁定"});
  expect(api.savePreference).toHaveBeenLastCalledWith("new", null, "新偏好", false);
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
  expect(screen.getByText(/已停用自动提取/)).toBeTruthy();
});
