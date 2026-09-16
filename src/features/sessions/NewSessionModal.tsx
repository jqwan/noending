import React, { useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { announceLaunch } from "../launcher/LaunchResultModal";
import ContextPreviewModal from "../launcher/ContextPreviewModal";
import {
  AGENT_LABELS,
  type Agent,
  type Workstream,
  type ContextDeliveryLevel,
  type PreparedLaunch,
} from "../../types";

/**
 * 全局 New Session（实施方案 §33/§41）：无 Workstream 前置。
 * 默认 Workstream = None、Agent = Default Agent，用户可直接 Start。
 * 启动成功后只弹 toast，「查看详情」是可选入口；Modal 直接关闭。
 */
export default function NewSessionModal({ onClose }: { onClose: () => void }) {
  const [workstreams, setWorkstreams] = useState<Workstream[]>([]);
  const [wsId, setWsId] = useState("none");
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [deliveryLevel, setDeliveryLevel] =
    useState<ContextDeliveryLevel>("balanced");
  const [prepared, setPrepared] = useState<PreparedLaunch | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    api
      .listWorkstreams()
      .then((ws) =>
        setWorkstreams(ws.filter((w) => w.visibility === "normal"))
      )
      .catch(console.error);
    api.getDefaultAgent().then(setDefaultAgent).catch(console.error);
    api.getContextDeliveryLevel().then(setDeliveryLevel).catch(console.error);
  }, []);

  const doPrepare = async (): Promise<PreparedLaunch | null> => {
    if (!defaultAgent) return null;
    const ids = wsId === "none" ? [] : [wsId];
    return await api.prepareNewSession(defaultAgent, ids);
  };

  const handleOpenPreview = async () => {
    if (!defaultAgent || preparing) return;
    setPreparing(true);
    setError("");
    try {
      const p = await doPrepare();
      if (p) {
        setPrepared(p);
        setPreviewOpen(true);
      }
    } catch (e: unknown) {
      setError(String(e));
    } finally {
      setPreparing(false);
    }
  };

  const start = async () => {
    if (!defaultAgent || busy) return;
    setBusy(true);
    setError("");
    try {
      const r = await api.launchNewSession(
        defaultAgent,
        wsId === "none" ? [] : [wsId]
      );
      announceLaunch("启动", r);
      onClose();
    } catch (e: unknown) {
      setError(String(e));
      setBusy(false);
    }
  };

  const handleClose = () => {
    if (prepared) {
      api.cancelPrepared(prepared.id).catch(console.error);
      setPrepared(null);
    }
    onClose();
  };

  const handleWsChange = (newWsId: string) => {
    if (prepared) {
      api.cancelPrepared(prepared.id).catch(console.error);
      setPrepared(null);
    }
    setWsId(newWsId);
  };

  const isNone = wsId === "none";
  const isOff = deliveryLevel === "off";

  return (
    <>
      <Modal title="New Session" onClose={handleClose}>
        <label className="field">
          <span>Workstream（可不选，Session 之后由 Sync 自动关联）</span>
          <select value={wsId} onChange={(e) => handleWsChange(e.target.value)}>
            <option value="none">None（直接开始）</option>
            {workstreams.map((w) => (
              <option key={w.id} value={w.id}>
                {w.title}
              </option>
            ))}
          </select>
        </label>

        {/* Context Visibility Row */}
        <div className="row-line" style={{ borderTop: 0 }}>
          <div>
            <div className="settings-row-label">Context Delivery</div>
            <div className="settings-row-hint">
              {isNone
                ? "未选择 Workstream · 0 tokens"
                : isOff
                ? "Context Delivery 已关闭 (Off) · 不注入"
                : `预计注入 Context · ${
                    deliveryLevel.charAt(0).toUpperCase() +
                    deliveryLevel.slice(1)
                  }`}
            </div>
          </div>
          <div className="row" style={{ gap: 8 }}>
            {!isNone && !isOff && (
              <button
                type="button"
                className="btn small"
                disabled={preparing || !defaultAgent}
                onClick={handleOpenPreview}
              >
                {preparing ? "准备中…" : "预览 Context"}
              </button>
            )}
          </div>
        </div>

        <div className="row-line" style={{ borderTop: 0 }}>
          <div>
            <div className="settings-row-label">Agent</div>
            <div className="settings-row-hint">
              {defaultAgent
                ? "使用 Settings 中的 Default Agent"
                : "未检测到可用的 Agent CLI — 请先在 Settings → Agents 配置"}
            </div>
          </div>
          {defaultAgent && (
            <span className="badge accent ws-btn" style={{ gap: 6 }}>
              <AgentIcon agent={defaultAgent} />
              {AGENT_LABELS[defaultAgent]}
            </span>
          )}
        </div>
        {error && (
          <div className="badge warn" style={{ marginTop: 8 }}>
            {error}
          </div>
        )}
        <div
          className="row"
          style={{ justifyContent: "flex-end", marginTop: 14 }}
        >
          <button className="btn" onClick={handleClose}>
            取消
          </button>
          <button
            className="btn primary"
            disabled={busy || !defaultAgent}
            onClick={start}
          >
            {busy ? "启动中…" : "Start"}
          </button>
        </div>
      </Modal>

      {previewOpen && prepared && (
        <ContextPreviewModal
          prepared={prepared}
          onClose={() => {
            setPreviewOpen(false);
            setPrepared(null);
          }}
          onRefresh={doPrepare}
          onLaunched={onClose}
        />
      )}
    </>
  );
}
