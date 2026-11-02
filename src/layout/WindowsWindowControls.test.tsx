import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import WindowsWindowControls from "./WindowsWindowControls";

const native = vi.hoisted(() => ({
  isMaximized: vi.fn(), isFullscreen: vi.fn(),
  minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn(),
  onResized: vi.fn(), onFocusChanged: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => native }));

let resize: () => void;
beforeEach(() => {
  vi.clearAllMocks();
  native.isMaximized.mockResolvedValue(false);
  native.isFullscreen.mockResolvedValue(false);
  for (const method of [native.minimize, native.toggleMaximize, native.close]) method.mockResolvedValue(undefined);
  native.onResized.mockImplementation((callback: () => void) => { resize = callback; return Promise.resolve(() => {}); });
  native.onFocusChanged.mockResolvedValue(() => {});
});
afterEach(cleanup);

it("runs native window actions and reflects maximization performed outside the app", async () => {
  render(<WindowsWindowControls />);
  await act(async () => {});
  fireEvent.click(screen.getByRole("button", { name: "最小化窗口" }));
  expect(native.minimize).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole("button", { name: "最大化窗口" }));
  expect(native.toggleMaximize).toHaveBeenCalledTimes(1);

  // A double-click, system menu or snap can maximize without clicking our button.
  native.isMaximized.mockResolvedValue(true);
  await act(async () => { resize(); });
  fireEvent.click(screen.getByRole("button", { name: "还原窗口" }));
  expect(native.toggleMaximize).toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByRole("button", { name: "关闭窗口" }));
  expect(native.close).toHaveBeenCalledTimes(1);
});

it("hides controls in native fullscreen and restores them after leaving it", async () => {
  render(<WindowsWindowControls />);
  await waitFor(() => expect(screen.getByRole("group", { name: "窗口控制" })).toBeTruthy());
  native.isFullscreen.mockResolvedValue(true);
  await act(async () => { resize(); });
  expect(screen.queryByRole("button", { name: "关闭窗口" })).toBeNull();
  native.isFullscreen.mockResolvedValue(false);
  await act(async () => { resize(); });
  expect(screen.getByRole("button", { name: "关闭窗口" })).toBeTruthy();
});
