import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
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
