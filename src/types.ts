// Mirror of the Rust domain API shapes (commands.rs).

export type Agent = "codex" | "claude_code" | "pi";

export interface Project {
  id: string;
  name: string;
  description: string;
  archived: boolean;
  created_at: string;
  updated_at: string;
}

export interface ProjectResource {
  id: string;
  project_id: string;
  kind: string;
  uri: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
}

export interface Workstream {
  id: string;
  project_id: string | null;
  title: string;
  description: string;
  lifecycle: "open" | "completed" | "abandoned";
  visibility: "normal" | "archived";
  created_at: string;
  updated_at: string;
}

export interface Session {
  id: string;
  agent: Agent;
  agent_session_id: string;
  title: string | null;
  cwd: string | null;
  project_id: string | null;
  raw_path: string;
  parent_agent_session_id: string | null;
  started_at: string | null;
  last_activity_at: string | null;
}

export interface SessionEvent {
  session_id: string;
  sequence: number;
  ts: string | null;
  kind: string;
  text: string | null;
  raw_ref: string;
  metadata: Record<string, unknown>;
}

export interface ContextItem {
  id: string;
  workstream_id: string;
  kind: string;
  status: string;
  authority: string;
  current_revision_id: string | null;
  supersedes_item_id: string | null;
  created_at: string;
  updated_at: string;
}

export interface ContextItemRevision {
  id: string;
  item_id: string;
  title: string;
  content: string;
  metadata: Record<string, unknown>;
  source_type: string | null;
  source_ref: string | null;
  sync_run_id: string | null;
  created_at: string;
}

export interface SessionWorkstreamBinding {
  session_id: string;
  workstream_id: string;
  role: string;
  last_seen_revision: string | null;
  last_sync_cursor: number;
  created_at: string;
  last_used_at: string;
}

export interface SyncRun {
  id: string;
  session_id: string;
  from_sequence: number;
  to_sequence: number;
  status: string;
  mutations: ContextMutation[];
  summary: string;
  error: string | null;
  created_at: string;
  runtime: string;
}

export type ContextMutation =
  | { op: "add"; workstream_id: string; item_kind: string; title: string; content: string; source_ref: string; authority: string }
  | { op: "update"; item_id: string; title: string; content: string; source_ref: string; authority: string }
  | { op: "supersede"; item_id: string; title: string; content: string; source_ref: string; authority: string }
  | { op: "resolve"; item_id: string; source_ref: string }
  | { op: "create_workstream"; project_id: string; title: string; reason: string }
  | { op: "conflict"; workstream_id: string; item_id: string; title: string; content: string; source_ref: string; reason: string };

export interface ContextSection {
  kind: string;
  title: string;
  content: string;
  authority: string;
  source_ref: string | null;
}

export interface WorkstreamContext {
  workstream: Workstream;
  core: ContextSection[];
  items: [ContextItem, ContextItemRevision][];
  related_sessions: Session[];
}

export interface SessionDetail {
  session: Session;
  events: SessionEvent[];
  bindings: [SessionWorkstreamBinding, string | null][];
  cursor: number;
}

export interface SearchHit {
  kind: string;
  ref_id: string;
  parent_id: string;
  title: string;
  snippet: string;
  rank: number;
}

export interface SessionContextBundle {
  mode: string;
  workstream_ids: string[];
  sections: ContextSection[];
  markdown: string;
  approx_tokens: number;
}

export interface LaunchResult {
  launched_via: string;
  command_line: string;
  context_file: string;
  bundle: SessionContextBundle;
  note: string;
}

export interface AssistantMessage {
  id: string;
  session_id: string;
  role: "user" | "assistant";
  content: string;
  action_json: string | null;
  runtime: string | null;
  created_at: string;
}

export const AGENT_LABELS: Record<Agent, string> = {
  codex: "Codex",
  claude_code: "Claude Code",
  pi: "Pi",
};

export const KIND_LABELS: Record<string, string> = {
  goal: "Goal",
  current_state: "Current State",
  constraint: "Constraint",
  decision: "Decision",
  open_question: "Open Question",
  todo: "Todo",
  finding: "Finding",
  issue: "Issue",
  risk: "Risk",
  note: "Note",
  reference: "Reference",
  artifact: "Artifact",
  requirement: "Requirement",
  decision_detail: "Decision Detail",
  research_note: "Research Note",
};

export const AUTHORITY_LABELS: Record<string, string> = {
  user_explicit: "用户明确",
  user_edit: "用户编辑",
  system_observed: "系统观察",
  agent_statement: "Agent 陈述",
  agent_inferred: "Agent 推断",
};
