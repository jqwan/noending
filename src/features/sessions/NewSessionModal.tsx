import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import { Modal } from "../../components/common";
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

/**
 * 全局新建 Session：确认即内嵌直启（`launchEmbeddedNew`，prepare + 起一个未绑定
 * 会话的内嵌终端一步完成），成功后带到独立终端视图；会话文件落盘被摄入发现后，
 * LaunchIntent 匹配把终端绑定到会话，那个视图就地变成会话的终端子页。
 * Modal 一打开仍先 Prepare——cwd 三级解析的预览事实（显示的就是这次启动真正
 * 使用的目录）由它提供。
 *
 * `workstreamId` 是可选预置入参：省略或 `"none"` 即 standalone（0 个所属任务完全合法）。
 * 预置后用户仍可改。下拉读 Workstream 卡片投影：标题相同的靠主路径才能分清，而启动
 * 目录恰由主路径决定，所以选项里必须能看到它。
 */
export type NewSessionModalProps = {
  onClose: () => void;
  /** 预置选中的所属任务；省略或 "none" = standalone。 */
  workstreamId?: string | null;
  /** 预置选中的 Agent；若指定则优先使用。未指定时读取全局默认或首个可用 CLI Agent。单次指定绝不修改全局设置。 */
  agent?: Agent | null;
  /** 启动成功后带到独立终端视图（内嵌新建的唯一落点）。 */
  navigate?: (r: import("../../app/routes").Route) => void;
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

export default function NewSessionModal({
  onClose,
  workstreamId,
  agent: initialAgent,
  navigate,
}: NewSessionModalProps) {
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
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api
      .listWorkstreamCards()
      .then((ws) =>
        setWorkstreams(ws.filter((w) => w.visibility === "normal"))
      )
      .catch(console.error);

    api
      .getAgentStatus()
      .then(setAgentStatus)
      .catch(console.error);

    if (!initialAgent) {
      api
        .getDefaultAgent()
        .then((def) => {
          if (def && CLI_AGENTS.includes(def)) {
            setSelectedAgent(def);
          }
        })
        .catch(console.error);
    }
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

  const prepareLaunch = useCallback(async (): Promise<PreparedLaunch | null> => {
    if (!selectedAgent) return null;
    const isExplicit = Boolean(selectedCwd && selectedCwd !== defaultCwd);
    const ownerId = ownerWorkstreamId === STANDALONE ? null : ownerWorkstreamId;
    if (isExplicit) {
      return api.prepareNewSession(selectedAgent, ownerId, selectedCwd);
    }
    return api.prepareNewSession(selectedAgent, ownerId);
  }, [selectedAgent, ownerWorkstreamId, selectedCwd, defaultCwd]);

  const { prepared, preparing, error, setError, prepare, release: releasePrepared } =
    usePreparedLaunch(prepareLaunch);

  const handleWsChange = (next: string) => {
    releasePrepared();
    setOwnerWorkstreamId(next);
    setSelectedCwd("");
  };

  const handleAgentChange = (next: Agent) => {
    releasePrepared();
    setSelectedAgent(next);
  };

  const handleCwdChange = async (val: string) => {
    if (val === "__BROWSE__") {
      try {
        const picked = await open({ directory: true, multiple: false, title: "选择工作目录" });
        if (typeof picked === "string" && picked.trim() !== "") {
          const trimmed = picked.trim();
          setCustomPaths((prev) => [trimmed, ...prev.filter((p) => p !== trimmed)]);
          releasePrepared();
          setSelectedCwd(trimmed);
        }
      } catch (e) {
        console.error("浏览目录失败:", e);
      }
      return;
    }
    releasePrepared();
    setSelectedCwd(val);
  };

  const handleClose = () => {
    releasePrepared();
    onClose();
  };

  /** 唯一启动路径：内嵌直启（prepare 预览的 cwd/任务事实原样传给
   *  launch_embedded_new，它内部重新 prepare + 强制 embedded）。返回的
   *  terminal_id 把用户带到独立终端视图——会话被发现后那里就地变成会话。 */
  const start = async () => {
    if (busy) return;
    releasePrepared(true);
    setBusy(true);
    setError("");
    try {
      const ownerId = ownerWorkstreamId === STANDALONE ? null : ownerWorkstreamId;
      const r = await api.launchEmbeddedNew(
        selectedAgent,
        ownerId,
        selectedCwd || undefined,
      );
      announceLaunch("启动", r);
      if (r.terminal_id && navigate) {
        navigate({ view: "terminal", terminalId: r.terminal_id });
      }
      onClose();
    } catch (e: unknown) {
      setBusy(false);
      setError(`启动失败：${String(e)}`);
    }
  };

  return (
    <Modal title="新建会话" onClose={handleClose}>
      <label className="field">
        <span>所属任务（可选）</span>
        <select value={ownerWorkstreamId} onChange={(e) => handleWsChange(e.target.value)}>
          <option value={STANDALONE}>无（直接开始）</option>
          {workstreams.map((w) => (
            <option key={w.id} value={w.id}>
              {workstreamLabel(w)}
            </option>
          ))}
        </select>
      </label>

      <label className="field">
        <span>Agent</span>
        <select
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

      <label className="field">
        <span>工作目录</span>
        <select
          value={effectiveCwd}
          onChange={(e) => void handleCwdChange(e.target.value)}
          disabled={busy}
        >
          {cwdOptions.length === 0 && (
            <option value="">{preparing ? "正在准备工作目录…" : "未设置工作目录"}</option>
          )}
          {cwdOptions.map((opt) => (
            <option key={opt.value} value={opt.value}>
              {opt.label}
            </option>
          ))}
          {!isTask && <option value="__BROWSE__">浏览其他目录…</option>}
        </select>
      </label>


      {prepared?.cwd_resolution?.fallback ? (
        <div
          className="settings-row-hint"
          style={{ color: "var(--warning)", marginTop: -4, marginBottom: 8 }}
        >
          {prepared.cwd_resolution.note || "本次没有从这条流程通常的目录启动"}
        </div>
      ) : null}

      {error && (
        <div
          className="badge warn"
          style={{ marginTop: 8, display: "flex", gap: 8, alignItems: "center" }}
        >
          <span style={{ flex: 1, overflowWrap: "anywhere" }}>{error}</span>
          {!preparing && !busy && (
            <button
              type="button"
              className="btn small"
              onClick={() => {
                releasePrepared();
                void prepare();
              }}
            >
              重试
            </button>
          )}
        </div>
      )}

      <div className="row" style={{ justifyContent: "flex-end", marginTop: 14 }}>
        <button className="btn" onClick={handleClose} disabled={busy}>
          取消
        </button>
        <button
          className="btn primary"
          disabled={busy || preparing || !selectedAgent || !prepared}
          onClick={start}
        >
          {busy ? "启动中…" : preparing ? "准备中…" : "启动"}
        </button>
      </div>
    </Modal>
  );
}
