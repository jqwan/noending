import React, { useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { announceLaunch } from "../launcher/LaunchResultModal";
import ContextPreviewModal from "../launcher/ContextPreviewModal";
import {
  AGENT_LABELS,
  type SessionDetail,
  type ContextDeliveryLevel,
  type PreparedLaunch,
} from "../../types";

interface Props {
  sessionId: string;
  onClose: () => void;
}

export default function ResumeSessionModal({ sessionId, onClose }: Props) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [deliveryLevel, setDeliveryLevel] =
    useState<ContextDeliveryLevel>("balanced");
  const [prepared, setPrepared] = useState<PreparedLaunch | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const doPrepare = async (): Promise<PreparedLaunch | null> => {
    return await api.prepareResumeSession(sessionId, []);
  };

  useEffect(() => {
    let cancelled = false;
    api
      .getSessionDetail(sessionId)
      .then((d) => {
        if (!cancelled) setDetail(d);
      })
      .catch(console.error);

    api
      .getContextDeliveryLevel()
      .then((lvl) => {
        if (!cancelled) setDeliveryLevel(lvl);
      })
      .catch(console.error);

    setPreparing(true);
    doPrepare()
      .then((p) => {
        if (!cancelled && p) setPrepared(p);
      })
      .catch((err) => {
        if (!cancelled) setError(String(err));
      })
      .finally(() => {
        if (!cancelled) setPreparing(false);
      });

    return () => {
      cancelled = true;
    };
  }, [sessionId]);

  const handleClose = () => {
    if (prepared) {
      api.cancelPrepared(prepared.id).catch(console.error);
      setPrepared(null);
    }
    onClose();
  };

  const handleOpenPreview = async () => {
    if (previewOpen) return;
    if (prepared) {
      setPreviewOpen(true);
      return;
    }
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

  const handleResume = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const res = prepared
        ? await api.launchPrepared(prepared.id)
        : await api.launchResumeSession(sessionId, []);
      announceLaunch("恢复", res);
      onClose();
    } catch (e: unknown) {
      setError(String(e));
      setBusy(false);
    }
  };

  if (!detail) {
    return (
      <Modal title="Resume Session" onClose={handleClose}>
        <div style={{ padding: "20px 0", color: "var(--text-muted)" }}>
          加载会话信息中…
        </div>
      </Modal>
    );
  }

  const { session, bindings } = detail;
  const isNone = bindings.length === 0;
  const isOff = deliveryLevel === "off";
  const approxTokens = prepared?.bundle.approx_tokens ?? 0;

  const wsDisplay = isNone
    ? "未关联 Workstream"
    : bindings.map(([, title]) => title).filter(Boolean).join(" · ") ||
      `${bindings.length} 个关联 Workstream`;

  return (
    <>
      <Modal title="Resume Session" onClose={handleClose}>
        {/* Agent Row */}
        <div className="row-line" style={{ borderTop: 0 }}>
          <div>
            <div className="settings-row-label">Agent</div>
            <div className="settings-row-hint">Session 所使用的执行 Agent</div>
          </div>
          <span className="badge accent ws-btn" style={{ gap: 6 }}>
            <AgentIcon agent={session.agent} />
            {AGENT_LABELS[session.agent]}
          </span>
        </div>

        {/* Workstreams Row */}
        <div className="row-line">
          <div>
            <div className="settings-row-label">Workstreams</div>
            <div className="settings-row-hint">{wsDisplay}</div>
          </div>
          <span className="badge">
            {bindings.length > 0 ? `${bindings.length} 个` : "0 绑定"}
          </span>
        </div>

        {/* Context Visibility Row */}
        <div className="row-line">
          <div>
            <div className="settings-row-label">Context Delivery</div>
            <div className="settings-row-hint">
              {isNone
                ? "No Workstream · 0 tokens"
                : isOff
                ? "Off · 不注入"
                : `${
                    deliveryLevel.charAt(0).toUpperCase() +
                    deliveryLevel.slice(1)
                  } · ~${approxTokens} tokens`}
            </div>
          </div>
          <div className="row" style={{ gap: 8 }}>
            {!isNone && !isOff && (
              <button
                type="button"
                className="btn small"
                disabled={preparing}
                onClick={handleOpenPreview}
              >
                {preparing ? "准备中…" : "Preview"}
              </button>
            )}
          </div>
        </div>

        {error && (
          <div className="badge warn" style={{ marginTop: 8 }}>
            {error}
          </div>
        )}

        <div
          className="row"
          style={{ justifyContent: "flex-end", marginTop: 18 }}
        >
          <button className="btn" onClick={handleClose} disabled={busy}>
            Cancel
          </button>
          <button
            className="btn primary"
            disabled={busy}
            onClick={handleResume}
          >
            {busy ? "恢复中…" : "Resume"}
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
