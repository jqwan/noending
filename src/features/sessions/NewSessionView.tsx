import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
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
  type PreparedLaunch,
  type ProjectCardData,
  type WorkstreamCardData,
} from "../../types";

/** 新会话页：预览启动目录，发送首条消息时才创建内嵌终端。 */
export type NewSessionViewProps = {
  /** 预置选中的所属任务；省略或 "none" = standalone。 */
  workstreamId?: string | null;
  /** 单次指定 Agent，不修改全局默认设置。 */
  agent?: Agent | null;
  /** 预置选中的项目 ID；省略或 "none" = 默认项目 NoEnding Workspace。 */
  projectId?: string | null;
  navigate: (r: Route) => void;
};

const STANDALONE = "none";
const DEFAULT_PROJECT_ID = "default";

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
    initialProjectId && initialProjectId !== "none" ? initialProjectId : DEFAULT_PROJECT_ID
  );
  const [agentStatus, setAgentStatus] = useState<Record<string, AgentStatusEntry> | null>(null);
  const [selectedAgent, setSelectedAgent] = useState<Agent>(() => {
    if (initialAgent && CLI_AGENTS.includes(initialAgent)) {
      return initialAgent;
    }
    return "codex";
  });
  const [defaultWorkspace, setDefaultWorkspace] = useState<string>("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const mounted = useRef(true);
  const launching = useRef(false);
  const agentChanged = useRef(false);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    let cancelled = false;
    api
      .listWorkstreamCards()
      .then((ws) => {
        if (!cancelled) {
          const normal = ws.filter((w) => w.visibility === "normal");
          setWorkstreams(normal);
          if ((!initialProjectId || initialProjectId === "none" || initialProjectId === DEFAULT_PROJECT_ID) && workstreamId && workstreamId !== STANDALONE) {
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
        (a) => agentStatus[a]?.terminal_cli
      )
    : CLI_AGENTS;

  const defaultProject = useMemo(() => {
    return (
      projects.find(
        (p) =>
          p.name === "NoEnding Workspace" ||
          (defaultWorkspace && p.search_paths?.includes(defaultWorkspace))
      ) ?? null
    );
  }, [projects, defaultWorkspace]);

  const defaultProjectId = defaultProject?.id ?? DEFAULT_PROJECT_ID;

  const isDefaultProject =
    selectedProjectId === DEFAULT_PROJECT_ID ||
    selectedProjectId === "none" ||
    (defaultProject !== null && selectedProjectId === defaultProject.id);

  const selectedProject = useMemo(() => {
    if (isDefaultProject) {
      if (defaultProject) return defaultProject;
      return {
        id: defaultProjectId,
        name: "NoEnding Workspace",
        name_customized: false,
        kind: "chat_directory",
        path_count: defaultWorkspace ? 1 : 0,
        missing_path_count: 0,
        workstream_count: 0,
        session_count: 0,
        representative_paths: defaultWorkspace ? [defaultWorkspace] : [],
        search_paths: defaultWorkspace ? [defaultWorkspace] : [],
        last_activity_at: null,
        updated_at: "",
      } as ProjectCardData;
    }
    return projects.find((p) => p.id === selectedProjectId) ?? null;
  }, [isDefaultProject, defaultProject, defaultProjectId, defaultWorkspace, projects, selectedProjectId]);

  const otherProjects = useMemo(() => {
    return projects.filter(
      (p) => p.id !== defaultProjectId && p.name !== "NoEnding Workspace"
    );
  }, [projects, defaultProjectId]);

  const projectDirs: string[] = useMemo(() => {
    if (!selectedProject) return defaultWorkspace ? [defaultWorkspace] : [];
    const paths =
      selectedProject.search_paths && selectedProject.search_paths.length > 0
        ? selectedProject.search_paths
        : selectedProject.representative_paths;
    if (paths && paths.length > 0) return paths;
    return defaultWorkspace ? [defaultWorkspace] : [];
  }, [selectedProject, defaultWorkspace]);

  const [selectedDirPath, setSelectedDirPath] = useState<string>("");
  const [dirGitStates, setDirGitStates] = useState<Record<string, boolean>>({});

  useEffect(() => {
    if (projectDirs.length > 0 && !projectDirs.includes(selectedDirPath)) {
      setSelectedDirPath(projectDirs[0]);
    }
  }, [projectDirs, selectedDirPath]);

  useEffect(() => {
    const target = selectedDirPath || projectDirs[0] || defaultWorkspace;
    if (!target || dirGitStates[target] !== undefined) return;
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
  }, [selectedDirPath, projectDirs, defaultWorkspace, dirGitStates]);

  // 新建会话的工作路径：由选中的项目目录决定；若为 NoEnding Workspace 默认项目且为独立会话，传 undefined 走后端默认解析
  const launchCwd = useMemo(() => {
    if (isDefaultProject) {
      if (ownerWorkstreamId !== STANDALONE) {
        return selectedDirPath || defaultWorkspace || undefined;
      }
      if (selectedDirPath && defaultWorkspace && selectedDirPath !== defaultWorkspace) {
        return selectedDirPath;
      }
      return undefined;
    }
    return selectedDirPath || projectDirs[0] || undefined;
  }, [isDefaultProject, ownerWorkstreamId, selectedDirPath, defaultWorkspace, projectDirs]);

  const prepareLaunch = useCallback(async (): Promise<PreparedLaunch | null> => {
    if (!selectedAgent) return null;
    const ownerId = ownerWorkstreamId === STANDALONE ? null : ownerWorkstreamId;
    if (launchCwd) {
      return api.prepareNewSession(selectedAgent, ownerId, launchCwd);
    }
    return api.prepareNewSession(selectedAgent, ownerId);
  }, [selectedAgent, ownerWorkstreamId, launchCwd]);

  const { prepared, preparing, error, setError, prepare, release: releasePrepared } =
    usePreparedLaunch(prepareLaunch);

  const handleWsChange = (next: string) => {
    if (next === ownerWorkstreamId) return;
    releasePrepared();
    setOwnerWorkstreamId(next);
    if (next === STANDALONE) {
      setSelectedProjectId(defaultProjectId);
    } else {
      const ws = workstreams.find((w) => w.id === next);
      if (ws?.projects.length === 1) {
        setSelectedProjectId(ws.projects[0].id);
      } else if (!ws?.projects.some((p) => p.id === selectedProjectId)) {
        setSelectedProjectId(defaultProjectId);
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
    setSelectedAgent(next);
  };

  const canSend = Boolean(message.trim() && selectedAgent && prepared && !preparing && !busy);

  const start = async () => {
    if (!canSend || launching.current) return;
    launching.current = true;
    agentChanged.current = true;
    releasePrepared();
    setBusy(true);
    setError("");
    try {
      const ownerId = ownerWorkstreamId === STANDALONE ? null : ownerWorkstreamId;
      const r = await api.launchEmbeddedNew(
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
      ? "无所属任务"
      : (selectedWorkstream?.title ?? "无所属任务");

  const projectLabel = isDefaultProject
    ? "NoEnding Workspace"
    : (selectedProject?.name ?? "NoEnding Workspace");

  const currentDir = selectedDirPath || projectDirs[0] || "";
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
            <label className="new-session-picker new-session-task">
              <Icon name="tasks" />
              <span className="new-session-picker-label truncate">{taskLabel}</span>
              <Icon name="chevronDown" />
              <select
                aria-label="所属任务（可选）"
                title={taskLabel}
                value={ownerWorkstreamId}
                onChange={(e) => handleWsChange(e.target.value)}
                disabled={busy}
              >
                <option value={STANDALONE}>无所属任务</option>
                {workstreams.map((w) => (
                  <option key={w.id} value={w.id}>{w.title}</option>
                ))}
              </select>
            </label>

            <label className="new-session-picker new-session-project">
              <Icon name="folder" />
              <span className="new-session-picker-label truncate">{projectLabel}</span>
              <Icon name="chevronDown" />
              <select
                aria-label="所属项目"
                title={projectLabel}
                value={isDefaultProject ? defaultProjectId : selectedProjectId}
                onChange={(e) => {
                  if (e.target.value === "__new_project__") {
                    void handleAddNewProject();
                    return;
                  }
                  handleProjectChange(e.target.value);
                }}
                disabled={busy}
              >
                <option value={defaultProjectId}>NoEnding Workspace</option>
                {otherProjects.map((p) => (
                  <option key={p.id} value={p.id}>{p.name}</option>
                ))}
                <option value="__new_project__">+ 新项目</option>
              </select>
            </label>

            <label className="new-session-picker new-session-dir">
              <Icon name={isGitDir ? "git" : "folder"} />
              <span className="new-session-picker-label truncate" title={currentDir || dirLabel}>
                {dirLabel}
              </span>
              <Icon name="chevronDown" />
              <select
                aria-label="项目目录"
                title={currentDir || dirLabel}
                value={selectedDirPath}
                onChange={(e) => handleDirChange(e.target.value)}
                disabled={busy || projectDirs.length <= 1}
              >
                {projectDirs.map((dir) => (
                  <option key={dir} value={dir}>
                    {dir}
                  </option>
                ))}
              </select>
            </label>
          </div>

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
    </main>
  );
}
