import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import SettingsView from "./SettingsView";

vi.mock("../../api", () => ({
  api: {
    openContextExtractionLogs: vi.fn().mockResolvedValue(undefined),
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

it("opens the Context extraction log directory from Advanced settings", async () => {
  render(<SettingsView section="advanced" navigate={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "打开日志目录" }));
  await waitFor(() => expect(api.openContextExtractionLogs).toHaveBeenCalledOnce());
});
