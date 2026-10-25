import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { api } from "../api";
import type { OperationStatus } from "../types";
import { useOperationState } from "./useOperationState";
vi.mock("../api", () => ({api:{getOperationStatus:vi.fn()}}));
afterEach(() => { cleanup(); vi.useRealTimers(); vi.clearAllMocks(); });
function Control({kind,id}:{kind:"session"|"task"|"project";id:string}) {
  const state=useOperationState("summary",kind,id);
  return <button disabled={state.checking || state.busy}>{kind}:{id}</button>;
}
it("restores a summary operation from the backend after navigation and isolates unrelated targets",async () => {
  const kind = "session" as const;
  let operations:OperationStatus[]=[{id:"cli",kind:"summary",state:"running",targets:[{kind,id:"one"}],agent:null}];
  vi.mocked(api.getOperationStatus).mockImplementation(async()=>operations);
  const first=render(<><Control kind={kind} id="one"/><Control kind={kind} id="other"/></>);
  await waitFor(()=>expect(screen.getByRole("button",{name:`${kind}:other`})).toHaveProperty("disabled",false));
  expect(screen.getByRole("button",{name:`${kind}:one`})).toHaveProperty("disabled",true);
  first.unmount();
  render(<Control kind={kind} id="one"/>);
  expect(screen.getByRole("button",{name:`${kind}:one`})).toHaveProperty("disabled",true);
  operations=[];
  await act(async()=>{window.dispatchEvent(new Event("focus"));});
  await waitFor(()=>expect(screen.getByRole("button",{name:`${kind}:one`})).toHaveProperty("disabled",false));
});
it("observes an externally started batch and its affected sessions through polling, including failure cleanup",async()=>{
  vi.useFakeTimers();
  let operations:OperationStatus[]=[];
  vi.mocked(api.getOperationStatus).mockImplementation(async()=>operations);
  render(<><Control kind="session" id="affected"/><Control kind="session" id="other"/></>);
  await act(async()=>{});
  expect(screen.getByRole("button",{name:"session:affected"})).toHaveProperty("disabled",false);
  operations=[{id:"mcp",kind:"summary",state:"running",targets:[{kind:"task",id:"task"},{kind:"session",id:"affected"}],agent:null}];
  await act(async()=>{await vi.advanceTimersByTimeAsync(1000);});
  expect(screen.getByRole("button",{name:"session:affected"})).toHaveProperty("disabled",true);
  expect(screen.getByRole("button",{name:"session:other"})).toHaveProperty("disabled",false);
  operations=[];
  await act(async()=>{await vi.advanceTimersByTimeAsync(1000);});
  expect(screen.getByRole("button",{name:"session:affected"})).toHaveProperty("disabled",false);
});

it("limits source synchronization to affected sessions while a full synchronization covers all sessions",async()=>{
  let operations:OperationStatus[]=[{id:"source",kind:"sync",state:"queued",agent:"codex",targets:[{kind:"source",id:"source"},{kind:"session",id:"inside"}]}];
  vi.mocked(api.getOperationStatus).mockImplementation(async()=>operations);
  function Sync({id}:{id:string}) { const state=useOperationState("sync","session",id);return <button disabled={state.busy || state.checking}>{id}</button>; }
  render(<><Sync id="inside"/><Sync id="outside"/></>);
  await waitFor(()=>expect(screen.getByRole("button",{name:"outside"})).toHaveProperty("disabled",false));
  expect(screen.getByRole("button",{name:"inside"})).toHaveProperty("disabled",true);
  operations=[{...operations[0],targets:[{kind:"all",id:""}],state:"running"}];
  await act(async()=>{window.dispatchEvent(new Event("focus"));});
  await waitFor(()=>expect(screen.getByRole("button",{name:"outside"})).toHaveProperty("disabled",true));
});
