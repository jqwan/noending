import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import AppShell from "./AppShell";
import type { Route } from "./routes";

const focusListener = vi.hoisted(() => ({
  callback: undefined as undefined | ((event: { payload: boolean }) => void),
}));
const appForeground = vi.hoisted(() => vi.fn().mockResolvedValue({ queued: true }));

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onFocusChanged: (callback: (event: { payload: boolean }) => void) => {
      focusListener.callback = callback;
      return Promise.resolve(() => {});
    },
  }),
}));
vi.mock("../api", () => ({ api: { appForeground } }));
vi.mock("../layout/Sidebar", () => ({ default: ({ hidden }: { hidden: boolean }) => <aside aria-label="会话侧边栏" hidden={hidden} /> }));
vi.mock("./Router", () => ({ default: ({ route }: { route: Route }) => <output aria-label="当前页面">{route.view}</output> }));
vi.mock("../components/Toast", () => ({ default: () => null }));
vi.mock("../components/CommandPalette", () => ({ default: () => null }));
vi.mock("../features/launcher/LaunchResultModal", () => ({ LaunchDetailsHost: () => null }));

beforeEach(() => localStorage.clear());
afterEach(cleanup);

it("starts on the new-session page", () => {
  render(<AppShell />);
  expect(screen.getByLabelText("当前页面").textContent).toBe("new-session");
});

it("keeps icon navigation fixed and shows the session sidebar only in the session section", () => {
  render(<AppShell />);
  const rail = screen.getByRole("navigation", { name: "工作区导航" });
  expect(screen.getByRole("complementary", { name: "会话侧边栏" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "项目" }));
  expect(screen.getByLabelText("当前页面").textContent).toBe("projects");
  expect(screen.queryByRole("complementary", { name: "会话侧边栏" })).toBeNull();
  expect(screen.queryByRole("separator")).toBeNull();
  expect(screen.queryByRole("button", { name: "收起侧边栏" })).toBeNull();
  expect(screen.getByRole("navigation", { name: "工作区导航" })).toBe(rail);
  fireEvent.click(screen.getByRole("button", { name: "会话" }));
  expect(screen.getByLabelText("当前页面").textContent).toBe("sessions");
  expect(screen.getByRole("complementary", { name: "会话侧边栏" })).toBeTruthy();
  expect(screen.getByRole("separator")).toBeTruthy();
});

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

it("queues ingestion only after the window returns from the background", async () => {
  render(<AppShell />);
  const changed = focusListener.callback!;
  changed({ payload: true });
  expect(appForeground).not.toHaveBeenCalled();
  changed({ payload: false });
  changed({ payload: true });
  changed({ payload: true });

  await waitFor(() => expect(appForeground).toHaveBeenCalledTimes(1));
});

it("normalizes legacy settings sections to agents view", async () => {
  const { normalizeRoute } = await import("./AppShell");
  expect(normalizeRoute({ view: "settings", section: "agents" })).toEqual({ view: "agents" });
  expect(normalizeRoute({ view: "settings", section: "sources" })).toEqual({ view: "agents" });
  expect(normalizeRoute({ view: "settings", section: "general" })).toEqual({ view: "settings", section: "general" });
  expect(normalizeRoute({ view: "workstreams" })).toEqual({ view: "workstreams" });
  expect(normalizeRoute({ view: "sessions", action: "new" })).toEqual({ view: "new-session" });
});
