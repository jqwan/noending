import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import { onEvent, EVT_SYNCED, type Route } from "../../app/routes";
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

type OpenMethod = "terminal" | "desktop" | "embedded";

type FilterTab = "all" | "tui" | "desktop";

interface SessionFormat {
  id: string;
  label: string;
  agent: Agent;
  defaultPath?: string;
  methods: OpenMethod[];
}

const SESSION_FORMATS: SessionFormat[] = [
  { id: "codex", label: "Codex 会话", agent: "codex", defaultPath: "~/.codex/sessions", methods: ["terminal", "embedded", "desktop"] },
  { id: "claude_code", label: "Claude Code 会话", agent: "claude_code", defaultPath: "~/.claude/projects", methods: ["terminal", "embedded"] },
  { id: "pi", label: "Pi 会话", agent: "pi", defaultPath: "~/.pi/agent/sessions", methods: ["terminal", "embedded"] },
  { id: "dsh", label: "DSH 会话", agent: "dsh", defaultPath: "~/.dsh", methods: ["desktop"] },
  { id: "qoder", label: "Qoder 会话", agent: "qoder", defaultPath: "~/.qoder-cn", methods: ["desktop"] },
  { id: "workbuddy", label: "WorkBuddy 会话", agent: "workbuddy", defaultPath: "~/.workbuddy", methods: ["desktop"] },
  { id: "zcode", label: "ZCode 会话", agent: "zcode", defaultPath: "~/.zcode", methods: ["desktop"] },
  { id: "antigravity_desktop", label: "Desktop 格式", agent: "antigravity", defaultPath: "~/.gemini/antigravity", methods: ["desktop"] },
  { id: "antigravity_cli", label: "CLI 格式", agent: "antigravity", defaultPath: "~/.gemini/antigravity-cli", methods: ["terminal", "embedded"] },
];

function formatOf(src: IngestSource): string {
  if (src.agent !== "antigravity") return src.agent;
  return src.path.includes("antigravity-cli") ? "antigravity_cli" : "antigravity_desktop";
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
}: {
  src: IngestSource;
  running: boolean;
  onScan: (id: string) => void;
  onReingest: (src: IngestSource) => void;
  onRemove: (src: IngestSource) => void;
  onCopy: (path: string) => void;
}) {
  return (
    <div className="agent-source-row">
      <div className="agent-source-main">
        <span
          role="button"
          tabIndex={0}
          className="mono small agent-source-path path-link"
          style={{
            userSelect: "all",
            color: "var(--text-primary)",
          }}
          title={`${src.path} · 点击在文件管理器中打开`}
          onClick={() => void openPath(src.path)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              void openPath(src.path);
            }
          }}
        >
          {src.path}
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
          title="增量同步：仅读取自上次停下后的新增内容"
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
            title="移除此自定义目录"
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
          <span className="badge warn" title="该目录当前在磁盘上不存在">
            目录不存在
          </span>
        )}
      </div>

      <button
        type="button"
        className="btn small ghost agent-reingest-btn"
        disabled={running}
        onClick={() => onReingest(src)}
        title="全量同步：从头完整读取该来源的全部文件，校准可能遗漏或变动的记录"
      >
        全量同步
      </button>
    </div>
  );
}

export default function AgentsView({
  navigate: _navigate,
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

  const reloadAll = useCallback(async () => {
    await Promise.all([reloadAgents(), reloadSources(), reloadStatus()]);
  }, [reloadAgents, reloadSources, reloadStatus]);

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
    showToast(ok ? "已复制到剪贴板" : "复制失败，请手动选择文字复制");
  };

  const handleRemoveSource = async (src: IngestSource) => {
    clearMessages();
    try {
      await api.removeIngestSource(src.id);
      showToast("已移除来源目录。");
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
      showToast("已排队：正在增量同步该来源。");
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
      showToast("已排队：正在全量同步该来源。");
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
      await api.reconcileAll();
      showToast("已排队：正在增量同步全部已启用来源。");
      await reloadStatus();
    } catch (e) {
      setRunning(false);
      setError(String(e));
    }
  };

  const handleSetResumeMethod = async (a: Agent, method: "terminal" | "desktop" | "embedded") => {
    clearMessages();
    try {
      await api.setResumeOpenMethod(a, method);
      setAgentStatus(await api.getAgentStatus());
      showToast(
        method === "desktop"
          ? "已改为 Desktop 打开：继续会话时将唤起对应应用。"
          : method === "embedded"
            ? "已改为内嵌终端打开：继续会话时在 NoEnding 内运行。"
            : "已改为 TUI 打开。"
      );
    } catch (e) {
      setError(String(e));
      showToast(`修改打开方式失败：${String(e)}`);
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
      showToast("已添加并启用来源目录。");
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

  return (
    <div className="main agents-page">
      <PageHeader title="代理" />

      <div className="agents-content">
        {error && (
          <div className="badge warn" style={{ marginBottom: 14, overflowWrap: "anywhere", display: "block", padding: "8px 12px" }}>
            {error}
          </div>
        )}

      {/* 顶部全局状态栏 */}
      <div className="agents-overview-card card">
        <div className="agents-overview-top">
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
          <div>
            <button
              className="btn primary"
              disabled={running || sources.length === 0}
              onClick={handleScanAll}
              title="增量同步：从每个会话上次停下的位置继续"
            >
              增量同步全部来源
            </button>
          </div>
        </div>

        <div className="muted small" style={{ overflowWrap: "anywhere" }}>
          {status ? (
            <>
              最近一次同步：{scopeLabel(status.scope, sources)} · 同步发现 {status.discovered} 条记录 · 写入 {status.messages} 条新消息
              {status.finished_at ? <span> · {timeAgo(status.finished_at)}</span> : null}
              {status.error && <span style={{ color: "var(--warning)", marginLeft: 6 }}>（{status.error}）</span>}
            </>
          ) : (
            "暂无同步记录"
          )}
        </div>
      </div>

      {/* 快速分类与搜索栏 */}
      <div className="agents-filter-bar">
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

        <div className="row" style={{ gap: 8, alignItems: "center" }}>
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
        </div>
      </div>

      {/* Agent 卡片网格 */}
      <div className="agents-grid">
        {visibleAgents.map((agent) => {
          const entry = agentStatus?.[agent];
          const isInitialTarget = initialAgent === agent;

          // Antigravity 统一聚合所有属于 antigravity 的来源
          const agentSources = sources.filter((s) => s.agent === agent);
          const agentFormats = SESSION_FORMATS.filter((f) => f.agent === agent);

          const hasCli = Boolean(entry?.terminal_cli);
          const hasDesktop = Boolean(entry?.desktop_app);

          return (
            <div
              key={agent}
              id={`agent-card-${agent}`}
              className="agent-card"
              style={{
                borderColor: isInitialTarget ? "var(--accent)" : undefined,
              }}
            >
              {/* 卡片头部：身份与能力胶囊 */}
              <div className="agent-card-header">
                <div className="agent-identity">
                  <AgentIcon agent={agent} size={26} />
                  <div className="row" style={{ gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                    <span className="agent-title">{AGENT_LABELS[agent]}</span>
                    {hasCli && (
                      <span
                        className={`badge ${entry?.detected ? "accent" : ""}`}
                        title={entry?.detected ? "已检测到 TUI 运行环境" : "未检测到 TUI"}
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
                      <span className="badge" title="仅进行历史会话同步">
                        仅同步
                      </span>
                    )}
                  </div>
                </div>

                {/* 支持多种打开方式的单格式 Agent（如 Codex）在卡片右上角展示切换按钮，不显示“打开方式”文本 */}
                {agentFormats.length === 1 && agentFormats[0].methods.length > 1 && (
                  <div className="settings-seg" role="group" aria-label="打开方式切换">
                    {agentFormats[0].methods.map((m) => {
                      const isSelected = (entry?.resume_open_method ?? "terminal") === m;
                      const absent = m === "desktop" && entry != null && !entry.desktop_app_present;
                      return (
                        <button
                          key={m}
                          type="button"
                          className={isSelected ? "on" : ""}
                          disabled={absent}
                          title={absent ? `未找到 ${entry?.desktop_app}，Desktop 不可用` : undefined}
                          onClick={() => void handleSetResumeMethod(agent, m)}
                        >
                          {m === "terminal" ? "TUI" : m === "embedded" ? "内嵌" : "Desktop"}
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>

              {/* 会话存储格式与来源目录列表 */}
              <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                {agentFormats.map((fmt, fIdx) => {
                  const formatSources = agentSources.filter((s) => formatOf(s) === fmt.id);
                  const isMultiFormat = agentFormats.length > 1;

                  return (
                    <div
                      key={fmt.id}
                      style={{
                        paddingTop: isMultiFormat && fIdx > 0 ? 10 : 0,
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
                            borderBottom: "1px solid var(--border-subtle)",
                          }}
                        >
                          <span style={{ fontSize: 13, fontWeight: 600 }}>{fmt.label}</span>
                          <span className="badge" title="该会话格式固定通过此方式打开会话">
                            {fmt.methods[0] === "terminal" ? "TUI" : "Desktop"}
                            {fmt.methods[0] === "desktop" && entry != null && !entry.desktop_app_present ? " · 未安装" : ""}
                          </span>
                        </div>
                      )}

                      {/* 来源目录列表 */}
                      <div>
                        {formatSources.length === 0 ? (
                          <div className="muted small" style={{ padding: "6px 0" }}>
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
                            />
                          ))
                        )}

                        <button
                          className="btn small ghost"
                          style={{ marginTop: 6 }}
                          onClick={() => void handlePickAndAddSource(agent, fmt.defaultPath)}
                        >
                          + 添加来源目录
                        </button>
                      </div>
                    </div>
                  );
                })}
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
