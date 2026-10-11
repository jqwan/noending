import { invoke } from "@tauri-apps/api/core";
import type {
  Preference, Agent, AgentRuntimeDiscovery, AgentRuntimeOverrides, AgentRuntimeSettings,

  IngestTaskStatus, LaunchResult, LocalDeletePreview, PathProbe,
  PermanentDeleteResult,
  Project, ProjectCardData, ProjectDetailData, ProjectWorkstreamRow, ProjectContextView, ProjectUpdateOutcome,
  AddProjectPathResult,
  RecentWorkspacePath, WorkspaceSettings,

  SearchHit, Session, SessionContextView, SessionDetail, SessionTracking,
  SessionMessageMark, SessionMessageWindow, TurnIntermediates,
  SessionUpdateOutcome, SummaryPromptKind, SummaryPromptTemplates, SummaryPromptSettings,
  Workstream, WorkstreamCardData, WorkstreamContext, WorkstreamContextView,
  WorkstreamUpdateOutcome,
} from "./types";

/** Notify mounted preference lists after summary or user writes. */
async function invokeWithPreferences<T>(command: string, args: Record<string, unknown>): Promise<T> {
  const result = await invoke<T>(command, args);
  window.dispatchEvent(new Event("noending:preferences"));
  return result;
}

export const api = {
  // Project membership derives from paths; archive state is independent.
  listProjects: () => invoke<Project[]>("list_projects"),
  /** 一次拿完整 Board 数据，替代 1 + N 的 getProjectDetail。 */
  listProjectCards: () => invoke<ProjectCardData[]>("list_project_cards"),
  getProjectDetail: (projectId: string) =>
    invoke<ProjectDetailData>("get_project_detail", { projectId }),
  listProjectWorkstreams: (projectId: string) =>
    invoke<ProjectWorkstreamRow[]>("list_project_workstreams", { projectId }),
  renameProject: (projectId: string, name: string) =>
    invoke<Project>("rename_project", { projectId, name }),
  archiveProject: (projectId: string) => invoke<Project>("archive_project", { projectId }),
  restoreProject: (projectId: string) => invoke<Project>("restore_project", { projectId }),
  deleteProjectPermanently: (projectId: string) => invoke<void>("delete_project_permanently", { projectId }),
  addProjectPath: (path: string) =>
    invoke<AddProjectPathResult>("add_project_path", { path }),

  getWorkspaceSettings: () => invoke<WorkspaceSettings>("get_workspace_settings"),
  /** Open the active Home's Context extraction diagnostics folder. */
  openContextExtractionLogs: () => invoke<void>("open_context_extraction_logs"),
  /** Requests a relocation for the NEXT launch; `restart_required` says so. */
  setNoendingHome: (newHome: string) =>
    invoke<WorkspaceSettings>("set_noending_home", { newHome }),

  listWorkstreams: (projectId?: string) =>
    invoke<Workstream[]>("list_workstreams", { projectId: projectId ?? null }),
  listWorkstreamCards: () => invoke<WorkstreamCardData[]>("list_workstream_cards"),
  createWorkstream: (title: string,description: string,projectIds: string[] = []) =>
    invoke<Workstream>("create_workstream",{title,description,projectIds}),
  /** 路径选择器的只读预览：这个字符串若作为工作路径会怎样、会投到哪个 Project。
   *  仅供建议——真正决定发生在 create 时。 */
  probeWorkspacePath: (path: string) =>
    invoke<PathProbe>("probe_workspace_path", { path }),
  /** Picker candidates: known WorkspacePaths + Session cwd history, ranked by
   *  recent activity. Pure reads. */
  listRecentWorkspacePaths: (limit = 8) =>
    invoke<RecentWorkspacePath[]>("list_recent_workspace_paths", { limit }),
  updateWorkstream: (w: Workstream) => invoke<void>("update_workstream", { workstream: w }),

  setWorkstreamProjects: (workstreamId: string,projectIds: string[]) =>
    invoke<void>("set_workstream_projects",{workstreamId,projectIds}),
  /** Archive blocks new sessions and preserves task data. */
  archiveWorkstream: (workstreamId: string) =>
    invoke<Workstream>("archive_workstream", { workstreamId }),
  restoreWorkstream: (workstreamId: string) =>
    invoke<Workstream>("restore_workstream", { workstreamId }),
  /** Only reachable for an archived Workstream; Sessions survive it. */
  deleteWorkstreamPermanently: (workstreamId: string) =>
    invoke<void>("delete_workstream_permanently", { workstreamId }),

  getWorkstreamContext: (workstreamId: string) =>
    invoke<WorkstreamContext>("get_workstream_context", { workstreamId }),
  listSessions: (projectId?: string, agent?: Agent, scope?: "unarchived" | "archived" | "all") =>
    invoke<Session[]>("list_sessions", {
      projectId: projectId ?? null,
      agent: agent ?? null,
      scope: scope ?? null,
    }),
  // Session Lifecycle: UI 只提交 session id。没有删除 job，也不删来源：
  // NoEnding 不删 Agent 自有的源；permanent delete 是本地清除。
  archiveSession: (sessionId: string) => invoke<Session>("archive_session", { sessionId }),
  restoreSession: (sessionId: string) => invoke<Session>("restore_session", { sessionId }),
  /** 无状态预览：fresh root source verdict + counts，没有 job。 */
  getSessionLocalDeletePreview: (sessionId: string) =>
    invoke<LocalDeletePreview>("get_session_local_delete_preview", { sessionId }),
  /** 仅已归档的会话可永久删除；不删除 Agent 源文件。 */
  permanentlyDeleteSession: (sessionId: string) =>
    invoke<PermanentDeleteResult>("permanently_delete_session", { sessionId }),
  getSessionTracking: (sessionId: string) => invoke<SessionTracking>("get_session_tracking", {sessionId}),
  getSessionDetail: (sessionId: string) => invoke<SessionDetail>("get_session_detail", { sessionId }),
  revealSessionSource: (sessionId: string) => invoke<void>("reveal_session_source", { sessionId }),
  /** 在系统默认浏览器打开 http(s) 链接（后端只放行 http/https）。 */
  openRemoteUrl: (url: string) => invoke<void>("open_remote_url", { url }),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  /** Conversation 的一页（骨架：用户消息 + 每轮最终回复）：`beforeOrdinal` 取更早
   *  的一页，`afterOrdinal` 取更新的一页，都不传就是最新一页。中间回复不占页，
   *  挂在各条最终回复的 `turn` 摘要上，展开时走 getSessionTurnIntermediates。 */
  getSessionMessages: (
    sessionId: string,
    page: { beforeOrdinal?: number | null; afterOrdinal?: number | null; limit?: number } = {},
  ) => invoke<SessionMessageWindow>("get_session_messages", {
    sessionId,
    beforeOrdinal: page.beforeOrdinal ?? null,
    afterOrdinal: page.afterOrdinal ?? null,
    limit: page.limit ?? null,
  }),
  /** 一轮展开后的中间回复：起点用户消息与最终回复之间的全部消息（旧→新）。 */
  getSessionTurnIntermediates: (sessionId: string, afterOrdinal: number, beforeOrdinal: number) =>
    invoke<TurnIntermediates>("get_turn_intermediates", { sessionId, afterOrdinal, beforeOrdinal }),
  /** 导航条的用户消息位置：当前会话里所有 user 消息，按顺序。 */
  getSessionUserMessageMarks: (sessionId: string) =>
    invoke<SessionMessageMark[]>("get_session_user_message_marks", { sessionId }),
  /** 摄入诊断：Settings 页面专用，默认只看 observation_count >= 2 的。 */  /** 添加或取消任务跟踪，不改变项目及目录。 */
  setSessionTaskTracking: (sessionId: string,workstreamId: string,tracked: boolean) =>
    invoke<Session>("set_session_task_tracking",{sessionId,workstreamId,tracked}),

  // Ingestion: 两个显式更新按钮 + 纯读取。后台摄入只由三个显式入口排队
  // （高级维护页）与两个自动触发（启动 / 回到前台的 freshness 回落）；普通页面打开从不触发。
  /** 高级维护：增量同步全部已启用来源。 */
  reconcileAll: () => invoke<{ queued: boolean }>("reconcile_all"),
  /** 高级维护：增量同步单个来源。 */
  reconcileSource: (sourceId: string) => invoke<{ queued: boolean }>("reconcile_source", { sourceId }),
  /** 高级维护：全量同步单个来源。 */
  reingestSource: (sourceId: string) => invoke<{ queued: boolean }>("reingest_source", { sourceId }),
  /** 单会话定向增量同步：重新读取该会话的成员文件并同步。 */
  refreshSession: (sessionId: string) =>
    invoke<{ queued: boolean }>("refresh_session", { sessionId }),
  /** 前台回落：只在距上次成功超过 freshness 阈值时才排队。 */
  appForeground: () => invoke<{ queued: boolean }>("app_foreground"),
  /** 最近一次后台摄入任务的结果（高级维护页展示）。 */
  getIngestionStatus: () => invoke<IngestTaskStatus | null>("get_ingestion_status"),

  // Explicit Context update (read + one-click update)
  /** Session 摘要 + 待更新状态。纯读取，不摄入、不调用模型。 */
  getSessionContext: (sessionId: string) =>
    invoke<SessionContextView>("get_session_context", { sessionId }),
  /** Workstream 当前 Context + revision + 待更新状态。纯读取。 */
  getWorkstreamContextState: (workstreamId: string) =>
    invoke<WorkstreamContextView>("get_workstream_context_state", { workstreamId }),
  /** 当前摘要与增量消息整合，最多启动一次 Agent。 */
  updateSessionContext: (sessionId: string) =>
    invokeWithPreferences<SessionUpdateOutcome>("update_session_context", { sessionId }),
  /** 从完整有效历史重新生成，不沿用当前摘要。 */
  regenerateSessionContext: (sessionId: string) =>
    invokeWithPreferences<SessionUpdateOutcome>("regenerate_session_context", { sessionId }),
  /** 一次点击 → 最多一次模型调用 → 相关 Session 摘要与 Workstream 状态一起更新。 */
  updateWorkstreamContext: (workstreamId: string) =>
    invokeWithPreferences<WorkstreamUpdateOutcome>("update_workstream_context", { workstreamId }),

  getProjectContextState: (projectId: string) =>
    invoke<ProjectContextView>("get_project_context_state", { projectId }),
  updateProjectContext: (projectId: string) =>
    invokeWithPreferences<ProjectUpdateOutcome>("update_project_context", { projectId }),
  regenerateProjectContext: (projectId: string) =>
    invokeWithPreferences<ProjectUpdateOutcome>("regenerate_project_context", { projectId }),

  regenerateWorkstreamContext: (workstreamId: string) =>
    invokeWithPreferences<WorkstreamUpdateOutcome>("regenerate_workstream_context", { workstreamId }),

  listPreferences: (projectId: string | null) => invoke<Preference[]>("list_preferences", { projectId }),
  promotePreference: (id: string) => invoke<Preference>("promote_preference",{id}),
  savePreference: (id: string | null, projectId: string | null, content: string, locked: boolean) =>
    invokeWithPreferences<Preference>("save_preference", { id, projectId, content, locked }),
  deletePreference: (id: string) => invokeWithPreferences<void>("delete_preference", { id }),

  getSummaryPromptSettings: () =>
    invoke<SummaryPromptSettings>("get_summary_prompt_settings"),
  setSummaryPromptTemplate: (kind: SummaryPromptKind, template: string | null) =>
    invoke<SummaryPromptSettings>("set_summary_prompt_template", { kind, template }),
  previewSummaryPrompt: (kind: SummaryPromptKind, templates: SummaryPromptTemplates) =>
    invoke<string>("preview_summary_prompt", { kind, templates }),

  /** 新建 Session 可预选一个跟踪任务（`null` = standalone）。 */
  prepareNewSession: (agent: Agent, initialWorkstreamId: string | null, cwd?: string, runtime?: AgentRuntimeOverrides) =>
    invoke<import("./types").PreparedLaunch>("prepare_new_session", {
      agent,
      runtime: runtime ?? null,
      initialWorkstreamId,
      cwd: cwd ?? null,
    }),
  launchPrepared: (preparedId: string) =>
    invoke<LaunchResult>("launch_prepared", { preparedId }),
  cancelPrepared: (preparedId: string) =>
    invoke<void>("cancel_prepared", { preparedId }),

  /** Agent 图标「继续」按钮：直接在会话格式对应的桌面应用里打开，无预览。
   *  没有桌面路由的格式（后端 desktop_resume_route 拒绝）在此报错。 */
  continueSessionDesktop: (sessionId: string) =>
    invoke<{ uri: string; note: string }>("continue_session_desktop", { sessionId }),

  /** 新建会话的内嵌直启：prepare + 起一个未绑定会话的内嵌终端（会话身份由
   *  Agent 通知或预设原生 ID 与摄入记录精确匹配后绑定）。返回携带 terminal_id。 */
  launchEmbeddedNew: (agent: Agent, initialWorkstreamId: string | null, cwd?: string, initialMessage?: string, runtime?: AgentRuntimeOverrides) =>
    invoke<LaunchResult>("launch_embedded_new", {
      agent,
      runtime: runtime ?? null,
      initialWorkstreamId,
      cwd: cwd ?? null,
      initialMessage: initialMessage ?? null,
    }),
  /** 保留的内嵌终端（含已退出，新→旧）：侧边栏读这里；手动关闭才移除。 */
  terminalList: () =>
    invoke<import("./types").TerminalSummary[]>("terminal_list"),
  /** 重启此终端当前绑定会话的 Agent，返回新终端；失败保留旧终端以便重试。 */
  terminalReconnect: (terminalId: string) =>
    invoke<import("./types").LaunchResult>("terminal_reconnect", { terminalId }),
  /** 侧边栏「运行中」的显式关闭：杀进程并移除记录。 */
  terminalClose: (terminalId: string) =>
    invoke<import("./types").TerminalSummary>("terminal_close", { terminalId }),

  // 内嵌终端：终端子页的直启入口——prepare + embedded launch 一步完成（不走
  // 继续会话弹窗），之后通过 attach 协议接管。输出走
  // terminal-output://{id} / terminal-exit://{id} 事件。
  launchEmbeddedResume: (sessionId: string) =>
    invoke<LaunchResult>("launch_embedded_resume", { sessionId }),
  terminalForSession: (sessionId: string) =>
    invoke<import("./types").TerminalSummary | null>("terminal_for_session", { sessionId }),
  terminalAttach: (terminalId: string) =>
    invoke<import("./types").TerminalSnapshot>("terminal_attach", { terminalId }),
  terminalInput: (terminalId: string, data: string) =>
    invoke<void>("terminal_input", { terminalId, data }),
  terminalResize: (terminalId: string, cols: number, rows: number) =>
    invoke<void>("terminal_resize", { terminalId, cols, rows }),
  // 原生读取系统剪贴板的非空文本；图片由 Agent TUI 自己处理。
  readClipboardForTerminal: () =>
    invoke<string | null>("read_clipboard_for_terminal"),

  listIngestSources: () =>
    invoke<import("./types").IngestSource[]>("list_ingest_sources"),
  addIngestSource: (agent: Agent, path: string) =>
    invoke<import("./types").IngestSource>("add_ingest_source", { agent, path }),
  setIngestSourceEnabled: (sourceId: string, enabled: boolean) =>
    invoke<void>("set_ingest_source_enabled", { sourceId, enabled }),
  setAllIngestSourcesEnabled: (enabled: boolean) =>
    invoke<number>("set_all_ingest_sources_enabled", { enabled }),
  removeIngestSource: (sourceId: string) =>
    invoke<void>("remove_ingest_source", { sourceId }),

  search: (query: string, limit?: number) => invoke<SearchHit[]>("search", { query, limit: limit ?? 30 }),
  getAgentStatus: () =>
    invoke<Record<string, import("./types").AgentStatusEntry>>("get_agent_status"),

  setAgentLaunchEnabled: (agent: Agent, enabled: boolean) => invoke<void>("set_agent_launch_enabled", { agent, enabled }),
  redetectAgent: (agent: Agent) => invoke<AgentRuntimeSettings>("redetect_agent", { agent }),

  getDefaultAgent: () => invoke<Agent | null>("get_default_agent"),
  setDefaultAgent: (agent: Agent) => invoke<void>("set_default_agent", { agent }),

  getAgentRuntimeSettings: (agent: Agent) =>
    invoke<AgentRuntimeSettings>("get_agent_runtime_settings", { agent }),
  setAgentRuntimeOverrides: (agent: Agent, overrides: AgentRuntimeOverrides) =>
    invoke<AgentRuntimeSettings>("set_agent_runtime_overrides", { agent, overrides }),
  refreshAgentRuntimeOptions: (agent: Agent) =>
    invoke<AgentRuntimeDiscovery>("refresh_agent_runtime_options", { agent }),
};

export type { Agent };
