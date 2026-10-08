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
 * 会话两个子页（概览 / 对话）共用的头部动作簇：移入回收站 / 增量同步 / 继续。
 *
 * 「继续」行为根据会话格式能力展现：
 *   - 兼具桌面与终端能力（目前为 codex）：展示聚合分列按钮，可切换「在桌面应用中继续」与「在终端中继续」；
 *   - 仅桌面能力（antigravity 桌面存储 / dsh / qoder / workbuddy / zcode）：仅显示桌面继续按钮；
 *   - 仅终端能力（antigravity CLI 存储 / pi / claude_code）：仅显示终端继续按钮。
 */
export default function SessionHeaderActions({ sessionId, agent, sourceKind, title, trashed, navigate, onChanged }: {
  sessionId: string;
  agent: Agent;
  /** 会话的 source_kind（antigravity 靠它区分两个存储）；未读到时按 agent 兜底。 */
  sourceKind: string | undefined;
  title: string;
  trashed: boolean;
  navigate: (r: Route) => void;
  /** 回收站 / 同步完成后由页面刷新数据。 */
  onChanged: () => void;
}) {
  const [confirmTrash, setConfirmTrash] = useState(false);
  const [trashBusy, setTrashBusy] = useState(false);
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

  const { disabled: launchDisabled, title: launchTitle } = trashed
    ? { disabled: true, title: "回收站中的会话不可继续" }
    : desktopContinueState({ agent, source_kind: sourceKind ?? "" }, agentStatus?.[agent] ?? null);

  const handleLaunch = () => {
    if (launching) return;
    setLaunching(true);
    void continueSessionDesktopWithToast(sessionId).finally(() => setLaunching(false));
  };

  /** 先跳后启：查到活终端（可能是预指定 id 自动绑定的，也可能是上次校验
   *  匹配绑定的）就跳独立终端视图；没有才启动一次内嵌 Resume。 */
  const openTerminal = async () => {
    if (terminalBusy) return;
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

  const doTrash = async () => {
    setTrashBusy(true);
    try {
      await api.trashSession(sessionId);
      showToast("已移入回收站");
      setConfirmTrash(false);
      onChanged();
    } catch (e) {
      showToast(`移入回收站失败：${String(e)}`);
    } finally {
      setTrashBusy(false);
    }
  };

  if (trashed) {
    // 回收站中的会话：后端拒绝继续——如实呈现为不可用。
    return (
      <button className="btn ghost icon-button" disabled aria-label="继续" title={launchTitle}>
        {canTerminal && !canDesktop ? <Icon name="terminal" /> : <Icon name="desktop" />}
      </button>
    );
  }

  return (
    <>
      <button
        className="btn ghost icon-button"
        aria-label="移入回收站"
        title="移入回收站"
        onClick={() => setConfirmTrash(true)}
        disabled={trashBusy}
      >
        <Icon name="trash" />
      </button>
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
              disabled={terminalBusy}
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
                disabled={terminalBusy}
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
          title="在终端中继续"
          disabled={terminalBusy}
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

      {confirmTrash && (
        <Modal title="移入回收站" onClose={() => { if (!trashBusy) setConfirmTrash(false); }}>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            <b>{title}</b> 会从 Sessions 列表、搜索与继续入口中消失，出现在 Sessions 页的「回收站」里。
          </p>
          {/* 要求把两个概念摆在同一处明确区分：「从任务移除」只改这条会话的
              所属任务（详情页的「更改 / 选择」），这里是全局回收站。 */}
          <div className="card hairline" style={{ marginBottom: 12 }}>
            <p style={{ margin: "0 0 6px" }}>
              <b>从任务移除</b> = 只修改这条会话的所属任务，会话本身留在列表里。
            </p>
            <p style={{ margin: 0 }}>
              <b>移入回收站</b> = 在 NoEnding 中全局隐藏该 Session。Agent 原始会话不会被删除。
            </p>
          </div>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirmTrash(false)} disabled={trashBusy}>取消</button>
            <button className="btn primary" onClick={doTrash} disabled={trashBusy}>
              {trashBusy ? "处理中…" : "移入回收站"}
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}
