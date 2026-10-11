import { useCallback, useEffect, useState } from "react";
import { useViewState } from "../../hooks/useViewState";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import { onEvent, EVT_SYNCED, type Route } from "../../app/routes";
import AgentConfiguration from "./AgentConfiguration";
import AgentIcon from "../../components/AgentIcon";
import Icon from "../../components/Icon";
import PageHeader from "../../layout/PageHeader";
import { copyToClipboard, Modal, openPath, timeAgo } from "../../components/common";
import { showToast } from "../../components/Toast";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  AGENT_LABELS,
  type Agent,
  type AgentStatusEntry,
  type IngestSource,
  type IngestTaskStatus,
} from "../../types";

const ALL_AGENTS = Object.keys(AGENT_LABELS) as Agent[];

type OpenMethod = "terminal" | "desktop";

type FilterTab = "all" | "tui" | "desktop";

interface SessionFormat {
  id: string;
  label: string;
  agent: Agent;
  defaultPath?: string;
  methods: OpenMethod[];
}

const SESSION_FORMATS: SessionFormat[] = [
  { id: "codex", label: "Codex 会话", agent: "codex", defaultPath: "~/.codex/sessions", methods: ["desktop"] },
  { id: "claude_code", label: "Claude Code 会话", agent: "claude_code", defaultPath: "~/.claude/projects", methods: ["terminal"] },
  { id: "pi", label: "Pi 会话", agent: "pi", defaultPath: "~/.pi/agent/sessions", methods: ["terminal"] },
  { id: "dsh", label: "DSH 会话", agent: "dsh", defaultPath: "~/.dsh", methods: ["desktop"] },
  { id: "qoder", label: "Qoder 会话", agent: "qoder", defaultPath: "~/.qoder-cn", methods: ["desktop"] },
  { id: "workbuddy", label: "WorkBuddy 会话", agent: "workbuddy", defaultPath: "~/.workbuddy", methods: ["desktop"] },
  { id: "zcode", label: "ZCode 会话", agent: "zcode", defaultPath: "~/.zcode", methods: ["desktop"] },
  { id: "antigravity_desktop", label: "Desktop 格式", agent: "antigravity", defaultPath: "~/.gemini/antigravity", methods: ["desktop"] },
  { id: "antigravity_cli", label: "CLI 格式", agent: "antigravity", defaultPath: "~/.gemini/antigravity-cli", methods: ["terminal"] },
];

function formatOf(src: IngestSource): string {
  if (src.agent !== "antigravity") return src.agent;
  return src.path.includes("antigravity-cli") ? "antigravity_cli" : "antigravity_desktop";
}

export function extractVersion(raw?: string | null): string {
  if (!raw) return "";
  const match = raw.match(/\b\d+(\.\d+)+(-[a-zA-Z0-9.]+)?\b/);
  return match ? match[0] : raw.trim();
}

export function formatCliStatus(agent: Agent, entry?: AgentStatusEntry): string {
  if (!entry?.terminal_cli) {
    return "无终端 CLI";
  }
  if (!entry?.detected) {
    return "未检测到 TUI CLI";
  }
  const ver = extractVersion(entry.version);
  if (!ver) return `${AGENT_LABELS[agent]} CLI 已就绪`;
  return `${AGENT_LABELS[agent]} CLI ${ver.startsWith("v") ? ver : `v${ver}`}`;
}

/** 任务是否正在后台运行（started_at 不为空且 finished_at 为空） */
function isTaskRunning(s: IngestTaskStatus | null): boolean {
  return Boolean(s && s.started_at && !s.finished_at);
}

/** 作用域中文说明 */
function scopeLabel(scope: string, sources: IngestSource[]): string {
  if (scope === "reconcile_all") return "增量扫描全部来源";
  const path = (prefix: string) =>
    sources.find((s) => scope === `${prefix}${s.id}`)?.path;
  if (scope.startsWith("reconcile_source:")) return `增量扫描 ${path("reconcile_source:") ?? "单个来源"}`;
  if (scope.startsWith("reingest_source:")) return `全量重扫 ${path("reingest_source:") ?? "单个来源"}`;
  if (scope.startsWith("refresh_session:")) return "刷新单个会话";
  return scope;
}

function SourceRow({
  src,
  running,
  onScan,
  onReingest,
  onRemove,
  onCopy,
  onToggle,
}: {
  src: IngestSource;
  running: boolean;
  onScan: (id: string) => void;
  onReingest: (src: IngestSource) => void;
  onRemove: (src: IngestSource) => void;
  onCopy: (path: string) => void;
  onToggle: (src: IngestSource, enabled: boolean) => void;
}) {
  return (
    <div className="agent-source-row">
      <div className="agent-source-main">
        <span
          role="button"
          tabIndex={0}
          className="mono small agent-source-path path-link path-text-with-icon"
          style={{
            userSelect: "all",
            color: "var(--text-primary)",
          }}
          title={src.path}
          onClick={() => void openPath(src.path)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              void openPath(src.path);
            }
          }}
        >
          <Icon name="folder" />
          <span>{src.path}</span>
        </span>
        <button
          type="button"
          className="btn ghost icon-button small"
          onClick={(e) => {
            e.stopPropagation();
            void onCopy(src.path);
          }}
          aria-label="复制完整路径"
          title="复制完整路径"
        >
          <Icon name="copy" />
        </button>
        <button
          type="button"
          className="btn small ghost icon-button"
          disabled={running}
          onClick={() => onScan(src.id)}
          aria-label="增量同步"
          title="增量同步"
        >
          <Icon name="refresh" />
        </button>
        {src.origin === "user" && (
          <button
            type="button"
            className="btn small ghost icon-button"
            disabled={running}
            onClick={() => onRemove(src)}
            aria-label="移除此自定义目录"
            title="移除目录"
          >
            <Icon name="trash" />
          </button>
        )}
        {src.origin === "default" && (
          <span className="badge" title="按当前 Agent 默认存储目录登记">
            默认
          </span>
        )}
        {!src.exists && (
          <span className="badge warn" title="目录不存在">
            目录不存在
          </span>
        )}
      </div>

      <label className="agent-toggle"><input type="checkbox" aria-label={`自动同步 ${src.path}`} checked={src.enabled} disabled={running} onChange={(e) => onToggle(src, e.target.checked)} />自动同步历史</label>
      <button
        type="button"
        className="btn small ghost agent-reingest-btn"
        disabled={running}
        onClick={() => onReingest(src)}
        title="全量同步"
      >
        全量同步
      </button>
    </div>
  );
}

export default function AgentsView({
  navigate,
  initialAgent,
}: {
  navigate: (r: Route) => void;
  initialAgent?: Agent;
}) {
  const [agentStatus, setAgentStatus] = useState<Record<string, AgentStatusEntry> | null>(null);
  const [sources, setSources] = useState<IngestSource[]>([]);
  const [status, setStatus] = useState<IngestTaskStatus | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState("");
  const [reingestTarget, setReingestTarget] = useState<IngestSource | null>(null);
  const [filterTab, setFilterTab] = useState<FilterTab>("all");
  const [searchQuery, setSearchQuery] = useState("");
  const [viewMode, setViewMode] = useViewState<"cards" | "list">("agents.viewMode", "cards");
  const [sessionCounts, setSessionCounts] = useState<Record<string, number>>({});

  const clearMessages = () => setError("");

  const reloadStatus = useCallback(async () => {
    try {
      const s = await api.getIngestionStatus();
      setStatus(s);
      setRunning(isTaskRunning(s));
    } catch (e) {
      console.error(e);
    }
  }, []);

  const reloadSources = useCallback(async () => {
    try {
      setSources(await api.listIngestSources());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const reloadAgents = useCallback(async () => {
    try {
      setAgentStatus(await api.getAgentStatus());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const reloadSessions = useCallback(async () => {
    try {
      const list = await api.listSessions();
      const counts: Record<string, number> = {};
      for (const s of list) {
        counts[s.agent] = (counts[s.agent] ?? 0) + 1;
      }
      setSessionCounts(counts);
    } catch (e) {
      console.error(e);
    }
  }, []);

  const reloadAll = useCallback(async () => {
    await Promise.all([reloadAgents(), reloadSources(), reloadStatus(), reloadSessions()]);
  }, [reloadAgents, reloadSources, reloadStatus, reloadSessions]);

  useEffect(() => {
    void reloadAll();
  }, [reloadAll]);

  // 后台摄入事件监听：任务完成时重查最新摄入状态，若队列中已无 running 任务才解除忙碌
  useEffect(() => {
    const unlisten = listen("ingestion-completed", () => {
      void reloadSources();
      void reloadStatus();
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [reloadSources, reloadStatus]);

  useEffect(() => onEvent(EVT_SYNCED, reloadAll), [reloadAll]);

  const copyPath = async (p: string) => {
    const ok = await copyToClipboard(p);
    showToast(ok ? "已复制" : "复制失败");
  };

  const handleRemoveSource = async (src: IngestSource) => {
    clearMessages();
    try {
      await api.removeIngestSource(src.id);
      showToast("已移除来源目录");
      await reloadSources();
    } catch (e) {
      setError(String(e));
      showToast(`移除失败：${String(e)}`);
    }
  };

  const handleScanSource = async (srcId: string) => {
    clearMessages();
    setRunning(true);
    try {
      await api.reconcileSource(srcId);
      showToast("正在增量同步…");
      await reloadStatus();
    } catch (e) {
      setRunning(false);
      setError(String(e));
    }
  };

  const handleConfirmReingest = async () => {
    const target = reingestTarget;
    setReingestTarget(null);
    if (!target) return;
    clearMessages();
    setRunning(true);
    try {
      await api.reingestSource(target.id);
      showToast("正在全量同步…");
      await reloadStatus();
    } catch (e) {
      setRunning(false);
      setError(String(e));
    }
  };

  const handleScanAgent = async (agent: Agent) => {
    clearMessages();
    const agentSources = sources.filter((s) => s.agent === agent);
    if (agentSources.length === 0) {
      showToast(`${AGENT_LABELS[agent]} 暂无来源目录`);
      return;
    }
    setRunning(true);
    try {
      for (const source of agentSources) {
        await api.reconcileSource(source.id);
      }
      showToast(`正在增量同步 ${AGENT_LABELS[agent]}…`);
      await reloadStatus();
    } catch (e) {
      setRunning(false);
      setError(String(e));
    }
  };

  const handleScanAll = async () => {
    clearMessages();
    setRunning(true);
    try {
      if (initialAgent) {
        for (const source of sources.filter((s) => s.agent === initialAgent)) await api.reconcileSource(source.id);
      } else await api.reconcileAll();
      showToast("正在增量同步…");
      await reloadStatus();
    } catch (e) {
      setRunning(false);
      setError(String(e));
    }
  };

  const handlePickAndAddSource = async (agent: Agent, defaultPath?: string) => {
    clearMessages();
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: `选择 ${AGENT_LABELS[agent]} 会话来源目录`,
        defaultPath: defaultPath || undefined,
      });
      if (!picked) return;
      const targetPath = typeof picked === "string" ? picked.trim() : picked;
      if (!targetPath) return;

      await api.addIngestSource(agent, targetPath);
      showToast("已添加来源目录");
      await reloadSources();
    } catch (e) {
      setError(String(e));
      showToast(`添加来源目录失败：${String(e)}`);
    }
  };

  const detectedCount = agentStatus
    ? ALL_AGENTS.filter((a) => agentStatus[a]?.detected || agentStatus[a]?.desktop_app_present).length
    : 0;

  const tuiCount = agentStatus
    ? ALL_AGENTS.filter((a) => Boolean(agentStatus[a]?.terminal_cli)).length
    : 0;
  const desktopCount = agentStatus
    ? ALL_AGENTS.filter((a) => Boolean(agentStatus[a]?.desktop_app)).length
    : 0;

  // 过滤展示的 Agent 列表
  const visibleAgents = ALL_AGENTS.filter((agent) => {
    const entry = agentStatus?.[agent];
    if (initialAgent) return agent === initialAgent;
    if (searchQuery.trim()) {
      const q = searchQuery.trim().toLowerCase();
      const matchName = AGENT_LABELS[agent].toLowerCase().includes(q);
      const matchKey = agent.toLowerCase().includes(q);
      if (!matchName && !matchKey) return false;
    }

    if (filterTab === "tui") {
      return Boolean(entry?.terminal_cli);
    }
    if (filterTab === "desktop") {
      return Boolean(entry?.desktop_app);
    }
    return true;
  });

  if (initialAgent) {
    const entry = agentStatus?.[initialAgent];
    const agentSources = sources.filter((s) => s.agent === initialAgent);
    const agentFormats = SESSION_FORMATS.filter((f) => f.agent === initialAgent);

    return (
      <div className="main agents-page agent-detail-page">
        <PageHeader
          title={
            <span className="session-title-with-icon" title={AGENT_LABELS[initialAgent]}>
              <AgentIcon agent={initialAgent} size={18} />
              <span className="session-title-text">{AGENT_LABELS[initialAgent]}</span>
            </span>
          }
          actions={
            <button
              className="btn ghost icon-button"
              disabled={running || agentSources.length === 0}
              onClick={handleScanAll}
              aria-label="增量同步全部"
              title="增量同步当前代理来源目录"
            >
              <Icon name="refresh" className={running ? "spinning" : ""} />
            </button>
          }
        />

        <div className="agents-content">
          {error && (
            <div className="badge warn" style={{ marginBottom: 14, overflowWrap: "anywhere", display: "block", padding: "8px 12px" }}>
              {error}
            </div>
          )}

          <AgentConfiguration
            key={initialAgent}
            agent={initialAgent}
            entry={entry}
            reload={reloadAgents}
            navigate={navigate}
            sourcesSection={
              <section className="card agent-settings-section agent-detail-sources">
                <h2>
                  <Icon name="folder" />
                  <span>历史会话来源</span>
                </h2>
                <p className="muted small" style={{ marginTop: 0 }}>
                  自动同步仅控制此目录的历史扫描。NoEnding 启动的终端仍会按 Agent 通知同步当前会话并绑定；手动同步不受此开关影响。
                </p>
                {agentFormats.map((fmt, fIdx) => {
                  const formatSources = agentSources.filter((s) => formatOf(s) === fmt.id);
                  const isMultiFormat = agentFormats.length > 1;

                  return (
                    <div
                      key={fmt.id}
                      style={{
                        paddingTop: isMultiFormat && fIdx > 0 ? 12 : 0,
                        borderTop: isMultiFormat && fIdx > 0 ? "1px solid var(--border-subtle)" : undefined,
                      }}
                    >
                      {/* 多格式 Agent（如 Antigravity）展示格式区分标题 */}
                      {isMultiFormat && (
                        <div
                          className="row between"
                          style={{
                            alignItems: "center",
                            gap: 10,
                            flexWrap: "wrap",
                            paddingBottom: 6,
                            marginBottom: 6,
                            borderBottom: "1px solid var(--border-subtle)",
                          }}
                        >
                          <span style={{ fontSize: 13, fontWeight: 600 }}>{fmt.label}</span>
                          <span className="badge" title="打开方式">
                            {fmt.methods[0] === "terminal" ? "TUI" : "Desktop"}
                            {fmt.methods[0] === "desktop" && entry != null && !entry.desktop_app_present ? " · 未安装" : ""}
                          </span>
                        </div>
                      )}

                      {/* 来源目录列表 */}
                      <div>
                        {formatSources.length === 0 ? (
                          <div className="muted small" style={{ padding: "8px 0" }}>
                            未配置监控来源目录。
                          </div>
                        ) : (
                          formatSources.map((src) => (
                            <SourceRow
                              key={src.id}
                              src={src}
                              running={running}
                              onScan={handleScanSource}
                              onReingest={setReingestTarget}
                              onRemove={handleRemoveSource}
                              onCopy={copyPath}
                              onToggle={(source, enabled) => {
                                void (async () => {
                                  try {
                                    await api.setIngestSourceEnabled(source.id, enabled);
                                    await reloadSources();
                                  } catch (e) {
                                    setError(String(e));
                                  }
                                })();
                              }}
                            />
                          ))
                        )}

                        <button
                          className="btn small ghost"
                          style={{ marginTop: 8 }}
                          disabled={running}
                          onClick={() => void handlePickAndAddSource(initialAgent, fmt.defaultPath)}
                        >
                          + 添加来源目录
                        </button>
                      </div>
                    </div>
                  );
                })}
              </section>
            }
          />
        </div>

        {/* 全量同步确认 Modal */}
        {reingestTarget && (
          <Modal title="全量同步" onClose={() => setReingestTarget(null)}>
            <p className="small" style={{ marginTop: 0 }}>
              将从头重新同步 <span className="mono">{reingestTarget.path}</span> 的全部会话文件。
            </p>
            <p className="small" style={{ color: "var(--text-secondary)" }}>
              已同步的事件身份标识（UUID）、会话归属、历史引用与 Context 衍生记录均完整保留，
              只有真正新增或变化的内容会被追加，<strong>绝不会清库重建或删除已有记录</strong>。
            </p>
            <div className="row" style={{ justifyContent: "flex-end", gap: 8, marginTop: 14 }}>
              <button className="btn" onClick={() => setReingestTarget(null)}>
                取消
              </button>
              <button className="btn primary" onClick={() => void handleConfirmReingest()}>
                全量同步
              </button>
            </div>
          </Modal>
        )}
      </div>
    );
  }

  return (
    <div className="main agents-page">
      <PageHeader
        title="代理"
        actions={
          <button
            className="btn ghost icon-button"
            disabled={running || sources.length === 0}
            onClick={handleScanAll}
            title="增量同步全部来源"
            aria-label="增量同步全部"
          >
            <Icon name="refresh" className={running ? "spinning" : ""} />
          </button>
        }
      />

      <div className="agents-content">
        {error && (
          <div className="badge warn" style={{ marginBottom: 14, overflowWrap: "anywhere", display: "block", padding: "8px 12px" }}>
            {error}
          </div>
        )}

        {/* 顶部工具栏与筛选 */}
        <div className="toolbar agents-toolbar">
          <div className="agents-filter-pills" role="tablist" aria-label="代理分类筛选">
            <button
              type="button"
              role="tab"
              aria-selected={filterTab === "all"}
              className={`agents-filter-pill ${filterTab === "all" ? "active" : ""}`}
              onClick={() => setFilterTab("all")}
            >
              全部 ({ALL_AGENTS.length})
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={filterTab === "tui"}
              className={`agents-filter-pill ${filterTab === "tui" ? "active" : ""}`}
              onClick={() => setFilterTab("tui")}
            >
              TUI ({tuiCount})
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={filterTab === "desktop"}
              className={`agents-filter-pill ${filterTab === "desktop" ? "active" : ""}`}
              onClick={() => setFilterTab("desktop")}
            >
              桌面端 ({desktopCount})
            </button>
          </div>

          <div className="row" style={{ gap: 10, alignItems: "center" }}>
            <div style={{ position: "relative", minWidth: 160 }}>
              <input
                type="text"
                placeholder="搜索 Agent…"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                style={{
                  fontSize: 12.5,
                  padding: "5px 24px 5px 10px",
                  borderRadius: "var(--radius-sm)",
                  background: "var(--bg-panel)",
                  border: "1px solid var(--border-subtle)",
                }}
              />
              {searchQuery && (
                <button
                  type="button"
                  className="btn small ghost icon-button"
                  style={{ position: "absolute", right: 2, top: "50%", transform: "translateY(-50%)", padding: 2 }}
                  onClick={() => setSearchQuery("")}
                  title="清除搜索"
                >
                  <Icon name="close" />
                </button>
              )}
            </div>

            <div className="agents-stat-badges">
              <span className="agents-stat-pill">
                <span className="dot" style={{ color: detectedCount > 0 ? "var(--success)" : "var(--text-muted)" }} />
                已就绪 {detectedCount} / {ALL_AGENTS.length} 个 Agent
              </span>
              <span className="agents-stat-pill">
                <span className="dot" style={{ color: "var(--accent)" }} />
                共 {sources.length} 个来源目录
              </span>
              {running && (
                <span className="agents-stat-pill active">
                  <span className="dot" style={{ background: "var(--accent)" }} />
                  后台处理中…
                </span>
              )}
            </div>

            <div className="settings-seg icon-seg" role="group" aria-label="展示方式">
              <button
                className={viewMode === "cards" ? "on" : ""}
                aria-pressed={viewMode === "cards"}
                aria-label="卡片视图"
                title="卡片视图"
                onClick={() => setViewMode("cards")}
              >
                <Icon name="grid" />
              </button>
              <button
                className={viewMode === "list" ? "on" : ""}
                aria-pressed={viewMode === "list"}
                aria-label="列表视图"
                title="列表视图"
                onClick={() => setViewMode("list")}
              >
                <Icon name="list" />
              </button>
            </div>
          </div>
        </div>

        {status && (
          <div className="muted small agents-sync-status-strip" style={{ overflowWrap: "anywhere" }}>
            最近一次同步：{scopeLabel(status.scope, sources)} · 同步发现 {status.discovered} 条记录 · 写入 {status.messages} 条新消息
            {status.finished_at ? <span> · {timeAgo(status.finished_at)}</span> : null}
            {status.error && <span style={{ color: "var(--warning)", marginLeft: 6 }}>（{status.error}）</span>}
          </div>
        )}

        {/* Agent 展示 */}
        <div className={viewMode === "cards" ? "agents-grid" : "agents-list"}>
          {visibleAgents.map((agent) => {
            const entry = agentStatus?.[agent];
            const agentSources = sources.filter((s) => s.agent === agent);
            const hasCli = Boolean(entry?.terminal_cli);
            const hasDesktop = Boolean(entry?.desktop_app);

            return (
              <div
                key={agent}
                id={`agent-card-${agent}`}
                className={`agent-card ${viewMode === "list" ? "agent-list-item" : ""}`}
                role="button"
                tabIndex={0}
                aria-label={`查看 ${AGENT_LABELS[agent]} 代理详情`}
                onClick={() => navigate({ view: "agents", agent })}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    navigate({ view: "agents", agent });
                  }
                }}
              >
                {/* 卡片头部：身份与能力胶囊 */}
                <div className="agent-card-header">
                  <div className="agent-identity">
                    <AgentIcon agent={agent} size={viewMode === "list" ? 22 : 26} />
                    <div className="row" style={{ gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                      <span className="agent-title">{AGENT_LABELS[agent]}</span>
                      {hasCli && (
                        <span
                          className={`badge ${entry?.detected ? "accent" : ""}`}
                          title={entry?.detected ? "TUI 已就绪" : "未检测到 TUI"}
                        >
                          TUI
                        </span>
                      )}
                      {hasDesktop && (
                        <span
                          className={`badge ${entry?.desktop_app_present ? "accent" : ""}`}
                          title={entry?.desktop_app_present ? `已检测到 ${entry?.desktop_app ?? "桌面应用"}` : "未检测到桌面应用"}
                        >
                          桌面端
                        </span>
                      )}
                      {!hasCli && !hasDesktop && (
                        <span className="badge" title="仅同步历史会话">
                          仅同步
                        </span>
                      )}
                    </div>
                  </div>
                </div>

                {viewMode === "cards" ? (
                  <div className="agent-card-body">
                    <div className="agent-card-status-line">
                      <Icon name="terminal" />
                      <span className="small truncate" title={formatCliStatus(agent, entry)}>
                        {formatCliStatus(agent, entry)}
                      </span>
                    </div>
                    <div className="agent-card-status-line muted">
                      <Icon name="desktop" />
                      <span className="small truncate" title={hasDesktop ? `${entry?.desktop_app}（${entry?.desktop_app_present ? "已检测到" : "未检测到"}）` : "未配置桌面应用"}>
                        {hasDesktop
                          ? `${entry?.desktop_app}（${entry?.desktop_app_present ? "已检测到" : "未检测到"}）`
                          : "未配置桌面应用"}
                      </span>
                    </div>
                    <div className="agent-card-status-line muted">
                      <Icon name="chat" />
                      <span className="small truncate" title={`${sessionCounts[agent] ?? 0} 个会话`}>
                        {sessionCounts[agent] ?? 0} 个会话
                      </span>
                    </div>
                  </div>
                ) : (
                  <div className="agent-list-meta muted small">
                    <span>{formatCliStatus(agent, entry)}</span>
                    <span>·</span>
                    <span>
                      {hasDesktop
                        ? `${entry?.desktop_app}（${entry?.desktop_app_present ? "已检测到" : "未检测到"}）`
                        : "未配置桌面应用"}
                    </span>
                    <span>·</span>
                    <span>{sessionCounts[agent] ?? 0} 个会话</span>
                    <span>·</span>
                    <span>{agentSources.length === 0 ? "未配置目录" : `${agentSources.length} 个目录`}</span>
                  </div>
                )}

                <div className="agent-card-footer">
                  <span className="muted small">
                    {agentSources.length === 0
                      ? "未配置来源目录"
                      : `${agentSources.filter((s) => s.enabled).length} / ${agentSources.length} 个来源目录`}
                  </span>
                  <div className="agent-card-actions" onClick={(e) => e.stopPropagation()}>
                    <button
                      type="button"
                      className="btn small ghost icon-button"
                      disabled={running}
                      onClick={() => void handleScanAgent(agent)}
                      aria-label={`增量同步 ${AGENT_LABELS[agent]}`}
                      title={`增量同步 ${AGENT_LABELS[agent]}`}
                    >
                      <Icon name="refresh" />
                    </button>
                    {hasCli && entry?.detected && (
                      <button
                        type="button"
                        className="btn small ghost icon-button"
                        onClick={() => navigate({ view: "new-session", agent })}
                        aria-label={`使用 ${AGENT_LABELS[agent]} 新建会话`}
                        title={`使用 ${AGENT_LABELS[agent]} 新建会话`}
                      >
                        <Icon name="plus" />
                      </button>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
        </div>

        {visibleAgents.length === 0 && (
          <div className="card muted small" style={{ textAlign: "center", padding: "36px 20px" }}>
            未找到匹配「{searchQuery}」的 Agent。
          </div>
        )}
      </div>

      {/* 全量同步确认 Modal */}
      {reingestTarget && (
        <Modal title="全量同步" onClose={() => setReingestTarget(null)}>
          <p className="small" style={{ marginTop: 0 }}>
            将从头重新同步 <span className="mono">{reingestTarget.path}</span> 的全部会话文件。
          </p>
          <p className="small" style={{ color: "var(--text-secondary)" }}>
            已同步的事件身份标识（UUID）、会话归属、历史引用与 Context 衍生记录均完整保留，
            只有真正新增或变化的内容会被追加，<strong>绝不会清库重建或删除已有记录</strong>。
          </p>
          <div className="row" style={{ justifyContent: "flex-end", gap: 8, marginTop: 14 }}>
            <button className="btn" onClick={() => setReingestTarget(null)}>
              取消
            </button>
            <button className="btn primary" onClick={() => void handleConfirmReingest()}>
              全量同步
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}
