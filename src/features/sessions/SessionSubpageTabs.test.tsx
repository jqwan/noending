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

it("shows both segments and navigates on click", () => {
  const { navigate } = renderTabs(undefined);

  expect(screen.getByTitle("概览")).toBeTruthy();
  expect(screen.getByTitle("对话")).toBeTruthy();
  expect(screen.queryByTitle("终端")).toBeNull();

  fireEvent.click(screen.getByTitle("对话"));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s1", entry: "conversation" });
  fireEvent.click(screen.getByTitle("概览"));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s1", entry: undefined });
});

it("marks the active segment", () => {
  const { view } = renderTabs("conversation");
  const active = view.container.querySelector(".settings-seg button.on");
  expect(active?.getAttribute("title")).toBe("对话");
});
