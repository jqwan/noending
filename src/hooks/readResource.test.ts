import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { createReadResource } from "./readResource";

afterEach(cleanup);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

it("restores the last snapshot on remount, keeps it after a failed refresh, and isolates object identities", async () => {
  const fresh = deferred<string>();
  let read = async (id: string) => `${id} initial`;
  const resource = createReadResource(id => read(id));
  await resource.ensure("a");
  const first = renderHook(() => resource.useValue("a"));
  expect(first.result.current).toBe("a initial");
  first.unmount();
  read = () => fresh.promise;
  const request = resource.refresh("a");
  const next = renderHook(({ id }) => resource.useValue(id), { initialProps: { id: "a" } });
  expect(next.result.current).toBe("a initial");
  next.rerender({ id: "b" });
  expect(next.result.current).toBeUndefined();
  next.rerender({ id: "a" });
  await act(async () => { fresh.resolve("updated"); await request; });
  expect(next.result.current).toBe("updated");
  read = async () => { throw new Error("unavailable"); };
  await expect(resource.refresh("a")).rejects.toThrow("unavailable");
  expect(next.result.current).toBe("updated");
});

it("ignores superseded reads and cannot resurrect an invalidated object", async () => {
  const old = deferred<string>(), latest = deferred<string>(), removed = deferred<string>();
  const reads = [old, latest, removed];
  const resource = createReadResource(() => reads.shift()!.promise);
  const first = resource.refresh("a");
  const second = resource.refresh("a");
  latest.resolve("latest"); await second;
  old.resolve("stale"); await first;
  expect(resource.get("a")).toBe("latest");
  const final = resource.refresh("a");
  resource.clear("a");
  removed.resolve("deleted"); await final;
  expect(resource.get("a")).toBeUndefined();
});

it("coalesces background checks while an explicit refresh supersedes an older read", async () => {
  const old = deferred<string>(), latest = deferred<string>();
  const reads = [old, latest];
  const resource = createReadResource(() => reads.shift()!.promise);
  const first = resource.revalidate("a");
  expect(resource.revalidate("a")).toBe(first);
  const fresh = resource.refresh("a");
  latest.resolve("current"); await fresh;
  old.resolve("outdated"); await first;
  expect(resource.get("a")).toBe("current");
});
