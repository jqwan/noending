import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useViewPreference, preferenceValues, useViewScroll, useViewState, viewState } from "./useViewState";

beforeEach(() => { viewState.clear(); localStorage.clear(); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
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

function Preferences() {
  const [mode, setMode] = useViewPreference("test.mode", "cards", preferenceValues("cards", "list"));
  return <><Page /><button onClick={() => setMode("list")}>{mode}</button></>;
}

it("restores opted-in preferences after a restart without persisting searches or scroll", () => {
  const first = render(<Preferences />);
  fireEvent.click(screen.getByRole("button", { name: "cards" }));
  fireEvent.change(screen.getByLabelText("搜索"), { target: { value: "临时搜索" } });
  screen.getByTestId("scroll").scrollTop = 420;
  fireEvent.scroll(screen.getByTestId("scroll"));
  first.unmount();
  viewState.clear();

  render(<Preferences />);
  screen.getByRole("button", { name: "list" });
  expect((screen.getByLabelText("搜索") as HTMLInputElement).value).toBe("");
  expect(screen.getByTestId("scroll").scrollTop).toBe(0);
});

it.each(["{broken", '"obsolete"'])("uses the default for invalid stored preferences: %s", saved => {
  localStorage.setItem("noending.view.test.mode", saved);
  render(<Preferences />);
  screen.getByRole("button", { name: "cards" });
});

it("keeps preferences usable when reading or writing storage fails", () => {
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("denied"); });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });
  const first = render(<Preferences />);
  fireEvent.click(screen.getByRole("button", { name: "cards" }));
  first.unmount();
  render(<Preferences />);
  screen.getByRole("button", { name: "list" });
});
