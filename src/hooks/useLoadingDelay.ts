import { useEffect, useState } from "react";

/**
 * 延迟展示加载骨架屏（避免本地极速响应 < 150ms 时出现占位白块/灰色骨架闪烁）。
 *
 * @param loading 当前是否处于数据加载中
 * @param delayMs 超过多少毫秒仍未加载完才展示骨架，默认 150ms
 */
export function useLoadingDelay(loading: boolean, delayMs = 150): boolean {
  const [show, setShow] = useState(false);

  useEffect(() => {
    if (!loading) {
      setShow(false);
      return;
    }
    const timer = setTimeout(() => {
      setShow(true);
    }, delayMs);
    return () => clearTimeout(timer);
  }, [loading, delayMs]);

  return show;
}
