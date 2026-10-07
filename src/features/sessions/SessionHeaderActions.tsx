import { useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import Icon from "../../components/Icon";
import AgentIcon from "../../components/AgentIcon";
import { continueSessionDesktopWithToast, desktopContinueState, useAgentStatus } from "./continueDesktop";
import type { Agent } from "../../types";

/**
 * 会话三个子页（概览 / 对话 / 终端）共用的头部动作簇：移入回收站 / 增量同步 /
 * 继续——与三段子页切换器并排，构成每个子页一致的右上角。
 *
 * 「继续」是 Agent 图标按钮，点击直接在会话格式对应的桌面应用里打开（无预览）。
 * 启用与否是格式事实：没有桌面路由的格式（claude_code / pi / antigravity_cli）
 * 置灰并说明原因，桌面应用未安装同样置灰；回收站中的会话整簇收起为一颗
 * 禁用的继续键——后端同样拒绝。内嵌终端不在这里：它的入口是终端子页按钮。
 */
export default function SessionHeaderActions({ sessionId, agent, sourceKind, title, trashed, onChanged }: {
  sessionId: string;
  agent: Agent;
  /** 会话的 source_kind（antigravity 靠它区分两个存储）；未读到时按 agent 兜底。 */
  sourceKind: string | undefined;
  title: string;
  trashed: boolean;
  /** 回收站 / 同步完成后由页面刷新数据。 */
  onChanged: () => void;
}) {
  const [confirmTrash, setConfirmTrash] = useState(false);
  const [trashBusy, setTrashBusy] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [launching, setLaunching] = useState(false);
  const agentStatus = useAgentStatus();

  const { disabled: launchDisabled, title: launchTitle } = trashed
    ? { disabled: true, title: "回收站中的会话不能继续；先恢复它。" }
    : desktopContinueState({ agent, source_kind: sourceKind ?? "" }, agentStatus?.[agent] ?? null);

  const handleLaunch = () => {
    if (launching) return;
    setLaunching(true);
    void continueSessionDesktopWithToast(sessionId).finally(() => setLaunching(false));
  };

  const handleSync = async () => {
    if (syncing) return;
    setSyncing(true);
    try {
      await api.refreshSession(sessionId);
      showToast("已排队：正在增量同步该会话最新对话…");
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
        <AgentIcon agent={agent} size={18} />
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
        title="增量同步：从磁盘同步该会话的最新对话"
        onClick={() => void handleSync()}
        disabled={syncing}
      >
        <Icon name="refresh" />
      </button>
      <button
        className="btn ghost icon-button"
        aria-label="继续"
        title={launchTitle}
        onClick={handleLaunch}
        disabled={launchDisabled || launching}
      >
        <AgentIcon agent={agent} size={18} />
      </button>

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
