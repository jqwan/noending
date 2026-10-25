import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import type { OperationStatus } from "../../types";
import SessionHeaderActions from "./SessionHeaderActions";
vi.mock("../../api", () => ({ api: {
  getOperationStatus: vi.fn(), refreshSession: vi.fn(), getAgentStatus: vi.fn().mockResolvedValue({}),
} }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const View = () => <SessionHeaderActions sessionId="one" agent="pi" sourceKind="jsonl" title="测试" archived={false} onChanged={vi.fn()} />;
it("keeps a queued synchronization disabled after acceptance and remount, then restores it when the worker finishes", async () => {
  let operations: OperationStatus[] = [];
  vi.mocked(api.getOperationStatus).mockImplementation(async () => operations);
  vi.mocked(api.refreshSession).mockImplementation(async () => {
    operations = [{id:"queued",kind:"sync",state:"queued",targets:[{kind:"session",id:"one"}],agent:null}];
    return {queued:true};
  });
  const first = render(<View />);
  const button = screen.getByRole("button",{name:"增量同步"});
  await waitFor(() => expect(button).toHaveProperty("disabled",false));
  fireEvent.click(button);
  await waitFor(() => expect(button.title).toBe("排队同步中…"));
  expect(button).toHaveProperty("disabled",true);
  fireEvent.click(button);
  expect(api.refreshSession).toHaveBeenCalledTimes(1);
  first.unmount();
  render(<View />);
  const resumed = screen.getByRole("button",{name:"增量同步"});
  await waitFor(() => expect(resumed.title).toBe("排队同步中…"));
  expect(resumed).toHaveProperty("disabled",true);
  operations = [{...operations[0],state:"running"}];
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  await waitFor(() => expect(resumed.title).toBe("同步中…"));
  operations = [];
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  await waitFor(() => expect(resumed).toHaveProperty("disabled",false));
});

it("does not release a submitted sync button based on a status read started before submission",async()=>{
  let operations:OperationStatus[]=[];
  vi.mocked(api.getOperationStatus).mockImplementation(async()=>operations);
  vi.mocked(api.refreshSession).mockImplementation(async()=>{
    operations=[{id:"queued",kind:"sync",state:"queued",targets:[{kind:"session",id:"one"}],agent:null}];
    return {queued:true};
  });
  render(<View/>);
  const button=screen.getByRole("button",{name:"增量同步"});
  await waitFor(()=>expect(button).toHaveProperty("disabled",false));
  let release!:(value:OperationStatus[])=>void;
  vi.mocked(api.getOperationStatus).mockImplementationOnce(()=>new Promise(resolve=>{release=resolve;}));
  act(()=>{window.dispatchEvent(new Event("focus"));});
  fireEvent.click(button);
  await waitFor(()=>expect(api.refreshSession).toHaveBeenCalledWith("one"));
  await act(async()=>{release([]);});
  expect(button).toHaveProperty("disabled",true);
  await waitFor(()=>expect(button.title).toBe("排队同步中…"));
});
