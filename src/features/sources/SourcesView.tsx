import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import WorkspacePathField from "../../components/WorkspacePathField";
import { copyToClipboard, Modal, timeAgo } from "../../components/common";
import { showToast } from "../../components/Toast";
import { AGENT_LABELS, type Agent, type IngestSource, type IngestTaskStatus } from "../../types";

/**
 * 高级维护：Session 来源管理 + 摄入入口。
 *
 * 普通主流程不出现这些按钮——摄入只在应用启动、前台回落与这里排队。每个来源可单独
 * 「重新扫描」（增量）或「重新入库」（从头重扫）；顶部按钮覆盖全部已启用来源。
 * 摄入在后台单线程执行，完成事件是 `ingestion-completed`，最近一次结果来自
 * `get_ingestion_status`（纯读取）。
 */
export default function SourcesView() {
  const [sources, setSources] = useState<IngestSource[]>([]);
  const [agent, setAgent] = useState<Agent>("codex");
  const [path, setPath] = useState("");
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

  const add = async () => {
    if (!path.trim() || busy) return;
    clearMessages();
    setBusy(true);
    try {
      await api.addIngestSource(agent, path.trim());
      setPath("");
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

      {/* 两行式：每行左标签右控件，与设置页其余条目同一套 row-line 语汇。
          原来挤在一行、按底部对齐，标签长短不一就错位，输入框也被压到占位文字被切断。 */}
      <div className="card" style={{ marginBottom: 16 }}>
        <div className="section-label" style={{ marginTop: 0 }}>添加来源</div>

        <div className="row-line" style={{ flexWrap: "wrap" }}>
          <div>
            <div className="settings-row-label">Agent</div>
            <div className="settings-row-hint">决定按哪种会话格式解析这个目录</div>
          </div>
          <select value={agent} onChange={(e) => setAgent(e.target.value as Agent)} style={{ width: 170 }}>
            {Object.entries(AGENT_LABELS).map(([k, v]) => (
              <option key={k} value={k}>{v}</option>
            ))}
          </select>
        </div>

        <div className="row-line" style={{ flexWrap: "wrap" }}>
          <div>
            <div className="settings-row-label">目录路径</div>
            <div className="settings-row-hint">支持 ~；递归扫描 .jsonl 会话文件</div>
          </div>
          <div style={{ flex: 1, minWidth: 160, maxWidth: 380 }}>
            <WorkspacePathField
              value={path}
              onChange={setPath}
              onSubmit={add}
              enableProbe={false}
              enableRecent={false}
              placeholder="~/somewhere/sessions"
            />
          </div>
        </div>

        <div className="row" style={{ justifyContent: "flex-end", gap: 8, marginTop: 12 }}>
          {busy && <span className="badge accent">添加中…</span>}
          <button className="btn primary" disabled={busy || !path.trim()} onClick={add}>
            添加来源
          </button>
        </div>
      </div>

      {notice && <div className="badge accent" style={{ marginBottom: 10 }}>{notice}</div>}
      {error && <div className="badge warn" style={{ marginBottom: 10 }}>{error}</div>}

      {/* 每行两段：上面是身份与状态、动作靠右，下面整行留给路径。
          挤在一行时路径（flex + 隐藏溢出）会被压成 0 宽，最该看的那个事实先消失。 */}
      <div className="card">
        {sources.length === 0 && <div className="muted small">暂无会话来源。</div>}
        {sources.map((src) => (
          <div key={src.id} style={{ padding: "10px 4px", borderBottom: "1px solid var(--border-subtle)" }}>
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
                <span className="badge">{AGENT_LABELS[src.agent]}</span>
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
      </div>

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
