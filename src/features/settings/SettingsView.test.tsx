import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
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

const templates = {session: "默认会话要求", task: "默认任务要求", project: "默认项目要求"};
beforeEach(() => {
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
