import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import AgentIcon from "../../components/AgentIcon";
import WorkspacePathField from "../../components/WorkspacePathField";
import { copyToClipboard, Modal, timeAgo } from "../../components/common";
import { showToast } from "../../components/Toast";
import {
  type Agent, type AgentStatusEntry, type IngestSource, type IngestTaskStatus,
} from "../../types";

/**
 * 高级维护：Session 来源管理 + 摄入入口，按会话格式分组。
 *
 * 普通主流程不出现这些按钮——摄入只在应用启动、前台回落与这里排队。每个来源可单独
 * 「重新扫描」（增量）或「重新入库」（从头重扫）；顶部按钮覆盖全部已启用来源。
 * 摄入在后台单线程执行，完成事件是 `ingestion-completed`，最近一次结果来自
 * `get_ingestion_status`（纯读取）。
 */
type OpenMethod = "terminal" | "desktop";

type SessionFormat = {
  id: string;
  label: string;
  agent: Agent;
  defaultPath?: string;
  /** 该格式「继续」可用的打开方式。真正的裁决在后端路由
   *  （continue_route / desktop_resume_route），这里只展示与选择。 */
  methods: OpenMethod[];
};

/** 会话格式：来源按它分组，也是添加来源的单位。Antigravity 的两个存储是两种格式（9 种的由来）。 */
const SESSION_FORMATS: SessionFormat[] = [
  { id: "codex", label: "Codex", agent: "codex", methods: ["terminal", "desktop"] },
  { id: "claude_code", label: "Claude Code", agent: "claude_code", methods: ["terminal"] },
  { id: "pi", label: "Pi", agent: "pi", methods: ["terminal"] },
  { id: "dsh", label: "dsh", agent: "dsh", methods: ["desktop"] },
  { id: "qoder", label: "Qoder", agent: "qoder", methods: ["desktop"] },
  { id: "workbuddy", label: "WorkBuddy", agent: "workbuddy", methods: ["desktop"] },
  { id: "zcode", label: "ZCode", agent: "zcode", methods: ["desktop"] },
  { id: "antigravity_desktop", label: "Antigravity", agent: "antigravity", defaultPath: "~/.gemini/antigravity", methods: ["desktop"] },
  { id: "antigravity_cli", label: "Antigravity CLI", agent: "antigravity", defaultPath: "~/.gemini/antigravity-cli", methods: ["terminal"] },
];

/** 一条来源属于哪个格式：antigravity 靠路径区分两个存储，其余格式即 agent。 */
function formatOf(src: IngestSource): string {
  if (src.agent !== "antigravity") return src.agent;
  return src.path.includes("antigravity-cli") ? "antigravity_cli" : "antigravity_desktop";
}

export default function SourcesView() {
  const [sources, setSources] = useState<IngestSource[]>([]);
  const [agentStatus, setAgentStatus] = useState<Record<string, AgentStatusEntry> | null>(null);
  /** 每个分组自己的待添加路径；打开输入行时预填该格式的标准存储。 */
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [addOpenFor, setAddOpenFor] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** 已排队但还没收到完成事件：这期间不允许再排队同样的动作。 */
  const [queued, setQueued] = useState(false);
  const [status, setStatus] = useState<IngestTaskStatus | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  /** 待确认的「重新入库」目标；确认框走应用统一的 Modal，不用原生 confirm。 */
  const [reingestTarget, setReingestTarget] = useState<IngestSource | null>(null);

  /** 每个动作前先清掉上一次的结论：上一次的失败红条挂在这一次成功操作旁边，
   *  读起来像是刚刚发生的。 */
  const clearMessages = () => { setError(""); setNotice(""); };

  const reload = useCallback(() => {
    api.listIngestSources().then(setSources).catch((e) => setError(String(e)));
  }, []);

  const reloadStatus = useCallback(() => {
    api.getIngestionStatus().then(setStatus).catch(console.error);
  }, []);

  useEffect(() => {
    reload();
    reloadStatus();
    api.getAgentStatus().then(setAgentStatus).catch(console.error);
  }, [reload, reloadStatus]);

  // 后台摄入完成：刷新来源列表与「最近一次摄入」。运行结果（含失败）由那一行常驻
  // 汇报——它才是记录；页面上的红条只留给用户自己触发的动作失败，免得同一句话说两遍。
  useEffect(() => {
    const unlisten = listen("ingestion-completed", () => {
      setQueued(false);
      reload();
      reloadStatus();
    });
    return () => { unlisten.then((f) => f()); };
  }, [reload, reloadStatus]);

  const copyPath = async (path: string) => {
    const ok = await copyToClipboard(path);
    showToast(ok ? "已复制到剪贴板" : "复制失败，请手动选中文字复制");
  };

  const toggle = async (src: IngestSource, enabled: boolean) => {
    clearMessages();
    try {
      await api.setIngestSourceEnabled(src.id, enabled);
      reload();
      if (enabled) setNotice("已启用。点这一行的「重新扫描」开始摄入。");
    } catch (e) {
      setError(String(e));
    }
  };

  const addFor = async (fmt: SessionFormat) => {
    const p = (drafts[fmt.id] ?? "").trim();
    if (!p || busy) return;
    clearMessages();
    setBusy(true);
    try {
      await api.addIngestSource(fmt.agent, p);
      setDrafts((d) => ({ ...d, [fmt.id]: "" }));
      setAddOpenFor(null);
      setNotice("已添加并启用。点这一行的「重新扫描」开始摄入。");
      reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (src: IngestSource) => {
    clearMessages();
    try {
      await api.removeIngestSource(src.id);
      reload();
    } catch (e) {
      setError(String(e));
    }
  };

  /** 两种「重新」都只是排队，区别只在打哪条命令与怎么说。 */
  const queue = async (src: IngestSource, action: "scan" | "reingest") => {
    clearMessages();
    try {
      if (action === "scan") await api.reconcileSource(src.id);
      else await api.reingestSource(src.id);
      setQueued(true);
      setNotice(action === "scan" ? "已排队：正在重新扫描这个来源。" : "已排队：正在重新入库这个来源。");
    } catch (e) {
      setError(String(e));
    }
  };

  const confirmReingest = async () => {
    const src = reingestTarget;
    setReingestTarget(null);
    if (src) await queue(src, "reingest");
  };

  const scanAll = async () => {
    clearMessages();
    try {
      await api.reconcileAll();
      setQueued(true);
      setNotice("已排队：正在重新扫描全部已启用来源。");
    } catch (e) {
      setError(String(e));
    }
  };

  const setMethod = async (fmt: SessionFormat, method: OpenMethod) => {
    clearMessages();
    try {
      await api.setResumeOpenMethod(fmt.agent, method);
      setAgentStatus(await api.getAgentStatus());
      setNotice(
        method === "desktop"
          ? "已改为桌面端打开：继续该格式的会话时打开对应应用。"
          : "已改为终端打开。"
      );
    } catch (e) {
      setError(String(e));
    }
  };

  const enabledCount = sources.filter((s) => s.enabled).length;

  return (
    <div>
      <p className="muted small" style={{ marginTop: 0 }}>
        仅扫描已启用的目录。这些按钮是高级维护入口，普通使用不需要它们。
      </p>

      <div className="row" style={{ marginBottom: 14, alignItems: "center" }}>
        {/* 按钮名不随排队状态改：名字是动作，状态由旁边的徽标说。 */}
        <button className="btn primary" disabled={queued} onClick={scanAll}
          title="只读取新增内容：从每个会话上次停下的位置继续">
          重新扫描全部来源
        </button>
        {queued && <span className="badge accent">已排队，后台处理中…</span>}
        <span className="muted small">
          当前 {enabledCount} / {sources.length} 个来源启用
        </span>
      </div>

      <IngestionStatus status={status} sources={sources} />

      {notice && <div className="badge accent" style={{ marginBottom: 10 }}>{notice}</div>}
      {error && <div className="badge warn" style={{ marginBottom: 10 }}>{error}</div>}

      {/* 每个会话格式一个分组：组内加来源路径，组头选择 resume 打开方式。
          两行式行布局与设置页其余条目同一套 row-line 语汇。 */}
      {SESSION_FORMATS.map((fmt) => {
        const rows = sources.filter((s) => formatOf(s) === fmt.id);
        const entry = agentStatus?.[fmt.agent];
        return (
          <div className="card" key={fmt.id} style={{ marginBottom: 16 }}>
            <div className="row between" style={{ gap: 10, alignItems: "center" }}>
              <div className="settings-row-label row" style={{ gap: 7 }}>
                <AgentIcon agent={fmt.agent} />
                {fmt.label}
              </div>
              <OpenMethodControl fmt={fmt} entry={entry} onChoose={(m) => void setMethod(fmt, m)} />
            </div>

            {rows.length === 0 && (
              <div className="muted small" style={{ padding: "8px 0 2px" }}>未添加来源。</div>
            )}
            {rows.map((src) => (
              <div key={src.id} style={{ padding: "10px 0", borderBottom: "1px solid var(--border-subtle)" }}>
                <div className="row between" style={{ gap: 10, alignItems: "center" }}>
                  <div className="row" style={{ gap: 8, alignItems: "center", minWidth: 0 }}>
                    <input
                      type="checkbox"
                      style={{ width: "auto" }}
                      checked={src.enabled}
                      disabled={queued}
                      title={src.enabled ? "停用这个来源" : "启用这个来源"}
                      onChange={(e) => toggle(src, e.target.checked)}
                    />
                    {src.origin === "default" && <span className="badge" title="按本机装了什么 Agent 自动登记">默认</span>}
                    {!src.exists && <span className="badge warn" title="该目录当前不存在">目录不存在</span>}
                    {/* 徽标说的是这一行的配置状态，不是"此刻正在摄入"——进行中由顶部 queued 表达。 */}
                    {src.enabled ? (
                      <span className="badge accent">已启用</span>
                    ) : (
                      <span className="muted small">未启用</span>
                    )}
                  </div>
                  <div className="row" style={{ gap: 8, flex: "none" }}>
                    <button className="btn small" disabled={queued} onClick={() => queue(src, "scan")}
                      title="只读取新增内容：从每个会话上次停下的位置继续">
                      重新扫描
                    </button>
                    <button className="btn small ghost" disabled={queued} onClick={() => setReingestTarget(src)}
                      title="从头重扫这个来源的全部文件；已摄入的事件及其引用保持不变">
                      重新入库
                    </button>
                    {src.origin === "user" && (
                      <button className="btn small ghost" disabled={queued} onClick={() => remove(src)}>
                        移除
                      </button>
                    )}
                  </div>
                </div>
                <div className="row" style={{ gap: 8, alignItems: "baseline", marginTop: 4 }}>
                  <span className="mono small" style={{ minWidth: 0, overflowWrap: "anywhere", userSelect: "all" }}>{src.path}</span>
                  <button className="link" style={{ flex: "none" }} onClick={() => copyPath(src.path)}>复制</button>
                </div>
              </div>
            ))}

            {addOpenFor === fmt.id ? (
              <div className="row" style={{ gap: 8, marginTop: 10 }}>
                <div style={{ flex: 1, minWidth: 160 }}>
                  <WorkspacePathField
                    value={drafts[fmt.id] ?? ""}
                    onChange={(v) => setDrafts((d) => ({ ...d, [fmt.id]: v }))}
                    onSubmit={() => void addFor(fmt)}
                    enableProbe={false}
                    enableRecent={false}
                    placeholder="~/somewhere/sessions"
                  />
                </div>
                <button className="btn small primary" disabled={busy || !(drafts[fmt.id] ?? "").trim()}
                  onClick={() => void addFor(fmt)}>
                  添加
                </button>
                <button className="btn small ghost" onClick={() => { setAddOpenFor(null); setDrafts((d) => ({ ...d, [fmt.id]: "" })); }}>
                  取消
                </button>
              </div>
            ) : (
              <button className="btn small ghost" style={{ marginTop: 10 }}
                onClick={() => {
                  setAddOpenFor(fmt.id);
                  // 标准存储先填上（antigravity 的两个默认目录）：用户改一下就能加。
                  setDrafts((d) => ({ ...d, [fmt.id]: d[fmt.id] ?? fmt.defaultPath ?? "" }));
                }}>
                + 添加路径
              </button>
            )}
            {busy && addOpenFor === fmt.id && <span className="badge accent" style={{ marginTop: 8 }}>添加中…</span>}
          </div>
        );
      })}

      {reingestTarget && (
        <Modal title="重新入库" onClose={() => setReingestTarget(null)}>
          <p className="small" style={{ marginTop: 0 }}>
            将从头重扫 <span className="mono">{reingestTarget.path}</span> 的全部会话文件。
            已摄入的事件及其引用保持不变，只有真正新增或变化的内容会被追加；会话归属、
            Context 条目与审计历史都保留。
          </p>
          <div className="row" style={{ justifyContent: "flex-end", gap: 8, marginTop: 14 }}>
            <button className="btn" onClick={() => setReingestTarget(null)}>取消</button>
            <button className="btn primary" onClick={confirmReingest}>重新入库</button>
          </div>
        </Modal>
      )}
    </div>
  );
}

/** 组头的打开方式控件。两种方式（目前只有 codex）是可选项，桌面端选项在应用
 *  未安装时禁用——找不到的桌面应用不支持；只有一种方式的格式展示为固定事实。 */
function OpenMethodControl({ fmt, entry, onChoose }: {
  fmt: SessionFormat;
  entry?: AgentStatusEntry;
  onChoose: (m: OpenMethod) => void;
}) {
  const current = entry?.resume_open_method ?? "terminal";
  const label = (m: OpenMethod) =>
    m === "terminal" ? "TUI / CLI" : entry?.desktop_app ? `桌面端（${entry.desktop_app}）` : "桌面端";
  // 在场未知（状态没回来）时不当作缺席。
  const desktopAbsent = entry != null && !entry.desktop_app_present;

  if (fmt.methods.length === 1) {
    const m = fmt.methods[0];
    return (
      <span className="badge" title="该会话格式只有这一种打开方式">
        {label(m)}
        {m === "desktop" && desktopAbsent ? " · 未安装" : ""}
      </span>
    );
  }
  return (
    <div className="settings-seg" role="group" aria-label="resume 打开方式">
      {fmt.methods.map((m) => {
        const absent = m === "desktop" && desktopAbsent;
        return (
          <button key={m}
            className={current === m ? "on" : ""}
            disabled={absent}
            title={absent ? `未找到 ${entry?.desktop_app}，桌面端打开不可用` : undefined}
            onClick={() => onChoose(m)}>
            {label(m)}
          </button>
        );
      })}
    </div>
  );
}

/** 作用域的中文说明。单个来源时报出它的路径——scope 里那个 id 对人没有意义，
 *  只说「单个来源」等于没说清刚扫的是哪一个。 */
function scopeLabel(scope: string, sources: IngestSource[]): string {
  if (scope === "reconcile_all") return "重新扫描全部来源";
  const path = (prefix: string) =>
    sources.find((s) => scope === `${prefix}${s.id}`)?.path;
  if (scope.startsWith("reconcile_source:")) return `重新扫描 ${path("reconcile_source:") ?? "单个来源"}`;
  if (scope.startsWith("reingest_source:")) return `重新入库 ${path("reingest_source:") ?? "单个来源"}`;
  if (scope.startsWith("refresh_session:")) return "刷新单个会话";
  return scope;
}

/** 最近一次后台摄入。一行事实就画一行——不套卡片、不占一个区块；没有记录时什么都不画。 */
function IngestionStatus({ status, sources }: {
  status: IngestTaskStatus | null;
  sources: IngestSource[];
}) {
  if (!status) return null;
  return (
    <>
      <div className="muted small" style={{ marginBottom: 14 }}>
        最近一次摄入：{scopeLabel(status.scope, sources)} · 发现 {status.discovered} 个会话 · 写入 {status.messages} 条新消息
        {status.finished_at ? <span> · {timeAgo(status.finished_at)}</span> : null}
      </div>
      {status.error && (
        <div className="badge warn" style={{ marginBottom: 14, overflowWrap: "anywhere" }}>{status.error}</div>
      )}
    </>
  );
}
