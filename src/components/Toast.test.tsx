import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ToastHost, { showToast } from "./Toast";

describe("Toast", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    act(() => {
      vi.runOnlyPendingTimers();
    });
    vi.useRealTimers();
    cleanup();
  });

  it("triggers action onClick when action button is clicked", () => {
    const onClick = vi.fn();
    render(<ToastHost />);

    act(() => {
      showToast("会话已启动", { label: "查看详情", onClick });
    });

    const btn = screen.getByRole("button", { name: "查看详情" });
    fireEvent.click(btn);
    expect(onClick).toHaveBeenCalledOnce();
  });

  it("deduplicates identical toasts without stacking duplicate elements", () => {
    render(<ToastHost />);

    act(() => {
      showToast("已复制到剪贴板");
      showToast("已复制到剪贴板");
      showToast("已复制到剪贴板");
    });

    const elements = screen.getAllByText("已复制到剪贴板");
    expect(elements).toHaveLength(1);
  });

  it("automatically removes toast after 5000ms", () => {
    render(<ToastHost />);

    act(() => {
      showToast("已复制到剪贴板");
    });
    expect(screen.getByText("已复制到剪贴板")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(4999);
    });
    expect(screen.getByText("已复制到剪贴板")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(screen.queryByText("已复制到剪贴板")).toBeNull();
  });

  it("resets auto-dismiss timer when duplicate toast is triggered", () => {
    render(<ToastHost />);

    act(() => {
      showToast("已复制到剪贴板");
    });

    // Advance 3000ms
    act(() => {
      vi.advanceTimersByTime(3000);
    });

    // Retrigger same toast, timer should reset to 5000ms
    act(() => {
      showToast("已复制到剪贴板");
    });

    // Another 3000ms passes (total 6000ms since first call, but only 3000ms since retrigger)
    act(() => {
      vi.advanceTimersByTime(3000);
    });
    expect(screen.getByText("已复制到剪贴板")).toBeTruthy();

    // Remaining 2000ms passes
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(screen.queryByText("已复制到剪贴板")).toBeNull();
  });
});
