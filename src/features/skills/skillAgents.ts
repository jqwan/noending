import type { Agent } from "../../types";

// Configuration adapters implemented in src-tauri/src/skills; history-only
// adapters do not automatically gain skill-management capabilities. ZCode,
// DSH and Qoder ride the shared ~/.agents/skills root; WorkBuddy has its own
// directories (it does not read the shared root).
export const SKILL_AGENTS: readonly Agent[] = [
  "codex", "claude_code", "pi", "antigravity", "zcode", "dsh", "qoder", "workbuddy",
];
