import { invoke } from "@tauri-apps/api/core";
import type {
  Agent, AgentRuntimeDiscovery, AgentRuntimeOverrides, AgentRuntimeSettings,
  ContextDeliveryLevel, ContextItem, ContextItemRevision, LaunchResult,
  PermanentDeletionPreview, PermanentDeletionResult, Project, ProjectCardData,
  ProjectDetailData, ProjectWorkstreamRow, SessionDeletionJob, WorkspaceSettings,
  WorkstreamPath, WorkstreamPathRow,
  ReviewFrontier, SearchHit, Session, SessionBindingRow, SessionContextBundle, SessionDetail, SessionWorkstreamBinding,
  SyncRun, Workstream, WorkstreamCardData, WorkstreamContext, WorkstreamReviewState, WorkstreamReviewWindow,
  WorkstreamReviewSummary,
} from "./types";

export const api = {
  // ---------------- Projects (方案 §11) ----------------
  //
  // v0.2 derives a Project from its WorkspacePaths. The read + rename surface is
  // the whole API: `create_project` / `update_project` / `delete_project` and the
  // resource commands are no longer registered in `lib.rs`, so their wrappers
  // below are dead by construction and only await removal with their last UI
  // call site. Calling one rejects at runtime — that is the point.
  listProjects: () => invoke<Project[]>("list_projects"),
  /** §8 — 一次拿完整 Board 数据，替代 1 + N 的 getProjectDetail。 */
  listProjectCards: () => invoke<ProjectCardData[]>("list_project_cards"),
  /** §10 — 全局「刷新工作区状态」：后台 reconcile，事件回报，立即返回。 */
  refreshWorkspaceProjects: () =>
    invoke<{ started: boolean }>("refresh_workspace_projects"),
  /** §16 — 定点刷新：只重观察这个 Project 自己的工作目录。 */
  refreshProjectWorkspace: (projectId: string) =>
    invoke<{ started: boolean }>("refresh_project_workspace", { projectId }),
  getProjectDetail: (projectId: string) =>
    invoke<ProjectDetailData>("get_project_detail", { projectId }),
  listProjectWorkstreams: (projectId: string) =>
    invoke<ProjectWorkstreamRow[]>("list_project_workstreams", { projectId }),
  renameProject: (projectId: string, name: string) =>
    invoke<Project>("rename_project", { projectId, name }),

  // ---------------- NoEnding Home (方案 §11, §22) ----------------
  getWorkspaceSettings: () => invoke<WorkspaceSettings>("get_workspace_settings"),
  /** Requests a relocation for the NEXT launch; `restart_required` says so. */
  setNoendingHome: (newHome: string) =>
    invoke<WorkspaceSettings>("set_noending_home", { newHome }),

  listWorkstreams: (projectId?: string) =>
    invoke<Workstream[]>("list_workstreams", { projectId: projectId ?? null }),
  listWorkstreamCards: () => invoke<WorkstreamCardData[]>("list_workstream_cards"),
  /**
   * `create_workstream(title, description, initialPath?)`.
   *
   * There is no Project argument to carry: v0.2 has no manual
   * Workstream→Project assignment. The path the user types is not a hint
   * either — it becomes the Workstream's position-0 path, or nothing at all
   * when the workspace layer cannot resolve it.
   */
  createWorkstream: (title: string, description: string, initialPath?: string) =>
    invoke<Workstream>("create_workstream", {
      title,
      description,
      initialPath: initialPath?.trim() || null,
    }),
  updateWorkstream: (w: Workstream) => invoke<void>("update_workstream", { workstream: w }),

  // ---------------- Workstream paths, lifecycle, recycle bin (方案 §11, §18) ----------------
  listWorkstreamPaths: (workstreamId: string) =>
    invoke<WorkstreamPathRow[]>("list_workstream_paths", { workstreamId }),
  addWorkstreamPath: (workstreamId: string, path: string) =>
    invoke<WorkstreamPath>("add_workstream_path", { workstreamId, path }),
  removeWorkstreamPath: (workstreamId: string, workstreamPathId: string) =>
    invoke<number>("remove_workstream_path", { workstreamId, workstreamPathId }),
  /** `orderedWorkspacePathIds` must be the complete current list. */
  reorderWorkstreamPaths: (workstreamId: string, orderedWorkspacePathIds: string[]) =>
    invoke<WorkstreamPath[]>("reorder_workstream_paths", { workstreamId, orderedWorkspacePathIds }),
  setWorkstreamLifecycle: (workstreamId: string, lifecycle: "active" | "completed") =>
    invoke<Workstream>("set_workstream_lifecycle", { workstreamId, lifecycle }),
  /** Archive is absolute in v0.2: it only moves the card into the recycle bin. */
  archiveWorkstream: (workstreamId: string) =>
    invoke<Workstream>("archive_workstream", { workstreamId }),
  restoreWorkstream: (workstreamId: string) =>
    invoke<Workstream>("restore_workstream", { workstreamId }),
  /** Only reachable for an archived Workstream; Sessions survive it. */
  deleteWorkstreamPermanently: (workstreamId: string) =>
    invoke<void>("delete_workstream_permanently", { workstreamId }),

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

  listSessions: (projectId?: string, agent?: Agent, scope?: "active" | "trash" | "all") =>
    invoke<Session[]>("list_sessions", {
      projectId: projectId ?? null,
      agent: agent ?? null,
      scope: scope ?? null,
    }),
  listAllSessions: () => invoke<Session[]>("list_sessions", { projectId: null, agent: null }),
  // Session Lifecycle & Deletion v0.1: the UI submits ids only — the
  // deletion plan and its targets never travel from the frontend.
  trashSession: (sessionId: string) => invoke<Session>("trash_session", { sessionId }),
  restoreSession: (sessionId: string) => invoke<Session>("restore_session", { sessionId }),
  prepareSessionPermanentDelete: (sessionId: string) =>
    invoke<PermanentDeletionPreview>("prepare_session_permanent_delete", { sessionId }),
  executeSessionPermanentDelete: (jobId: string) =>
    invoke<PermanentDeletionResult>("execute_session_permanent_delete", { jobId }),
  cancelSessionPermanentDelete: (jobId: string) =>
    invoke<void>("cancel_session_permanent_delete", { jobId }),
  getSessionDeletionJob: (sessionId: string) =>
    invoke<SessionDeletionJob | null>("get_session_deletion_job", { sessionId }),
  getSessionDetail: (sessionId: string) => invoke<SessionDetail>("get_session_detail", { sessionId }),
  bindSessionWorkstream: (sessionId: string, workstreamId: string, role: string) =>
    invoke<void>("bind_session_workstream", { sessionId, workstreamId, role }),
  unbindSessionWorkstream: (sessionId: string, workstreamId: string) =>
    invoke<void>("unbind_session_workstream", { sessionId, workstreamId }),
  replaceSessionBindings: (sessionId: string, bindings: { workstream_id: string; role: string }[]) =>
    invoke<void>("replace_session_bindings", { sessionId, bindings }),
  listSessionBindings: () => invoke<SessionBindingRow[]>("list_session_bindings"),

  syncAll: () => invoke<{ started: boolean }>("sync_all"),
  syncSource: (sourceId: string) => invoke<{ started: boolean }>("sync_source", { sourceId }),
  reingestSource: (sourceId: string) => invoke<{ started: boolean }>("reingest_source", { sourceId }),
  /** Ingest one Session now; Context extraction only runs while Intelligence is on. */
  syncSession: (sessionId: string) =>
    invoke<{ applied: number; ingested: number; context_processing_enabled: boolean }>(
      "sync_session",
      { sessionId },
    ),
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

  /** Context Intelligence — orthogonal to delivery level. Off = Base
   *  Experience: Sessions are still ingested and indexed, but no Context is
   *  extracted, classified or injected. See src/app/experience.tsx. */
  getContextIntelligenceEnabled: () =>
    invoke<boolean>("get_context_intelligence_enabled"),
  setContextIntelligenceEnabled: (enabled: boolean) =>
    invoke<void>("set_context_intelligence_enabled", { enabled }),

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
