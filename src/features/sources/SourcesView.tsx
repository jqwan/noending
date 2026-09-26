import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import WorkspacePathField from "../../components/WorkspacePathField";
import { timeAgo } from "../../components/common";
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

  // 后台摄入完成：刷新来源列表与最近一次摄入结果。
  useEffect(() => {
    const unlisten = listen("ingestion-completed", (e) => {
      const p = e.payload as {
        discovered?: number;
        messages?: number;
        error?: string | null;
      };
      setQueued(false);
      if (p.error) {
        setError(p.error);
      } else {
        setNotice(`摄入完成：发现 ${p.discovered ?? 0} 个会话，写入 ${p.messages ?? 0} 条新消息。`);
      }
      reload();
      reloadStatus();
    });
    return () => { unlisten.then((f) => f()); };
  }, [reload, reloadStatus]);

  const toggle = async (src: IngestSource, enabled: boolean) => {
    await api.setIngestSourceEnabled(src.id, enabled).catch((e) => setError(String(e)));
    reload();
    if (enabled) {
      setNotice("已启用。点击该行的「重新扫描」开始摄入。");
    }
  };

  const add = async () => {
    if (!path.trim()) return;
    setBusy(true);
    setError("");
    try {
      await api.addIngestSource(agent, path.trim());
      setPath("");
      setNotice("已添加并启用。点击该行的「重新扫描」开始摄入。");
      reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (src: IngestSource) => {
    await api.removeIngestSource(src.id).catch((e) => setError(String(e)));
    reload();
  };

  const scanOne = async (src: IngestSource) => {
    setError("");
    setNotice("");
    try {
      await api.reconcileSource(src.id);
      setQueued(true);
      setNotice("已排队：正在重新扫描这个来源。");
    } catch (e) {
      setError(String(e));
    }
  };

  const reingest = async (src: IngestSource) => {
    if (!window.confirm(
      `重新入库「${src.path}」？\n\n将从头重扫该来源的全部会话文件：已摄入的事件及其引用保持不变，仅真正新增或变化的内容会被追加（会话归属、Context 条目与审计历史保留）。`
    )) {
      return;
    }
    setError("");
    setNotice("");
    try {
      await api.reingestSource(src.id);
      setQueued(true);
      setNotice("已排队：正在重新入库这个来源。");
    } catch (e) {
      setError(String(e));
    }
  };

  const scanAll = async () => {
    setError("");
    setNotice("");
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
        <button className="btn primary" disabled={queued} onClick={scanAll}>
          {queued ? "正在摄入…" : "重新扫描全部来源"}
        </button>
        {queued && <span className="badge accent">已排队，后台处理中…</span>}
        <span className="muted small">
          当前 {enabledCount} / {sources.length} 个来源启用
        </span>
      </div>

      <IngestionStatusCard status={status} />

      <div className="card" style={{ marginBottom: 16 }}>
        <div className="row" style={{ alignItems: "flex-end", gap: 10, flexWrap: "wrap" }}>
          <label className="field" style={{ marginBottom: 0 }}>
            <span>Agent（决定解析哪种会话格式）</span>
            <select value={agent} onChange={(e) => setAgent(e.target.value as Agent)} style={{ width: 170 }}>
              {Object.entries(AGENT_LABELS).map(([k, v]) => (
                <option key={k} value={k}>{v}</option>
              ))}
            </select>
          </label>
          <div className="field" style={{ flex: 1, minWidth: 260, marginBottom: 0 }}>
            <span>目录路径（支持 ~，递归扫描 .jsonl 会话文件）</span>
            <WorkspacePathField
              value={path}
              onChange={setPath}
              onSubmit={add}
              enableProbe={false}
              enableRecent={false}
              placeholder="/path/to/another/codex-home 或 ~/somewhere/sessions"
            />
          </div>
          <button className="btn primary" disabled={busy || !path.trim()} onClick={add}>
            {busy ? "添加中…" : "添加来源"}
          </button>
        </div>
      </div>

      {notice && <div className="badge accent" style={{ marginBottom: 10 }}>{notice}</div>}
      {error && <div className="badge warn" style={{ marginBottom: 10 }}>{error}</div>}

      <div className="card">
        {sources.length === 0 && <div className="muted small">暂无会话来源。</div>}
        {sources.map((src) => (
          <div key={src.id} className="row" style={{ padding: "8px 4px", borderBottom: "1px solid var(--border-subtle)", alignItems: "center", gap: 8 }}>
            <input
              type="checkbox"
              style={{ width: "auto" }}
              checked={src.enabled}
              disabled={queued}
              onChange={(e) => toggle(src, e.target.checked)}
            />
            <span className="badge">{AGENT_LABELS[src.agent]}</span>
            <span
              className="mono small"
              title={src.path}
              style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
            >
              {src.path}
            </span>
            {src.origin === "default" && <span className="badge">默认</span>}
            {!src.exists && <span className="badge warn" title="该目录当前不存在">目录不存在</span>}
            {/* 徽标说的是这一行的配置状态，不是"此刻正在摄入"——进行中由顶部 queued 表达。 */}
            {src.enabled ? (
              <span className="badge accent">已启用</span>
            ) : (
              <span className="muted small">未启用</span>
            )}
            <button className="btn small" disabled={queued} onClick={() => scanOne(src)}>
              重新扫描
            </button>
            <button className="btn small ghost" disabled={queued} onClick={() => reingest(src)}>
              重新入库
            </button>
            {src.origin === "user" && (
              <button className="btn small ghost" disabled={queued} onClick={() => remove(src)}>
                移除
              </button>
            )}
          </div>
        ))}
      </div>

      <details className="muted small" style={{ marginTop: 10 }}><summary>重新扫描与重新入库</summary><p>重新扫描只读取新增内容；重新入库会重扫全部文件，保留现有关联与上下文。</p></details>
    </div>
  );
}

/** `IngestTaskStatus.scope` 的中文说明。 */
function scopeLabel(scope: string): string {
  if (scope === "reconcile_all") return "重新扫描全部来源";
  if (scope.startsWith("reconcile_source:")) return "重新扫描单个来源";
  if (scope.startsWith("reingest_source:")) return "重新入库";
  if (scope.startsWith("refresh_session:")) return "刷新单个会话";
  return scope;
}

/** 最近一次后台摄入的结果。没有任何记录时不占位。 */
function IngestionStatusCard({ status }: { status: IngestTaskStatus | null }) {
  if (!status) return null;
  return (
    <section className="card" style={{ marginBottom: 16 }}>
      <div className="section-label" style={{ marginTop: 0 }}>最近一次摄入</div>
      <div className="small">
        {scopeLabel(status.scope)} · 发现 {status.discovered} 个会话 · 写入 {status.messages} 条新消息
        {status.finished_at ? <span className="muted"> · {timeAgo(status.finished_at)}</span> : null}
      </div>
      {status.error && (
        <div className="badge warn" style={{ marginTop: 8, overflowWrap: "anywhere" }}>{status.error}</div>
      )}
    </section>
  );
}
