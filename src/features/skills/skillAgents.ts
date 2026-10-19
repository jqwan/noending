import type { Agent } from "../../types";

// Configuration adapters implemented in src-tauri/src/skills; history-only
// adapters do not automatically gain skill-management capabilities.
export const SKILL_AGENTS: readonly Agent[] = ["codex", "claude_code", "pi", "antigravity"];
