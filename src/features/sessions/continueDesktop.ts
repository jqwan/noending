import { useEffect, useState } from "react";
import { api } from "../../api";
import { showToast } from "../../components/Toast";
import { capsOf } from "./sessionFormats";
import type { Agent, AgentStatusEntry } from "../../types";
import type { Route } from "../../app/routes";

/**
 * 「继续」的共享逻辑：按钮可用性（格式能力 + 桌面端在场）、模式首选项、
 * 悬停说明、以及打开动作本身（桌面端直开 / 内嵌终端跳转）。
 * 使用者：会话三子页的头部动作簇、Sessions 看板的行/卡片按钮、任务卡片与任务会话列表。
 */

export const CONTINUE_MODE_STORAGE_KEY = "noending.continue_mode";

export type ContinueMode = "desktop" | "terminal";

export function getSavedContinueMode(): ContinueMode {
  return localStorage.getItem(CONTINUE_MODE_STORAGE_KEY) === "terminal" ? "terminal" : "desktop";
}

export function setSavedContinueMode(mode: ContinueMode): void {
  localStorage.setItem(CONTINUE_MODE_STORAGE_KEY, mode);
}

/** 判定特定会话当前应呈现的继续方式：
 *  - 兼具桌面与终端能力（如 codex）：遵循用户在聚合按钮中保存的默认偏好；
 *  - 仅终端能力（claude_code / pi / antigravity_cli）：固定终端；
 *  - 其余（仅桌面能力或均不支持）：固定桌面（不支持时由 desktopContinueState 置灰）。
 */
export function resolveSessionContinueMode(
  session: { agent: Agent; source_kind?: string | null },
  savedMode: ContinueMode = getSavedContinueMode(),
): ContinueMode {
  const caps = capsOf(session.agent, session.source_kind);
  if (caps.terminal && caps.desktop) {
    return savedMode;
  }
  if (caps.terminal && !caps.desktop) {
    return "terminal";
  }
  return "desktop";
}

/** 「继续」按钮的可用性与说明。格式判别只需要 agent 与 source_kind。 */
export function desktopContinueState(
  session: { agent: Agent; source_kind?: string | null },
  status: AgentStatusEntry | null | undefined,
): { disabled: boolean; title: string } {
  const caps = capsOf(session.agent, session.source_kind);
  const desktopAbsent = caps.desktop && status != null && !status.desktop_app_present;
  return {
    disabled: !caps.desktop || desktopAbsent,
    title: !caps.desktop
      ? "不支持桌面端"
      : desktopAbsent
        ? `未找到 ${status?.desktop_app ?? "桌面应用"}`
        : "在桌面应用中继续",
  };
}

/** 直接在桌面应用里打开会话；结果与失败都走 toast。 */
export function continueSessionDesktopWithToast(sessionId: string): Promise<void> {
  return api.continueSessionDesktop(sessionId)
    .then((open) => showToast(open.note || "已在桌面应用中打开"))
    .catch((e) => showToast(`打开失败：${String(e)}`));
}

/** 在内嵌终端中恢复会话：查到活终端即跳，没有则启动内嵌 Resume 记录后再跳。 */
export async function continueSessionTerminal(
  session: { id: string; title?: string | null; agent: Agent },
  navigate: (r: Route) => void,
): Promise<void> {
  const seed = {
    initialTitle: session.title ?? "",
    initialAgent: session.agent,
    initialSessionId: session.id,
  };
  const existing = await api.terminalForSession(session.id);
  if (existing) {
    navigate({ view: "terminal", terminalId: existing.terminal_id, ...seed });
    return;
  }
  await api.launchEmbeddedResume(session.id);
  const created = await api.terminalForSession(session.id);
  if (!created) throw new Error("启动已完成，但找不到内嵌终端记录");
  navigate({ view: "terminal", terminalId: created.terminal_id, ...seed });
}

/** 在内嵌终端中恢复会话并捕获错误展示 Toast。 */
export async function continueSessionTerminalWithToast(
  session: { id: string; title?: string | null; agent: Agent },
  navigate: (r: Route) => void,
): Promise<void> {
  try {
    await continueSessionTerminal(session, navigate);
  } catch (e) {
    showToast(`终端不可用：${String(e)}`);
  }
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
