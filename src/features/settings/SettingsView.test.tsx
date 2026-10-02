import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SettingsView from "./SettingsView";

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
  },
}));

afterEach(() => {
  cleanup();
  localStorage.clear();
  delete document.documentElement.dataset.theme;
});

it("applies and persists themes, then restores system appearance", () => {
  render(<SettingsView section="appearance" navigate={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "深色" }));
  expect(document.documentElement.dataset.theme).toBe("dark");
  expect(localStorage.getItem("noending.theme")).toBe("dark");
  expect(screen.getByRole("button", { name: "深色" }).getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "跟随系统" }));
  expect(document.documentElement.dataset.theme).toBeUndefined();
  expect(localStorage.getItem("noending.theme")).toBeNull();
});

it("shows local data settings without pricing controls", async () => {
  render(<SettingsView section="advanced" navigate={vi.fn()} />);
  expect(await screen.findByText("/tmp/noending/data/noending.db")).toBeTruthy();
  expect(screen.queryByRole("heading", { name: "用量价格表" })).toBeNull();
  expect(screen.queryByRole("textbox", { name: "自定义价格 JSON" })).toBeNull();
  expect(screen.queryByRole("button", { name: "立即刷新" })).toBeNull();
});
