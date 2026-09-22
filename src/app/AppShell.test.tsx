import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import AppShell from "./AppShell";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("./experience", () => ({ refreshBaseExperience: vi.fn() }));
vi.mock("../layout/Sidebar", () => ({ default: () => null }));
vi.mock("./Router", () => ({ default: () => null }));
vi.mock("../components/Toast", () => ({ default: () => null }));
vi.mock("../components/CommandPalette", () => ({ default: () => null }));
vi.mock("../features/launcher/LaunchResultModal", () => ({ LaunchDetailsHost: () => null }));

beforeEach(() => localStorage.clear());
afterEach(cleanup);

it("remembers keyboard resizing, clamps width, and resets on double click", () => {
  const view = render(<AppShell />);
  const divider = screen.getByRole("separator");
  fireEvent.keyDown(divider, { key: "ArrowRight" });
  expect(localStorage.getItem("noending.sidebarWidth")).toBe("256");
  view.unmount();
  render(<AppShell />);
  const restored = screen.getByRole("separator");
  expect(restored.getAttribute("aria-valuenow")).toBe("256");
  fireEvent.keyDown(restored, { key: "End" });
  fireEvent.keyDown(restored, { key: "ArrowRight" });
  expect(restored.getAttribute("aria-valuenow")).toBe("360");
  fireEvent.keyDown(restored, { key: "Home" });
  fireEvent.keyDown(restored, { key: "ArrowLeft" });
  expect(restored.getAttribute("aria-valuenow")).toBe("200");
  fireEvent.doubleClick(restored);
  expect(localStorage.getItem("noending.sidebarWidth")).toBe("248");
});

it("hides the resize handle while the sidebar is collapsed", () => {
  render(<AppShell />);
  fireEvent.click(screen.getByRole("button", { name: "收起侧边栏" }));
  expect(screen.queryByRole("separator")).toBeNull();
  expect(localStorage.getItem("noending.sidebarCollapsed")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "展开侧边栏" }));
  expect(screen.getByRole("separator")).toBeTruthy();
});
