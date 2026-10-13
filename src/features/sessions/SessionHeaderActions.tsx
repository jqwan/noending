import { useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
import { capsOf } from "./sessionFormats";
import { continueSessionDesktopWithToast, desktopContinueState, useAgentStatus } from "./continueDesktop";
import type { Agent, AgentStatusEntry } from "../../types";

export function DesktopContinueButton({ sessionId, agent, sourceKind, archived, agentStatus, className = "btn ghost icon-button", label = "在桌面应用中继续" }: {
  sessionId: string; agent: Agent; sourceKind?: string; archived: boolean;
  agentStatus?: Record<string, AgentStatusEntry> | null; className?: string; label?: string;
}) {
  const localStatus = useAgentStatus(agentStatus === undefined);
  const status = agentStatus === undefined ? localStatus : agentStatus;
  const [busy, setBusy] = useState(false);
  if (!capsOf(agent, sourceKind).desktop) return null;
  const availability = desktopContinueState({ agent, source_kind: sourceKind, archived_at: archived ? "archived" : null }, status?.[agent]);
  return <button className={className} aria-label={label} title={availability.title}
    disabled={busy || availability.disabled} onClick={(e) => {
      e.stopPropagation();
      if (busy) return;
      setBusy(true);
      void continueSessionDesktopWithToast(sessionId).finally(() => setBusy(false));
    }}><Icon name="desktop" /></button>;
}

/** Record/detail actions. Terminal navigation lives in the shared titlebar switcher. */
export default function SessionHeaderActions({ sessionId, agent, sourceKind, title, archived, onChanged, onPurge, onRestore }: {
  sessionId: string; agent: Agent; sourceKind: string | undefined; title: string;
  archived: boolean; onChanged: () => void;
  onPurge?: () => void; onRestore?: () => void;
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
  const handleRestore = async () => {
    if (archiveBusy) return;
    if (onRestore) {
      onRestore();
      return;
    }
    setArchiveBusy(true);
    try {
      await api.restoreSession(sessionId);
      showToast("已恢复");
      onChanged();
    } catch (e) {
      showToast(`恢复失败：${String(e)}`);
    } finally {
      setArchiveBusy(false);
    }
  };
  return <>
    <DesktopContinueButton sessionId={sessionId} agent={agent} sourceKind={sourceKind} archived={archived} />
    <button className="btn ghost icon-button" aria-label="增量同步" title="同步最新对话" disabled={syncing}
      onClick={() => void handleSync()}><Icon name="refresh" /></button>
    {!archived ? (
      <button className="btn ghost icon-button" aria-label="归档" title="归档"
        disabled={archiveBusy} onClick={() => setConfirmArchive(true)}><Icon name="archive" /></button>
    ) : (
      <>
        <button className="btn ghost icon-button" aria-label="取消归档" title="取消归档"
          disabled={archiveBusy} onClick={() => void handleRestore()}><Icon name="unarchive" /></button>
        {onPurge && (
          <button className="btn ghost icon-button danger" aria-label="永久删除" title="永久删除"
            disabled={archiveBusy} onClick={onPurge}><Icon name="trash" /></button>
        )}
      </>
    )}
      {confirmArchive && (
        <Modal title="归档" onClose={() => { if (!archiveBusy) setConfirmArchive(false); }}>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            <b>{title}</b> 会移至「已归档」，归档后不能通过 NoEnding 继续。消息同步、搜索和摘要更新保持可用。
          </p>
          <div className="card hairline" style={{ marginBottom: 12 }}>
            <p style={{ margin: "0 0 6px" }}>
              <b>从任务移除</b> = 只取消这条会话对该任务的跟踪，会话本身留在列表里。
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
