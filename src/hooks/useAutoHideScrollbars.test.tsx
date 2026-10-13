import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useAutoHideScrollbars } from "./useAutoHideScrollbars";

function Fixture() {
  useAutoHideScrollbars();
  return <div aria-label="scroll pane" style={{ height: 80, overflow: "auto" }}>
    <textarea aria-label="scrolling text" rows={2} defaultValue={"line\n".repeat(100)} />
    <div style={{ height: 400 }} />
  </div>;
}

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it("reveals only the scrolling Windows pane, refreshes its idle period and cleans up on unmount", () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows NT 10.0");
  vi.useFakeTimers();
  const view = render(<Fixture />);
  const pane = screen.getByLabelText("scroll pane");
  const text = screen.getByRole("textbox");
  // These attributes are the hook's interface to the scrollbar stylesheet;
  // actual hover/drag rendering is verified in a browser, not jsdom.
  expect(document.documentElement.hasAttribute("data-auto-hide-scrollbars")).toBe(true);
  fireEvent.scroll(text);
  expect(text.hasAttribute("data-scrollbar-active")).toBe(true);
  expect(pane.hasAttribute("data-scrollbar-active")).toBe(false);
  vi.advanceTimersByTime(700);
  fireEvent.scroll(text);
  vi.advanceTimersByTime(700);
  expect(text.hasAttribute("data-scrollbar-active")).toBe(true);
  vi.advanceTimersByTime(700);
  expect(text.hasAttribute("data-scrollbar-active")).toBe(false);

  fireEvent.scroll(pane);
  view.unmount();
  expect(pane.hasAttribute("data-scrollbar-active")).toBe(false);
  expect(document.documentElement.hasAttribute("data-auto-hide-scrollbars")).toBe(false);
  expect(vi.getTimerCount()).toBe(0);
});

it("leaves macOS scrollbar policy native", () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh; Intel Mac OS X");
  render(<Fixture />);
  fireEvent.scroll(screen.getByRole("textbox"));
  expect(document.documentElement.hasAttribute("data-auto-hide-scrollbars")).toBe(false);
  expect(screen.getByRole("textbox").hasAttribute("data-scrollbar-active")).toBe(false);
});
