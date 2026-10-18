import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useLoadingDelay } from "./useLoadingDelay";

describe("useLoadingDelay", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("does not show skeleton immediately when loading starts", () => {
    const { result } = renderHook(() => useLoadingDelay(true, 150));
    expect(result.current).toBe(false);
  });

  it("does not show skeleton if loading finishes before delay", () => {
    let loading = true;
    const { result, rerender } = renderHook(() => useLoadingDelay(loading, 150));

    expect(result.current).toBe(false);

    // 50ms 后数据加载完成
    act(() => {
      vi.advanceTimersByTime(50);
    });
    loading = false;
    rerender();

    // 即使再过去 200ms，骨架屏也绝不展示
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(result.current).toBe(false);
  });

  it("shows skeleton if loading takes longer than delay", () => {
    const { result } = renderHook(() => useLoadingDelay(true, 150));

    expect(result.current).toBe(false);

    act(() => {
      vi.advanceTimersByTime(149);
    });
    expect(result.current).toBe(false);

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(result.current).toBe(true);
  });

  it("hides skeleton when loading finishes after having shown it", () => {
    let loading = true;
    const { result, rerender } = renderHook(() => useLoadingDelay(loading, 150));

    act(() => {
      vi.advanceTimersByTime(160);
    });
    expect(result.current).toBe(true);

    loading = false;
    rerender();
    expect(result.current).toBe(false);
  });
});
