import { useEffect, useState } from "react";
import { api } from "../../api";
import { showToast } from "../../components/Toast";
import { capsOf } from "./sessionFormats";
import type { Agent, AgentStatusEntry } from "../../types";

/**
 * 「继续 = 直开桌面应用」的共享逻辑：按钮可用性（格式能力 + 桌面端在场）、
 * 悬停说明、以及打开动作本身。使用者：会话三子页的头部动作簇、Sessions
 * 看板的行/卡片按钮、任务卡片与任务会话列表。
 */

/** 「继续」按钮的可用性与说明。格式判别只需要 agent 与 source_kind。 */
export function desktopContinueState(
  session: { agent: Agent; source_kind?: string | null; archived_at?: string | null },
  status: AgentStatusEntry | null | undefined,
): { disabled: boolean; title: string } {
  if (session.archived_at) return { disabled: true, title: "已归档的会话不能继续，请先取消归档" };
  const caps = capsOf(session.agent, session.source_kind);
  const desktopAbsent = caps.desktop && status != null && !status.desktop_app_present;
  return {
    disabled: !caps.desktop || desktopAbsent,
    title: !caps.desktop
      ? "该会话格式没有桌面端打开方式"
      : desktopAbsent
        ? `未找到 ${status?.desktop_app ?? "桌面应用"}，桌面端不可用`
        : "在桌面应用中继续",
  };
}

/** 直接在桌面应用里打开会话；结果与失败都走 toast。 */
export function continueSessionDesktopWithToast(sessionId: string): Promise<void> {
  return api.continueSessionDesktop(sessionId)
    .then((open) => showToast(open.note || "已在桌面应用中打开该会话。"))
    .catch((e) => showToast(`打开失败：${String(e)}`));
}

/** Agent 状态表（桌面端名字与在场是「继续」置灰判断的输入）。读不到时返回
 *  null——在场未知不当作缺席。 */
export function useAgentStatus(): Record<string, AgentStatusEntry> | null {
  const [status, setStatus] = useState<Record<string, AgentStatusEntry> | null>(null);
  useEffect(() => {
    let live = true;
    api.getAgentStatus()
      .then((s) => {
        if (live) setStatus(s);
      })
      .catch(() => {
        // 状态读不到时按格式能力渲染。
      });
    return () => {
      live = false;
    };
  }, []);
  return status;
}
