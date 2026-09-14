import React from "react";
import type { Agent } from "../types";

/**
 * Small per-agent mark (14–16px) for New / Resume / Start buttons.
 * Visually one family (14px geometric marks, currentColor) while staying
 * distinguishable: Claude ◈ · Codex ◇ · Pi ◎. Text stays dominant next to it.
 */
export default function AgentIcon({ agent, size = 14 }: { agent: Agent; size?: number }) {
  return (
    <svg
      className="agent-icon"
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      aria-hidden="true"
    >
      {agent === "claude_code" && (
        <path d="M8 1.2 L14.8 8 L8 14.8 L1.2 8 Z" fill="currentColor" />
      )}
      {agent === "codex" && (
        <path
          d="M8 1.8 L14.2 8 L8 14.2 L1.8 8 Z"
          stroke="currentColor"
          strokeWidth="1.6"
          strokeLinejoin="round"
        />
      )}
      {agent === "pi" && (
        <>
          <circle cx="8" cy="8" r="6" stroke="currentColor" strokeWidth="1.6" />
          <circle cx="8" cy="8" r="1.8" fill="currentColor" />
        </>
      )}
    </svg>
  );
}
