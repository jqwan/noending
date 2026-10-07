import type { Agent } from "../../types";

/**
 * 9 种会话格式的静态能力表——与后端 adapters 的路由事实同源（continue_route /
 * desktop_resume_route）。外部终端已退役，「继续」只有两种形态：
 *   - 桌面打开：格式有桌面端路由时，Agent 图标按钮直开对应应用；
 *   - 内嵌终端：格式有 TUI CLI 时，会话页终端子页一键直启。
 * 两者都不是用户偏好，是格式事实——所以这张表是代码常量，不是设置项。
 */
export type SessionFormatCaps = { terminal: boolean; desktop: boolean };

export const FORMAT_CAPS: Record<string, SessionFormatCaps> = {
  codex: { terminal: true, desktop: true },
  claude_code: { terminal: true, desktop: false },
  pi: { terminal: true, desktop: false },
  dsh: { terminal: false, desktop: true },
  qoder: { terminal: false, desktop: true },
  workbuddy: { terminal: false, desktop: true },
  zcode: { terminal: false, desktop: true },
  antigravity_desktop: { terminal: false, desktop: true },
  antigravity_cli: { terminal: true, desktop: false },
};

/** 会话所属的格式 id：antigravity 靠 source_kind 区分两个存储，其余格式即 agent。 */
export function formatIdOf(agent: Agent, sourceKind: string): string {
  if (agent !== "antigravity") return agent;
  return sourceKind.includes("cli") ? "antigravity_cli" : "antigravity_desktop";
}

/** 会话的格式能力。source_kind 还没读到时按 agent 键兜底（非 antigravity 恰好
 *  正确；antigravity 短暂按 IDE 存储处理，加载完成后自行修正）。 */
export function capsOf(agent: Agent, sourceKind: string | undefined | null): SessionFormatCaps {
  return FORMAT_CAPS[formatIdOf(agent, sourceKind ?? "")] ?? { terminal: false, desktop: false };
}
