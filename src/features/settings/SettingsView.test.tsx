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

it("shows the configured local database path", async () => {
  render(<SettingsView section="advanced" navigate={vi.fn()} />);
  expect(await screen.findByText("/tmp/noending/data/noending.db")).toBeTruthy();
});

it("renders all settings in a single unified view", async () => {
  render(<SettingsView navigate={vi.fn()} />);
  expect(screen.getByText("主题")).toBeTruthy();
  expect(screen.getByText("Context 更新诊断")).toBeTruthy();
  expect(await screen.findByText("/tmp/noending/data/noending.db")).toBeTruthy();
});
