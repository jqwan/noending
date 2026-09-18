import React, { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api";
import { AGENT_LABELS, type Agent, type IngestSource } from "../../types";

/**
 * Session 来源管理 + 摄入入口（方案 §2.1：Sources → Session 来源）。
 * - 默认 agent 根目录（~/.codex 等）以"未启用"状态预置，是否摄入由用户决定；
 * - 每个来源可以单独「同步」（增量）或「重新摄入」（从头重扫源文件，
 *   重新抓取，绑定与上下文条目保留）；
 * - 摄入在后台执行（不阻塞界面），进度通过 sync-* 事件推送。
 */
export default function SourcesView({ onSync, embedded }: { onSync?: () => void; embedded?: boolean }) {
  const [sources, setSources] = useState<IngestSource[]>([]);
  const [agent, setAgent] = useState<Agent>("codex");
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [progress, setProgress] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  const reload = useCallback(() => {
    api.listIngestSources().then(setSources).catch((e) => setError(String(e)));
  }, []);

  useEffect(reload, [reload]);

  // 后台摄入事件：started / per-session progress / completed / failed
  useEffect(() => {
    const unlisten = Promise.all([
      listen("sync-started", () => {
        setSyncing(true);
        setProgress("正在扫描 Session 来源…");
      }),
      listen("sync-progress", (e) => {
        const p = e.payload as { agent?: string; title?: string };
        setProgress(`正在处理：${p.title || "（未命名 Session）"}`);
      }),
      listen("sync-completed", (e) => {
        const p = e.payload as { discovered?: number; events?: number };
        setSyncing(false);
        setProgress("");
        setNotice(`摄入完成：发现 ${p.discovered ?? 0} 个 Session，摄入 ${p.events ?? 0} 条新事件。`);
        reload();
        onSync?.();
      }),
      listen("sync-failed", (e) => {
        const p = e.payload as { error?: string };
        setSyncing(false);
        setProgress("");
        setError(p.error || "同步失败");
      }),
    ]);
    return () => { unlisten.then((fns) => fns.forEach((f) => f())); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reload]);

  const toggle = async (src: IngestSource, enabled: boolean) => {
    await api.setIngestSourceEnabled(src.id, enabled).catch((e) => setError(String(e)));
    reload();
    if (enabled) {
      setNotice("已启用。点击该行的「同步」开始摄入。");
    }
  };

  const add = async () => {
    if (!path.trim()) return;
    setBusy(true);
    setError("");
    try {
      await api.addIngestSource(agent, path.trim());
      setPath("");
      setNotice("已添加并启用。点击该行的「同步」开始摄入。");
      reload();
      onSync?.();
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

  const syncOne = async (src: IngestSource) => {
    setError("");
    try {
      await api.syncSource(src.id);
    } catch (e) {
      setError(String(e));
    }
  };

  const reingest = async (src: IngestSource) => {
    if (!window.confirm(
      `重新摄入「${src.path}」？\n\n将从头重扫该来源的全部 Session 文件：已摄入的事件及其引用保持不变，仅真正新增或变化的内容会被追加（绑定、Context 条目与审计历史保留）。`
    )) {
      return;
    }
    setError("");
    try {
      await api.reingestSource(src.id);
    } catch (e) {
      setError(String(e));
    }
  };

  const syncAll = async () => {
    setError("");
    try {
      await api.syncAll();
    } catch (e) {
      setError(String(e));
    }
  };

  const enabledCount = sources.filter((s) => s.enabled).length;

  return (
    <div className={embedded ? "" : "main"}>
      {!embedded && (
        <>
          <h1>Session 来源</h1>
          <p className="page-sub">
            只有勾选启用的目录会被扫描摄入。默认 Agent 目录（~/.codex、~/.claude、~/.pi）仅作为候选预置，是否摄入由你决定；也可以添加任意自定义目录，按所选 Agent 的 Session 格式（内容指纹校验）递归扫描。摄入在后台执行，不会阻塞界面。
          </p>
        </>
      )}
      {embedded && (
        <p className="muted small" style={{ marginTop: 0 }}>
          只有勾选启用的目录会被扫描摄入；默认 Agent 目录仅作为候选预置。摄入在后台执行。
        </p>
      )}

      <div className="row" style={{ marginBottom: 14, alignItems: "center" }}>
        <button className="btn primary" disabled={syncing} onClick={syncAll}>
          {syncing ? "正在摄入…" : "同步全部来源"}
        </button>
        {syncing && <span className="badge accent">{progress || "正在摄入…"}</span>}
        <span className="muted small">
          当前 {enabledCount} / {sources.length} 个来源启用
        </span>
      </div>

      <div className="card" style={{ marginBottom: 16 }}>
        <div className="row" style={{ alignItems: "flex-end", gap: 10, flexWrap: "wrap" }}>
          <label className="field" style={{ marginBottom: 0 }}>
            <span>Agent（决定解析哪种 Session 格式）</span>
            <select value={agent} onChange={(e) => setAgent(e.target.value as Agent)} style={{ width: 170 }}>
              {Object.entries(AGENT_LABELS).map(([k, v]) => (
                <option key={k} value={k}>{v}</option>
              ))}
            </select>
          </label>
          <label className="field" style={{ flex: 1, minWidth: 260, marginBottom: 0 }}>
            <span>目录路径（支持 ~，递归扫描 .jsonl Session 文件）</span>
            <input
              type="text"
              value={path}
              placeholder="/path/to/another/codex-home 或 ~/somewhere/sessions"
              onChange={(e) => setPath(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && add()}
            />
          </label>
          <button className="btn primary" disabled={busy || !path.trim()} onClick={add}>
            {busy ? "添加中…" : "添加来源"}
          </button>
        </div>
      </div>

      {notice && <div className="badge accent" style={{ marginBottom: 10 }}>{notice}</div>}
      {error && <div className="badge warn" style={{ marginBottom: 10 }}>{error}</div>}

      <div className="card">
        {sources.length === 0 && <div className="muted small">暂无 Session 来源。</div>}
        {sources.map((src) => (
          <div key={src.id} className="row" style={{ padding: "8px 4px", borderBottom: "1px solid var(--border-subtle)", alignItems: "center", gap: 8 }}>
            <input
              type="checkbox"
              style={{ width: "auto" }}
              checked={src.enabled}
              disabled={syncing}
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
            {/* 徽标说的是这一行的配置状态，不是"此刻正在摄入"——
                真正的进行中状态由顶部的 progress 表达（§25：状态必须是真的）。 */}
            {src.enabled ? (
              <span className="badge accent">已启用</span>
            ) : (
              <span className="muted small">未启用</span>
            )}
            <button className="btn small" disabled={syncing} onClick={() => syncOne(src)}>
              同步
            </button>
            <button className="btn small ghost" disabled={syncing} onClick={() => reingest(src)}>
              重新摄入
            </button>
            {src.origin === "user" && (
              <button className="btn small ghost" disabled={syncing} onClick={() => remove(src)}>
                移除
              </button>
            )}
          </div>
        ))}
      </div>

      <p className="muted small" style={{ marginTop: 10 }}>
        「同步」按增量抓取新的 Session 内容；「重新摄入」从头重扫该来源的全部 Session 文件（事件 ID 与引用保持不变，用于修复游标异常或漏抓内容，已有的 Workstream 关联与 Context 保留）。
      </p>
    </div>
  );
}
