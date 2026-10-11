import type { Agent, AgentRuntimeSettings } from "../../types";
/** Deterministic mock of the backend settings contract, used by UI regressions. */
export function runtimeFixture(agent: Agent = "codex"): AgentRuntimeSettings {
  return {
    agent, detected: true, executable: `/bin/${agent}`, version: "test",
    overrides: { model: "saved-model", provider: null, effort: "high" },
    capabilities: { model: "discoverable", provider: agent === "pi" ? "discoverable" : "unsupported", effort: "suggested" },
    models: [], model_source: "not_loaded", effort_levels: ["low", "high"], warnings: [],
  };
}
