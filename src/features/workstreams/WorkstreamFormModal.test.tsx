import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach,beforeEach,expect,it,vi } from "vitest";
import { api } from "../../api";
import WorkstreamFormModal from "./WorkstreamFormModal";
import type { ProjectCardData,Workstream } from "../../types";
vi.mock("../../api",()=>({api:{listProjectCards:vi.fn(),createWorkstream:vi.fn(),updateWorkstream:vi.fn(),setWorkstreamProjects:vi.fn(),addProjectPath:vi.fn()}}));
vi.mock("@tauri-apps/plugin-dialog",()=>({open:vi.fn()}));
const task:Workstream={id:"t",title:"任务",description:"目标",visibility:"normal",created_at:"",updated_at:""};
const project={id:"p",name:"聊天项目",kind:"directory_group",purpose:"chat",search_paths:["/one","/two"]} as ProjectCardData;
beforeEach(()=>{vi.clearAllMocks();vi.mocked(api.listProjectCards).mockResolvedValue([project]);vi.mocked(api.createWorkstream).mockResolvedValue(task);});
afterEach(cleanup);
it("creates with explicit project ids, including chat projects",async()=>{
  render(<WorkstreamFormModal onClose={vi.fn()} initialProjectId="p"/>);
  fireEvent.change(screen.getByRole("textbox",{name:"标题"}),{target:{value:"新任务"}});
  fireEvent.submit(screen.getByRole("textbox",{name:"标题"}).closest("form")!);
  await waitFor(()=>expect(api.createWorkstream).toHaveBeenCalledWith("新任务","",["p"]));
});
it("allows removing the preset project before creation",async()=>{
  render(<WorkstreamFormModal onClose={vi.fn()} initialProjectId="p"/>);
  fireEvent.click(await screen.findByRole("button",{name:"移除项目 聊天项目"}));
  fireEvent.change(screen.getByRole("textbox",{name:"标题"}),{target:{value:"独立任务"}});
  fireEvent.submit(screen.getByRole("textbox",{name:"标题"}).closest("form")!);
  await waitFor(()=>expect(api.createWorkstream).toHaveBeenCalledWith("独立任务","",[]));
});
it("editing retains selected project ids even when its directory set changes",async()=>{
  render(<WorkstreamFormModal workstream={task} projects={[{id:"p",name:"聊天项目"}]} onClose={vi.fn()}/>);
  fireEvent.change(screen.getByRole("textbox",{name:"标题"}),{target:{value:"更新标题"}});
  fireEvent.click(screen.getByRole("button",{name:"保存"}));
  await waitFor(()=>expect(api.setWorkstreamProjects).toHaveBeenCalledWith("t",["p"]));
  expect(api.updateWorkstream).toHaveBeenCalledWith({...task,title:"更新标题"});
});
it("keeps failed saves open with actionable error",async()=>{
  vi.mocked(api.createWorkstream).mockRejectedValue(new Error("项目已删除"));
  render(<WorkstreamFormModal onClose={vi.fn()} initialProjectId="p"/>);
  fireEvent.change(screen.getByRole("textbox",{name:"标题"}),{target:{value:"任务"}});
  fireEvent.submit(screen.getByRole("textbox",{name:"标题"}).closest("form")!);
  expect((await screen.findByRole("alert")).textContent).toContain("项目已删除");
  expect(screen.getByRole("dialog")).toBeTruthy();
});
it("opens project multi-select list and allows toggling projects", async () => {
  const secondProject = { id: "p2", name: "后端服务", kind: "directory_group", purpose: "code", search_paths: ["/three"] } as unknown as ProjectCardData;
  const missingProject = { id: "p3", name: "缺失项目", path_count: 1, missing_path_count: 1, search_paths: ["/missing"] } as unknown as ProjectCardData;
  const noEndingChat = { id: "nec", name: "NoEnding Chat", kind: "directory_group", purpose: "chat", search_paths: ["/chat"] } as unknown as ProjectCardData;
  vi.mocked(api.listProjectCards).mockResolvedValue([project, secondProject, missingProject, noEndingChat]);
  render(<WorkstreamFormModal onClose={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "添加关联项目" }));
  expect(await screen.findByRole("checkbox", { name: "NoEnding Chat" })).toBeTruthy();
  expect(screen.queryByRole("checkbox", { name: "聊天项目" })).toBeNull();
  expect(screen.queryByRole("checkbox", { name: "缺失项目" })).toBeNull();
  const checkbox2 = screen.getByRole("checkbox", { name: "后端服务" });
  expect(checkbox2).toHaveProperty("checked", false);
  fireEvent.click(checkbox2);
  expect(checkbox2).toHaveProperty("checked", true);
  expect(screen.getByRole("button", { name: "移除项目 后端服务" })).toBeTruthy();
  fireEvent.change(screen.getByRole("textbox", { name: "标题" }), { target: { value: "多项目任务" } });
  fireEvent.submit(screen.getByRole("textbox", { name: "标题" }).closest("form")!);
  await waitFor(() => expect(api.createWorkstream).toHaveBeenCalledWith("多项目任务", "", ["p2"]));
});
