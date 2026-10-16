import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import { useObjectMatches } from "./useObjectMatches";

vi.mock("../../api", () => ({ api: { matchObjects: vi.fn() } }));
beforeEach(() => vi.mocked(api.matchObjects).mockReset());
afterEach(cleanup);
const candidates = ["old", "new", "before", "after"];

it("skips empty queries and ignores a superseded response", async () => {
  let finishOld!: (ids: string[]) => void;
  vi.mocked(api.matchObjects)
    .mockImplementationOnce(() => new Promise(resolve => { finishOld = resolve; }))
    .mockResolvedValueOnce(["new"]);
  const { result, rerender } = renderHook(({ query }) => useObjectMatches("session", query, 0, candidates), { initialProps: { query: "  " } });
  expect(result.current.ids).toBeNull();
  expect(result.current.pending).toBe(false);
  expect(api.matchObjects).not.toHaveBeenCalled();
  rerender({ query: "old" });
  await waitFor(() => expect(api.matchObjects).toHaveBeenCalledWith("session", "old", candidates));
  rerender({ query: " new " });
  expect(result.current.ids).toBeNull();
  expect(result.current.pending).toBe(true);
  await waitFor(() => expect([...result.current.ids ?? []]).toEqual(["new"]));
  await act(async () => finishOld(["old"]));
  expect([...result.current.ids!]).toEqual(["new"]);
});

it("reports failures and retries, then refreshes matches when board data changes", async () => {
  vi.mocked(api.matchObjects).mockRejectedValueOnce(new Error("unavailable")).mockResolvedValueOnce(["before"]).mockResolvedValueOnce(["after"]);
  const { result, rerender } = renderHook(({ revision }) => useObjectMatches("project", "name", revision, candidates), { initialProps: { revision: 1 } });
  await waitFor(() => expect(result.current.error).toContain("unavailable"));
  expect(result.current.pending).toBe(false);
  act(() => result.current.retry());
  expect(result.current.pending).toBe(true);
  await waitFor(() => expect([...result.current.ids ?? []]).toEqual(["before"]));
  rerender({ revision: 2 });
  expect(result.current.ids).toBeNull();
  await waitFor(() => expect([...result.current.ids ?? []]).toEqual(["after"]));
});


it("invalidates results when only the filter scope changes and ignores its previous response", async () => {
  const first = ["old"];
  const second = ["new"];
  let finishOld!: (ids: string[]) => void;
  vi.mocked(api.matchObjects).mockImplementationOnce(() => new Promise(resolve => { finishOld = resolve; })).mockResolvedValueOnce(["new"]);
  const { result, rerender } = renderHook(({ scope }) => useObjectMatches("session", "same", 0, scope), {initialProps: {scope:first}});
  await waitFor(() => expect(api.matchObjects).toHaveBeenCalledWith("session","same",first));
  rerender({scope:second});
  expect(result.current.pending).toBe(true);
  expect(result.current.ids).toBeNull();
  await waitFor(() => expect([...result.current.ids ?? []]).toEqual(["new"]));
  expect(api.matchObjects).toHaveBeenLastCalledWith("session","same",second);
  await act(async () => finishOld(["old"]));
  expect([...result.current.ids!]).toEqual(["new"]);
});
