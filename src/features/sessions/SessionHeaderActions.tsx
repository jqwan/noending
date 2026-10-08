import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
import { capsOf } from "./sessionFormats";
import {
  continueSessionDesktopWithToast,
  continueSessionTerminal,
  desktopContinueState,
  getSavedContinueMode,
  setSavedContinueMode,
  useAgentStatus,
  type ContinueMode,
} from "./continueDesktop";
import type { Agent } from "../../types";
import type { Route } from "../../app/routes";

/**
 * 会话概览 / 对话共用的归档、同步和继续操作。继续按钮按格式能力显示桌面、
 * 终端或两者的分列按钮；共享偏好决定支持两者时的默认方式。
 * 已归档会话仍可同步，桌面和终端继续均禁用。终端使用原生身份查找已有连接，
 * 没有连接时再启动 Resume。
 */
export default function SessionHeaderActions({ sessionId, agent, sourceKind, title, archived, navigate, onChanged }: {
  sessionId: string;
  agent: Agent;
  /** 会话的 source_kind（antigravity 靠它区分两个存储）；未读到时按 agent 兜底。 */
  sourceKind: string | undefined;
  title: string;
  archived: boolean;
  navigate: (r: Route) => void;
  /** 已归档 / 同步完成后由页面刷新数据。 */
  onChanged: () => void;
}) {
  const [confirmArchive, setConfirmArchive] = useState(false);
  const [archiveBusy, setArchiveBusy] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [launching, setLaunching] = useState(false);
  const [terminalBusy, setTerminalBusy] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [continueMode, setContinueMode] = useState<ContinueMode>(() => getSavedContinueMode());
  const menuRef = useRef<HTMLDivElement>(null);
  const agentStatus = useAgentStatus();

  const caps = capsOf(agent, sourceKind);
  const canTerminal = caps.terminal;
  const canDesktop = caps.desktop;

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) setMenuOpen(false);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMenuOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [menuOpen]);

  const { disabled: launchDisabled, title: launchTitle } = archived
    ? { disabled: true, title: "已归档的会话不能继续；先恢复它。" }
    : desktopContinueState({ agent, source_kind: sourceKind ?? "" }, agentStatus?.[agent] ?? null);

  const handleLaunch = () => {
    if (launching || archived || launchDisabled) return;
    setLaunching(true);
    void continueSessionDesktopWithToast(sessionId).finally(() => setLaunching(false));
  };

  /** 查到已关联的活终端就跳转；没有才启动一次内嵌 Resume。 */
  const openTerminal = async () => {
    if (terminalBusy || archived) return;
    setTerminalBusy(true);
    try {
      // 共享跳转携带身份种子：终端首帧即显示本会话，不闪「新会话」。
      await continueSessionTerminal({ id: sessionId, title, agent }, navigate);
    } catch (e) {
      showToast(`终端不可用：${String(e)}`);
    } finally {
      setTerminalBusy(false);
    }
  };

  const handleSync = async () => {
    if (syncing) return;
    setSyncing(true);
    try {
      await api.refreshSession(sessionId);
      showToast("正在同步最新对话…");
      onChanged();
    } catch (e) {
      showToast(`同步失败：${String(e)}`);
    } finally {
      setTimeout(() => setSyncing(false), 500);
    }
  };

  const doArchive = async () => {
    setArchiveBusy(true);
    try {
      await api.archiveSession(sessionId);
      showToast("已归档");
      setConfirmArchive(false);
      onChanged();
    } catch (e) {
      showToast(`归档失败：${String(e)}`);
    } finally {
      setArchiveBusy(false);
    }
  };

  return (
    <>
      {!archived && <button
        className="btn ghost icon-button"
        aria-label="归档"
        title="归档"
        onClick={() => setConfirmArchive(true)}
        disabled={archiveBusy}
      >
        <Icon name="archive" />
      </button>}
      <button
        className="btn ghost icon-button"
        aria-label="增量同步"
        title="同步最新对话"
        onClick={() => void handleSync()}
        disabled={syncing}
      >
        <Icon name="refresh" />
      </button>

      {/* 聚合继续按钮 / 单独继续按钮：
          - 兼具桌面与终端能力（codex）：聚合分列按钮，可在两者之间切换；
          - 仅终端能力（claude_code / pi / antigravity_cli）：仅显示终端按钮；
          - 仅桌面能力（antigravity_desktop / dsh / qoder / workbuddy / zcode）：仅显示桌面按钮。 */}
      {canTerminal && canDesktop && (
        <div className="continue-split-btn" ref={menuRef}>
          {continueMode === "desktop" ? (
            <button
              className="continue-main-btn"
              aria-label="在桌面应用中继续"
              title={launchTitle}
              onClick={handleLaunch}
              disabled={launchDisabled || launching}
            >
              <Icon name="desktop" />
            </button>
          ) : (
            <button
              className="continue-main-btn"
              aria-label="在终端中继续"
              title="在终端中继续"
              disabled={terminalBusy || archived}
              onClick={() => void openTerminal()}
            >
              <Icon name="terminal" />
            </button>
          )}
          <span className="continue-divider" />
          <button
            className={`continue-dropdown-btn ${menuOpen ? "open" : ""}`}
            aria-label="切换继续方式"
            title="切换继续方式"
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen((o) => !o)}
          >
            <Icon name="chevronDown" />
          </button>

          {menuOpen && (
            <div className="continue-split-menu">
              <button
                className="continue-menu-item"
                disabled={launchDisabled || launching}
                title={launchTitle}
                onClick={() => {
                  setContinueMode("desktop");
                  setSavedContinueMode("desktop");
                  setMenuOpen(false);
                  handleLaunch();
                }}
              >
                <Icon name="desktop" />
                <span className="menu-label">在桌面应用中继续</span>
                {continueMode === "desktop" && (
                  <span className="menu-check">
                    <Icon name="check" />
                  </span>
                )}
              </button>
              <button
                className="continue-menu-item"
                disabled={terminalBusy || archived}
                title="在终端中继续"
                onClick={() => {
                  setContinueMode("terminal");
                  setSavedContinueMode("terminal");
                  setMenuOpen(false);
                  void openTerminal();
                }}
              >
                <Icon name="terminal" />
                <span className="menu-label">在终端中继续</span>
                {continueMode === "terminal" && (
                  <span className="menu-check">
                    <Icon name="check" />
                  </span>
                )}
              </button>
            </div>
          )}
        </div>
      )}

      {canTerminal && !canDesktop && (
        <button
          className="btn ghost icon-button"
          aria-label="在终端中继续"
          title="打开内嵌终端（已有活终端则直接跳转）"
          disabled={terminalBusy || archived}
          onClick={() => void openTerminal()}
        >
          <Icon name="terminal" />
        </button>
      )}

      {!canTerminal && canDesktop && (
        <button
          className="btn ghost icon-button"
          aria-label="在桌面应用中继续"
          title={launchTitle}
          onClick={handleLaunch}
          disabled={launchDisabled || launching}
        >
          <Icon name="desktop" />
        </button>
      )}

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
    </>
  );
}
