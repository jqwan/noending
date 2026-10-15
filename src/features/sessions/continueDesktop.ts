import { useEffect, useState } from "react";
import { api } from "../../api";
import { showToast } from "../../components/Toast";
import { capsOf } from "./sessionFormats";
import { preferredTerminal, terminalDestination } from "./sessionNavigation";
import { disposeTerminal } from "./terminalCache";
import type { Agent, AgentStatusEntry, TerminalResumeState, TerminalSummary } from "../../types";
import { EVT_TERMINALS, type Route } from "../../app/routes";

/**
 * 「继续」的共享逻辑：按钮可用性（格式能力 + 桌面端在场）、终端优先入口、
 * 悬停说明、以及打开动作本身（桌面端直开 / 内嵌终端跳转）。
 * 使用者：会话三子页的头部动作簇、Sessions 看板的行/卡片按钮、任务卡片与任务会话列表。
 */

/** A single continue entry always prefers CLI capability over desktop capability. */
export function resolveSessionContinueMode(
  session: { agent: Agent; source_kind?: string | null },
): "terminal" | "desktop" {
  return capsOf(session.agent, session.source_kind).terminal ? "terminal" : "desktop";
}

/** 「继续」按钮的可用性与说明。格式判别只需要 agent 与 source_kind。 */
export function desktopContinueState(
  session: { agent: Agent; source_kind?: string | null; archived_at?: string | null; cwd_exists?: boolean },
  status: AgentStatusEntry | null | undefined,
): { disabled: boolean; title: string } {
  if (session.archived_at) return { disabled: true, title: "已归档的会话不能继续，请先取消归档" };
  if (session.cwd_exists === false) return { disabled: true, title: "工作目录不存在，无法继续会话" };
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

/** Running instances remain viewable even when a new resume is unavailable. */
export function terminalContinueState(
  sessionId: string | null,
  terminals: readonly { live: boolean }[],
  resume: TerminalResumeState | undefined,
  archived = false,
): { disabled: boolean; title: string } {
  if (terminals.some(t => t.live)) return { disabled: false, title: "打开运行中的终端" };
  if (archived) return { disabled: true, title: "已归档的会话不能继续，请先取消归档" };
  if (!sessionId) return { disabled: true, title: "终端尚未绑定会话，无法重新连接" };
  if (!resume) return { disabled: true, title: "正在检查终端可用性" };
  return { disabled: !resume.available, title: resume.available
    ? terminals.length ? "重新连接会话" : "在终端中继续"
    : resume.reason ?? "无法在终端中继续" };
}

const openingSessions = new Set<string>();

export function terminalForContinue(sessionId: string, terminals: TerminalSummary[]) {
  return preferredTerminal(sessionId, terminals.filter(t => t.live)) ?? preferredTerminal(sessionId, terminals);
}

/** Shared explicit continue: prefer a live instance, reconnect an exited one,
 * otherwise launch. An explicit terminal ID acts only on that instance. */
export async function continueSessionTerminal(
  session: { id: string; title?: string | null; agent: Agent; archived_at?: string | null },
  navigate: (r: Route) => void,
  terminalId?: string,
): Promise<void> {
  // Recheck identity/liveness: a sidebar snapshot must never reconnect a
  // terminal that the Agent has since switched to another native session.
  const rows = (await api.terminalList()).filter(t => t.session_id === session.id);
  const existing = terminalId ? rows.find(t => t.terminal_id === terminalId) : terminalForContinue(session.id, rows);
  if (terminalId && !existing) throw new Error("终端已关闭或已切换会话，请重试");
  if (existing?.live) {
    navigate({ ...terminalDestination(existing), initialTitle: session.title ?? existing.session_title ?? undefined });
    return;
  }
  const state = (await api.getSessionTerminalStates([session.id]))[session.id];
  if (!state?.available) throw new Error(state?.reason ?? "无法确认终端可用性");
  const launched = existing ? await api.terminalReconnect(existing.terminal_id, session.id) : await api.launchEmbeddedResume(session.id);
  if (!launched.terminal_id) throw new Error("启动已完成，但找不到内嵌终端记录");
  if (existing) disposeTerminal(existing.terminal_id);
  window.dispatchEvent(new CustomEvent(EVT_TERMINALS));
  navigate({ view: "terminal", terminalId: launched.terminal_id, initialSessionId: session.id,
    initialAgent: session.agent, initialTitle: session.title ?? undefined });
}

/** Coalesce simultaneous sidebar/header clicks; errors leave exited output intact. */
export async function openSessionTerminal(
  session: { id: string; title?: string | null; agent: Agent; archived_at?: string | null },
  navigate: (r: Route) => void,
  terminalId?: string,
): Promise<void> {
  if (openingSessions.has(session.id)) return;
  openingSessions.add(session.id);
  try { await continueSessionTerminal(session, navigate, terminalId); }
  catch (e) { showToast(`终端不可用：${String(e)}`); }
  finally { openingSessions.delete(session.id); }
}

export const continueSessionTerminalWithToast = openSessionTerminal;

/** Agent 状态表（桌面端名字与在场是「继续」置灰判断的输入）。读不到时返回
 *  null——在场未知不当作缺席。 */
export function useAgentStatus(enabled = true): Record<string, AgentStatusEntry> | null {
  const [status, setStatus] = useState<Record<string, AgentStatusEntry> | null>(null);
  useEffect(() => {
    if (!enabled) return;
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
  }, [enabled]);
  return status;
}
