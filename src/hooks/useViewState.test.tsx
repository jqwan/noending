import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it } from "vitest";
import { useViewScroll, useViewState, viewState } from "./useViewState";

beforeEach(() => viewState.clear());
afterEach(cleanup);
function Page({ ready = true }: { ready?: boolean }) {
  const [query, setQuery] = useViewState("test.query", "");
  const ref = useViewScroll("test.scroll", ready);
  return <div ref={ref} data-testid="scroll"><input aria-label="搜索" value={query} onChange={e => setQuery(e.target.value)} /></div>;
}
it("restores filters immediately and scroll only when content is ready", () => {
  const first = render(<Page />);
  fireEvent.change(screen.getByLabelText("搜索"), { target: { value: "任务" } });
  const scroll = screen.getByTestId("scroll");
  scroll.scrollTop = 420;
  fireEvent.scroll(scroll);
  first.unmount();
  const second = render(<Page ready={false} />);
  expect((screen.getByLabelText("搜索") as HTMLInputElement).value).toBe("任务");
  expect(screen.getByTestId("scroll").scrollTop).toBe(0);
  second.rerender(<Page />);
  expect(screen.getByTestId("scroll").scrollTop).toBe(420);
});
