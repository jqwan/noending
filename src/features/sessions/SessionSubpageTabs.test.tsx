import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SessionSubpageTabs from "./SessionSubpageTabs";
import { api } from "../../api";
import type { AgentStatusEntry } from "../../types";

vi.mock("../../api", () => ({
  api: { getAgentStatus: vi.fn() },
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function statusOf(over: Partial<AgentStatusEntry>): Record<string, AgentStatusEntry> {
  return {
    codex: {
      name: "Codex",
      detected: true,
      executable: "/usr/local/bin/codex",
      version: null,
      terminal_cli: true,
      desktop_app: null,
      desktop_app_present: false,
      resume_open_method: "terminal",
      ...over,
    },
  };
}

function renderTabs(entry: "conversation" | "terminal" | undefined, agent: "codex" | "qoder", gate: string | null) {
  const navigate = vi.fn();
  const view = render(
    <SessionSubpageTabs
      sessionId="s1"
      entry={entry}
      agent={agent}
      terminalGate={gate}
      navigate={navigate}
    />,
  );
  return { navigate, view };
}

it("shows all three segments for a TUI agent and navigates on click", async () => {
  vi.mocked(api.getAgentStatus).mockResolvedValue(statusOf({}));
  const { navigate } = renderTabs(undefined, "codex", null);

  await screen.findByTitle("终端");
  expect(screen.getByTitle("概览")).toBeTruthy();
  expect(screen.getByTitle("对话")).toBeTruthy();

  fireEvent.click(screen.getByTitle("对话"));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s1", entry: "conversation" });
  fireEvent.click(screen.getByTitle("终端"));
  expect(navigate).toHaveBeenCalledWith({ view: "session", sessionId: "s1", entry: "terminal" });
});

it("hides the terminal segment for an agent without a TUI CLI", async () => {
  vi.mocked(api.getAgentStatus).mockResolvedValue(statusOf({ terminal_cli: false }));
  renderTabs(undefined, "qoder", null);

  await screen.findByTitle("对话");
  expect(screen.queryByTitle("终端")).toBeNull();
});

it("renders only the basic two segments while the capability fact has not loaded", () => {
  vi.mocked(api.getAgentStatus).mockReturnValue(new Promise(() => {}) as never);
  renderTabs(undefined, "codex", null);
  expect(screen.getByTitle("概览")).toBeTruthy();
  expect(screen.queryByTitle("终端")).toBeNull();
});

it("disables the terminal segment with the stated reason when resume is gated", async () => {
  vi.mocked(api.getAgentStatus).mockResolvedValue(statusOf({}));
  renderTabs("terminal", "codex", "源会话已不存在，无法继续");

  const terminal = (await screen.findByTitle("源会话已不存在，无法继续")) as HTMLButtonElement;
  expect(terminal.disabled).toBe(true);
});

it("marks the active segment", async () => {
  vi.mocked(api.getAgentStatus).mockResolvedValue(statusOf({}));
  const { view } = renderTabs("conversation", "codex", null);
  await screen.findByTitle("终端");
  const active = view.container.querySelector(".settings-seg button.on");
  expect(active?.getAttribute("title")).toBe("对话");
});
