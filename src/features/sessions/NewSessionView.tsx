import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import AgentIcon from "../../components/AgentIcon";
import Icon from "../../components/Icon";
import SidebarLogo from "../../components/SidebarLogo";
import type { Route } from "../../app/routes";
import { announceLaunch } from "../launcher/LaunchResultModal";
import { usePreparedLaunch } from "../launcher/usePreparedLaunch";
import {
  AGENT_LABELS,
  type Agent,
  type AgentStatusEntry,
  type PreparedLaunch,
  type RecentWorkspacePath,
  type WorkstreamCardData,
  type WorkstreamPathRow,
} from "../../types";

/** 新会话页：预览启动目录，发送首条消息时才创建内嵌终端。 */
export type NewSessionViewProps = {
  /** 预置选中的所属任务；省略或 "none" = standalone。 */
  workstreamId?: string | null;
  /** 单次指定 Agent，不修改全局默认设置。 */
  agent?: Agent | null;
  navigate: (r: Route) => void;
};

const STANDALONE = "none";

/** 支持终端 CLI 启动的 Agent 集合（Qoder、WorkBuddy、DSH、ZCode 等纯桌面/无 CLI Agent 不在此列） */
const CLI_AGENTS: Agent[] = ["codex", "claude_code", "pi", "antigravity"];

/** 下拉选项里路径的紧凑形态：末段才是识别信息，整条路径留给 title。 */
function pathTail(path: string): string {
  const segs = path.split(/[\\/]/).filter(Boolean);
  const tail = segs.slice(-2).join("/");
  return segs.length > 2 ? `…/${tail}` : tail;
}

function workstreamLabel(w: WorkstreamCardData): string {
  if (w.primary_path) return `${w.title} · ${pathTail(w.primary_path)}`;
  return `${w.title} · 无工作路径`;
}

export default function NewSessionView({
  workstreamId,
  agent: initialAgent,
  navigate,
}: NewSessionViewProps) {
  const [workstreams, setWorkstreams] = useState<WorkstreamCardData[]>([]);
  const [ownerWorkstreamId, setOwnerWorkstreamId] = useState(
    workstreamId && workstreamId !== STANDALONE ? workstreamId : STANDALONE
  );
  const [agentStatus, setAgentStatus] = useState<Record<string, AgentStatusEntry> | null>(null);
  const [selectedAgent, setSelectedAgent] = useState<Agent>(() => {
    if (initialAgent && CLI_AGENTS.includes(initialAgent)) {
      return initialAgent;
    }
    return "codex";
  });
  const [defaultWorkspace, setDefaultWorkspace] = useState<string>("");
  const [recentPaths, setRecentPaths] = useState<RecentWorkspacePath[]>([]);
  const [taskPaths, setTaskPaths] = useState<WorkstreamPathRow[]>([]);
  const [selectedCwd, setSelectedCwd] = useState<string>("");
  const [customPaths, setCustomPaths] = useState<string[]>([]);
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
        if (!cancelled) setWorkstreams(ws.filter((w) => w.visibility === "normal"));
      })
      .catch(console.error);

    api
      .getAgentStatus()
      .then((status) => { if (!cancelled) setAgentStatus(status); })
      .catch(console.error);

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
  }, [initialAgent]);

  useEffect(() => {
    let cancelled = false;
    api
      .getWorkspaceSettings?.()
      ?.then((s) => {
        if (!cancelled && s?.default_workspace) {
          setDefaultWorkspace(s.default_workspace);
        }
      })
      ?.catch(console.error);

    api
      .listRecentWorkspacePaths?.()
      ?.then((rows) => {
        if (!cancelled && rows) {
          setRecentPaths(rows);
        }
      })
      ?.catch(console.error);

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (ownerWorkstreamId === STANDALONE) {
      setTaskPaths([]);
      return;
    }
    let cancelled = false;
    api
      .listWorkstreamPaths?.(ownerWorkstreamId)
      ?.then((rows) => {
        if (!cancelled && rows) {
          setTaskPaths(rows);
        }
      })
      ?.catch(console.error);

    return () => {
      cancelled = true;
    };
  }, [ownerWorkstreamId]);

  const cliAgents = agentStatus
    ? (Object.keys(AGENT_LABELS) as Agent[]).filter(
        (a) => agentStatus[a]?.terminal_cli
      )
    : CLI_AGENTS;

  const isTask = ownerWorkstreamId !== STANDALONE;

  // 默认工作目录：
  // 1. 若来自任务：如果有工作路径，默认为主目录（第 0 条）；若无目录，默认为 NoEnding 默认工作区
  // 2. 若直接开始（无任务）：默认为 NoEnding 默认工作区
  const defaultCwd = useMemo(() => {
    if (isTask) {
      if (taskPaths.length > 0) {
        return taskPaths[0].canonical_path;
      }
      return defaultWorkspace;
    }
    return defaultWorkspace;
  }, [isTask, taskPaths, defaultWorkspace]);

  // 可选工作目录列表：
  // 1. 若来自任务：为当前任务下的工作目录列表，没有目录时为 NoEnding 默认工作区
  // 2. 若无任务：从已有工作目录进行选择（NoEnding 默认工作区置顶，配合最近/已知目录）
  const cwdOptions = useMemo<{ value: string; label: string }[]>(() => {
    if (isTask) {
      if (taskPaths.length > 0) {
        return taskPaths.map((p, idx) => ({
          value: p.canonical_path,
          label: idx === 0 ? `${p.canonical_path} (主目录)` : p.canonical_path,
        }));
      }
      if (defaultWorkspace) {
        return [
          {
            value: defaultWorkspace,
            label: `${defaultWorkspace} (NoEnding 默认工作区)`,
          },
        ];
      }
      return [];
    }

    const opts: { value: string; label: string }[] = [];
    if (defaultWorkspace) {
      opts.push({
        value: defaultWorkspace,
        label: `${defaultWorkspace} (NoEnding 默认工作区)`,
      });
    }
    for (const p of customPaths) {
      if (p !== defaultWorkspace && !opts.some((o) => o.value === p)) {
        opts.push({ value: p, label: p });
      }
    }
    for (const r of recentPaths) {
      if (r.path && r.path !== defaultWorkspace && !opts.some((o) => o.value === r.path)) {
        opts.push({
          value: r.path,
          label: r.project_name ? `${r.path} (${r.project_name})` : r.path,
        });
      }
    }
    return opts;
  }, [isTask, taskPaths, defaultWorkspace, customPaths, recentPaths]);

  const effectiveCwd =
    selectedCwd && cwdOptions.some((o) => o.value === selectedCwd)
      ? selectedCwd
      : defaultCwd;

  const launchCwd = selectedCwd && selectedCwd !== defaultCwd && cwdOptions.some((o) => o.value === selectedCwd)
    ? selectedCwd
    : undefined;

  const prepareLaunch = useCallback(async (): Promise<PreparedLaunch | null> => {
    if (!selectedAgent) return null;
    const ownerId = ownerWorkstreamId === STANDALONE ? null : ownerWorkstreamId;
    if (launchCwd) {
      return api.prepareNewSession(selectedAgent, ownerId, launchCwd);
    }
    return api.prepareNewSession(selectedAgent, ownerId);
  }, [selectedAgent, ownerWorkstreamId, launchCwd, selectedCwd, defaultCwd]);

  const { prepared, preparing, error, setError, prepare, release: releasePrepared } =
    usePreparedLaunch(prepareLaunch);
  const displayedCwd = prepared?.cwd ?? effectiveCwd;

  const handleWsChange = (next: string) => {
    if (next === ownerWorkstreamId) return;
    releasePrepared();
    setTaskPaths([]);
    setOwnerWorkstreamId(next);
    setSelectedCwd("");
  };

  const handleAgentChange = (next: Agent) => {
    agentChanged.current = true;
    if (next === selectedAgent) return;
    releasePrepared();
    setSelectedAgent(next);
  };

  const handleCwdChange = async (val: string) => {
    if (val === "__BROWSE__") {
      try {
        const picked = await open({ directory: true, multiple: false, title: "选择工作目录" });
        if (typeof picked === "string" && picked.trim() !== "") {
          const trimmed = picked.trim();
          if (trimmed === selectedCwd) return;
          setCustomPaths((prev) => [trimmed, ...prev.filter((p) => p !== trimmed)]);
          releasePrepared();
          setSelectedCwd(trimmed);
        }
      } catch (e) {
        console.error("浏览目录失败:", e);
      }
      return;
    }
    if (val === selectedCwd) return;
    releasePrepared();
    setSelectedCwd(val);
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

  return (
    <main className="main new-session-page" aria-labelledby="new-session-title">
      <div className="new-session-content">
        <div className="new-session-heading">
          <SidebarLogo size={34} />
          <h1 id="new-session-title">开启新会话</h1>
        </div>

        <form onSubmit={(e) => { e.preventDefault(); void start(); }}>
          <div className="new-session-context">
            <label className="new-session-picker new-session-task">
              <Icon name="tasks" />
              <select
                aria-label="所属任务（可选）"
                title="所属任务（可选）"
                value={ownerWorkstreamId}
                onChange={(e) => handleWsChange(e.target.value)}
                disabled={busy}
              >
                <option value={STANDALONE}>无所属任务</option>
                {workstreams.map((w) => (
                  <option key={w.id} value={w.id}>{workstreamLabel(w)}</option>
                ))}
              </select>
            </label>

            <label className="new-session-picker new-session-path">
              <Icon name="folder" />
              <select
                aria-label="工作路径"
                title={displayedCwd || "工作路径"}
                value={displayedCwd}
                onChange={(e) => void handleCwdChange(e.target.value)}
                disabled={busy}
              >
                {cwdOptions.length === 0 && !displayedCwd && (
                  <option value="">{preparing ? "正在准备工作路径…" : "未设置工作路径"}</option>
                )}
                {displayedCwd && !cwdOptions.some((o) => o.value === displayedCwd) && (
                  <option value={displayedCwd}>{displayedCwd} (本次工作路径)</option>
                )}
                {cwdOptions.map((opt) => (
                  <option key={opt.value} value={opt.value}>{opt.label}</option>
                ))}
                {!isTask && <option value="__BROWSE__">浏览其他目录…</option>}
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
                <select
                  aria-label="Agent"
                  title="Agent"
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
              {prepared.cwd_resolution.note || "本次没有从这条流程通常的目录启动"}
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
