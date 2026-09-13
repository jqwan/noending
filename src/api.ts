import { invoke } from "@tauri-apps/api/core";
import type {
  Agent, ContextItem, ContextItemRevision, LaunchResult, Project, ProjectResource,
  SearchHit, Session, SessionContextBundle, SessionDetail, SessionWorkstreamBinding,
  SyncRun, Workstream, WorkstreamContext,
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
  createWorkstream: (projectId: string | null, title: string, description: string) =>
    invoke<Workstream>("create_workstream", { projectId, title, description }),
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

  listSessions: (projectId?: string, agent?: Agent) =>
    invoke<Session[]>("list_sessions", { projectId: projectId ?? null, agent: agent ?? null }),
  listAllSessions: () => invoke<Session[]>("list_sessions", { projectId: null, agent: null }),
  getSessionDetail: (sessionId: string) => invoke<SessionDetail>("get_session_detail", { sessionId }),
  assignSessionProject: (sessionId: string, projectId: string | null) =>
    invoke<void>("assign_session_project", { sessionId, projectId }),
  bindSessionWorkstream: (sessionId: string, workstreamId: string, role: string) =>
    invoke<void>("bind_session_workstream", { sessionId, workstreamId, role }),

  syncAll: () => invoke<{ discovered: number; events: number }>("sync_all"),
  syncSession: (sessionId: string) => invoke<{ applied: number }>("sync_session", { sessionId }),
  listSyncRuns: (limit?: number) => invoke<SyncRun[]>("list_sync_runs", { limit: limit ?? 50 }),

  launchNewSession: (agent: Agent, workstreamIds: string[], cwd?: string) =>
    invoke<LaunchResult>("launch_new_session", { agent, workstreamIds, cwd: cwd ?? null }),
  launchResumeSession: (sessionId: string, extraWorkstreamIds: string[]) =>
    invoke<LaunchResult>("launch_resume_session", { sessionId, extraWorkstreamIds }),
  previewBundle: (workstreamIds: string[], mode?: string) =>
    invoke<SessionContextBundle>("preview_context_bundle", { workstreamIds, mode: mode ?? null }),

  search: (query: string, limit?: number) => invoke<SearchHit[]>("search", { query, limit: limit ?? 30 }),
  getStats: () => invoke<Record<string, number>>("get_stats"),
  getAgentStatus: () =>
    invoke<Record<string, { name: string; detected: boolean; executable: string | null; version: string | null }>>("get_agent_status"),

  assistantSend: (sessionId: string | null, text: string) =>
    invoke<{ session_id: string; content: string; runtime: string }>("assistant_send", { sessionId, text }),
  assistantMessages: (sessionId: string) =>
    invoke<import("./types").AssistantMessage[]>("assistant_messages", { sessionId }),
  assistantConfigGet: () =>
    invoke<{ agent: string; model: string; provider: string; effort: string }>("assistant_config_get"),
  assistantConfigSet: (agent: string, model: string, provider: string, effort: string) =>
    invoke<void>("assistant_config_set", { agent, model, provider, effort }),
  assistantExecuteAction: (actionJson: string) =>
    invoke<{ ok: boolean; kind: string; launched_via: string; note: string }>("assistant_execute_action", { actionJson }),
};

export type { Agent, SessionWorkstreamBinding };
