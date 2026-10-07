import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionSubpageTabs from "./SessionSubpageTabs";

// 终端段的出现与否是会话格式的静态能力（sessionFormats 能力表，与后端
// adapters 的路由事实同源），不再读 agent 状态——本组件无异步行为。

afterEach(() => {
  cleanup();
});

function renderTabs(entry: "conversation" | "terminal" | undefined, agent: "codex" | "qoder" | "antigravity", sourceKind: string | undefined, gate: string | null) {
  const navigate = vi.fn();
  const view = render(
    <SessionSubpageTabs
      sessionId="s1"
      entry={entry}
      agent={agent}
      sourceKind={sourceKind}
      terminalGate={gate}
      navigate={navigate}
    />,
  );
  return { navigate, view };
}

it("shows all three segments for a TUI format and navigates on click", () => {
  const { navigate } = renderTabs(undefined, "codex", "codex", null);

  expect(screen.getByTitle("终端")).toBeTruthy();
  expect(screen.getByTitle("概览")).toBeTruthy();
  expect(screen.getByTitle("对话")).toBeTruthy();

  fireEvent.click(screen.getByTitle("对话"));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s1", entry: "conversation" });
  fireEvent.click(screen.getByTitle("终端"));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s1", entry: "terminal" });
});

it("hides the terminal segment for a format without a TUI CLI", () => {
  renderTabs(undefined, "qoder", "qoder", null);

  expect(screen.getByTitle("对话")).toBeTruthy();
  expect(screen.queryByTitle("终端")).toBeNull();
});

it("hides the terminal segment for antigravity desktop sessions even though the agent has a CLI", () => {
  // 能力跟格式走，不跟 agent 走：antigravity 桌面存储没有 CLI。
  renderTabs(undefined, "antigravity", "antigravity_ide_conversation", null);

  expect(screen.queryByTitle("终端")).toBeNull();
});

it("shows the terminal segment for antigravity CLI sessions", () => {
  renderTabs(undefined, "antigravity", "antigravity_cli_conversation", null);

  expect(screen.getByTitle("终端")).toBeTruthy();
});

it("disables the terminal segment with the stated reason when resume is gated", () => {
  renderTabs("terminal", "codex", "codex", "源会话已不存在，无法继续");

  const terminal = screen.getByTitle("源会话已不存在，无法继续") as HTMLButtonElement;
  expect(terminal.disabled).toBe(true);
});

it("marks the active segment", () => {
  const { view } = renderTabs("conversation", "codex", "codex", null);
  const active = view.container.querySelector(".settings-seg button.on");
  expect(active?.getAttribute("title")).toBe("对话");
});
