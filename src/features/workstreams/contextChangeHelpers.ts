import type { ContextChange } from "../../types";

export function contextChangeActorLabel(actor: string): string {
  const lower = actor.toLowerCase();
  if (lower === "user") return "User";
  if (lower === "system") return "System";
  if (lower === "agent" || lower === "sync" || lower.startsWith("sync:")) return "Agent";
  return actor;
}

export function contextChangeKindLabel(kind: ContextChange["kind"]): string {
  switch (kind) {
    case "added":
      return "+ 新增";
    case "edited":
      return "↻ 更新";
    case "resolved":
      return "✓ 完成";
    case "superseded":
      return "⇄ 替代";
    case "deleted":
      return "✕ 删除";
    case "conflict_created":
      return "⚠ 冲突";
    case "conflict_resolved":
      return "✓ 裁决";
    default:
      return kind;
  }
}

export function contextChangeBadgeClass(kind: ContextChange["kind"]): string {
  switch (kind) {
    case "conflict_created":
      return "warn";
    case "conflict_resolved":
    case "resolved":
      return "accent";
    default:
      return "";
  }
}
