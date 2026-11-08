import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { createLiveReadResource } from "./liveReadResource";

const events = [{ name: "live-test-change" }, { name: "live-test-bind" }];
const emit = () => window.dispatchEvent(new Event(events[0].name));
function deferred<T>() {
  let resolve!: (value: T) => void;
  return { promise: new Promise<T>(done => { resolve = done; }), resolve: (value: T) => resolve(value) };
}
afterEach(() => { cleanup(); vi.useRealTimers(); });

it("shares a read, merges notifications, and never publishes a superseded snapshot", async () => {
  vi.useFakeTimers();
  const old = deferred<string>();
  const read = vi.fn().mockReturnValueOnce(old.promise).mockResolvedValue("current");
  const resource = createLiveReadResource<string>(read, events);
  const first = renderHook(() => resource.useValue());
  const second = renderHook(() => resource.useValue());
  await act(async () => {});
  expect(read).toHaveBeenCalledTimes(1);
  act(() => { emit(); emit(); window.dispatchEvent(new Event(events[1].name)); });
  await act(async () => { old.resolve("outdated"); });
  expect(first.result.current.value).toBeUndefined();
  expect(second.result.current.value).toBeUndefined();
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(first.result.current.value).toBe("current");
  expect(second.result.current.value).toBe("current");
  expect(read).toHaveBeenCalledTimes(2);
});

it("keeps hidden readers dormant, catches up once on reveal, and preserves the snapshot on errors", async () => {
  vi.useFakeTimers();
  const read = vi.fn().mockResolvedValue("initial");
  const resource = createLiveReadResource<string>(read, events);
  const view = renderHook(({ enabled }) => resource.useValue(enabled), { initialProps: { enabled: false } });
  act(emit);
  await act(async () => { await vi.advanceTimersByTimeAsync(200); });
  expect(read).not.toHaveBeenCalled();
  view.rerender({ enabled: true });
  await act(async () => {});
  expect(view.result.current.value).toBe("initial");
  act(emit); // A scheduled refresh is cancelled when the last visible reader hides.
  view.rerender({ enabled: false });
  read.mockResolvedValue("updated");
  act(() => { emit(); emit(); });
  await act(async () => { await vi.advanceTimersByTimeAsync(200); });
  expect(read).toHaveBeenCalledTimes(1);
  view.rerender({ enabled: true });
  await act(async () => {});
  expect(view.result.current.value).toBe("updated");
  expect(read).toHaveBeenCalledTimes(2);
  view.rerender({ enabled: false });
  view.rerender({ enabled: true });
  await act(async () => {});
  expect(read).toHaveBeenCalledTimes(2); // Showing a clean cache is not a refresh trigger.
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  read.mockRejectedValue(new Error("unavailable"));
  act(emit);
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(view.result.current.failed).toBe(true);
  expect(view.result.current.value).toBe("updated");
  read.mockResolvedValue("recovered");
  act(emit);
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(view.result.current.failed).toBe(false);
  expect(view.result.current.value).toBe("recovered");
  log.mockRestore();
});

it("a hidden observer does not pause visible consumers and unmount cancels pending work", async () => {
  vi.useFakeTimers();
  const read = vi.fn().mockResolvedValue("initial");
  const resource = createLiveReadResource<string>(read, events);
  const hidden = renderHook(() => resource.useValue(false));
  const visible = renderHook(() => resource.useValue());
  await act(async () => {});
  read.mockResolvedValue("updated");
  act(emit);
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(hidden.result.current.value).toBe("updated");
  expect(visible.result.current.value).toBe("updated");
  expect(read).toHaveBeenCalledTimes(2);
  act(emit);
  visible.unmount();
  hidden.unmount();
  await act(async () => { await vi.advanceTimersByTimeAsync(100); });
  expect(read).toHaveBeenCalledTimes(2);
});
