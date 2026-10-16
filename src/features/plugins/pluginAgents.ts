import type { Agent } from "../../types";

// Configuration adapters implemented in src-tauri/src/plugins; history-only
// adapters do not automatically gain plugin-management capabilities.
export const PLUGIN_AGENTS: readonly Agent[] = ["codex", "claude_code", "pi", "antigravity"];
