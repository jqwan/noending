import { useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
import { capsOf } from "./sessionFormats";
import { continueSessionDesktopWithToast, desktopContinueState, useAgentStatus } from "./continueDesktop";
import type { Agent } from "../../types";

export function DesktopContinueButton({ sessionId, agent, sourceKind, archived }: {
  sessionId: string; agent: Agent; sourceKind?: string; archived: boolean;
}) {
  const status = useAgentStatus();
  const [busy, setBusy] = useState(false);
  if (!capsOf(agent, sourceKind).desktop) return null;
  const availability = desktopContinueState({ agent, source_kind: sourceKind, archived_at: archived ? "archived" : null }, status?.[agent]);
  return <button className="btn ghost icon-button" aria-label="在桌面应用中继续" title={availability.title}
    disabled={busy || availability.disabled} onClick={() => {
      if (busy) return;
      setBusy(true);
      void continueSessionDesktopWithToast(sessionId).finally(() => setBusy(false));
    }}><Icon name="desktop" /></button>;
}

/** Record/detail actions. Terminal navigation lives in the shared titlebar switcher. */
export default function SessionHeaderActions({ sessionId, agent, sourceKind, title, archived, onChanged }: {
  sessionId: string; agent: Agent; sourceKind: string | undefined; title: string;
  archived: boolean; onChanged: () => void;
}) {
  const [confirmArchive, setConfirmArchive] = useState(false);
  const [archiveBusy, setArchiveBusy] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const handleSync = async () => {
    if (syncing) return;
    setSyncing(true);
    try { await api.refreshSession(sessionId); showToast("正在同步最新对话…"); onChanged(); }
    catch (e) { showToast(`同步失败：${String(e)}`); }
    finally { setSyncing(false); }
  };
  const doArchive = async () => {
    setArchiveBusy(true);
    try { await api.archiveSession(sessionId); showToast("已归档"); setConfirmArchive(false); onChanged(); }
    catch (e) { showToast(`归档失败：${String(e)}`); }
    finally { setArchiveBusy(false); }
  };
  return <>
    <DesktopContinueButton sessionId={sessionId} agent={agent} sourceKind={sourceKind} archived={archived} />
    <span className="session-action-divider" aria-hidden="true" />
    <button className="btn ghost icon-button" aria-label="增量同步" title="同步最新对话" disabled={syncing}
      onClick={() => void handleSync()}><Icon name="refresh" /></button>
    <button className="btn ghost icon-button" aria-label="归档" title={archived ? "该会话已归档" : "归档"}
      disabled={archived || archiveBusy} onClick={() => setConfirmArchive(true)}><Icon name="archive" /></button>
      {confirmArchive && (
        <Modal title="归档" onClose={() => { if (!archiveBusy) setConfirmArchive(false); }}>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            <b>{title}</b> 会移至「已归档」，归档后不能通过 NoEnding 继续。消息同步、搜索和摘要更新保持可用。
          </p>
          {/* 要求把两个概念摆在同一处明确区分：「从任务移除」只改这条会话的
              所属任务（详情页的「更改 / 选择」），这里是全局已归档。 */}
          <div className="card hairline" style={{ marginBottom: 12 }}>
            <p style={{ margin: "0 0 6px" }}>
              <b>从任务移除</b> = 只修改这条会话的所属任务，会话本身留在列表里。
            </p>
            <p style={{ margin: 0 }}>
              <b>归档</b> = 将会话移至「已归档」。Agent 原始会话不会被删除。
            </p>
          </div>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirmArchive(false)} disabled={archiveBusy}>取消</button>
            <button className="btn primary" onClick={doArchive} disabled={archiveBusy}>
              {archiveBusy ? "处理中…" : "归档"}
            </button>
          </div>
        </Modal>
      )}
  </>;
}
