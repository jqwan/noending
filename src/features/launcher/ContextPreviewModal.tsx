import React, { useState } from "react";
import { Modal } from "../../components/common";
import { api } from "../../api";
import { announceLaunch } from "./LaunchResultModal";
import type { PreparedLaunch } from "../../types";
import { KIND_LABELS, AUTHORITY_LABELS } from "../../types";
import { RuntimeIntentBadges } from "../settings/AgentRuntimeSettings";

interface Props {
  prepared: PreparedLaunch;
  onClose: () => void;
  onRefresh: () => Promise<PreparedLaunch | null>;
  onLaunched?: () => void;
}

export default function ContextPreviewModal({
  prepared: initialPrepared,
  onClose,
  onRefresh,
  onLaunched,
}: Props) {
  const [prepared, setPrepared] = useState<PreparedLaunch>(initialPrepared);
  const [activeTab, setActiveTab] = useState<"markdown" | "sections">("markdown");
  const [busy, setBusy] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [staleError, setStaleError] = useState<string | null>(null);

  const handleClose = () => {
    api.cancelPrepared(prepared.id).catch(console.error);
    onClose();
  };

  const handleLaunch = async () => {
    if (busy) return;
    setBusy(true);
    setStaleError(null);
    try {
      const res = await api.launchPrepared(prepared.id);
      announceLaunch("启动", res);
      onLaunched?.();
      onClose();
    } catch (e: unknown) {
      const msg = String(e);
      if (msg.includes("stale") || msg.includes("过期") || msg.includes("变化")) {
        setStaleError("底层 Context 或 Runtime 配置已发生变化（Stale），请刷新预览后重新启动。");
      } else {
        setStaleError(msg);
      }
      setBusy(false);
    }
  };

  const handleRefresh = async () => {
    setRefreshing(true);
    setStaleError(null);
    const oldId = prepared.id;
    try {
      const refreshed = await onRefresh();
      if (refreshed) {
        setPrepared(refreshed);
        if (oldId !== refreshed.id) {
          api.cancelPrepared(oldId).catch(console.error);
        }
      }
    } catch (e: unknown) {
      setStaleError(`刷新失败: ${String(e)}`);
    } finally {
      setRefreshing(false);
    }
  };

  const isOff = prepared.delivery_level === "off";
  const isEmpty = prepared.bundle.sections.length === 0;

  return (
    <Modal title="Context 准备就绪预览" onClose={handleClose}>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 14 }}>
        <span className="badge accent">模式: {prepared.mode === "new" ? "New Session" : "Resume"}</span>
        <span className="badge">等级: {prepared.delivery_level}</span>
        <span className="badge">
          Workstreams: {prepared.workstream_ids.length > 0 ? `${prepared.workstream_ids.length} 个` : "无 (0 绑定)"}
        </span>
        <span className="badge info">估算 Token: ~{prepared.bundle.approx_tokens}</span>
        <RuntimeIntentBadges agent={prepared.agent} runtime={prepared.runtime} />
      </div>

      {staleError && (
        <div
          className="badge warn"
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "8px 12px",
            marginBottom: 12,
            width: "100%",
            boxSizing: "border-box",
          }}
        >
          <span>{staleError}</span>
          <button
            className="btn small"
            style={{ marginLeft: 8 }}
            disabled={refreshing}
            onClick={handleRefresh}
          >
            {refreshing ? "刷新中…" : "刷新预览"}
          </button>
        </div>
      )}

      {isOff ? (
        <div className="card" style={{ padding: 16, color: "var(--text-muted)" }}>
          Context Delivery 当前已关闭 (Off)。本次启动不会注入任何上下文文件。
        </div>
      ) : isEmpty ? (
        <div className="card" style={{ padding: 16, color: "var(--text-muted)" }}>
          当前未选择 Workstream 或该 Workstream 暂无可交付的上下文内容。
        </div>
      ) : (
        <>
          <div className="row" style={{ gap: 8, marginBottom: 10 }}>
            <button
              className={`btn small ${activeTab === "markdown" ? "primary" : ""}`}
              onClick={() => setActiveTab("markdown")}
            >
              Markdown 内容
            </button>
            <button
              className={`btn small ${activeTab === "sections" ? "primary" : ""}`}
              onClick={() => setActiveTab("sections")}
            >
              条目详情 ({prepared.bundle.sections.length})
            </button>
          </div>

          {activeTab === "markdown" ? (
            <div
              className="card mono small"
              style={{
                whiteSpace: "pre-wrap",
                maxHeight: 280,
                overflowY: "auto",
                lineHeight: 1.5,
              }}
            >
              {prepared.bundle.markdown}
            </div>
          ) : (
            <div style={{ maxHeight: 280, overflowY: "auto", display: "flex", flexDirection: "column", gap: 6 }}>
              {prepared.bundle.sections.map((s, idx) => (
                <div key={idx} className="card small" style={{ padding: "8px 12px" }}>
                  <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 4 }}>
                    <strong>{s.title || KIND_LABELS[s.kind] || s.kind}</strong>
                    <span className="badge small">{s.kind}</span>
                  </div>
                  <div style={{ color: "var(--text-muted)", fontSize: "0.85em" }}>
                    {s.content.length > 120 ? `${s.content.slice(0, 120)}…` : s.content}
                  </div>
                  {s.authority && (
                    <div style={{ marginTop: 4, fontSize: "0.8em", color: "var(--text-muted)" }}>
                      权威来源: {AUTHORITY_LABELS[s.authority] || s.authority}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </>
      )}

      <div className="row" style={{ justifyContent: "space-between", marginTop: 18 }}>
        <button
          className="btn"
          disabled={refreshing || busy}
          onClick={handleRefresh}
        >
          {refreshing ? "刷新中…" : "刷新预览"}
        </button>

        <div className="row" style={{ gap: 8 }}>
          <button className="btn" onClick={handleClose} disabled={busy}>
            取消
          </button>
          <button
            className="btn primary"
            disabled={busy || refreshing || !!staleError}
            onClick={handleLaunch}
          >
            {busy ? "启动中…" : "确认并立即启动"}
          </button>
        </div>
      </div>
    </Modal>
  );
}
