import { AGENT_LABELS, type Agent } from "../../types";

/** 无标题 / title 未知的 Session：不拿 root_agent_session_id 当标题糊用户。 */
export const UNTITLED_SESSION = "未命名会话";

/** 无 cwd 的 Session 仍然合法（Workstream 不是路径，Session 也不持有路径）。 */
export const NO_CWD = "未设置";

/** 后端新增未知 Agent 时的兜底，不留空白单元格。 */
const UNKNOWN_AGENT = "未知 Agent";

/** 有 cwd、但还没被 NoEnding 登记成 WorkspacePath 的 Project 单元格状态。 */
export const PROJECT_PENDING = "待解析";

/** 会话没有记录工作目录时的项目占位。 */
export const PROJECT_NO_PATH = "无工作目录";

export function agentDisplayLabel(agent: Agent | null | undefined): string {
  const key = (agent ?? "") as string;
  return (AGENT_LABELS as Record<string, string>)[key] ?? UNKNOWN_AGENT;
}

/** title 由首条用户消息派生，可能为 null / 全空白。 */
export function sessionDisplayTitle(title: string | null | undefined): string {
  const t = (title ?? "").replace(/\s+/g, " ").trim();
  return t === "" ? UNTITLED_SESSION : t;
}
