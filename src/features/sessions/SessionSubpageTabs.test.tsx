import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionSubpageTabs from "./SessionSubpageTabs";

// 概览 / 对话两段子页切换。终端不是子页——它是一等独立视图，入口在
// SessionHeaderActions 的终端按钮（先跳后启）。

afterEach(() => {
  cleanup();
});

function renderTabs(entry: "conversation" | undefined) {
  const navigate = vi.fn();
  const view = render(
    <SessionSubpageTabs sessionId="s1" entry={entry} navigate={navigate} />,
  );
  return { navigate, view };
}

it("navigates between overview and conversation and updates the active segment", () => {
  const { navigate, view } = renderTabs(undefined);
  expect(screen.getByRole("button", { name: "概览" }).getAttribute("aria-pressed")).toBe("true");

  fireEvent.click(screen.getByTitle("对话"));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s1", entry: "conversation" });
  view.rerender(<SessionSubpageTabs sessionId="s1" entry="conversation" navigate={navigate} />);
  expect(screen.getByRole("button", { name: "对话" }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByRole("button", { name: "概览" }).getAttribute("aria-pressed")).toBe("false");
  fireEvent.click(screen.getByTitle("概览"));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s1", entry: undefined });
});
