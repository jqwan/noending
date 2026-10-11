import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import RuntimeFields, { CatalogStatus, NATIVE_RUNTIME } from "../agents/RuntimeFields";
import { useAgentRuntime } from "../agents/useAgentRuntime";
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
import { isChatDirectoryProject } from "../projects/projectKind";

/** 新会话页：预览启动目录，发送首条消息时才创建内嵌终端。 */
export type NewSessionViewProps = {
  /** 预置选中的所属任务；省略或 "none" = standalone。 */
  workstreamId?: string | null;
  /** 单次指定 Agent，不修改全局默认设置。 */
  agent?: Agent | null;
  /** 预置选中的项目 ID；省略或 "none" = 未归属项目，使用默认工作目录。 */
  projectId?: string | null;
  navigate: (r: Route) => void;
};

const STANDALONE = "none";
const UNASSIGNED_PROJECT = "none";

/** 支持终端 CLI 启动的 Agent 集合（Qoder、WorkBuddy、DSH、ZCode 等纯桌面/无 CLI Agent 不在此列） */
const CLI_AGENTS: Agent[] = ["codex", "claude_code", "pi", "antigravity"];

export default function NewSessionView({
  workstreamId,
  agent: initialAgent,
  projectId: initialProjectId,
  navigate,
}: NewSessionViewProps) {
  const [workstreams, setWorkstreams] = useState<WorkstreamCardData[]>([]);
  const [projects, setProjects] = useState<ProjectCardData[]>([]);
  const [ownerWorkstreamId, setOwnerWorkstreamId] = useState(
    workstreamId && workstreamId !== STANDALONE ? workstreamId : STANDALONE
  );
  const [selectedProjectId, setSelectedProjectId] = useState<string>(
    initialProjectId && initialProjectId !== "none" ? initialProjectId : UNASSIGNED_PROJECT
  );
  const [agentStatus, setAgentStatus] = useState<Record<string, AgentStatusEntry> | null>(null);
  const [selectedAgent, setSelectedAgent] = useState<Agent>(() => {
    if (initialAgent && CLI_AGENTS.includes(initialAgent)) {
      return initialAgent;
    }
    return "codex";
  });
  const [runtimeOpen, setRuntimeOpen] = useState(false);
  const [runtimeMode, setRuntimeMode] = useState<"inherit" | "native" | "custom">("inherit");
  const [runtimeOverride, setRuntimeOverride] = useState<AgentRuntimeOverrides>(NATIVE_RUNTIME);
  const runtime = useAgentRuntime(selectedAgent, runtimeOpen);
  const launchRuntime = runtimeMode === "inherit" ? undefined : runtimeMode === "native" ? NATIVE_RUNTIME : runtimeOverride;
  const [defaultWorkspace, setDefaultWorkspace] = useState<string>("");
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
    api
      .listWorkstreamCards()
      .then((ws) => {
        if (!cancelled) {
          const normal = ws.filter((w) => w.visibility === "normal");
          setWorkstreams(normal);
          if ((!initialProjectId || initialProjectId === UNASSIGNED_PROJECT) && workstreamId && workstreamId !== STANDALONE) {
            const targetWs = normal.find((w) => w.id === workstreamId);
            if (targetWs?.projects.length === 1) {
              setSelectedProjectId(targetWs.projects[0].id);
            }
          }
        }
      })
      .catch(console.error);

    api
      .listProjectCards()
      .then((cards) => {
        if (!cancelled) setProjects(cards);
      })
      .catch(console.error);

    api
      .getAgentStatus()
      .then((status) => { if (!cancelled) setAgentStatus(status); })
      .catch(console.error);

    api
      .getWorkspaceSettings?.()
      ?.then((s) => {
        if (!cancelled && s?.default_workspace) {
          setDefaultWorkspace(s.default_workspace);
        }
      })
      ?.catch(console.error);

    if (!initialAgent) {
      api
        .getDefaultAgent()
        .then((def) => {
          if (!cancelled && !agentChanged.current && def && CLI_AGENTS.includes(def)) {
            setSelectedAgent(def);
          }
        })
        .catch(console.error);
    }
    return () => { cancelled = true; };
  }, [initialAgent, workstreamId, initialProjectId]);

  const cliAgents = agentStatus
    ? (Object.keys(AGENT_LABELS) as Agent[]).filter(
        (a) => agentStatus[a]?.terminal_cli && agentStatus[a]?.launch_enabled
      )
    : CLI_AGENTS;

  const agentAvailable = !agentStatus || Boolean(agentStatus[selectedAgent]?.terminal_cli && agentStatus[selectedAgent]?.launch_enabled);
  useEffect(() => {
    if (agentStatus && !agentAvailable && cliAgents.length) {
      setSelectedAgent(cliAgents[0]);
      setRuntimeMode("inherit");
    }
  }, [agentStatus, agentAvailable, cliAgents.join(",")]);

  const selectedProject = projects.find((p) => p.id === selectedProjectId) ?? null;
  const isUnassignedProject = selectedProjectId === UNASSIGNED_PROJECT ||
    (selectedProject !== null && isChatDirectoryProject(selectedProject));

  const otherProjects = useMemo(
    () => projects.filter((p) => !isChatDirectoryProject(p)),
    [projects],
  );

  const projectDirs: string[] = useMemo(() => {
    if (isUnassignedProject) return defaultWorkspace ? [defaultWorkspace] : [];
    if (!selectedProject) return [];
    return selectedProject.search_paths?.length
      ? selectedProject.search_paths
      : selectedProject.representative_paths;
  }, [isUnassignedProject, selectedProject, defaultWorkspace]);

  const [selectedDirPath, setSelectedDirPath] = useState<string>("");
  const [dirGitStates, setDirGitStates] = useState<Record<string, boolean>>({});

  useEffect(() => {
    if (projectDirs.length > 0 && !projectDirs.includes(selectedDirPath)) {
      setSelectedDirPath(projectDirs[0]);
    }
  }, [projectDirs, selectedDirPath]);

  const probeTarget = isUnassignedProject ? defaultWorkspace :
    projectDirs.includes(selectedDirPath) ? selectedDirPath : projectDirs[0];
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

  // 未归属项目由后端解析当前默认工作目录，与任务关联保持独立。
  const launchCwd = isUnassignedProject
    ? undefined
    : projectDirs.includes(selectedDirPath) ? selectedDirPath : projectDirs[0];

  const prepareLaunch = useCallback(async (): Promise<PreparedLaunch | null> => {
    if (!selectedAgent || !agentAvailable) return null;
    const ownerId = ownerWorkstreamId === STANDALONE ? null : ownerWorkstreamId;
    if (launchRuntime) return api.prepareNewSession(selectedAgent, ownerId, launchCwd, launchRuntime);
    if (launchCwd) {
      return api.prepareNewSession(selectedAgent, ownerId, launchCwd);
    }
    return api.prepareNewSession(selectedAgent, ownerId);
  }, [selectedAgent, ownerWorkstreamId, launchCwd, launchRuntime, agentAvailable]);

  const { prepared, preparing, error, setError, prepare, release: releasePrepared } =
    usePreparedLaunch(prepareLaunch);

  const handleWsChange = (next: string) => {
    if (next === ownerWorkstreamId) return;
    releasePrepared();
    setOwnerWorkstreamId(next);
    if (next === STANDALONE) {
      setSelectedProjectId(UNASSIGNED_PROJECT);
    } else {
      const ws = workstreams.find((w) => w.id === next);
      if (ws?.projects.length === 1) {
        setSelectedProjectId(ws.projects[0].id);
      } else if (!ws?.projects.some((p) => p.id === selectedProjectId)) {
        setSelectedProjectId(UNASSIGNED_PROJECT);
      }
    }
  };

  const handleProjectChange = (next: string) => {
    if (next === selectedProjectId) return;
    releasePrepared();
    setSelectedProjectId(next);
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
        setSelectedProjectId(res.project_id);
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
    setRuntimeMode("inherit");
    setRuntimeOverride(NATIVE_RUNTIME);
    setSelectedAgent(next);
  };

  const canSend = Boolean(message.trim() && selectedAgent && agentAvailable && prepared && !preparing && !busy);

  const start = async () => {
    if (!canSend || launching.current) return;
    launching.current = true;
    agentChanged.current = true;
    releasePrepared();
    setBusy(true);
    setError("");
    try {
      const ownerId = ownerWorkstreamId === STANDALONE ? null : ownerWorkstreamId;
      const r = launchRuntime ? await api.launchEmbeddedNew(selectedAgent, ownerId, launchCwd, message, launchRuntime) : await api.launchEmbeddedNew(
        selectedAgent,
        ownerId,
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

  const selectedWorkstream = workstreams.find((w) => w.id === ownerWorkstreamId);
  const taskLabel =
    ownerWorkstreamId === STANDALONE
      ? "无任务"
      : (selectedWorkstream?.title ?? "无任务");

  const projectLabel = isUnassignedProject ? "无项目" : (selectedProject?.name ?? "无项目");

  const currentDir = isUnassignedProject ? defaultWorkspace : launchCwd || "";
  const dirLabel = useMemo(() => {
    if (!currentDir) return "默认工作目录";
    return ellipsisPathMiddle(currentDir, 42);
  }, [currentDir]);

  const isGitDir = Boolean(
    selectedProject?.kind === "git" ||
    (currentDir && dirGitStates[currentDir])
  );

  const agentLabel = AGENT_LABELS[selectedAgent] ?? selectedAgent;

  return (
    <main className="main new-session-page" aria-labelledby="new-session-title">
      <div className="new-session-content">
        <div className="new-session-heading">
          <div className="new-session-heading-title">
            <SidebarLogo size={36} />
            <h1 id="new-session-title">开启新会话</h1>
          </div>
          <p className="new-session-subtitle">选择目标任务与执行项目，立即启动 Agent 展开工作</p>
        </div>

        <form onSubmit={(e) => { e.preventDefault(); void start(); }}>
          <div className="new-session-context">
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
              aria-label={`所属任务：${taskLabel}`}
              title={taskLabel}
            >
              <Icon name={ownerWorkstreamId === STANDALONE ? "inbox" : "tasks"} />
              <span className="new-session-picker-label truncate">{taskLabel}</span>
              <Icon name="chevronDown" className="new-session-chevron" />
              <select
                aria-label="所属任务（可选）"
                title={taskLabel}
                value={ownerWorkstreamId}
                onChange={(e) => {
                  if (e.target.value === "__new_workstream__") {
                    setShowNewWorkstreamModal(true);
                    return;
                  }
                  handleWsChange(e.target.value);
                }}
                disabled={busy}
                tabIndex={-1}
              >
                <option value={STANDALONE}>无任务</option>
                {workstreams.map((w) => (
                  <option key={w.id} value={w.id}>{w.title}</option>
                ))}
                <option value="__new_workstream__">+ 新任务</option>
              </select>

              {taskMenuOpen && (
                <div className="new-session-dropdown-menu" role="menu" aria-label="所属任务列表">
                  <div className="new-session-menu-list">
                    <button
                      type="button"
                      className={`new-session-menu-item${ownerWorkstreamId === STANDALONE ? " selected" : ""}`}
                      onClick={() => {
                        handleWsChange(STANDALONE);
                        setTaskMenuOpen(false);
                      }}
                    >
                      <span className="menu-item-icon"><Icon name="inbox" /></span>
                      <span className="menu-item-label">无任务</span>
                      {ownerWorkstreamId === STANDALONE && (
                        <span className="menu-item-check"><Icon name="check" /></span>
                      )}
                    </button>
                    {workstreams.map((w) => {
                      const isSelected = ownerWorkstreamId === w.id;
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
              <Icon name={isUnassignedProject ? "inbox" : "project"} />
              <span className="new-session-picker-label truncate">{projectLabel}</span>
              <Icon name="chevronDown" className="new-session-chevron" />
              <select
                aria-label="所属项目"
                title={projectLabel}
                value={isUnassignedProject ? UNASSIGNED_PROJECT : selectedProjectId}
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
                <option value={UNASSIGNED_PROJECT}>无项目</option>
                {otherProjects.map((p) => (
                  <option key={p.id} value={p.id}>{p.name}</option>
                ))}
                <option value="__new_project__">+ 新项目</option>
              </select>

              {projectMenuOpen && (
                <div className="new-session-dropdown-menu" role="menu" aria-label="所属项目列表">
                  <div className="new-session-menu-list">
                    <button
                      type="button"
                      className={`new-session-menu-item${isUnassignedProject ? " selected" : ""}`}
                      onClick={() => {
                        handleProjectChange(UNASSIGNED_PROJECT);
                        setProjectMenuOpen(false);
                      }}
                      title="无项目"
                    >
                      <span className="menu-item-icon"><Icon name="inbox" /></span>
                      <span className="menu-item-label">无项目</span>
                      {isUnassignedProject && (
                        <span className="menu-item-check"><Icon name="check" /></span>
                      )}
                    </button>
                    {otherProjects.map((p) => {
                      const isSelected = !isUnassignedProject && selectedProjectId === p.id;
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
                  <Icon name={isGitDir ? "git" : "folder"} />
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
                          const isDirGit =
                            selectedProject?.kind === "git" ||
                            dirGitStates[dir] === true;
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
                                <Icon name={isDirGit ? "git" : "folder"} />
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

          <details className="new-session-runtime" open={runtimeOpen} onToggle={(e) => setRuntimeOpen(e.currentTarget.open)}>
            <summary>模型与推理设置 · {runtimeMode === "native" ? "Agent 默认" : runtimeMode === "custom" ? runtimeOverride.model ?? "单次设置" : "使用代理页设置"}</summary>
            <div className="row between">
              <label>本次启动参数 <select aria-label="本次启动参数" value={runtimeMode} disabled={busy} onChange={(e) => {
                releasePrepared();
                const mode = e.target.value as typeof runtimeMode;
                if (mode === "custom") setRuntimeOverride(runtime.settings?.overrides ?? NATIVE_RUNTIME);
                setRuntimeMode(mode);
              }}><option value="inherit">使用代理页设置</option><option value="native">使用 Agent 默认</option><option value="custom" disabled={!runtime.settings}>单次自定义</option></select></label>
              <button type="button" className="btn small" disabled={runtime.loading || busy} onClick={() => void runtime.refresh(true)}>{runtime.loading ? "获取中…" : "刷新模型列表"}</button>
            </div>
            {runtimeMode === "custom" && runtime.options && <RuntimeFields value={runtimeOverride} options={runtime.options} onDiscover={() => void runtime.refresh()} disabled={busy} onChange={(v) => { releasePrepared(); setRuntimeOverride(v); }} />}
            {runtime.options && <CatalogStatus options={runtime.options} />}
            {runtime.error && <p className="muted small">{runtime.error} · 可使用默认参数继续启动。</p>}
            <p className="muted small">仅影响这次新建会话，不修改代理页设置。</p>
          </details>
          {!agentAvailable && <p role="alert">没有启用的新建会话代理，请前往代理设置启用。</p>}
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
              <label className="new-session-picker new-session-agent">
                <AgentIcon agent={selectedAgent} size={18} />
                <span className="new-session-picker-label truncate">{agentLabel}</span>
                <Icon name="chevronDown" />
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

          {prepared?.cwd_resolution?.fallback && (
            <p className="new-session-note" role="status">
              {prepared.cwd_resolution.note || "未从默认工作目录启动"}
            </p>
          )}
          {error && (
            <div className="new-session-error" role="alert">
              <span>{error}</span>
              {!preparing && !busy && (
                <button type="button" className="btn small" onClick={() => void prepare()}>重试</button>
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
            const cards = await api.listWorkstreamCards().catch(() => null);
            if (cards) {
              setWorkstreams(cards);
            } else {
              setWorkstreams((prev) => [
                {
                  id: newWs.id,
                  projects: [],
                  title: newWs.title,
                  description: newWs.description,
                  visibility: newWs.visibility,
                  created_at: newWs.created_at,
                  updated_at: newWs.updated_at,
                  summary: null,
                  last_activity_at: null,
                  session_count: 0,
                  latest_session: null,
                  path_count: 0,
                },
                ...prev,
              ]);
            }
            releasePrepared();
            setOwnerWorkstreamId(newWs.id);
            setShowNewWorkstreamModal(false);
          }}
          initialProjectId={!isUnassignedProject ? selectedProjectId : undefined}
        />
      )}
    </main>
  );
}
