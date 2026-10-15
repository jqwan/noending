import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import { agentDisplayLabel, sessionDisplayTitle } from "./SessionTable";
import type { LocalDeletePreview, SourceAvailability } from "../../types";

/**
 * 已归档的「删除」确认弹窗：打开即读预览（新鲜结论 + 计数），确认后执行
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
        warning: "Root 源会话仍然存在：仅删除 NoEnding 本地副本。",
        detail:
          "原始 Agent 会话文件不会被删除。下一次同步会从它重新同步；若需彻底删除请清理源会话文件。",
        toast: "；源会话仍在，下次同步将重新摄入",
      };
    case "missing":
      return {
        warning: null,
        detail: "Root 源会话已不存在，本地数据删除后无法找回。",
        toast: "；源会话已不存在",
      };
    default:
      return {
        warning: "无法确认 Root 源会话的状态。",
        detail:
          "原始文件不会被删除；若源文件仍存在，下次同步将重新录入。",
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
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
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
      await api.permanentlyDeleteSession(sessionId);
      showToast(`已删除本地数据${note.toast}`);
      if (mounted.current) {
        if (onDeleted) onDeleted();
        else onClose();
      }
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
    <Modal title="永久删除这项会话？" onClose={requestClose}>
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
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>不可撤销</div>

          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            确定要永久删除会话 <b>{sessionDisplayTitle(preview.session_title)}</b>
            <span className="muted">（{agentDisplayLabel(preview.agent)}）</span> 的本地记录吗？
          </p>

          <ul className="purge-list" style={{ margin: "0 0 12px" }}>
            <li>{fmtCount(preview.message_count)} 条会话消息</li>
            <li>{fmtCount(preview.session_context_count)} 条 Context 摘要记录</li>
          </ul>

          <div style={{ margin: "10px 0 0", maxWidth: "72ch" }}>
            {note.warning && <div className="session-source-warning">{note.warning}</div>}
            <p className="muted small" style={{ margin: "4px 0 0" }}>{note.detail}</p>
          </div>

          <p className="muted small" style={{ margin: "10px 0 0", maxWidth: "72ch" }}>
            关联的内嵌终端将关闭并移除；关联任务及其 Context 内容不受影响。
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
