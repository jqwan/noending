import { readViewPreference, writeViewPreference } from "../../hooks/useViewState";
import { AGENT_LABELS, type Agent, type AgentRuntimeOverrides } from "../../types";
import { NATIVE_RUNTIME } from "../agents/RuntimeFields";

interface NewSessionPreferences {
  projectId: string;
  agent: Agent | null;
  workstreamIds: string[];
  directories: Record<string, string>;
  drafts: Record<string, string>;
}

/** Older rows predate `workstreamIds`; keep their drafts and choices readable. */
type StoredPreferences = Omit<NewSessionPreferences, "workstreamIds"> & { workstreamIds?: string[] };

const KEY = "new-session.preferences";
const DEFAULTS: StoredPreferences = { projectId: "", agent: null, workstreamIds: [], directories: {}, drafts: {} };

function isStringRecord(value: unknown): value is Record<string, string> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    && Object.values(value).every(item => typeof item === "string");
}

function isStringList(value: unknown): value is string[] {
  return Array.isArray(value) && value.every(item => typeof item === "string");
}

function isPreferences(value: unknown): value is StoredPreferences {
  if (typeof value !== "object" || value === null) return false;
  const prefs = value as StoredPreferences;
  return typeof prefs.projectId === "string"
    && (prefs.agent === null || Object.keys(AGENT_LABELS).includes(prefs.agent))
    && (prefs.workstreamIds === undefined || isStringList(prefs.workstreamIds))
    && isStringRecord(prefs.directories) && isStringRecord(prefs.drafts);
}

export function readNewSessionPreferences(): NewSessionPreferences {
  const preferences = readViewPreference(KEY, DEFAULTS, isPreferences);
  return { ...preferences, workstreamIds: preferences.workstreamIds ?? [] };
}

/** Write immediately, including when an async launch succeeds after this page unmounts. */
export function updateNewSessionPreferences(update: (current: NewSessionPreferences) => NewSessionPreferences) {
  writeViewPreference(KEY, update(readNewSessionPreferences()));
}

function isRuntime(value: unknown): value is AgentRuntimeOverrides {
  if (!value || typeof value !== "object") return false;
  const runtime = value as AgentRuntimeOverrides;
  return [runtime.model, runtime.provider, runtime.effort].every(field => field === null || typeof field === "string");
}

export function lastNewSessionRuntime(agent: Agent) {
  return readViewPreference(`new-session.runtime.${agent}`, NATIVE_RUNTIME, isRuntime);
}

export function rememberNewSessionRuntime(agent: Agent, runtime: AgentRuntimeOverrides) {
  writeViewPreference(`new-session.runtime.${agent}`, runtime);
}
