import { invoke } from "@tauri-apps/api/core";
import type {
  Agent, AgentRuntimeDiscovery, AgentRuntimeOverrides, AgentRuntimeSettings, AppInfo,
  ContextDeliveryLevel, ContextItem, ContextItemRevision, LaunchResult, Project, ProjectResource,
  ReviewFrontier, SearchHit, Session, SessionBindingRow, SessionContextBundle, SessionDetail, SessionWorkstreamBinding,
  SyncRun, Workstream, WorkstreamCardData, WorkstreamContext, WorkstreamReviewState, WorkstreamReviewWindow,
  WorkstreamReviewSummary,
} from "./types";

export const api = {
  listProjects: () => invoke<Project[]>("list_projects"),
  createProject: (name: string, description: string) =>
    invoke<Project>("create_project", { name, description }),
  deleteProject: (projectId: string) => invoke<void>("delete_project", { projectId }),
  addResource: (projectId: string, kind: string, uri: string) =>
    invoke<ProjectResource>("add_project_resource", { projectId, kind, uri: uri || null }),
  listResources: (projectId: string) =>
    invoke<ProjectResource[]>("list_project_resources", { projectId }),
  removeResource: (resourceId: string) => invoke<void>("remove_project_resource", { resourceId }),

  listWorkstreams: (projectId?: string) =>
    invoke<Workstream[]>("list_workstreams", { projectId: projectId ?? null }),
  listWorkstreamCards: () => invoke<WorkstreamCardData[]>("list_workstream_cards"),
  createWorkstream: (projectId: string | null, title: string, description: string, defaultCwd?: string) =>
    invoke<Workstream>("create_workstream", {
      projectId,
      title,
      description,
      defaultCwd: defaultCwd?.trim() ? defaultCwd : null,
    }),
  updateWorkstream: (w: Workstream) => invoke<void>("update_workstream", { workstream: w }),
  archiveWorkstream: (workstreamId: string) =>
    invoke<void>("archive_workstream", { workstreamId }),
  mergeWorkstreams: (sourceId: string, targetId: string) =>
    invoke<void>("merge_workstreams", { sourceId, targetId }),

  getWorkstreamContext: (workstreamId: string) =>
    invoke<WorkstreamContext>("get_workstream_context", { workstreamId }),
  addContextItem: (workstreamId: string, kind: string, title: string, content: string) =>
    invoke<ContextItem>("add_context_item", { args: { workstreamId, kind, title, content } }),
  editContextItem: (itemId: string, title: string, content: string) =>
    invoke<void>("edit_context_item", { args: { itemId, title, content } }),
  setItemStatus: (itemId: string, status: string) =>
    invoke<void>("set_item_status", { itemId, status }),
  getItemHistory: (itemId: string) =>
    invoke<ContextItemRevision[]>("get_item_history", { itemId }),
  deleteContextItem: (itemId: string) => invoke<void>("delete_context_item", { itemId }),
  getContextRevisionSource: (revisionId: string) =>
    invoke<import("./types").ContextSourceDetail | null>("get_context_revision_source", { revisionId }),
  getWorkstreamReviewState: (workstreamId: string) =>
    invoke<WorkstreamReviewState | null>("get_workstream_review_state", { workstreamId }),
  getWorkstreamReviewWindow: (workstreamId: string) =>
    invoke<WorkstreamReviewWindow>("get_workstream_review_window", { workstreamId }),
  markWorkstreamReviewed: (workstreamId: string, frontier: ReviewFrontier) =>
    invoke<WorkstreamReviewState>("mark_workstream_reviewed", { workstreamId, frontier }),
  getWorkstreamReviewSummary: (workstreamId: string) =>
    invoke<WorkstreamReviewSummary>("get_workstream_review_summary", { workstreamId }),
  listWorkstreamReviewSummaries: () =>
    invoke<WorkstreamReviewSummary[]>("list_workstream_review_summaries"),
  listConflicts: (workstreamId: string, includeClosed?: boolean) =>
    invoke<import("./types").ContextConflict[]>("list_conflicts", { workstreamId, includeClosed: includeClosed ?? false }),
  resolveConflict: (conflictId: string, status: string, resolution?: string) =>
    invoke<void>("resolve_conflict", { conflictId, status, resolution: resolution ?? null }),
  resolveConflictWithEdit: (
    conflictId: string,
    status: string,
    resolution?: string,
    edit?: { title: string; content: string },
  ) =>
    invoke<void>("resolve_conflict_with_edit", {
      args: {
        conflict_id: conflictId,
        status,
        resolution: resolution ?? null,
        edit: edit ?? null,
      },
    }),
  getConflictReviewCase: (conflictId: string) =>
    invoke<import("./types").ConflictReviewCase | null>("get_conflict_review_case", { conflictId }),
  listConflictReviewCases: (workstreamId: string, includeClosed?: boolean) =>
    invoke<import("./types").ConflictReviewCase[]>("list_conflict_review_cases", {
      workstreamId,
      includeClosed: includeClosed ?? false,
    }),

  listSessions: (projectId?: string, agent?: Agent) =>
    invoke<Session[]>("list_sessions", { projectId: projectId ?? null, agent: agent ?? null }),
  listAllSessions: () => invoke<Session[]>("list_sessions", { projectId: null, agent: null }),
  getSessionDetail: (sessionId: string) => invoke<SessionDetail>("get_session_detail", { sessionId }),
  assignSessionProject: (sessionId: string, projectId: string | null) =>
    invoke<void>("assign_session_project", { sessionId, projectId }),
  bindSessionWorkstream: (sessionId: string, workstreamId: string, role: string) =>
    invoke<void>("bind_session_workstream", { sessionId, workstreamId, role }),
  unbindSessionWorkstream: (sessionId: string, workstreamId: string) =>
    invoke<void>("unbind_session_workstream", { sessionId, workstreamId }),
  replaceSessionBindings: (sessionId: string, bindings: { workstream_id: string; role: string }[]) =>
    invoke<void>("replace_session_bindings", { sessionId, bindings }),
  listSessionBindings: () => invoke<SessionBindingRow[]>("list_session_bindings"),
  getAppInfo: () => invoke<AppInfo>("get_app_info"),

  syncAll: () => invoke<{ started: boolean }>("sync_all"),
  syncSource: (sourceId: string) => invoke<{ started: boolean }>("sync_source", { sourceId }),
  reingestSource: (sourceId: string) => invoke<{ started: boolean }>("reingest_source", { sourceId }),
  syncSession: (sessionId: string) => invoke<{ applied: number }>("sync_session", { sessionId }),
  listSyncRuns: (limit?: number) => invoke<SyncRun[]>("list_sync_runs", { limit: limit ?? 50 }),

  launchNewSession: (agent: Agent, workstreamIds: string[], cwd?: string) =>
    invoke<LaunchResult>("launch_new_session", { agent, workstreamIds, cwd: cwd ?? null }),
  launchResumeSession: (sessionId: string, extraWorkstreamIds: string[]) =>
    invoke<LaunchResult>("launch_resume_session", { sessionId, extraWorkstreamIds }),
  prepareNewSession: (agent: Agent, workstreamIds: string[], cwd?: string) =>
    invoke<import("./types").PreparedLaunch>("prepare_new_session", { agent, workstreamIds, cwd: cwd ?? null }),
  prepareResumeSession: (sessionId: string, extraWorkstreamIds: string[]) =>
    invoke<import("./types").PreparedLaunch>("prepare_resume_session", { sessionId, extraWorkstreamIds }),
  launchPrepared: (preparedId: string) =>
    invoke<LaunchResult>("launch_prepared", { preparedId }),
  cancelPrepared: (preparedId: string) =>
    invoke<void>("cancel_prepared", { preparedId }),

  listIngestSources: () =>
    invoke<import("./types").IngestSource[]>("list_ingest_sources"),
  addIngestSource: (agent: Agent, path: string) =>
    invoke<import("./types").IngestSource>("add_ingest_source", { agent, path }),
  setIngestSourceEnabled: (sourceId: string, enabled: boolean) =>
    invoke<void>("set_ingest_source_enabled", { sourceId, enabled }),
  removeIngestSource: (sourceId: string) =>
    invoke<void>("remove_ingest_source", { sourceId }),

  search: (query: string, limit?: number) => invoke<SearchHit[]>("search", { query, limit: limit ?? 30 }),
  getStats: () => invoke<Record<string, number>>("get_stats"),
  getAgentStatus: () =>
    invoke<Record<string, { name: string; detected: boolean; executable: string | null; version: string | null }>>("get_agent_status"),

  getDefaultAgent: () => invoke<Agent | null>("get_default_agent"),
  setDefaultAgent: (agent: Agent) => invoke<void>("set_default_agent", { agent }),

  getAgentRuntimeSettings: (agent: Agent) =>
    invoke<AgentRuntimeSettings>("get_agent_runtime_settings", { agent }),
  setAgentRuntimeOverrides: (agent: Agent, overrides: AgentRuntimeOverrides) =>
    invoke<AgentRuntimeSettings>("set_agent_runtime_overrides", { agent, overrides }),
  refreshAgentRuntimeOptions: (agent: Agent) =>
    invoke<AgentRuntimeDiscovery>("refresh_agent_runtime_options", { agent }),

  getContextDeliveryLevel: () => invoke<ContextDeliveryLevel>("get_context_delivery_level"),
  setContextDeliveryLevel: (level: ContextDeliveryLevel) =>
    invoke<void>("set_context_delivery_level", { level }),

  assistantSend: (sessionId: string | null, text: string) =>
    invoke<{ session_id: string; content: string; runtime: string }>("assistant_send", { sessionId, text }),
  assistantMessages: (sessionId: string) =>
    invoke<import("./types").AssistantMessage[]>("assistant_messages", { sessionId }),
  assistantConfigGet: () => invoke<{ agent: string }>("assistant_config_get"),
  assistantConfigSet: (agent: string) => invoke<void>("assistant_config_set", { agent }),
  assistantExecuteAction: (actionJson: string) =>
    invoke<{ ok: boolean; kind: string; launched_via: string; note: string }>("assistant_execute_action", { actionJson }),
};

export type { Agent, SessionWorkstreamBinding };
