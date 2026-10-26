import type {
  CollaborationConnectionState,
  CollaborationMemberState,
  CollaborationRunStatus,
  CollaborationWakeMethod,
} from "../../types";

/**
 * 成员状态的展示语义。
 *
 * 连接与处理阶段是两件事：有有效心跳只说明参与通道活着，不说明模型能响应。
 * 2 分钟「暂未响应」只在待消息阶段（listening）成立，起点是后端给的最早未查收
 * 他人消息时间 awaiting_since；处理中只看处理时长，不套这条限制。
 * 残留进程不会被这段文案写成「工作正常」。
 */
export type MemberTone =
  | "waiting"
  | "listening"
  | "processing"
  | "stalled"
  | "idle"
  | "disconnected"
  | "left"
  | "failed";

export type RunTone = "starting" | "active" | "stopped";

/** 待消息阶段超过这个时长仍未查收，才显示「暂未响应」。 */
export const STALL_MS = 2 * 60 * 1000;
export const LEASE_MS = 30 * 1000;

export function memberStatus(
  member: CollaborationMemberState,
  now: number,
): { tone: MemberTone; label: string } {
  if (member.connection_state === "left") return { tone: "left", label: "已退出" };
  if (member.connection_state === "disconnected") return { tone: "disconnected", label: "连接中断" };
  if (member.connection_state === "pending") {
    return member.wake_status === "failed"
      ? { tone: "failed", label: "唤醒失败" }
      : { tone: "waiting", label: "等待加入" };
  }
  // A stopped heartbeat produces no further database writes or events.
  const heartbeat = member.last_heartbeat_at ? Date.parse(member.last_heartbeat_at) : Number.NaN;
  if (!Number.isFinite(heartbeat) || now - heartbeat > LEASE_MS) {
    return { tone: "disconnected", label: "连接中断" };
  }
  if (member.phase === "processing") return { tone: "processing", label: "处理中" };
  if (member.phase === "listening") {
    const since = member.awaiting_since ? Date.parse(member.awaiting_since) : Number.NaN;
    if (Number.isFinite(since) && now - since > STALL_MS) {
      return { tone: "stalled", label: "暂未响应" };
    }
    return { tone: "listening", label: "正在监听" };
  }
  return { tone: "idle", label: "已连接" };
}

export const RUN_LABEL: Record<CollaborationRunStatus, string> = {
  starting: "等待加入",
  active: "协作中",
  stopped: "已停止",
};

export const WAKE_LABEL: Record<CollaborationWakeMethod, string> = {
  terminal: "内嵌终端",
  desktop: "桌面应用",
};

/** 只有实际支持终端唤醒的会话才提供内嵌选项。 */
export function defaultWakeMethod(modes: CollaborationWakeMethod[]): CollaborationWakeMethod | null {
  if (modes.includes("terminal")) return "terminal";
  if (modes.includes("desktop")) return "desktop";
  return null;
}

/**
 * 桌面成员无法由 NoEnding 替用户通知，只能给出可复制的加入提示。
 * 提示只含定位信息与职责，不复制协作消息正文——正文只有一份。
 */
export function joinPrompt(args: {
  taskId: string;
  taskTitle: string;
  roomId: string;
  runId: string;
  selfSessionId: string;
  coordinator: boolean;
}): string {
  return [
    `加入协作室：${args.taskTitle}`,
    `任务 ${args.taskId}`,
    `房间 ${args.roomId} · 运行 ${args.runId}`,
    `你的会话 ${args.selfSessionId}${args.coordinator ? "（本次负责人）" : ""}`,
    "用 noending 技能 join 该房间与运行，然后 wait 取未读、ack 推进阅读位置。",
    "协作消息只在本房间读取，正文不复制到信箱；任务进展由负责人维护。",
  ].join("\n");
}

export function connectionLabel(state: CollaborationConnectionState): string {
  switch (state) {
    case "pending": return "尚未加入";
    case "connected": return "参与连接有效";
    case "disconnected": return "参与连接中断";
    case "left": return "已退出参与";
  }
}
