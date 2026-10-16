import type { TerminalSummary } from "../../types";

export function terminalActivityLabel(terminal: TerminalSummary): string {
  const label = !terminal.live ? "已退出" : ({
    unknown: "运行中", idle: "等待消息", working: "正在处理", waiting: "等待输入",
    completed: "回答完成", failed: "处理失败", interrupted: "已中断",
  }[terminal.activity?.state ?? "unknown"]);
  if (terminal.activity?.unread) return terminal.live && terminal.activity.state === "completed"
    ? "回答完成未查看" : `${label} · 有未查看的回答`;
  return label;
}

/** A session can contain several independent Agent runs; unread is orthogonal. */
export function terminalActivityClass(terminals: TerminalSummary[]): string {
  const live = terminals.filter(t => t.live);
  const states = live.map(t => t.activity?.state);
  const state = states.includes("waiting") ? "waiting" : states.includes("working") ? "working"
    : states.includes("failed") ? "failed" : live.length ? "idle" : "exited";
  return `terminal-activity-${state}${terminals.some(t => t.activity?.unread) ? " terminal-activity-unread" : ""}`;
}

export function terminalActivitiesLabel(terminals: TerminalSummary[]): string {
  return [...new Set(terminals.map(terminalActivityLabel))].join("；");
}
