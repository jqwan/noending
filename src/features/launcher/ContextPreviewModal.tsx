import { useEffect, useState } from "react";
import { Modal } from "../../components/common";
import { api } from "../../api";
import { useDeliveryOff } from "../../app/experience";
import { announceLaunch } from "./LaunchResultModal";
import { deliveryLevelLabel, runtimeIntentText } from "./LaunchPreviewRows";
import type { PreparedLaunch } from "../../types";
import { KIND_LABELS, AUTHORITY_LABELS } from "../../types";

interface Props {
  prepared: PreparedLaunch;
  onClose: () => void;
  onRefresh: () => Promise<PreparedLaunch | null>;
  onLaunched?: () => void;
}

/**
 * Context 注入预览；只有 Delivery 未关闭时才可能被打开。
 *
 * 这里显示的一切都取自 PreparedLaunch 冻结的那份 bundle：预览的就是 Agent
 * 真正会收到的东西。关闭注入时本组件整体不挂载（不是 CSS 隐藏），也不显示
 * 任何 token 计数或「已关闭」字样。
 */
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
  const deliveryOff = useDeliveryOff();

  const handleClose = () => {
    api.cancelPrepared(prepared.id).catch(console.error);
    onClose();
  };

  // 预览开着的时候注入被关掉：自己先回收令牌，再把关闭权交出去。
  // 调用方收到 onClose 时认为令牌已由预览释放，少这一句就会留下一个
  // 只能等 TTL 清理的 preparation。
  useEffect(() => {
    if (!deliveryOff) return;
    api.cancelPrepared(prepared.id).catch(console.error);
    onClose();
  }, [deliveryOff, prepared.id, onClose]);

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
        setStaleError("状态已变化：Context 或 Runtime 与预览不一致，请刷新预览后重新启动。");
      } else {
        setStaleError(msg);
      }
      setBusy(false);
    }
  };

  const handleRefresh = async () => {
    setRefreshing(true);
    setStaleError(null);
    try {
      const refreshed = await onRefresh();
      if (refreshed) {
        setPrepared(refreshed);
      }
    } catch (e: unknown) {
      setStaleError(`刷新失败: ${String(e)}`);
    } finally {
      setRefreshing(false);
    }
  };

  const isEmpty = prepared.bundle.sections.length === 0;

  if (deliveryOff) return null;

  return (
    <Modal title="Context 注入预览" onClose={handleClose}>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 14 }}>
        <span className="badge accent">
          {prepared.mode === "new" ? "新建会话" : "继续会话"}
        </span>
        <span className="badge">
          注入等级: {deliveryLevelLabel(prepared.delivery_level)}
        </span>
        <span className="badge">
          所属任务：{" "}
          {prepared.owner_workstream_id === null ? "未归属" : "1 个"}
        </span>
        <span className="badge">
          工作目录: {prepared.cwd || "未指定"}
        </span>
        <span className="badge">
          Runtime: {runtimeIntentText(prepared.agent, prepared.runtime)}
        </span>
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

      {isEmpty ? (
        <div className="card" style={{ padding: 16, color: "var(--text-muted)" }}>
          当前未选择任务，或该任务暂无可交付的 Context 内容。
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
