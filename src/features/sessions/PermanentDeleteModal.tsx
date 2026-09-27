import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import { agentDisplayLabel, sessionDisplayTitle } from "./SessionTable";
import type { LocalDeletePreview, SourceAvailability } from "../../types";

/**
 * 回收站里的「删除」确认弹窗：打开即读预览（新鲜结论 + 计数），确认后执行
 * `permanently_delete_session`。动作名统一叫「删除」，源状态只决定确认前的那句结果说明——
 * 源还在就说明会重新入库，源没了就说明无法找回。
 */

/** 大数加千位分隔。 */
const fmtCount = (n: number) => n.toLocaleString("en-US");

/** 源状态决定的说明：弹窗里的警示 + 结果段，以及删除后 toast 的收尾。 */
function outcome(status: SourceAvailability): { warning: string | null; detail: string; toast: string } {
  switch (status) {
    case "present":
      return {
        warning: "Root 源会话仍然存在：删掉的是 NoEnding 这份副本。",
        detail:
          "源文件不会被删除。下一次同步会从它重新摄入——新的会话 id、没有所属任务、没有摘要，"
          + "启动记录也不会恢复。想让它彻底消失，需要删除源会话文件。",
        toast: "；源会话仍在，下次同步会作为新会话重新入库",
      };
    case "missing":
      return {
        warning: null,
        detail: "Root 源会话已不存在——本地数据删除后无法找回。",
        toast: "；源会话已不存在，无法找回",
      };
    default:
      return {
        warning: "无法确认 Root 源会话的状态。",
        detail:
          "源文件不会被删除；如果它其实还在，下一次同步会把这个会话作为新会话重新入库。",
        toast: "",
      };
  }
}

export default function PermanentDeleteModal({ sessionId, onClose, onDeleted }: {
  sessionId: string;
  onClose: () => void;
  /** 删除成功后调用（此时弹窗不再走 onClose）：父级负责收尾与刷新。 */
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

  /** 预览没到之前不渲染，所以这里的兜底分支只服务类型。 */
  const note = outcome(preview?.root_source_status ?? "unavailable");

  const execute = async () => {
    if (busy || !preview) return;
    setBusy(true);
    setErrorText(null);
    try {
      const r = await api.permanentlyDeleteSession(sessionId);
      const redaction =
        r.redacted_revisions > 0
          ? `，并脱敏了 ${fmtCount(r.redacted_revisions)} 条 Context 来源`
          : "";
      showToast(`已删除本地数据${redaction}${note.toast}`);
      onDeleted?.();
      onClose();
    } catch (e) {
      console.error(e);
      setBusy(false);
      setErrorText(`删除失败：${String(e)}`);
    }
  };

  /** 删除进行中禁止 Escape / 背板关闭：中途消失会让人不知道结果。 */
  const requestClose = () => {
    if (!busy) onClose();
  };

  return (
    <Modal title="删除" onClose={requestClose}>
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
            将删除 <b>{sessionDisplayTitle(preview.session_title)}</b>
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

          <div style={{ margin: "10px 0 0", maxWidth: "72ch" }}>
            {note.warning && <div className="session-source-warning">{note.warning}</div>}
            <p className="muted small" style={{ margin: "4px 0 0" }}>{note.detail}</p>
          </div>

          <p className="muted small" style={{ margin: "10px 0 0", maxWidth: "72ch" }}>
            任务及其 Context 内容不受影响；受影响 Context 的「来源」将显示为「来源会话已删除」。
          </p>

          <div className="row" style={{ justifyContent: "flex-end", marginTop: 16 }}>
            <button className="btn" disabled={busy} onClick={onClose}>取消</button>
            <button
              className="btn danger"
              disabled={busy}
              onClick={execute}
            >
              {busy ? "删除中…" : "删除"}
            </button>
          </div>
        </>
      )}
    </Modal>
  );
}
