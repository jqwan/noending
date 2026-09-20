import { useSyncExternalStore } from "react";
import { api } from "../api";
import type { ContextDeliveryLevel } from "../types";

/**
 * Base Experience（方案 v0.1 §11.9）：全局唯一的智能开关读取点。
 *
 * 规则只有一条：off = 不挂载，而不是 CSS 隐藏。路由、深链和后端命令一律保持
 * 可达（§24 的例外条款），所以这里只管"要不要出现在主路径上"。
 *
 * Context Intelligence 与 Context Delivery 是两个正交开关（§11.1）：
 * delivery=off 只关掉对外注入，摄入与提取照旧；intelligence=off 才关掉提取。
 */
export type BaseExperience = {
  intelligenceEnabled: boolean;
  deliveryLevel: ContextDeliveryLevel;
};

const BASE_EXPERIENCE: BaseExperience = {
  intelligenceEnabled: false,
  deliveryLevel: "off",
};

let snapshot: BaseExperience = BASE_EXPERIENCE;
const listeners = new Set<() => void>();

/**
 * Read both switches once. Failing to read them falls back to Base Experience:
 * we never surface intelligence UI we cannot confirm is switched on.
 */
export async function refreshBaseExperience(): Promise<void> {
  let next = BASE_EXPERIENCE;
  try {
    const [intelligenceEnabled, deliveryLevel] = await Promise.all([
      api.getContextIntelligenceEnabled(),
      api.getContextDeliveryLevel(),
    ]);
    next = { intelligenceEnabled, deliveryLevel };
  } catch (err) {
    console.error(err);
  }
  snapshot = next;
  listeners.forEach((notify) => notify());
}

export function useBaseExperience(): BaseExperience {
  return useSyncExternalStore(
    (onStoreChange) => {
      listeners.add(onStoreChange);
      return () => listeners.delete(onStoreChange);
    },
    () => snapshot,
    () => snapshot,
  );
}

/** Render children only while Context Intelligence is switched on. */
export function IntelligenceOnly({ children }: { children: React.ReactNode }) {
  return useBaseExperience().intelligenceEnabled ? <>{children}</> : null;
}

/** True when the launch preview should show nothing at all (§15). */
export function useDeliveryOff(): boolean {
  return useBaseExperience().deliveryLevel === "off";
}
