import type { Agent } from "../../types";

// Configuration adapters implemented in src-tauri/src/extensions; history-only
// adapters do not automatically gain extension-management capabilities.
export const EXTENSION_AGENTS: readonly Agent[] = ["codex", "claude_code", "pi", "antigravity"];
