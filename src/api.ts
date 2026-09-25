import { invoke } from "@tauri-apps/api/core";
import type {
  Agent, AgentRuntimeDiscovery, AgentRuntimeOverrides, AgentRuntimeSettings,
  ContextDeliveryLevel, ContextItem, ContextItemRevision, CreateWorkstreamReport,
  IngestionDiagnostic, LaunchResult, LocalDeletePreview, PathProbe,
  PermanentDeleteResult,
  Project, ProjectCardData, ProjectDetailData, ProjectWorkstreamRow,
  RecentWorkspacePath, WorkspaceSettings,
  WorkstreamPath, WorkstreamPathRow,
  ReviewFrontier, SearchHit, Session, SessionDetail,
  SyncRun, Workstream, WorkstreamCardData, WorkstreamContext, WorkstreamReviewState, WorkstreamReviewWindow,
  WorkstreamReviewSummary,
} from "./types";

export const api = {
  // ---------------- Projects (方案 §11) ----------------
  //
  // v0.2 derives Projects from WorkspacePaths; reads and rename are the full
  // client surface.
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
   * `create_workstream(title, description, initialPaths?)`.
   *
   * There is no Project argument to carry: v0.2 has no manual
   * Workstream→Project assignment. Each entry of `initialPaths` is a raw
   * string that becomes an ordered WorkstreamPath — first ACCEPTED entry wins
   * the primary seat — and the report says per entry what landed and what was
   * refused, so the UI never has to re-read and silently drop.
   */
  createWorkstream: (title: string, description: string, initialPaths: string[] = []) =>
    invoke<CreateWorkstreamReport>("create_workstream", {
      title,
      description,
      initialPaths,
    }),
  /**
   * Read-only preview for the path picker: what would happen if this string
   * were attached as a working path, and which Project it would project onto.
   * Advisory only — the attacher decides at create time.
   */
  probeWorkspacePath: (path: string) =>
    invoke<PathProbe>("probe_workspace_path", { path }),
  /** Picker candidates: known WorkspacePaths + Session cwd history, ranked by
   *  recent activity. Pure reads; nothing here creates a WorkspacePath. */
  listRecentWorkspacePaths: (limit = 8) =>
    invoke<RecentWorkspacePath[]>("list_recent_workspace_paths", { limit }),
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
  // Session Lifecycle (重构方案 §19/§20): the UI submits session ids only —
  // there is no deletion job and no source deletion anywhere: NoEnding never
  // deletes Agent-owned sources; permanent delete is a LOCAL purge.
  trashSession: (sessionId: string) => invoke<Session>("trash_session", { sessionId }),
  restoreSession: (sessionId: string) => invoke<Session>("restore_session", { sessionId }),
  /** §20.2 无状态预览：fresh root source verdict + counts，没有 job。 */
  getSessionLocalDeletePreview: (sessionId: string) =>
    invoke<LocalDeletePreview>("get_session_local_delete_preview", { sessionId }),
  /** §20.3 执行本地清除：trashed + fresh root missing 才允许。 */
  permanentlyDeleteSession: (sessionId: string) =>
    invoke<PermanentDeleteResult>("permanently_delete_session", { sessionId }),
  getSessionDetail: (sessionId: string) => invoke<SessionDetail>("get_session_detail", { sessionId }),
  /** §11 摄入诊断：Settings 页面专用，默认只看 observation_count >= 2 的。 */
  listIngestionDiagnostics: (minObservations?: number) =>
    invoke<IngestionDiagnostic[]>("list_ingestion_diagnostics", {
      minObservations: minObservations ?? null,
    }),
  /**
   * 设置 / 清空 Session 唯一的所属任务。`workstreamId === null` 即「未归属任务」。
   * 只写 `sessions.owner_workstream_id`，不碰 WorkstreamPath、cwd 或 Project。
   */
  setSessionOwnerWorkstream: (sessionId: string, workstreamId: string | null) =>
    invoke<Session>("set_session_owner_workstream", { sessionId, workstreamId }),

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

  /** §15 — 新建 Session 最多带一个所属任务（`null` = standalone）。 */
  prepareNewSession: (agent: Agent, ownerWorkstreamId: string | null, cwd?: string) =>
    invoke<import("./types").PreparedLaunch>("prepare_new_session", {
      agent,
      ownerWorkstreamId,
      cwd: cwd ?? null,
    }),
  /** §16 — Resume 不再传任何 Workstream：用 Session 当前的 Owner。 */
  prepareResumeSession: (sessionId: string) =>
    invoke<import("./types").PreparedLaunch>("prepare_resume_session", { sessionId }),
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

export type { Agent };
