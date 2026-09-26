import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import { agentDisplayLabel, sessionDisplayTitle } from "./SessionTable";
import type { LocalDeletePreview } from "../../types";

/**
 * Session 永久删除：无状态的本地清除。打开即读删除预览（新鲜结论 + 计数），确认后执行
 * `permanently_delete_session`。没有 job、没有取消、没有重试状态机：后端只在
 * （trashed + fresh root missing）时允许执行，且只删 NoEnding 本地数据——Agent 源会话不被触碰。
 */

/** 大数加千位分隔。 */
const fmtCount = (n: number) => n.toLocaleString("en-US");

/** can_permanently_delete = false 时的原因（只由 root_source_status 决定）。 */
function unavailableWhy(status: LocalDeletePreview["root_source_status"]): string {
  return status === "present"
    ? "Root 源会话仍然存在。"
    : "无法确认 Root 源会话的状态。";
}

export default function PermanentDeleteModal({ sessionId, onClose, onDeleted }: {
  sessionId: string;
  onClose: () => void;
  /** 永久删除成功后调用（此时弹窗不再走 onClose）：父级负责收尾与刷新。 */
  onDeleted?: () => void;
}) {
  const [loading, setLoading] = useState(true);
  const [preview, setPreview] = useState<LocalDeletePreview | null>(null);
  /** 预览读取失败 / 执行失败的错误文本；null = 无错误。 */
  const [errorText, setErrorText] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    setLoading(true);
    setPreview(null);
    setErrorText(null);
    api.getSessionLocalDeletePreview(sessionId)
      .then((p) => { setPreview(p); setLoading(false); })
      .catch((e) => {
        console.error(e);
        setErrorText(String(e));
        setLoading(false);
      });
  }, [sessionId]);
  useEffect(() => { load(); }, [load]);

  const execute = async () => {
    if (busy || !preview?.can_permanently_delete) return;
    setBusy(true);
    setErrorText(null);
    try {
      const r = await api.permanentlyDeleteSession(sessionId);
      showToast(
        r.redacted_revisions > 0
          ? `已永久删除本地数据，并脱敏了 ${fmtCount(r.redacted_revisions)} 条 Context 来源`
          : "已永久删除本地数据",
      );
      onDeleted?.();
      onClose();
    } catch (e) {
      console.error(e);
      setBusy(false);
      setErrorText(`永久删除失败：${String(e)}`);
    }
  };

  /** 删除进行中禁止 Escape / 背板关闭：中途消失会让人不知道结果。 */
  const requestClose = () => {
    if (!busy) onClose();
  };

  return (
    <Modal title="永久删除" onClose={requestClose}>
      {loading && (
        <div className="muted" style={{ padding: "12px 0" }}>
          正在读取删除预览…
        </div>
      )}

      {!loading && errorText !== null && (
        <>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10, overflowWrap: "anywhere" }}>
            {errorText}
          </div>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={onClose}>关闭</button>
            <button className="btn primary" onClick={load}>重试</button>
          </div>
        </>
      )}

      {!loading && errorText === null && preview && (
        <>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            将永久删除 <b>{sessionDisplayTitle(preview.session_title)}</b>
            <span className="muted">（{agentDisplayLabel(preview.agent)}）</span> 在 NoEnding 中的本地数据：
          </p>

          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
            只删除 NoEnding 本地数据，不会删除 Agent 数据。
          </div>

          <div className="section-label" style={{ margin: "14px 0 2px" }}>将删除</div>
          <ul className="purge-list">
            <li>{fmtCount(preview.message_count)} 条会话消息</li>
            <li>{fmtCount(preview.member_count)} 个执行成员</li>
            <li>{fmtCount(preview.sync_run_count)} 条同步记录</li>
            <li>{fmtCount(preview.launch_intent_count)} 条启动记录</li>
            <li>上下文来源改写 {fmtCount(preview.context_revision_redaction_count)} 条</li>
          </ul>

          {preview.can_permanently_delete ? (
            <p className="muted small" style={{ margin: "10px 0 0", maxWidth: "72ch" }}>
              Root 源会话已不存在——本地数据删除后无法找回。
            </p>
          ) : (
            <div className="small" style={{ margin: "10px 0 0", maxWidth: "72ch" }}>
              <div className="session-source-warning">
                永久删除不可用：{unavailableWhy(preview.root_source_status)}
              </div>
              <p className="muted small" style={{ margin: "4px 0 0" }}>
                会话仍保留在回收站，可以随时恢复。
              </p>
            </div>
          )}

          <p className="muted small" style={{ margin: "10px 0 0", maxWidth: "72ch" }}>
            任务及其 Context 内容不受影响；受影响 Context 的「来源」将显示为「来源会话已永久删除」。
          </p>

          <div className="row" style={{ justifyContent: "flex-end", marginTop: 16 }}>
            <button className="btn" disabled={busy} onClick={onClose}>取消</button>
            <button
              className="btn danger"
              disabled={busy || !preview.can_permanently_delete}
              onClick={execute}
            >
              {busy ? "删除中…" : "永久删除"}
            </button>
          </div>
        </>
      )}
    </Modal>
  );
}
