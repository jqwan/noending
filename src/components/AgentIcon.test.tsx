import { cleanup, render } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import AgentIcon from "./AgentIcon";
import type { Agent } from "../types";

afterEach(cleanup);

const ALL_AGENTS: Agent[] = [
  "antigravity",
  "claude_code",
  "codex",
  "dsh",
  "pi",
  "qoder",
  "workbuddy",
  "zcode",
];

it("renders vector SVGs for all agents", () => {
  for (const agent of ALL_AGENTS) {
    const { container } = render(
      <AgentIcon agent={agent} size={20} className="custom-class" />
    );
    const svg = container.querySelector("svg.agent-icon");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("width")).toBe("20");
    expect(svg?.getAttribute("height")).toBe("20");
    expect(svg?.getAttribute("aria-hidden")).toBe("true");
    expect(svg?.classList.contains("custom-class")).toBe(true);
    cleanup();
  }
});

it("returns null for unknown agent", () => {
  const { container } = render(
    // @ts-expect-error test unknown agent fallback
    <AgentIcon agent="nonexistent_agent" />
  );
  expect(container.querySelector(".agent-icon")).toBeNull();
});
