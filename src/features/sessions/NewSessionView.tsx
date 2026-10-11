import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import { NATIVE_RUNTIME } from "../agents/RuntimeFields";
import { useAgentRuntime } from "../agents/useAgentRuntime";
import RuntimePopover from "./RuntimePopover";
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
import PageHeader from "../../layout/PageHeader";

/** 新会话页：预览启动目录，发送首条消息时才创建内嵌终端。 */
export type NewSessionViewProps = {
  /** 预置选中的跟踪任务；省略或 "none" = standalone。 */
  workstreamId?: string | null;
  /** 单次指定 Agent，不修改全局默认设置。 */
  agent?: Agent | null;
  /** 预置选中的项目 ID；省略时选择默认工作目录所属的 NoEnding Chat 项目。 */
  projectId?: string | null;
  navigate: (r: Route) => void;
};

const STANDALONE = "none";

/** 支持终端 CLI 启动的 Agent 集合（Qoder、WorkBuddy、DSH、ZCode 等纯桌面/无 CLI Agent 不在此列） */
const CLI_AGENTS: Agent[] = ["codex", "claude_code", "pi", "antigravity"];

export default function NewSessionView({
  workstreamId,
  agent: initialAgent,
  projectId: initialProjectId,
  navigate,
}: NewSessionViewProps) {
  const allowedAgents = CLI_AGENTS;
  const [workstreams, setWorkstreams] = useState<WorkstreamCardData[]>([]);
  const [projects, setProjects] = useState<ProjectCardData[]>([]);
  const [trackedWorkstreamId, setTrackedWorkstreamId] = useState(
    workstreamId && workstreamId !== STANDALONE ? workstreamId : STANDALONE
  );
  const [selectedProjectId, setSelectedProjectId] = useState<string>(
    initialProjectId && initialProjectId !== "none" ? initialProjectId : ""
  );
  const [agentStatus, setAgentStatus] = useState<Record<string, AgentStatusEntry> | null>(null);
  const [selectedAgent, setSelectedAgent] = useState<Agent>(() => {
    if (initialAgent && allowedAgents.includes(initialAgent)) {
      return initialAgent;
    }
    return "codex";
  });
  const [runtimeOverride, setRuntimeOverride] = useState<AgentRuntimeOverrides>(NATIVE_RUNTIME);
  const runtime = useAgentRuntime(selectedAgent, true);
  const launchRuntime = useMemo(() => {
    if (runtimeOverride.model || runtimeOverride.effort) {
      return runtimeOverride;
    }
    return undefined;
  }, [runtimeOverride]);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [showNewWorkstreamModal, setShowNewWorkstreamModal] = useState(false);
  const [taskMenuOpen, setTaskMenuOpen] = useState(false);
  const [projectMenuOpen, setProjectMenuOpen] = useState(false);
  const [dirMenuOpen, setDirMenuOpen] = useState(false);
  const taskRef = useRef<HTMLDivElement>(null);
  const projectRef = useRef<HTMLDivElement>(null);
  const dirRef = useRef<HTMLDivElement>(null);
  const mounted = useRef(true);
  const launching = useRef(false);
  const agentChanged = useRef(false);
  const projectChanged = useRef(false);
  const [projectError, setProjectError] = useState("");
  const [projectLoadAttempt, setProjectLoadAttempt] = useState(0);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    if (!taskMenuOpen && !projectMenuOpen && !dirMenuOpen) return;
    const handleClickOutside = (e: MouseEvent) => {
      if (taskMenuOpen && !taskRef.current?.contains(e.target as Node)) {
        setTaskMenuOpen(false);
      }
      if (projectMenuOpen && !projectRef.current?.contains(e.target as Node)) {
        setProjectMenuOpen(false);
      }
      if (dirMenuOpen && !dirRef.current?.contains(e.target as Node)) {
        setDirMenuOpen(false);
      }
    };
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setTaskMenuOpen(false);
        setProjectMenuOpen(false);
        setDirMenuOpen(false);
      }
    };
    window.addEventListener("mousedown", handleClickOutside);
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("mousedown", handleClickOutside);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [taskMenuOpen, projectMenuOpen, dirMenuOpen]);

  useEffect(() => {
    let cancelled = false;
    setProjectError("");
    Promise.all([api.listWorkstreamCards(), api.listProjectCards(), api.getWorkspaceSettings()])
      .then(async ([ws, cards, settings]) => {
        if (cancelled) return;
        const normal = ws.filter((w) => w.visibility === "normal");
        const targetWs = normal.find((w) => w.id === workstreamId);
        let projectId = initialProjectId && initialProjectId !== "none" ? initialProjectId : "";
        let defaultDir = "";
        if (!projectId && targetWs?.projects.length === 1) {
          projectId = targetWs.projects[0].id;
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
            cards = await api.listProjectCards();
          }
        }
        if (cancelled) return;
        setProjects(cards);
        setWorkstreams(normal);
        if (!projectChanged.current) {
          setSelectedProjectId(projectId);
          if (defaultDir) setSelectedDirPath(defaultDir);
          if (workstreamId && workstreamId !== STANDALONE && !targetWs) setTrackedWorkstreamId(STANDALONE);
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
          if (!cancelled && !agentChanged.current && def && allowedAgents.includes(def)) {
            setSelectedAgent(def);
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
    if (agentStatus && !agentAvailable && cliAgents.length) {
      setSelectedAgent(cliAgents[0]);
      setRuntimeOverride(NATIVE_RUNTIME);
    }
  }, [agentStatus, agentAvailable, cliAgents.join(",")]);

  const selectedProject = projects.find((p) => p.id === selectedProjectId) ?? null;

  const otherProjects = useMemo(
    () => projects,
    [projects],
  );

  const projectDirs: string[] = useMemo(() => {
    if (!selectedProject) return [];
    return selectedProject.search_paths?.length
      ? selectedProject.search_paths
      : selectedProject.representative_paths;
  }, [selectedProject]);

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
  const eligibleTask = trackedWorkstreamId === STANDALONE || availableTasks.some(w => w.id === trackedWorkstreamId);
  const hasDirectory = Boolean(launchCwd);

  const prepareLaunch = useCallback(async (): Promise<PreparedLaunch | null> => {
    if (!selectedAgent || !agentAvailable || !eligibleTask || !hasDirectory) return null;
    const initialTaskId = trackedWorkstreamId === STANDALONE ? null : trackedWorkstreamId;
    if (launchRuntime) return api.prepareNewSession(selectedAgent, initialTaskId, launchCwd, launchRuntime);
    if (launchCwd) {
      return api.prepareNewSession(selectedAgent, initialTaskId, launchCwd);
    }
    return api.prepareNewSession(selectedAgent, initialTaskId);
  }, [selectedAgent, trackedWorkstreamId, launchCwd, launchRuntime, agentAvailable, eligibleTask, hasDirectory]);

  const { prepared, preparing, error, setError, prepare, release: releasePrepared } =
    usePreparedLaunch(prepareLaunch);

  const handleWsChange = (next: string) => {
    if (next === trackedWorkstreamId) return;
    releasePrepared();
    setTrackedWorkstreamId(next);

  };

  const handleProjectChange = (next: string) => {
    if (next === selectedProjectId) return;
    projectChanged.current = true;
    setProjectError("");
    releasePrepared();
    setSelectedProjectId(next);
    if (!workstreams.find(w=>w.id===trackedWorkstreamId)?.projects.some(p=>p.id===next)) setTrackedWorkstreamId(STANDALONE);
  };

  const handleDirChange = (next: string) => {
    if (next === selectedDirPath) return;
    releasePrepared();
    setSelectedDirPath(next);
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
      setProjects(fresh);
      if (res.project_id) {
        handleProjectChange(res.project_id);
        setSelectedDirPath(res.path.canonical_path || pickedPath);
      }
      showToast(`已添加到项目「${res.project_name}」`);
    } catch (e) {
      console.error(e);
      showToast(`添加项目失败：${String(e)}`);
    }
  };


  const handleAgentChange = (next: Agent) => {
    agentChanged.current = true;
    if (next === selectedAgent) return;
    releasePrepared();
    setRuntimeOverride(NATIVE_RUNTIME);
    setSelectedAgent(next);
  };

  const canSend = Boolean(message.trim() && selectedAgent && agentAvailable && prepared && eligibleTask && hasDirectory && !preparing && !busy);

  const start = async () => {
    if (!canSend || launching.current) return;
    launching.current = true;
    agentChanged.current = true;
    releasePrepared();
    setBusy(true);
    setError("");
    try {
      const initialTaskId = trackedWorkstreamId === STANDALONE ? null : trackedWorkstreamId;
      const r = launchRuntime ? await api.launchEmbeddedNew(selectedAgent, initialTaskId, launchCwd, message, launchRuntime) : await api.launchEmbeddedNew(
        selectedAgent,
        initialTaskId,
        launchCwd,
        message,
      );
      announceLaunch("启动", r);
      if (r.terminal_id) {
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


  const selectedWorkstream = workstreams.find((w) => w.id === trackedWorkstreamId);
  const taskLabel =
    trackedWorkstreamId === STANDALONE
      ? "无任务"
      : (selectedWorkstream?.title ?? "无任务");

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
          <div className="new-session-heading-title">
            <SidebarLogo size={36} />
            <h1 id="new-session-title">开启新会话</h1>
          </div>
          <p className="new-session-subtitle">选择项目与跟踪任务，立即启动 Agent 展开工作</p>
        </div>

        <form onSubmit={(e) => { e.preventDefault(); void start(); }}>
          <div className="new-session-context">
            <div
              className={`new-session-picker new-session-project has-custom-menu${projectMenuOpen ? " open" : ""}`}
              ref={projectRef}
              onClick={(e) => {
                if (busy) return;
                if ((e.target as HTMLElement).closest(".new-session-dropdown-menu")) return;
                setProjectMenuOpen((v) => !v);
                setTaskMenuOpen(false);
                setDirMenuOpen(false);
              }}
              onKeyDown={(e) => {
                if (busy) return;
                if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
                  if (!projectMenuOpen) {
                    e.preventDefault();
                    setProjectMenuOpen(true);
                    setTaskMenuOpen(false);
                    setDirMenuOpen(false);
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
                {otherProjects.map((p) => (
                  <option key={p.id} value={p.id}>{p.name}</option>
                ))}
                <option value="__new_project__">+ 新项目</option>
              </select>

              {projectMenuOpen && (
                <div className="new-session-dropdown-menu" role="menu" aria-label="所属项目列表">
                  <div className="new-session-menu-list">
                    {otherProjects.map((p) => {
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
                    <span>添加项目...</span>
                  </button>
                </div>
              )}
            </div>

            <div
              className={`new-session-picker new-session-task has-custom-menu${taskMenuOpen ? " open" : ""}`}
              ref={taskRef}
              onClick={(e) => {
                if (busy) return;
                if ((e.target as HTMLElement).closest(".new-session-dropdown-menu")) return;
                setTaskMenuOpen((v) => !v);
                setProjectMenuOpen(false);
                setDirMenuOpen(false);
              }}
              onKeyDown={(e) => {
                if (busy) return;
                if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
                  if (!taskMenuOpen) {
                    e.preventDefault();
                    setTaskMenuOpen(true);
                    setProjectMenuOpen(false);
                    setDirMenuOpen(false);
                  }
                }
              }}
              tabIndex={busy ? -1 : 0}
              role="button"
              aria-haspopup="menu"
              aria-expanded={taskMenuOpen}
              aria-label={`跟踪任务：${taskLabel}`}
              title={taskLabel}
            >
              <Icon name={trackedWorkstreamId === STANDALONE ? "minus" : "tasks"} />
              <span className="new-session-picker-label truncate">{taskLabel}</span>
              <Icon name="chevronDown" className="new-session-chevron" />
              <select
                aria-label="跟踪任务（可选）"
                title={taskLabel}
                value={eligibleTask ? trackedWorkstreamId : STANDALONE}
                onChange={(e) => {
                  if (e.target.value === "__new_workstream__") {
                    if (Boolean(selectedProject)) setShowNewWorkstreamModal(true);
                    return;
                  }
                  handleWsChange(e.target.value);
                }}
                disabled={busy}
                tabIndex={-1}
              >
                <option value={STANDALONE}>无任务</option>
                {availableTasks.map((w) => (
                  <option key={w.id} value={w.id}>{w.title}</option>
                ))}
                {Boolean(selectedProject) && <option value="__new_workstream__">+ 新任务</option>}
              </select>

              {taskMenuOpen && (
                <div className="new-session-dropdown-menu" role="menu" aria-label="跟踪任务列表">
                  <div className="new-session-menu-list">
                    <button
                      type="button"
                      className={`new-session-menu-item${trackedWorkstreamId === STANDALONE ? " selected" : ""}`}
                      onClick={() => {
                        handleWsChange(STANDALONE);
                        setTaskMenuOpen(false);
                      }}
                    >
                      <span className="menu-item-icon"><Icon name="minus" /></span>
                      <span className="menu-item-label">无任务</span>
                      {trackedWorkstreamId === STANDALONE && (
                        <span className="menu-item-check"><Icon name="check" /></span>
                      )}
                    </button>
                    {availableTasks.map((w) => {
                      const isSelected = trackedWorkstreamId === w.id;
                      return (
                        <button
                          key={w.id}
                          type="button"
                          className={`new-session-menu-item${isSelected ? " selected" : ""}`}
                          onClick={() => {
                            handleWsChange(w.id);
                            setTaskMenuOpen(false);
                          }}
                          title={w.title}
                        >
                          <span className="menu-item-icon"><Icon name="tasks" /></span>
                          <span className="menu-item-label">{w.title}</span>
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
                    disabled={!selectedProject}
                    onClick={() => {
                      setShowNewWorkstreamModal(true);
                      setTaskMenuOpen(false);
                    }}
                  >
                    <span className="action-icon"><Icon name="plus" /></span>
                    <span>添加任务...</span>
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
                    setTaskMenuOpen(false);
                    setProjectMenuOpen(false);
                  }}
                  onKeyDown={(e) => {
                    if (busy || !canSwitchDir) return;
                    if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
                      if (!dirMenuOpen) {
                        e.preventDefault();
                        setDirMenuOpen(true);
                        setTaskMenuOpen(false);
                        setProjectMenuOpen(false);
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

          {!agentAvailable && <p role="alert">没有可用的新建会话代理，请确保本地已安装支持的 Agent CLI。</p>}
          <div className="new-session-composer" aria-busy={busy}>
            <textarea
              autoFocus
              aria-label="首条消息"
              placeholder="描述你想完成的任务…"
              value={message}
              onChange={(e) => setMessage(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing && e.keyCode !== 229) {
                  e.preventDefault();
                  void start();
                }
              }}
              disabled={busy}
              rows={4}
            />
            <div className="new-session-composer-footer">
              <div className="new-session-composer-footer-left" />
              <div className="new-session-composer-footer-right">
                <label className="new-session-picker new-session-agent">
                  <AgentIcon agent={selectedAgent} size={16} />
                  <span className="new-session-picker-label">{agentLabel}</span>
                  <Icon name="chevronDown" className="new-session-chevron" />
                  <select
                    aria-label="Agent"
                    title={agentLabel}
                    value={selectedAgent}
                    onChange={(e) => handleAgentChange(e.target.value as Agent)}
                    disabled={busy}
                  >
                    {cliAgents.map((a) => (
                      <option key={a} value={a}>
                        {AGENT_LABELS[a]}
                        {agentStatus && !agentStatus[a]?.detected ? " (未检测到 TUI)" : ""}
                      </option>
                    ))}
                  </select>
                </label>
                <RuntimePopover
                  agent={selectedAgent}
                  runtime={runtime}
                  value={runtimeOverride}
                  disabled={busy}
                  onChange={(v) => {
                    releasePrepared();
                    setRuntimeOverride(v);
                  }}
                />
                <button
                  type="submit"
                  className="new-session-send"
                  aria-label={busy ? "启动中…" : "发送"}
                  title={busy ? "启动中…" : preparing ? "准备中…" : "发送"}
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

          {!eligibleTask && <p className="new-session-note">请先选择该任务关联的项目。</p>}
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
          <p className="new-session-hint">Enter 发送 · Shift + Enter 换行</p>
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
                releasePrepared();setTrackedWorkstreamId(created.id);
              }
            } catch(e) { showToast(`读取任务失败：${String(e)}`); }
            setShowNewWorkstreamModal(false);
          }}
          initialProjectId={selectedProjectId || undefined}
        />
      )}
    </main>
  );
}
