import React, { useEffect, useRef, useState } from "react";
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
  const [tokenEstimate, setTokenEstimate] = useState<number>(0);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const preparedRef = useRef<PreparedLaunch | null>(null);

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
      .then(async (p) => {
        if (!p) return;
        // P2: 如果异步返回前组件已卸载，取消该准备，防止孤儿 preparation
        if (cancelled) {
          api.cancelPrepared(p.id).catch(console.error);
          return;
        }
        preparedRef.current = p;
        setPrepared(p);
        setTokenEstimate(p.bundle.approx_tokens);

        // UX: Prepare 内部会执行 session sync，在此刷新 session detail 确保 UI binding 与 prepare 一致
        try {
          const freshDetail = await api.getSessionDetail(sessionId);
          if (!cancelled) {
            setDetail(freshDetail);
          }
        } catch (e) {
          console.error(e);
        }
      })
      .catch((err) => {
        if (!cancelled) setError(String(err));
      })
      .finally(() => {
        if (!cancelled) setPreparing(false);
      });

    return () => {
      cancelled = true;
      // 卸载时释放持有的 preparation
      if (preparedRef.current) {
        api.cancelPrepared(preparedRef.current.id).catch(console.error);
        preparedRef.current = null;
      }
    };
  }, [sessionId]);

  const handleClose = () => {
    if (preparedRef.current) {
      api.cancelPrepared(preparedRef.current.id).catch(console.error);
      preparedRef.current = null;
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
        preparedRef.current = p;
        setPrepared(p);
        setTokenEstimate(p.bundle.approx_tokens);
        setPreviewOpen(true);
        const refreshed = await api.getSessionDetail(sessionId);
        setDetail(refreshed);
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

    // P1: 任何 launchPrepared 调用之后，无论成功失败，原 PreparedLaunch 都不得再次使用
    const current = preparedRef.current;
    preparedRef.current = null;
    setPrepared(null);

    try {
      const res = current
        ? await api.launchPrepared(current.id)
        : await api.launchResumeSession(sessionId, []);
      announceLaunch("恢复", res);
      onClose();
    } catch (e: unknown) {
      const errStr = String(e);
      const isStaleOrExpired =
        errStr.includes("stale") ||
        errStr.includes("过期") ||
        errStr.includes("已被使用") ||
        errStr.includes("不存在") ||
        errStr.includes("变化");

      if (isStaleOrExpired) {
        // stale / expired 时自动重新 prepareResumeSession 并刷新 session detail
        setPreparing(true);
        try {
          const fresh = await doPrepare();
          if (fresh) {
            preparedRef.current = fresh;
            setPrepared(fresh);
            setTokenEstimate(fresh.bundle.approx_tokens);
            const freshDetail = await api.getSessionDetail(sessionId);
            setDetail(freshDetail);
          }
          setError("Context 已发生变化，启动计划已刷新，请再次确认 Resume。");
        } catch (prepErr) {
          setError(`启动失败（${errStr}），且自动刷新失败：${String(prepErr)}`);
        } finally {
          setPreparing(false);
        }
      } else {
        // 其他错误同样重新生成有效 preparation 保持可用
        setPreparing(true);
        try {
          const fresh = await doPrepare();
          if (fresh) {
            preparedRef.current = fresh;
            setPrepared(fresh);
            setTokenEstimate(fresh.bundle.approx_tokens);
          }
        } catch (prepErr) {
          console.error(prepErr);
        } finally {
          setPreparing(false);
        }
        setError(errStr);
      }
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
  // 以 PreparedLaunch 中的有效 Workstream 为权威来源，若尚在 prepare 中则 fallback 到 bindings
  const effectiveWsIds = prepared
    ? prepared.workstream_ids
    : bindings.map(([b]) => b.workstream_id);
  const isNone = effectiveWsIds.length === 0;
  const isOff = deliveryLevel === "off";
  const approxTokens = prepared?.bundle.approx_tokens ?? tokenEstimate;

  const wsDisplay = isNone
    ? "未关联 Workstream"
    : bindings
        .filter(([b]) => effectiveWsIds.includes(b.workstream_id))
        .map(([, title]) => title)
        .filter(Boolean)
        .join(" · ") || `${effectiveWsIds.length} 个关联 Workstream`;

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
            {effectiveWsIds.length > 0 ? `${effectiveWsIds.length} 个` : "0 绑定"}
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
            disabled={busy || preparing}
            onClick={handleResume}
          >
            {busy ? "恢复中…" : preparing ? "准备中…" : "Resume"}
          </button>
        </div>
      </Modal>

      {previewOpen && prepared && (
        <ContextPreviewModal
          prepared={prepared}
          onClose={() => {
            setPreviewOpen(false);
            setPrepared(null);
            preparedRef.current = null;
            // Preview 关闭时其内部会 cancel 当前 ID，此处重新准备一份新的 preparation 保持就绪
            setPreparing(true);
            doPrepare()
              .then((fresh) => {
                if (fresh) {
                  preparedRef.current = fresh;
                  setPrepared(fresh);
                  setTokenEstimate(fresh.bundle.approx_tokens);
                }
              })
              .catch(console.error)
              .finally(() => setPreparing(false));
          }}
          onRefresh={async () => {
            const p = await doPrepare();
            if (p) {
              preparedRef.current = p;
              setPrepared(p);
              setTokenEstimate(p.bundle.approx_tokens);
              api.getSessionDetail(sessionId).then(setDetail).catch(console.error);
            }
            return p;
          }}
          onLaunched={() => {
            preparedRef.current = null;
            setPrepared(null);
            onClose();
          }}
        />
      )}
    </>
  );
}
