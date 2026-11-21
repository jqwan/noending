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

  it("shows one copy of a repeated toast and dismisses it after the latest occurrence", () => {
    render(<ToastHost />);

    act(() => {
      showToast("已复制到剪贴板");
    });

    act(() => {
      vi.advanceTimersByTime(3000);
    });

    act(() => {
      showToast("已复制到剪贴板");
    });
    expect(screen.getAllByText("已复制到剪贴板")).toHaveLength(1);

    act(() => {
      vi.advanceTimersByTime(4999);
    });
    expect(screen.getByText("已复制到剪贴板")).toBeTruthy();

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(screen.queryByText("已复制到剪贴板")).toBeNull();
  });
});
