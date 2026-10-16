import type { TerminalSummary } from "../../types";

export function terminalActivityLabel(terminal: TerminalSummary): string {
  const label = !terminal.live ? "已退出" : ({
    unknown: "已连接，Agent 状态未知", idle: "等待消息", working: "正在处理", waiting: "等待输入",
    completed: "回答完成", failed: "处理失败", interrupted: "已中断",
  }[terminal.activity?.state ?? "unknown"]);
  if (terminal.activity?.unread) return terminal.live && terminal.activity.state === "completed"
    ? "回答完成未查看" : `${label} · 有未查看的回答`;
  return label;
}
