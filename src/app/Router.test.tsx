import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import Router from "./Router";
import type { Agent } from "../types";

const { detail, conversation, terminal } = vi.hoisted(() => ({
  detail: vi.fn(() => <div>detail</div>),
  conversation: vi.fn(() => <div>conversation</div>),
  terminal: vi.fn(() => <div>terminal</div>),
}));
vi.mock("../features/sessions/SessionDetailView", () => ({ default: detail }));
vi.mock("../features/sessions/SessionConversationView", () => ({ default: conversation }));
vi.mock("../features/sessions/SessionTerminalView", () => ({ default: terminal }));
vi.mock("../features/sessions/NewSessionView", () => ({
  default: ({ workstreamId, agent, projectId }: { workstreamId?: string; agent?: Agent; projectId?: string }) => (
    <div>
      <output aria-label="新会话预置">{`${workstreamId ?? ""}:${agent ?? ""}:${projectId ?? ""}`}</output>
      <input aria-label="首条消息" defaultValue="" />
    </div>
  ),
}));

beforeEach(() => vi.clearAllMocks());
afterEach(cleanup);

it("routes new-session presets and resets the draft when any preset changes", () => {
  const navigate = vi.fn();
  const goBack = vi.fn();
  const { rerender } = render(<Router route={{ view: "new-session", workstreamId: "w1", agent: "codex" }} navigate={navigate} goBack={goBack} actionSeq={0} />);
  expect(screen.getByLabelText("新会话预置").textContent).toBe("w1:codex:");
  for (const preset of [
    { workstreamId: "w2", agent: "codex" as const },
    { workstreamId: "w2", agent: "claude_code" as const },
    { workstreamId: "w2", agent: "claude_code" as const, projectId: "p1" },
  ]) {
    fireEvent.change(screen.getByLabelText("首条消息"), { target: { value: "之前的草稿" } });
    rerender(<Router route={{ view: "new-session", ...preset }} navigate={navigate} goBack={goBack} actionSeq={0} />);
    expect(screen.getByLabelText("新会话预置").textContent).toBe(`${preset.workstreamId}:${preset.agent}:${"projectId" in preset ? preset.projectId : ""}`);
    expect((screen.getByLabelText("首条消息") as HTMLInputElement).value).toBe("");
  }
});

it("dispatches session overview, conversation and terminal routes with their identities", () => {
  const navigate = vi.fn();
  const goBack = vi.fn();
  const props = { navigate, goBack, actionSeq: 0 };
  const sessionRoute = { view: "session" as const, sessionId: "s1", initialTitle: "T", initialAgent: "codex" as const };
  const { rerender } = render(<Router route={sessionRoute} {...props} />);
  expect(detail).toHaveBeenLastCalledWith(expect.objectContaining({ sessionId: "s1", initialTitle: "T", initialAgent: "codex", navigate, goBack }), expect.anything());
  expect(conversation).not.toHaveBeenCalled();
  rerender(<Router route={{ ...sessionRoute, entry: "conversation", initialTotal: 12 }} {...props} />);
  expect(conversation).toHaveBeenLastCalledWith(expect.objectContaining({ sessionId: "s1", initialTitle: "T", initialAgent: "codex", initialTotal: 12, navigate }), expect.anything());
  rerender(<Router route={{ view: "terminal", terminalId: "t1" }} {...props} />);
  expect(terminal).toHaveBeenLastCalledWith(expect.objectContaining({ terminalId: "t1", navigate }), expect.anything());
});
