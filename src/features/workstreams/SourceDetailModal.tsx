import React, { useEffect, useState } from "react";
import { api } from "../../api";
import { Modal, timeAgo } from "../../components/common";
import AgentIcon from "../../components/AgentIcon";
import { AGENT_LABELS, AUTHORITY_LABELS, type ContextSourceDetail } from "../../types";

interface Props {
  revisionId: string;
  onClose: () => void;
  onNavigateSession?: (sessionId: string) => void;
}

export default function SourceDetailModal({
  revisionId,
  onClose,
  onNavigateSession,
}: Props) {
  const [detail, setDetail] = useState<ContextSourceDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    api
      .getContextRevisionSource(revisionId)
      .then((res) => {
        if (!active) return;
        setDetail(res);
      })
      .catch((e) => {
        if (!active) return;
        setError(String(e));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [revisionId]);

  return (
    <Modal title="Context 来源追溯 (Provenance)" onClose={onClose}>
      {loading && <div className="muted" style={{ padding: "16px 0" }}>正在解析来源凭据…</div>}
      {error && <div className="badge warn" style={{ marginBottom: 12 }}>{error}</div>}

      {!loading && !detail && !error && (
        <div className="muted small" style={{ padding: "16px 0" }}>
          未找到该修订版本的来源信息。
        </div>
      )}

      {!loading && detail && (
        <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          <div className="row-line" style={{ borderTop: 0 }}>
            <div>
              <div className="settings-row-label">可信度与权威 (Authority)</div>
              <div className="settings-row-hint">系统对该项当前事实的信度级别</div>
            </div>
            <span className="badge accent">
              {AUTHORITY_LABELS[detail.authority] ?? detail.authority}
            </span>
          </div>

          <div className="row-line">
            <div>
              <div className="settings-row-label">来源类型 (Source Type)</div>
              <div className="settings-row-hint">产生该修订版本的机制</div>
            </div>
            <span className="badge">
              {detail.source_type ?? "unknown"}
            </span>
          </div>

          {detail.agent && (
            <div className="row-line">
              <div>
                <div className="settings-row-label">提取 Agent</div>
                <div className="settings-row-hint">记录此信息的 Agent</div>
              </div>
              <span className="badge ws-btn" style={{ gap: 6 }}>
                <AgentIcon agent={detail.agent} />
                {AGENT_LABELS[detail.agent]}
              </span>
            </div>
          )}

          {detail.session_id && (
            <div className="row-line">
              <div>
                <div className="settings-row-label">所属 Session</div>
                <div className="settings-row-hint">
                  {detail.session_title ?? detail.session_id}
                </div>
              </div>
              {onNavigateSession && (
                <button
                  className="btn small ghost"
                  onClick={() => {
                    onClose();
                    onNavigateSession(detail.session_id!);
                  }}
                >
                  查看会话
                </button>
              )}
            </div>
          )}

          {detail.event_sequence !== null && detail.event_sequence !== undefined && (
            <div className="row-line">
              <div>
                <div className="settings-row-label">消息序号 (Sequence)</div>
                <div className="settings-row-hint">在源 Transcript 中的事件序号</div>
              </div>
              <span className="mono small">#{detail.event_sequence}</span>
            </div>
          )}

          {detail.event_ts && (
            <div className="row-line">
              <div>
                <div className="settings-row-label">观测时间 (Observed)</div>
                <div className="settings-row-hint">{detail.event_ts}</div>
              </div>
              <span className="small muted">{timeAgo(detail.event_ts)}</span>
            </div>
          )}

          {detail.evidence && (
            <div style={{ marginTop: 6 }}>
              <div className="section-label" style={{ margin: "0 0 6px" }}>原文证据 (Evidence)</div>
              <div
                style={{
                  padding: "10px 12px",
                  background: "var(--bg-panel)",
                  borderRadius: "var(--radius-sm)",
                  borderLeft: "3px solid var(--accent)",
                  fontSize: 13,
                  lineHeight: 1.5,
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-word",
                  fontFamily: "var(--font-mono, monospace)",
                }}
              >
                {detail.evidence}
              </div>
            </div>
          )}

          {detail.sync_run_id && (
            <div className="row-line">
              <div>
                <div className="settings-row-label">Sync Run ID</div>
                <div className="settings-row-hint">原子合并提交批次</div>
              </div>
              <span className="mono small">{detail.sync_run_id}</span>
            </div>
          )}

          {detail.source_ref && (
            <div className="row-line">
              <div>
                <div className="settings-row-label">内部引用 (Source Reference)</div>
                <div className="settings-row-hint">事件定位或引用标识符</div>
              </div>
              <span className="mono small">{detail.source_ref}</span>
            </div>
          )}
        </div>
      )}

      <div className="row" style={{ justifyContent: "flex-end", marginTop: 20 }}>
        <button className="btn" onClick={onClose}>关闭</button>
      </div>
    </Modal>
  );
}
