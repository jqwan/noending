import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import "./newSession.css";
import { api } from "../../api";

import { useAgentRuntime } from "../agents/useAgentRuntime";
import RuntimePopover from "./RuntimePopover";
import { readNewSessionPreferences, updateNewSessionPreferences, lastNewSessionRuntime, rememberNewSessionRuntime } from "./newSessionPreferences";
import AgentIcon from "../../components/AgentIcon";
import Icon from "../../components/Icon";
import SidebarLogo from "../../components/SidebarLogo";
import { showToast } from "../../components/Toast";
import type { Route } from "../../app/routes";
import { announceLaunch } from "../launcher/LaunchResultModal";
import { usePreparedLaunch } from "../launcher/usePreparedLaunch";
import { ellipsisPathMiddle } from "./SessionTable";
import {
  AGENT_LABELS,
  type Agent,
  type AgentStatusEntry,
  type AgentRuntimeOverrides,
  type PreparedLaunch,
  type ProjectCardData,
  type WorkstreamCardData,
} from "../../types";
import WorkstreamFormModal from "../workstreams/WorkstreamFormModal";
import { isSelectableProject } from "../projects/projectKind";
import PageHeader from "../../layout/PageHeader";

/** 新会话页：预览启动目录，打开终端时可携带首条消息。 */
export type NewSessionViewProps = {
  /** 预置选中的跟踪任务；省略或 "none" = standalone。 */
  workstreamId?: string | null;
  /** 单次指定 Agent，不修改全局默认设置。 */
  agent?: Agent | null;
  /** 预置选中的项目 ID；省略时恢复手动选择，首次使用选择默认工作目录所属项目。 */
  projectId?: string | null;
  navigate: (r: Route) => void;
};

const STANDALONE = "none";
// Match the launcher's UTF-16 limit (String.length); keep long drafts intact.
const INITIAL_MESSAGE_LIMIT = 2000;

/** 支持终端 CLI 启动的 Agent 集合（Qoder、WorkBuddy、DSH、ZCode 等纯桌面/无 CLI Agent 不在此列） */
const CLI_AGENTS: Agent[] = ["codex", "claude_code", "pi", "antigravity"];

export default function NewSessionView({
  workstreamId,
  agent: initialAgent,
  projectId: initialProjectId,
  navigate,
}: NewSessionViewProps) {
  const allowedAgents = CLI_AGENTS;
  const [savedPreferences] = useState(readNewSessionPreferences);
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [workstreams, setWorkstreams] = useState<WorkstreamCardData[]>([]);
  const [projects, setProjects] = useState<ProjectCardData[]>([]);
  const [trackedWorkstreamIds, setTrackedWorkstreamIds] = useState<string[]>(
    workstreamId && workstreamId !== STANDALONE ? [workstreamId] : savedPreferences.workstreamIds
  );
  const [selectedProjectId, setSelectedProjectId] = useState<string>(
    initialProjectId && initialProjectId !== "none" ? initialProjectId : ""
  );
  const [agentStatus, setAgentStatus] = useState<Record<string, AgentStatusEntry> | null>(null);
  const [selectedAgent, setSelectedAgent] = useState<Agent>(() => {
    if (initialAgent && allowedAgents.includes(initialAgent)) {
      return initialAgent;
    }
    return savedPreferences.agent && allowedAgents.includes(savedPreferences.agent) ? savedPreferences.agent : "codex";
  });
  const [runtimeChoice, setRuntimeChoice] = useState(() => ({ agent: selectedAgent, value: lastNewSessionRuntime(selectedAgent) }));
  const runtimeOverride = useMemo(() => runtimeChoice.agent === selectedAgent ? runtimeChoice.value
    : lastNewSessionRuntime(selectedAgent), [runtimeChoice, selectedAgent]);
  const setRuntimeOverride = (value: AgentRuntimeOverrides) => setRuntimeChoice({ agent: selectedAgent, value });
  const runtime = useAgentRuntime(selectedAgent, true);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [showNewWorkstreamModal, setShowNewWorkstreamModal] = useState(false);
  const [projectMenuOpen, setProjectMenuOpen] = useState(false);
  const [dirMenuOpen, setDirMenuOpen] = useState(false);
  const [agentMenuOpen, setAgentMenuOpen] = useState(false);
  const projectRef = useRef<HTMLDivElement>(null);
  const dirRef = useRef<HTMLDivElement>(null);
  const agentRef = useRef<HTMLDivElement>(null);
  const mounted = useRef(true);
  const launching = useRef(false);
  const agentChanged = useRef(false);
  const projectChanged = useRef(false);
  const messageChanged = useRef(false);
  const [projectError, setProjectError] = useState("");
  const [projectLoadAttempt, setProjectLoadAttempt] = useState(0);

  const selectWorkstreams = (next: string[]) => {
    setTrackedWorkstreamIds(next);
    updateNewSessionPreferences(current => ({ ...current, workstreamIds: next }));
  };

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    if (!projectMenuOpen && !dirMenuOpen && !agentMenuOpen) return;
    const handleClickOutside = (e: MouseEvent) => {
      if (projectMenuOpen && !projectRef.current?.contains(e.target as Node)) {
        setProjectMenuOpen(false);
      }
      if (dirMenuOpen && !dirRef.current?.contains(e.target as Node)) {
        setDirMenuOpen(false);
      }
      if (agentMenuOpen && !agentRef.current?.contains(e.target as Node)) {
        setAgentMenuOpen(false);
      }
    };
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setProjectMenuOpen(false);
        setDirMenuOpen(false);
        setAgentMenuOpen(false);
      }
    };
    window.addEventListener("mousedown", handleClickOutside);
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("mousedown", handleClickOutside);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [projectMenuOpen, dirMenuOpen, agentMenuOpen]);

  useEffect(() => {
    let cancelled = false;
    setProjectError("");
    Promise.all([api.listWorkstreamCards(), api.listProjectCards(), api.getWorkspaceSettings()])
      .then(async ([ws, cards, settings]) => {
        if (cancelled) return;
        cards = cards.filter(p => p.visibility !== "archived");
        const preferences = readNewSessionPreferences();
        const normal = ws.filter((w) => w.visibility === "normal");
        const targetWs = normal.find((w) => w.id === workstreamId);
        let projectId = initialProjectId && initialProjectId !== "none" ? initialProjectId : "";
        let defaultDir = "";
        if (!projectId && targetWs?.projects.length) {
          projectId = targetWs.projects[0].id;
        }
        if (projectId && !cards.some(p => p.id === projectId)) projectId = "";
        if (!projectId && !initialProjectId && (!workstreamId || workstreamId === STANDALONE) && cards.some(p => p.id === preferences.projectId)) {
          projectId = preferences.projectId;
        }
        if (!projectId) {
          defaultDir = settings.default_workspace;
          // 使用目录身份而非名称；聊天项目可以改名或转为 Git 项目。
          projectId = cards.find((p) => [...p.search_paths, ...p.representative_paths].includes(defaultDir))?.id ?? "";
          if (!projectId) {
            // 首次使用时默认目录可能尚未注册，由现有目录注册流程派生项目。
            const result = await api.addProjectPath(defaultDir);
            if (cancelled) return;
            projectId = result.project_id;
            defaultDir = result.path.canonical_path;
            cards = (await api.listProjectCards()).filter(p => p.visibility !== "archived");
          }
        }
        if (cancelled) return;
        setProjects(cards);
        setWorkstreams(normal);
        const presetProject = Boolean(
          (initialProjectId && initialProjectId !== "none" && cards.some(p => p.id === initialProjectId))
          || (targetWs && targetWs.projects.some(p => p.id === projectId)),
        );
        // An entry preset must stay selected even when it belongs to another
        // project, so the page can ask the user to pick one of its projects.
        const presetWorkstreams = workstreamId && workstreamId !== STANDALONE ? [workstreamId] : null;
        const tracked = presetWorkstreams ?? preferences.workstreamIds
          .filter(id => normal.some(w => w.id === id && w.projects.some(p => p.id === projectId)));
        if (initialAgent && allowedAgents.includes(initialAgent) && !agentChanged.current) {
          updateNewSessionPreferences(current => ({ ...current, agent: initialAgent }));
        }
        if (!projectChanged.current) {
          setSelectedProjectId(projectId);
          setTrackedWorkstreamIds(tracked);
          updateNewSessionPreferences(current => ({
            ...current,
            workstreamIds: tracked,
            projectId: presetProject ? projectId : current.projectId,
          }));
          const project = cards.find(p => p.id === projectId);
          const directories = project?.search_paths.length ? project.search_paths : (project?.representative_paths ?? []);
          const rememberedDir = preferences.directories[projectId];
          setSelectedDirPath(directories.includes(rememberedDir) ? rememberedDir
            : directories.includes(defaultDir) ? defaultDir : (directories[0] ?? ""));
          if (!messageChanged.current) setMessage(preferences.drafts[projectId] ?? preferences.drafts[""] ?? "");
          // Text typed before the project query completed belongs to the resolved project.
          if (preferences.drafts[""] !== undefined) {
            updateNewSessionPreferences(current => {
              const drafts = { ...current.drafts };
              drafts[projectId] = messageChanged.current ? drafts[""] : (drafts[projectId] ?? drafts[""]);
              delete drafts[""];
              return { ...current, drafts };
            });
          }
        }
      })
      .catch((reason) => {
        if (!cancelled) setProjectError(`读取项目失败：${String(reason)}`);
      });

    api
      .getAgentStatus()
      .then((status) => { if (!cancelled) setAgentStatus(status); })
      .catch(console.error);

    if (!initialAgent) {
      api
        .getDefaultAgent()
        .then((def) => {
          if (!cancelled && def && allowedAgents.includes(def)) {
            setDefaultAgent(def);
            if (!agentChanged.current && (!savedPreferences.agent || !allowedAgents.includes(savedPreferences.agent))) {
              setSelectedAgent(def);
            }
          }
        })
        .catch(console.error);
    }
    return () => { cancelled = true; };
  }, [initialAgent, workstreamId, initialProjectId, projectLoadAttempt]);

  const cliAgents = agentStatus
    ? (Object.keys(AGENT_LABELS) as Agent[]).filter(
        (a) => allowedAgents.includes(a) && agentStatus[a]?.terminal_cli && agentStatus[a]?.detected && agentStatus[a]?.launch_enabled
      )
    : allowedAgents;

  const agentAvailable = allowedAgents.includes(selectedAgent) && (!agentStatus || Boolean(agentStatus[selectedAgent]?.terminal_cli && agentStatus[selectedAgent]?.detected && agentStatus[selectedAgent]?.launch_enabled));
  useEffect(() => {
    if (!agentStatus || !cliAgents.length || (agentChanged.current && agentAvailable)) return;
    const preferred = [initialAgent, savedPreferences.agent, defaultAgent, cliAgents[0]]
      .find((candidate): candidate is Agent => Boolean(candidate && cliAgents.includes(candidate)));
    if (preferred && preferred !== selectedAgent) {
      setSelectedAgent(preferred);
      setRuntimeChoice({ agent: preferred, value: lastNewSessionRuntime(preferred) });
    }
  }, [agentStatus, agentAvailable, selectedAgent, initialAgent, savedPreferences.agent, defaultAgent, cliAgents.join(",")]);

  const selectedProject = projects.find((p) => p.id === selectedProjectId) ?? null;

  const selectableProjects = useMemo(() => {
    return projects
      .filter((p) => p.id === selectedProjectId || isSelectableProject(p))
      .sort((a, b) => {
        const aTime = a.last_activity_at || a.updated_at || a.created_at || "";
        const bTime = b.last_activity_at || b.updated_at || b.created_at || "";
        const cmp = bTime.localeCompare(aTime);
        return cmp !== 0 ? cmp : a.name.localeCompare(b.name);
      });
  }, [projects, selectedProjectId]);

  const [projectDetailPaths, setProjectDetailPaths] = useState<Record<string, { canonical_path: string; exists: boolean }[]>>({});

  useEffect(() => {
    if (!selectedProjectId || !api.getProjectDetail) return;
    let cancelled = false;
    api
      .getProjectDetail(selectedProjectId)
      .then((detail) => {
        if (!cancelled && detail?.workspace_paths) {
          setProjectDetailPaths((prev) => ({
            ...prev,
            [selectedProjectId]: detail.workspace_paths.map((wp) => ({
              canonical_path: wp.canonical_path,
              exists: wp.exists,
            })),
          }));
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [selectedProjectId]);

  const projectDirs: string[] = useMemo(() => {
    if (!selectedProject) return [];
    const rawDirs = selectedProject.search_paths?.length
      ? selectedProject.search_paths
      : selectedProject.representative_paths;
    const detailPaths = projectDetailPaths[selectedProject.id];
    if (detailPaths && detailPaths.length > 0) {
      const nonMissing = new Set(
        detailPaths.filter((wp) => wp.exists).map((wp) => wp.canonical_path)
      );
      const filtered = rawDirs.filter((d) => nonMissing.has(d));
      if (filtered.length > 0) return filtered;
    }
    return rawDirs;
  }, [selectedProject, projectDetailPaths]);

  const [selectedDirPath, setSelectedDirPath] = useState<string>("");
  const [dirGitStates, setDirGitStates] = useState<Record<string, boolean>>({});

  useEffect(() => {
    if (projectDirs.length > 0 && !projectDirs.includes(selectedDirPath)) {
      setSelectedDirPath(projectDirs[0]);
    }
  }, [projectDirs, selectedDirPath]);

  const probeTarget = projectDirs.includes(selectedDirPath) ? selectedDirPath : projectDirs[0];
  useEffect(() => {
    const target = probeTarget;
    if (!target || selectedProject?.kind === "git" || dirGitStates[target] !== undefined) return;
    if (!api.probeWorkspacePath) return;
    let cancelled = false;
    api
      .probeWorkspacePath(target)
      .then((probe) => {
        if (!cancelled && probe) {
          setDirGitStates((prev) => ({
            ...prev,
            [target]: probe.git_state === "detected",
          }));
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [probeTarget, selectedProject?.kind, dirGitStates]);

  const launchCwd = projectDirs.includes(selectedDirPath) ? selectedDirPath : projectDirs[0];

  const availableTasks = workstreams.filter(w => w.visibility === "normal" && w.projects.some(p => p.id === selectedProjectId));
  const eligibleTask = trackedWorkstreamIds.every(id => availableTasks.some(w => w.id === id));
  const hasDirectory = Boolean(launchCwd);

  const prepareLaunch = useCallback(async (): Promise<PreparedLaunch | null> => {
    if (!selectedAgent || !agentAvailable || !eligibleTask || !hasDirectory) return null;
    return api.prepareNewSession(selectedAgent, trackedWorkstreamIds, launchCwd, runtimeOverride);
  }, [selectedAgent, trackedWorkstreamIds, launchCwd, runtimeOverride, agentAvailable, eligibleTask, hasDirectory]);

  const { prepared, preparing, error, setError, prepare, release: releasePrepared } =
    usePreparedLaunch(prepareLaunch);

  const toggleTask = (id: string) => {
    releasePrepared();
    selectWorkstreams(trackedWorkstreamIds.includes(id)
      ? trackedWorkstreamIds.filter(task => task !== id)
      : [...trackedWorkstreamIds, id]);
  };

  const handleProjectChange = (next: string) => {
    if (next === selectedProjectId) return;
    projectChanged.current = true;
    setProjectError("");
    releasePrepared();
    setSelectedProjectId(next);
    const preferences = readNewSessionPreferences();
    setSelectedDirPath(preferences.directories[next] ?? "");
    setMessage(preferences.drafts[next] ?? "");
    messageChanged.current = false;
    const tracked = trackedWorkstreamIds.filter(id => workstreams.find(w => w.id === id)?.projects.some(p => p.id === next));
    updateNewSessionPreferences(current => ({ ...current, projectId: next, workstreamIds: tracked }));
    setTrackedWorkstreamIds(tracked);
  };

  const handleDirChange = (next: string) => {
    if (next === selectedDirPath) return;
    releasePrepared();
    setSelectedDirPath(next);
    updateNewSessionPreferences(current => ({ ...current, directories: { ...current.directories, [selectedProjectId]: next } }));
  };

  const handleAddNewProject = async () => {
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "选择项目目录",
      });
      if (!picked) return;
      const pickedPath = Array.isArray(picked) ? picked[0] : picked;
      if (!pickedPath) return;

      const res = await api.addProjectPath(pickedPath);
      const fresh = await api.listProjectCards();
      setProjects(fresh.filter(p => p.visibility !== "archived"));
      if (res.project_id) {
        handleProjectChange(res.project_id);
        const directory = res.path.canonical_path || pickedPath;
        setSelectedDirPath(directory);
        updateNewSessionPreferences(current => ({ ...current, directories: { ...current.directories, [res.project_id!]: directory } }));
      }
      showToast(`已添加到项目「${res.project_name}」`);
    } catch (e) {
      console.error(e);
      showToast(`添加项目失败：${String(e)}`);
    }
  };


  const handleAgentChange = (next: Agent) => {
    agentChanged.current = true;
    updateNewSessionPreferences(current => ({ ...current, agent: next }));
    if (next === selectedAgent) return;
    releasePrepared();
    setRuntimeChoice({ agent: next, value: lastNewSessionRuntime(next) });
    setSelectedAgent(next);
  };

  const handleMessageChange = (next: string) => {
    messageChanged.current = true;
    setMessage(next);
    updateNewSessionPreferences(current => {
      const drafts = { ...current.drafts };
      if (next) drafts[selectedProjectId] = next;
      else delete drafts[selectedProjectId];
      return { ...current, drafts };
    });
  };

  const hasMessage = Boolean(message.trim());
  const messageTooLong = message.length > INITIAL_MESSAGE_LIMIT;
  const launchLabel = hasMessage ? "发送" : "打开终端";
  const canSend = Boolean(!messageTooLong && selectedAgent && agentAvailable && prepared && eligibleTask && hasDirectory && !preparing && !busy);

  const start = async () => {
    if (!canSend || launching.current) return;
    launching.current = true;
    agentChanged.current = true;
    releasePrepared();
    setBusy(true);
    setError("");
    try {
      const r = await api.launchEmbeddedNew(selectedAgent, trackedWorkstreamIds, launchCwd, hasMessage ? message : undefined, runtimeOverride);
      announceLaunch("启动", r);
      if (r.terminal_id) {
        updateNewSessionPreferences(current => {
          const drafts = { ...current.drafts };
          // Do not erase a newer draft written while this launch was in flight.
          if (drafts[selectedProjectId] === message) delete drafts[selectedProjectId];
          return { ...current, drafts };
        });
        if (mounted.current) setMessage("");
        navigate({ view: "terminal", terminalId: r.terminal_id });
      } else {
        throw new Error("未返回新会话终端");
      }
    } catch (e: unknown) {
      if (!mounted.current) return;
      launching.current = false;
      setBusy(false);
      // launchEmbeddedNew 会重新 prepare；失败后恢复预览以便重试。
      await prepare();
      setError(`启动失败：${String(e)}`);
    }
  };


  const projectLabel = selectedProject?.name ?? "选择项目";

  const currentDir = launchCwd || "";
  const dirLabel = useMemo(() => {
    if (!currentDir) return "默认工作目录";
    return ellipsisPathMiddle(currentDir, 42);
  }, [currentDir]);

  const agentLabel = AGENT_LABELS[selectedAgent] ?? selectedAgent;

  return (
    <main className="main new-session-page" aria-labelledby="new-session-title">
      <PageHeader title="新建会话" />
      <div className="new-session-content">
        <div className="new-session-heading">
          <SidebarLogo size={64} />
          <h1 id="new-session-title">你想在 {selectedProject?.name ?? "这里"} 中构建什么？</h1>
          <section className="new-session-tasks" aria-label="跟踪任务（可选）">
            <div className="new-session-tasks-head">
              <div className="new-session-tasks-title">
                <Icon name="tasks" />
                <span>跟踪任务 <span className="muted">· 可多选</span></span>
                {trackedWorkstreamIds.length > 0 && (
                  <span className="badge accent">{trackedWorkstreamIds.length} 已选</span>
                )}
              </div>
              <button type="button" className="btn ghost icon-button small" aria-label="新建跟踪任务" title="新建任务"
                disabled={busy || !selectedProject} onClick={() => setShowNewWorkstreamModal(true)}><Icon name="plus" /></button>
            </div>
            <div className="new-session-task-list">
              {availableTasks.map(task => <label key={task.id} className={`new-session-task-option${trackedWorkstreamIds.includes(task.id) ? " selected" : ""}`}>
                <input type="checkbox" checked={trackedWorkstreamIds.includes(task.id)} disabled={busy} onChange={() => toggleTask(task.id)} />
                <span className="truncate" title={task.title}>{task.title}</span>
              </label>)}
              {availableTasks.length === 0 && <p className="muted small">当前项目暂无可跟踪任务</p>}
            </div>
          </section>
        </div>

        <form className="new-session-form" onSubmit={(e) => { e.preventDefault(); void start(); }}>
          <div className="new-session-context">
            <div
              className={`new-session-picker new-session-project has-custom-menu${projectMenuOpen ? " open" : ""}`}
              ref={projectRef}
              onClick={(e) => {
                if (busy) return;
                if ((e.target as HTMLElement).closest(".new-session-dropdown-menu")) return;
                setProjectMenuOpen((v) => !v);
                setDirMenuOpen(false);
                setAgentMenuOpen(false);
              }}
              onKeyDown={(e) => {
                if (busy) return;
                if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
                  if (!projectMenuOpen) {
                    e.preventDefault();
                    setProjectMenuOpen(true);
                    setDirMenuOpen(false);
                    setAgentMenuOpen(false);
                  }
                }
              }}
              tabIndex={busy ? -1 : 0}
              role="button"
              aria-haspopup="menu"
              aria-expanded={projectMenuOpen}
              aria-label={`所属项目：${projectLabel}`}
              title={projectLabel}
            >
              <Icon name="project" />
              <span className="new-session-picker-label truncate">{projectLabel}</span>
              <Icon name="chevronDown" className="new-session-chevron" />
              <select
                aria-label="所属项目"
                title={projectLabel}
                value={selectedProjectId}
                onChange={(e) => {
                  if (e.target.value === "__new_project__") {
                    void handleAddNewProject();
                    return;
                  }
                  handleProjectChange(e.target.value);
                }}
                disabled={busy}
                tabIndex={-1}
              >
                {!selectedProject && <option value="" disabled>选择项目</option>}
                {selectableProjects.map((p) => (
                  <option key={p.id} value={p.id}>{p.name}</option>
                ))}
                <option value="__new_project__">+ 新增项目</option>
              </select>

              {projectMenuOpen && (
                <div className="new-session-dropdown-menu" role="menu" aria-label="所属项目列表">
                  <div className="new-session-menu-list">
                    {selectableProjects.map((p) => {
                      const isSelected = selectedProjectId === p.id;
                      return (
                        <button
                          key={p.id}
                          type="button"
                          className={`new-session-menu-item${isSelected ? " selected" : ""}`}
                          onClick={() => {
                            handleProjectChange(p.id);
                            setProjectMenuOpen(false);
                          }}
                          title={p.name}
                        >
                          <span className="menu-item-icon"><Icon name="project" /></span>
                          <span className="menu-item-label">{p.name}</span>
                          {isSelected && (
                            <span className="menu-item-check"><Icon name="check" /></span>
                          )}
                        </button>
                      );
                    })}
                  </div>
                  <div className="new-session-menu-divider" />
                  <button
                    type="button"
                    className="new-session-menu-action"
                    onClick={() => {
                      void handleAddNewProject();
                      setProjectMenuOpen(false);
                    }}
                  >
                    <span className="action-icon"><Icon name="plus" /></span>
                    <span>新增项目</span>
                  </button>
                </div>
              )}
            </div>

            {(() => {
              const canSwitchDir = projectDirs.length > 1;
              return (
                <div
                  className={`new-session-picker new-session-dir${canSwitchDir ? " has-custom-menu" : ""}${dirMenuOpen && canSwitchDir ? " open" : ""}`}
                  ref={dirRef}
                  onClick={(e) => {
                    if (busy || !canSwitchDir) return;
                    if ((e.target as HTMLElement).closest(".new-session-dropdown-menu")) return;
                    setDirMenuOpen((v) => !v);
                    setProjectMenuOpen(false);
                    setAgentMenuOpen(false);
                  }}
                  onKeyDown={(e) => {
                    if (busy || !canSwitchDir) return;
                    if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
                      if (!dirMenuOpen) {
                        e.preventDefault();
                        setDirMenuOpen(true);
                        setProjectMenuOpen(false);
                        setAgentMenuOpen(false);
                      }
                    }
                  }}
                  tabIndex={busy || !canSwitchDir ? -1 : 0}
                  role={canSwitchDir ? "button" : undefined}
                  aria-haspopup={canSwitchDir ? "menu" : undefined}
                  aria-expanded={canSwitchDir ? dirMenuOpen : undefined}
                  aria-label={`项目目录：${currentDir || dirLabel}`}
                  title={currentDir || dirLabel}
                >
                  <Icon name="folder" />
                  <span className="new-session-picker-label truncate" title={currentDir || dirLabel}>
                    {dirLabel}
                  </span>
                  {canSwitchDir && <Icon name="chevronDown" className="new-session-chevron" />}
                  <select
                    aria-label="项目目录"
                    title={currentDir || dirLabel}
                    value={selectedDirPath}
                    onChange={(e) => handleDirChange(e.target.value)}
                    disabled={busy || !canSwitchDir}
                    tabIndex={-1}
                  >
                    {projectDirs.map((dir) => (
                      <option key={dir} value={dir}>
                        {dir}
                      </option>
                    ))}
                  </select>

                  {dirMenuOpen && canSwitchDir && (
                    <div className="new-session-dropdown-menu" role="menu" aria-label="项目目录列表">
                      <div className="new-session-menu-list">
                        {projectDirs.map((dir) => {
                          const isSelected = (selectedDirPath || projectDirs[0]) === dir;
                          return (
                            <button
                              key={dir}
                              type="button"
                              className={`new-session-menu-item${isSelected ? " selected" : ""}`}
                              onClick={() => {
                                handleDirChange(dir);
                                setDirMenuOpen(false);
                              }}
                              title={dir}
                            >
                              <span className="menu-item-icon">
                                <Icon name="folder" />
                              </span>
                              <span className="menu-item-label">{dir}</span>
                              {isSelected && (
                                <span className="menu-item-check"><Icon name="check" /></span>
                              )}
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  )}
                </div>
              );
            })()}
          </div>

          {!agentAvailable && <p role="alert">没有可用的新建会话 Agent，请确保本地已安装支持的 Agent CLI。</p>}
          <div className="new-session-composer" aria-busy={busy}>
            <textarea
              autoFocus
              aria-label="首条消息"
              placeholder="描述你想完成的任务，或直接打开终端…"
              aria-invalid={messageTooLong}
              aria-describedby="new-session-message-hint"
              value={message}
              onChange={(e) => handleMessageChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing && e.keyCode !== 229) {
                  e.preventDefault();
                  void start();
                }
              }}
              disabled={busy}
              rows={2}
            />
            <div className="new-session-composer-footer">
              <div className="new-session-composer-footer-left">
                <div
                  className={`new-session-picker new-session-agent has-custom-menu${agentMenuOpen ? " open" : ""}`}
                  ref={agentRef}
                  onClick={(e) => {
                    if (busy) return;
                    if ((e.target as HTMLElement).closest(".new-session-dropdown-menu")) return;
                    setAgentMenuOpen((v) => !v);
                    setProjectMenuOpen(false);
                    setDirMenuOpen(false);
                  }}
                  onKeyDown={(e) => {
                    if (busy) return;
                    if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
                      if (!agentMenuOpen) {
                        e.preventDefault();
                        setAgentMenuOpen(true);
                        setProjectMenuOpen(false);
                        setDirMenuOpen(false);
                      }
                    }
                  }}
                  tabIndex={busy ? -1 : 0}
                  role="button"
                  aria-haspopup="menu"
                  aria-expanded={agentMenuOpen}
                  aria-label={`Agent：${agentLabel}`}
                  title={agentLabel}
                >
                  <AgentIcon agent={selectedAgent} size={16} />
                  <span className="new-session-picker-label">{agentLabel}</span>
                  <Icon name="chevronDown" className="new-session-chevron" />
                  <select
                    aria-label="Agent"
                    title={agentLabel}
                    value={selectedAgent}
                    onChange={(e) => handleAgentChange(e.target.value as Agent)}
                    disabled={busy}
                    tabIndex={-1}
                  >
                    {cliAgents.map((a) => (
                      <option key={a} value={a}>
                        {AGENT_LABELS[a]}
                        {agentStatus && !agentStatus[a]?.detected ? " (未检测到 TUI)" : ""}
                      </option>
                    ))}
                  </select>

                  {agentMenuOpen && (
                    <div className="new-session-dropdown-menu dropup" role="menu" aria-label="Agent 列表">
                      <div className="new-session-menu-list">
                        {cliAgents.map((a) => {
                          const isSelected = a === selectedAgent;
                          const notDetected = agentStatus && !agentStatus[a]?.detected;
                          return (
                            <button
                              key={a}
                              type="button"
                              className={`new-session-menu-item${isSelected ? " selected" : ""}`}
                              onClick={() => {
                                handleAgentChange(a);
                                setAgentMenuOpen(false);
                              }}
                              title={AGENT_LABELS[a]}
                            >
                              <span className="menu-item-icon">
                                <AgentIcon agent={a} size={16} />
                              </span>
                              <span className="menu-item-label">
                                {AGENT_LABELS[a]}
                                {notDetected && <span className="menu-item-extra muted"> (未检测到 TUI)</span>}
                              </span>
                              {isSelected && (
                                <span className="menu-item-check"><Icon name="check" /></span>
                              )}
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  )}
                </div>
              </div>
              <div className="new-session-composer-footer-right">
                <RuntimePopover
                  agent={selectedAgent}
                  runtime={runtime}
                  value={runtimeOverride}
                  disabled={busy}
                  onChange={(v) => {
                    releasePrepared();
                    setRuntimeOverride(v);
                    rememberNewSessionRuntime(selectedAgent, v);
                  }}
                />
                <button
                  type="submit"
                  className="new-session-send"
                  aria-label={busy ? "启动中…" : launchLabel}
                  title={busy ? "启动中…" : preparing ? "准备中…" : launchLabel}
                  disabled={!canSend}
                >
                  {busy ? <span className="new-session-spinner" aria-hidden="true" /> : (
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                      <path d="M12 19V5m-6 6 6-6 6 6" />
                    </svg>
                  )}
                </button>
              </div>
            </div>
          </div>

          {!eligibleTask && (
            <p className="new-session-note">
              {trackedWorkstreamIds.some(id => workstreams.find(w => w.id === id)?.projects.length === 0)
                ? "该任务未关联项目，无法新建会话；请先在任务中关联项目。"
                : "请先选择该任务关联的项目。"}
            </p>
          )}
          {prepared?.cwd_resolution?.fallback && (
            <p className="new-session-note" role="status">
              {prepared.cwd_resolution.note || "未从默认工作目录启动"}
            </p>
          )}
          {(projectError || error) && (
            <div className="new-session-error" role="alert">
              <span>{projectError || error}</span>
              {!preparing && !busy && (
                <button type="button" className="btn small" onClick={() => projectError ? setProjectLoadAttempt((n) => n + 1) : void prepare()}>重试</button>
              )}
            </div>
          )}
          <p id="new-session-message-hint" className="new-session-hint" role={messageTooLong ? "alert" : undefined}>
            {messageTooLong
              ? `首条消息最多 ${INITIAL_MESSAGE_LIMIT} 字符，当前 ${message.length}。请缩短内容，或清空后直接打开终端输入。`
              : `Enter ${launchLabel} · Shift + Enter 换行 · ${message.length}/${INITIAL_MESSAGE_LIMIT}`}
          </p>
        </form>
      </div>

      {showNewWorkstreamModal && (
        <WorkstreamFormModal
          onClose={() => setShowNewWorkstreamModal(false)}
          onCreated={async (newWs) => {
            try {
              const cards=await api.listWorkstreamCards();
              setWorkstreams(cards);
              const created=cards.find(w=>w.id===newWs.id);
              if(created?.visibility==="normal" && created.projects.some(p=>p.id===selectedProjectId)) {
                releasePrepared();
                selectWorkstreams([...new Set([...trackedWorkstreamIds, created.id])]);
              }
            } catch(e) { showToast(`读取任务失败：${String(e)}`); }
            setShowNewWorkstreamModal(false);
          }}
          initialProjectId={selectedProject && isSelectableProject(selectedProject) ? selectedProjectId : undefined}
        />
      )}
    </main>
  );
}
