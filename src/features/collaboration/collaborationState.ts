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
    return { tone: "listening", label: "等待消息" };
  }
  // 参与通道在，但没有进入等待：只说连接事实，不写成「在听」。
  return { tone: "idle", label: "仅连接" };
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

export function connectionLabel(state: CollaborationConnectionState): string {
  switch (state) {
    case "pending": return "尚未加入";
    case "connected": return "参与连接有效";
    case "disconnected": return "参与连接中断";
    case "left": return "已退出参与";
  }
}
