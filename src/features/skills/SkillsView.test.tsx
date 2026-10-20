import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import SkillsView from "./SkillsView";
import SkillDetailView, { clearSkillDetailCache } from "./SkillDetailView";
import type { Route } from "../../app/routes";
import { viewState } from "../../hooks/useViewState";
import type { SkillInventory, SkillLocation, SkillProjectScope, SkillResource, SkillSourceKind, ProjectCardData, ProjectDetailData, WorkspacePath } from "../../types";

vi.mock("../../api", () => ({ api: {
  listProjects: vi.fn(), listProjectCards: vi.fn(), getProjectDetail: vi.fn(), listSkills: vi.fn(), listSkillCatalog: vi.fn(),
  getSkillDetail: vi.fn(),
  registerSkillSource: vi.fn(), unregisterSkill: vi.fn(), uninstallSkill: vi.fn(), skillUninstallPreview: vi.fn(), skillRegistrationPreview: vi.fn(), registerDiscoveredSkill: vi.fn(), disableSkillAll: vi.fn(), enableSkillAll: vi.fn(), enableSkillProjects: vi.fn(), disableSkillProjects: vi.fn(),
  openPath: vi.fn(),
} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../../components/Toast", () => ({ showToast: vi.fn() }));
const loc: SkillLocation = { id: "pi-user", agent: "pi", scope: "user", path: "/home/test/.pi/agent/skills/docs/SKILL.md", label: "Pi" };
const resource = (overrides: Partial<SkillResource> = {}): SkillResource => {
  return { id: "server", name: "docs", description: "Project documentation", path: loc.path, locations: [loc.id], entry_paths: [loc.path], entry_links: [], revision: "original-revision", registered: true, error: null, ...overrides };
};
const projectScope = (overrides: Partial<SkillProjectScope> = {}): SkillProjectScope => ({ project_id: "p1", project_name: "Alpha", workspaces: [], enabled: false, full: false, ...overrides });
const projectCard = (overrides: Partial<ProjectCardData> = {}): ProjectCardData => ({ id: "p1", name: "Alpha", name_customized: false, visibility: "normal", kind: "single_directory", path_count: 1, missing_path_count: 0, workstream_count: 0, session_count: 0, representative_paths: ["/work/one"], search_paths: ["/work/one"], last_activity_at: null, updated_at: "", ...overrides });
const bundle = (row: SkillResource, content: string, scopes: SkillProjectScope[] = [], sourceKind?: SkillSourceKind) =>
  ({ detail: { resource: row, content }, locations: inventory.locations, scopes, source_kind: sourceKind ?? (row.entry_links.some(link => !link) ? "agent" : "external") });
let inventory: SkillInventory;
const navigation = vi.fn();
function SkillTestApp({ initialProjectId }: { initialProjectId?: string }) {
  const [route, setRoute] = useState<Route>({ view: "skills", projectId: initialProjectId });
  const navigate = (next: Route) => { navigation(next); setRoute(next); };
  if (route.view === "skill") return <SkillDetailView key={route.resourceId} {...route} navigate={navigate} />;
  return <SkillsView initialProjectId={initialProjectId} navigate={navigate} />;
}
beforeEach(() => {
  vi.clearAllMocks();
  clearSkillDetailCache();
  viewState.clear(); localStorage.clear();
  inventory = { locations: [loc], resources: [resource()], warnings: [] };
  vi.mocked(api.listProjects).mockResolvedValue([]);
  vi.mocked(api.listProjectCards).mockResolvedValue([]);
  vi.mocked(api.listSkills).mockImplementation(async () => inventory);
  vi.mocked(api.listSkillCatalog).mockImplementation(async () => inventory);
  vi.mocked(api.getSkillDetail).mockImplementation(async id => {
    const row = inventory.resources.find(r => r.id === id);
    return row ? bundle(row, "# Documentation workflow") : null;
  });
  vi.mocked(api.openPath).mockResolvedValue();
});
afterEach(cleanup);

it("reads Skill content and enablement without exposing a Skill editor", async () => {
  const sourcePath = "/home/test/skills/review/SKILL.md";
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills" }], resources: [resource({ id: "review", name: "review", path: sourcePath, locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] })], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(inventory.resources[0], "# Review workflow\n\nRead source changes."));
  render(<SkillTestApp />);
  fireEvent.click(await screen.findByRole("button", { name: /review/ }));
  expect(await screen.findByRole("heading", { name: "描述" })).toBeTruthy();
  expect(screen.getByText("Project documentation")).toBeTruthy();
  expect(screen.queryByRole("heading", { name: "Review workflow" })).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "查看全文" }));
  const dialog = await screen.findByRole("dialog", { name: "review · SKILL.md" });
  expect(await within(dialog).findByRole("heading", { name: "Review workflow" })).toBeTruthy();

  fireEvent.click(within(dialog).getByRole("button", { name: "原文" }));
  expect(within(dialog).getByText(/# Review workflow/)).toBeTruthy();
  fireEvent.click(within(dialog).getByRole("button", { name: "预览" }));
  expect(within(dialog).getByRole("heading", { name: "Review workflow" })).toBeTruthy();

  fireEvent.click(within(dialog).getByRole("button", { name: "关闭" }));
  expect(screen.queryByRole("dialog")).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: sourcePath }));
  expect(api.openPath).toHaveBeenCalledWith(sourcePath);
  expect(navigation).toHaveBeenCalledWith(expect.objectContaining({ view: "skill", resourceId: "review", initialTitle: "review" }));
});

it("previews YAML frontmatter metadata and markdown content in full-text modal", async () => {
  const rawContent = `---\nname: wechat-decrypt\ndescription: Read WeChat history on macOS.\n---\n# Instructions\n\nRun the decrypt tool.`;
  const skill = resource({ id: "wechat", name: "wechat-decrypt", description: "Read WeChat history on macOS." });
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, rawContent));
  render(<SkillDetailView resourceId="wechat" initialTitle="wechat-decrypt" navigate={navigation} />);

  fireEvent.click(await screen.findByRole("button", { name: "查看全文" }));
  const dialog = await screen.findByRole("dialog", { name: "wechat-decrypt · SKILL.md" });

  // In preview mode: frontmatter metadata is rendered
  expect(within(dialog).getByText("name")).toBeTruthy();
  expect(within(dialog).getByText("wechat-decrypt")).toBeTruthy();
  expect(within(dialog).getByText("description")).toBeTruthy();
  expect(within(dialog).getByText("Read WeChat history on macOS.")).toBeTruthy();
  expect(within(dialog).getByRole("heading", { name: "Instructions" })).toBeTruthy();

  // In source mode: full raw content including delimiters is visible
  fireEvent.click(within(dialog).getByRole("button", { name: "原文" }));
  expect(within(dialog).getByText((_, el) => el?.tagName === "PRE" && el.textContent === rawContent)).toBeTruthy();
});

it("displays Windows Skill paths without the verbatim prefix while opening the original path", async () => {
  const source = String.raw`\\?\C:\Users\me/skills/review/SKILL.md`;
  const directory = String.raw`\\?\C:\Users\me/skills/review`;
  const displayed = String.raw`C:\Users\me\skills\review\SKILL.md`;
  const skill = resource({ id: "review", name: "review", path: source });
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review"));
  vi.mocked(api.skillRegistrationPreview).mockResolvedValue({ source_path: directory, target_path: String.raw`C:\Users\me/.noending/skills/review`, entry_count: 1, physical_entries: 0 });
  vi.mocked(api.skillUninstallPreview).mockResolvedValue({ source_path: directory, entry_count: 1, delete_source_required: false });
  render(<SkillDetailView resourceId="review" navigate={navigation} />);
  const sourceButton = await screen.findByRole("button", { name: displayed });
  expect(sourceButton.title).toBe(`${displayed} · 点击在文件管理器中打开`);
  fireEvent.click(sourceButton);
  expect(api.openPath).toHaveBeenCalledWith(source);
  fireEvent.click(screen.getByRole("button", { name: "迁移到 NoEnding" }));
  expect(await within(screen.getByRole("dialog")).findByText(String.raw`C:\Users\me\skills\review`)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "取消" }));
  fireEvent.click(screen.getByRole("button", { name: "卸载 Skill" }));
  expect(await within(screen.getByRole("dialog")).findByText(String.raw`C:\Users\me\skills\review`)).toBeTruthy();
});

it("normalizes only the path in Windows scan warnings", async () => {
  inventory = { resources: [], locations: [], warnings: [String.raw`C:\Workspace\project/.claude/skills：目录名称无效。 (os error 267)`, String.raw`\\?\UNC\server\share/skills：拒绝访问`, "其他扫描提示"] };
  render(<SkillsView navigate={navigation} />);
  expect(await screen.findByText(String.raw`C:\Workspace\project\.claude\skills：目录名称无效。 (os error 267)`)).toBeTruthy();
  expect(screen.getByText(String.raw`\\server\share\skills：拒绝访问`)).toBeTruthy();
  expect(screen.getByText("其他扫描提示")).toBeTruthy();
});

it("uninstalls a Skill and locks the keep-source option when the source is in an Agent directory", async () => {
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.skillUninstallPreview).mockResolvedValue({ delete_source_required: true, entry_count: 2, source_path: "/home/test/skills/review" });
  vi.mocked(api.uninstallSkill).mockResolvedValue({ message: "Removed" });
  render(<SkillDetailView resourceId="review" initialTitle="review" navigate={navigation} />);
  fireEvent.click(await screen.findByRole("button", { name: "卸载 Skill" }));
  const confirm = await screen.findByRole("button", { name: "删除并卸载" }) as HTMLButtonElement;
  await waitFor(() => expect(confirm.disabled).toBe(false));
  const keepSource = screen.getByRole("checkbox") as HTMLInputElement;
  expect(keepSource.checked).toBe(true);
  expect(keepSource.disabled).toBe(true);
  expect(screen.getByText("/home/test/skills/review")).toBeTruthy();
  fireEvent.click(confirm);
  await waitFor(() => expect(api.uninstallSkill).toHaveBeenCalledWith("review", "original-revision", true));
});

it("retains a linked Skill on failed removal and returns to the board after success without deleting its source", async () => {
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.skillUninstallPreview).mockResolvedValue({ delete_source_required: false, entry_count: 1, source_path: "/home/test/skills/review" });
  vi.mocked(api.uninstallSkill).mockRejectedValueOnce(new Error("目录被占用"))
    .mockImplementationOnce(async () => { inventory.resources = []; return { message: "Removed" }; });
  render(<SkillTestApp />);
  fireEvent.click(await screen.findByRole("button", { name: /review/ }));
  fireEvent.click(await screen.findByRole("button", { name: "卸载 Skill" }));
  const keepSource = screen.getByRole("checkbox") as HTMLInputElement;
  const confirm = await screen.findByRole("button", { name: "仅移除" }) as HTMLButtonElement;
  await waitFor(() => expect(confirm.disabled).toBe(false));
  expect(keepSource.checked).toBe(false);
  expect(keepSource.disabled).toBe(false);
  fireEvent.click(confirm);
  await screen.findAllByText("Error: 目录被占用");
  expect(screen.getByLabelText("技能详情")).toBeTruthy();
  expect(navigation).toHaveBeenCalledTimes(1);
  fireEvent.click(confirm);
  await waitFor(() => expect(navigation).toHaveBeenLastCalledWith({ view: "skills" }));
  expect(api.uninstallSkill).toHaveBeenLastCalledWith("review", "original-revision", false);
  expect(screen.queryByLabelText("技能详情")).toBeNull();
});

it("shows user-level coverage and disables it by clearing the select-all scope", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: [entry], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.disableSkillAll).mockResolvedValue({ message: "Disabled" });
  render(<SkillDetailView resourceId="review" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  expect(within(section).getByText("用户级")).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Codex" })).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Pi" })).toBeTruthy();
  expect(within(section).queryByText(entry)).toBeNull();
  fireEvent.click(within(section).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置技能" });
  const master = await within(dialog).findByRole("checkbox", { name: /全选/ }) as HTMLInputElement;
  expect(master.checked).toBe(true);
  fireEvent.click(master);
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.disableSkillAll).toHaveBeenCalledWith("review"));
});

it("keeps unrelated detail actions available while a scope change runs", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: [entry], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  let finish!: () => void;
  vi.mocked(api.disableSkillAll).mockImplementationOnce(() => new Promise(resolve => { finish = () => resolve({ message: "Disabled" }); }));
  render(<SkillDetailView resourceId="review" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  fireEvent.click(within(section).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置技能" });
  fireEvent.click(await within(dialog).findByRole("checkbox", { name: /全选/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.disableSkillAll).toHaveBeenCalled());
  expect((within(dialog).getByRole("button", { name: "保存中…" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "卸载 Skill" }) as HTMLButtonElement).disabled).toBe(false);
  expect((screen.getByRole("button", { name: "迁移到 NoEnding" }) as HTMLButtonElement).disabled).toBe(false);
  await act(async () => { finish(); });
});

it("adds project-level configuration from the detail page", async () => {
  const skill = resource({ id: "review", name: "review", locations: [], entry_paths: [], entry_links: [] });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.listProjectCards).mockResolvedValue([
    projectCard(),
    projectCard({ id: "p2", name: "Beta", representative_paths: ["/work/two"], search_paths: ["/work/two"] }),
  ]);
  vi.mocked(api.enableSkillProjects).mockResolvedValue({ message: "Enabled" });
  render(<SkillDetailView resourceId="review" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  fireEvent.click(within(section).getByRole("button", { name: "配置" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /Alpha/ }));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableSkillProjects).toHaveBeenCalledWith(["p1"], "review"));
});

it("marks a physical source scope without offering disable", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: [entry], entry_links: [false] });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  render(<SkillDetailView resourceId="review" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  expect(within(section).getAllByText("物理入口").length).toBeGreaterThan(0);
});

it.each([true, false])("registers a physical Skill source with migration=%s", async migrateSource => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: [entry], entry_links: [false], registered: false });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.skillRegistrationPreview).mockResolvedValue({ source_path: entry, target_path: "/home/test/.noending/skills/review", entry_count: 1, physical_entries: 1 });
  vi.mocked(api.registerDiscoveredSkill).mockImplementation(async () => {
    if (!migrateSource) skill.registered = true;
    return { message: "Registered", source_path: migrateSource ? "/home/test/.noending/skills/review/SKILL.md" : skill.path, resource_id: migrateSource ? "migrated-review" : "review", name: "review", description: "Review" };
  });
  render(<SkillTestApp />);
  fireEvent.click(screen.getByRole("button", { name: "未托管" }));
  fireEvent.click(await screen.findByRole("button", { name: /review/ }));
  const section = await screen.findByLabelText("源目录");
  expect(screen.queryByRole("button", { name: "配置" })).toBeNull();
  fireEvent.click(within(section).getByRole("button", { name: "托管到 NoEnding" }));
  const dialog = await screen.findByRole("dialog", { name: "托管到 NoEnding" });
  expect(await within(dialog).findByText(/1 个（其中 1 个物理入口）/)).toBeTruthy();
  const removeSource = within(dialog).getByRole("checkbox", { name: "迁移源目录到 NoEnding" }) as HTMLInputElement;
  expect(removeSource.checked).toBe(false);
  expect(removeSource.disabled).toBe(false);

  if (migrateSource) fireEvent.click(removeSource);
  else {
    expect(within(dialog).getByText(/对应作用域会保持启用，不能直接停用/)).toBeTruthy();
  }
  fireEvent.click(within(dialog).getByRole("button", { name: "托管" }));
  await waitFor(() => expect(api.registerDiscoveredSkill).toHaveBeenCalledWith("review", migrateSource));
  if (migrateSource) await waitFor(() => expect(navigation).toHaveBeenLastCalledWith(expect.objectContaining({ view: "skill", resourceId: "migrated-review", initialTitle: "review" })));
  else await waitFor(() => expect(within(screen.getByLabelText("启用范围")).getByRole("button", { name: "配置" })).toBeTruthy());
});

it("references a discovered Skill in place and refreshes the same detail as managed", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", name: "review", path: "/home/test/external/review/SKILL.md", locations: ["shared"], entry_paths: [entry], entry_links: [true], registered: false });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.skillRegistrationPreview).mockResolvedValue({ source_path: "/home/test/external/review/SKILL.md", target_path: "/home/test/.noending/skills/review", entry_count: 1, physical_entries: 0 });
  vi.mocked(api.registerDiscoveredSkill).mockImplementation(async () => {
    skill.registered = true;
    return { message: "Registered", source_path: skill.path, resource_id: "review", name: "review", description: "Review" };
  });
  render(<SkillDetailView resourceId="review" initialTitle="review" navigate={navigation} />);
  expect(await screen.findByText("外部目录")).toBeTruthy();
  const section = await screen.findByLabelText("源目录");
  fireEvent.click(within(section).getByRole("button", { name: "托管到 NoEnding" }));
  const dialog = await screen.findByRole("dialog", { name: "托管到 NoEnding" });
  expect(await within(dialog).findByText("1 个链接入口")).toBeTruthy();
  const removeSource = within(dialog).getByRole("checkbox", { name: "迁移源目录到 NoEnding" }) as HTMLInputElement;
  expect(removeSource.checked).toBe(false);
  expect(removeSource.disabled).toBe(false);
  fireEvent.click(within(dialog).getByRole("button", { name: "托管" }));
  await waitFor(() => expect(api.registerDiscoveredSkill).toHaveBeenCalledWith("review", false));
  await waitFor(() => expect(within(screen.getByLabelText("启用范围")).getByRole("button", { name: "配置" })).toBeTruthy());
  expect(within(section).getByRole("button", { name: "迁移到 NoEnding" })).toBeTruthy();
  expect(navigation).not.toHaveBeenCalled();
  expect(api.getSkillDetail).toHaveBeenCalledTimes(2);
});

it.each([true, false])("unregisters an external Skill with removeLinks=%s and refreshes its management status", async removeLinks => {
  const entry = "/home/test/.claude/skills/review/SKILL.md";
  const skill = resource({ id: "review", name: "review", path: entry, locations: [loc.id], entry_paths: [entry], entry_links: [false] });
  inventory.resources = [skill];
  vi.mocked(api.unregisterSkill).mockImplementation(async () => {
    skill.registered = false;
    return { message: "已取消托管" };
  });
  render(<SkillDetailView resourceId="review" navigate={navigation} />);
  const section = await screen.findByLabelText("源目录");
  expect(within(section).getByRole("button", { name: "迁移到 NoEnding" })).toBeTruthy();
  fireEvent.click(within(section).getByRole("button", { name: "取消托管" }));
  const dialog = await screen.findByRole("dialog", { name: "取消托管" });
  const cleanup = within(dialog).getByRole("checkbox", { name: "同时移除所有链接入口" }) as HTMLInputElement;
  expect(cleanup.checked).toBe(false);
  if (removeLinks) fireEvent.click(cleanup);
  fireEvent.click(within(dialog).getByRole("button", { name: "确认取消托管" }));
  await waitFor(() => expect(api.unregisterSkill).toHaveBeenCalledWith("review", removeLinks));
  await within(section).findByRole("button", { name: "托管到 NoEnding" });
  expect(screen.queryByRole("button", { name: "配置" })).toBeNull();
  expect(within(section).queryByRole("button", { name: "取消托管" })).toBeNull();
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(navigation).not.toHaveBeenCalled();
  expect(api.uninstallSkill).not.toHaveBeenCalled();
});

it.each(["library", "unregistered"])("does not offer unregister for a %s Skill", async kind => {
  const skill = resource({ registered: kind !== "unregistered" });
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Skill", [], kind === "library" ? "library" : "external"));
  render(<SkillDetailView resourceId={skill.id} navigate={navigation} />);
  const section = await screen.findByLabelText("源目录");
  expect(within(section).queryByRole("button", { name: "取消托管" })).toBeNull();
});

it("cancels unregister without changing the Skill and keeps failures retryable", async () => {
  const skill = resource({ id: "review", locations: [], entry_paths: [], entry_links: [] });
  inventory.resources = [skill];
  vi.mocked(api.unregisterSkill).mockRejectedValueOnce(new Error("链接被占用"))
    .mockImplementationOnce(async () => { inventory.resources = []; return { message: "已取消托管" }; });
  render(<SkillTestApp />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  fireEvent.click(await screen.findByRole("button", { name: "取消托管" }));
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: /^取消$/ }));
  expect(api.unregisterSkill).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "取消托管" }));
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "确认取消托管" }));
  await within(screen.getByRole("dialog")).findByRole("alert");
  expect(screen.getByLabelText("技能详情")).toBeTruthy();
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "确认取消托管" }));
  await waitFor(() => expect(navigation).toHaveBeenLastCalledWith({ view: "skills" }));
  expect(screen.queryByLabelText("技能详情")).toBeNull();
});

it("registers a discovered Skill from its card", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: [entry], entry_links: [true], registered: false });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.skillRegistrationPreview).mockResolvedValue({ source_path: "/home/test/external/review/SKILL.md", target_path: "/home/test/.noending/skills/review", entry_count: 1, physical_entries: 0 });
  vi.mocked(api.registerDiscoveredSkill).mockResolvedValue({ message: "Registered", source_path: skill.path, resource_id: "review", name: "review", description: "Review" });
  render(<SkillTestApp />);
  fireEvent.click(screen.getByRole("button", { name: "未托管" }));
  const card = await screen.findByRole("button", { name: /review/ });
  expect(within(card).queryByRole("button", { name: "配置" })).toBeNull();
  fireEvent.click(within(card).getByRole("button", { name: "托管" }));
  const dialog = await screen.findByRole("dialog", { name: "托管到 NoEnding" });
  await within(dialog).findByText("1 个链接入口");
  fireEvent.click(within(dialog).getByRole("button", { name: "托管" }));
  await waitFor(() => expect(api.registerDiscoveredSkill).toHaveBeenCalledWith("review", false));
});

it("keeps a disabled registered Skill listed and enables it from the detail", async () => {
  const skillLocation: SkillLocation = { ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", name: "review", path: "/home/test/skills/review/SKILL.md", locations: [], entry_paths: [], entry_links: [], registered: true });
  inventory = { locations: [skillLocation], resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.enableSkillAll).mockResolvedValue({ message: "Enabled" });
  render(<SkillTestApp />);
  const row = await screen.findByRole("button", { name: /review/ });
  expect(within(row).getByText("未启用")).toBeTruthy();
  expect(within(row).queryByRole("button", { name: "托管" })).toBeNull();
  fireEvent.click(row);
  const section = await screen.findByLabelText("启用范围");
  expect(within(section).getByText("当前未在任何作用域启用。")).toBeTruthy();
  fireEvent.click(within(section).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置技能" });
  fireEvent.click(await within(dialog).findByRole("checkbox", { name: /全选/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableSkillAll).toHaveBeenCalledWith("review"));
});

it("enables a Skill at user level from its card", async () => {
  const userLocation: SkillLocation = { ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", name: "review", locations: [], entry_paths: [], entry_links: [] });
  inventory = { locations: [userLocation], resources: [skill], warnings: [] };
  vi.mocked(api.enableSkillAll).mockResolvedValue({ message: "Enabled" });
  render(<SkillTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置技能" });
  fireEvent.click(await within(dialog).findByRole("checkbox", { name: /全选/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableSkillAll).toHaveBeenCalledWith("review"));
  expect(navigation).not.toHaveBeenCalled();
});

it("offers only the fill action for a physical user entry", async () => {
  const userLocation: SkillLocation = { ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [false] });
  inventory = { locations: [userLocation], resources: [skill], warnings: [] };
  vi.mocked(api.enableSkillAll).mockResolvedValue({ message: "Enabled" });
  render(<SkillTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置技能" });
  const master = await within(dialog).findByRole("checkbox", { name: /全选/ }) as HTMLInputElement;
  expect(master.checked).toBe(true);
  expect(master.disabled).toBe(true);
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableSkillAll).toHaveBeenCalledWith("review"));
});

it("applies project-level changes from a card, including removals", async () => {
  const userLocation: SkillLocation = { ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const projectLocation: SkillLocation = { ...loc, id: "project-skill", agent: null, scope: "project", path: "/work/one/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", name: "review", locations: ["project-skill"], entry_paths: ["/work/one/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [userLocation, projectLocation], resources: [skill], warnings: [] };
  vi.mocked(api.listProjectCards).mockResolvedValue([
    projectCard({ id: "p1", name: "Alpha", search_paths: ["/work/one"] }),
    projectCard({ id: "p2", name: "Beta", representative_paths: ["/work/two"], search_paths: ["/work/two"] }),
    projectCard({ id: "p3", name: "Empty", path_count: 0, missing_path_count: 0, representative_paths: [], search_paths: [] }),
  ]);
  vi.mocked(api.enableSkillProjects).mockResolvedValue({ message: "Enabled" });
  vi.mocked(api.disableSkillProjects).mockResolvedValue({ message: "Disabled" });
  render(<SkillTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const alpha = await screen.findByRole("checkbox", { name: /Alpha/ }) as HTMLInputElement;
  expect(alpha.checked).toBe(true);
  expect(alpha.disabled).toBe(false);
  expect((screen.getByRole("checkbox", { name: /Empty/ }) as HTMLInputElement).disabled).toBe(true);
  fireEvent.click(alpha);
  fireEvent.click(screen.getByRole("checkbox", { name: /Beta/ }));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableSkillProjects).toHaveBeenCalledWith(["p2"], "review"));
  expect(api.disableSkillProjects).toHaveBeenCalledWith(["p1"], "review");
});

it("converts user level to the remaining projects when one is unchecked", async () => {
  const userLocation: SkillLocation = { ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [userLocation], resources: [skill], warnings: [] };
  vi.mocked(api.listProjectCards).mockResolvedValue([
    projectCard(),
    projectCard({ id: "p2", name: "Beta", representative_paths: ["/work/two"], search_paths: ["/work/two"] }),
  ]);
  vi.mocked(api.disableSkillAll).mockResolvedValue({ message: "Disabled" });
  vi.mocked(api.enableSkillProjects).mockResolvedValue({ message: "Enabled" });
  render(<SkillTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置技能" });
  const master = await within(dialog).findByRole("checkbox", { name: /全选/ }) as HTMLInputElement;
  const beta = within(dialog).getByRole("checkbox", { name: /Beta/ }) as HTMLInputElement;
  expect(master.checked).toBe(true);
  expect(beta.checked).toBe(true);
  fireEvent.click(beta);
  expect(master.checked).toBe(false);
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableSkillProjects).toHaveBeenCalledWith(["p1"], "review"));
  expect(api.disableSkillAll).toHaveBeenCalledWith("review");
  expect(api.enableSkillAll).not.toHaveBeenCalled();
  expect(vi.mocked(api.disableSkillAll).mock.invocationCallOrder[0])
    .toBeLessThan(vi.mocked(api.enableSkillProjects).mock.invocationCallOrder[0]);
});

it("refreshes the board silently after saving a scope change", async () => {
  const userLocation: SkillLocation = { ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", name: "review", locations: [], entry_paths: [], entry_links: [] });
  const other = resource({ id: "lint", name: "lint", locations: [], entry_paths: [], entry_links: [] });
  inventory = { locations: [userLocation], resources: [skill, other], warnings: [] };
  let finish: (value: SkillInventory) => void = () => {};
  vi.mocked(api.listSkillCatalog)
    .mockImplementationOnce(async () => inventory)
    .mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  vi.mocked(api.enableSkillAll).mockResolvedValue({ message: "Enabled" });
  render(<SkillTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置技能" });
  fireEvent.click(await within(dialog).findByRole("checkbox", { name: /全选/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.listSkillCatalog).toHaveBeenCalledTimes(2));
  expect(screen.getByRole("button", { name: /lint/ })).toBeTruthy();
  expect(within(dialog).getByRole("button", { name: "保存中…" })).toBeTruthy();
  await act(async () => { finish(inventory); });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});

it("does not mark a project-enabled Skill as disabled", async () => {
  const projectLocation: SkillLocation = { ...loc, id: "project-skill", agent: null, scope: "project", path: "/work/one/.agents/skills", label: "共享 Skills" };
  inventory = {
    locations: [projectLocation],
    resources: [resource({ id: "zeta", name: "zeta", locations: ["project-skill"], entry_paths: ["/work/one/.agents/skills/zeta/SKILL.md"], entry_links: [true] })],
    warnings: [],
  };
  render(<SkillTestApp />);
  const card = await screen.findByRole("button", { name: /zeta/ });
  expect(within(card).queryByText("未启用")).toBeNull();
});

it("sorts by name by default and by enablement scope on request", async () => {
  const userLocation: SkillLocation = { ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills" };
  const projectLocation: SkillLocation = { ...loc, id: "project-skill", agent: null, scope: "project", path: "/work/one/.agents/skills" };
  inventory = {
    locations: [userLocation, projectLocation],
    resources: [
      resource({ id: "zeta", name: "zeta", locations: ["project-skill"], entry_paths: ["/work/one/.agents/skills/zeta/SKILL.md"], entry_links: [true] }),
      resource({ id: "alpha", name: "alpha", locations: [], entry_paths: [], entry_links: [] }),
      resource({ id: "beta", name: "beta", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/beta/SKILL.md"], entry_links: [true] }),
    ],
    warnings: [],
  };
  render(<SkillTestApp />);
  const section = await screen.findByRole("region", { name: "技能" });
  const names = () => within(section).getAllByRole("button", { name: /^(alpha|beta|zeta)/ }).map(button => button.textContent?.match(/^(alpha|beta|zeta)/)?.[0]);
  expect(names()).toEqual(["alpha", "beta", "zeta"]);
  fireEvent.change(screen.getByRole("combobox", { name: "排序" }), { target: { value: "scope" } });
  expect(names()).toEqual(["beta", "zeta", "alpha"]);
});

it("summarizes user-level Agent coverage read-only when every Agent is covered", async () => {
  const locations: SkillLocation[] = [
    { ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" },
    { ...loc, id: "claude", agent: "claude_code", path: "/home/test/.claude/skills", label: "Claude Code Skills" },
    { ...loc, id: "antigravity", agent: "antigravity", path: "/home/test/.gemini/antigravity-cli/skills", label: "Antigravity Skills" },
  ];
  const entries = [
    "/home/test/.agents/skills/review/SKILL.md",
    "/home/test/.claude/skills/review/SKILL.md",
    "/home/test/.gemini/antigravity-cli/skills/review/SKILL.md",
  ];
  const skill = resource({ id: "review", name: "review", locations: locations.map(l => l.id), entry_paths: entries, entry_links: [true, true, true] });
  inventory = { locations, resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  render(<SkillDetailView resourceId="review" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  expect(within(section).getByText("用户级")).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Codex" })).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Pi" })).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Antigravity" })).toBeTruthy();
});

it("shows project enablement read-only without a project association", async () => {
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes.", [projectScope({
    workspaces: [
      { workspace_path_id: "w1", path: "/work/alpha", entries: [{ path: "/work/alpha/.agents/skills/review/SKILL.md", link: true, label: "共享 Skills", agents: ["codex", "pi", "antigravity"] }] },
      { workspace_path_id: "w2", path: "/work/beta", entries: [] },
    ],
    enabled: true,
  }), projectScope({
    project_id: "p2",
    project_name: "Beta",
    workspaces: [{ workspace_path_id: "w3", path: "/work/beta", entries: [] }],
  })]));
  render(<SkillDetailView resourceId="review" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  expect(api.getSkillDetail).toHaveBeenCalledWith("review");
  expect(api.listSkillCatalog).not.toHaveBeenCalled();
  expect(within(section).getByText("Alpha")).toBeTruthy();
  expect(within(section).queryByText("/work/alpha/.agents/skills/review/SKILL.md")).toBeNull();
  expect(within(section).getByRole("img", { name: "Antigravity" })).toBeTruthy();
  expect(within(section).queryByText("Beta")).toBeNull();
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("marks a physical project scope as a source directory and keeps the detail read-only", async () => {
  const skill = resource({ id: "review", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getSkillDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes.", [projectScope({
    workspaces: [{ workspace_path_id: "w1", path: "/work/alpha", entries: [
      { path: "/work/alpha/.agents/skills/review/SKILL.md", link: true, label: "共享 Skills", agents: ["codex", "pi", "antigravity"] },
      { path: "/work/alpha/.claude/skills/review/SKILL.md", link: true, label: "Claude Code Skills", agents: ["claude_code"] },
    ] }],
    enabled: true,
    full: true,
  }), projectScope({
    project_id: "p2",
    project_name: "Beta",
    workspaces: [{ workspace_path_id: "w2", path: "/work/beta", entries: [{ path: "/work/beta/.claude/skills/review/SKILL.md", link: false, label: "Claude Code Skills", agents: ["claude_code"] }] }],
    enabled: true,
    full: true,
  })]));
  render(<SkillDetailView resourceId="review" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  expect(within(section).getByText("Beta")).toBeTruthy();
  expect(within(section).getAllByText("物理入口").length).toBeGreaterThan(0);
});

it("keeps a referenced project source selected when clearing select-all", async () => {
  const path = "/work/one/.claude/skills/review/SKILL.md";
  const skill = resource({ id: "review", name: "review", path, locations: ["project"], entry_paths: [path], entry_links: [false] });
  inventory = { locations: [{ ...loc, id: "project", scope: "project", path: "/work/one/.claude/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.listProjectCards).mockResolvedValue([projectCard(), projectCard({ id: "p2", name: "Beta", search_paths: ["/work/two"] })]);
  render(<SkillTestApp />);
  fireEvent.click(within(await screen.findByRole("button", { name: /review/ })).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置技能" });
  const alpha = await within(dialog).findByRole("checkbox", { name: /Alpha/ }) as HTMLInputElement;
  const beta = within(dialog).getByRole("checkbox", { name: /Beta/ }) as HTMLInputElement;
  const master = within(dialog).getByRole("checkbox", { name: /全选/ });
  expect(alpha.checked).toBe(true);
  expect(alpha.disabled).toBe(true);
  fireEvent.click(master);
  expect(beta.checked).toBe(true);
  fireEvent.click(master);
  expect(alpha.checked).toBe(true);
  expect(beta.checked).toBe(false);
  expect((within(dialog).getByRole("button", { name: "保存" }) as HTMLButtonElement).disabled).toBe(true);
});

it("switches managed and unmanaged Skills and clears filters within the selected category", async () => {
  const registered = resource({ id: "registered", name: "docs", registered: true, locations: [], entry_paths: [], entry_links: [] });
  const discovered = resource({ id: "discovered", name: "native", registered: false });
  inventory = { locations: [loc], resources: [registered, discovered], warnings: [] };
  render(<SkillTestApp />);
  const categories = screen.getByRole("group", { name: "技能托管状态" });
  const managed = within(categories).getByRole("button", { name: "已托管" });
  const unmanaged = within(categories).getByRole("button", { name: "未托管" });
  expect(within(categories).getAllByRole("button")).toHaveLength(2);
  expect(managed.getAttribute("aria-pressed")).toBe("true");
  await screen.findByRole("button", { name: /docs/ });
  expect(screen.queryByRole("button", { name: /native/ })).toBeNull();
  expect(screen.getByText("共 1 项")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "清除筛选" })).toBeNull();

  fireEvent.click(unmanaged);
  expect(unmanaged.getAttribute("aria-pressed")).toBe("true");
  expect(managed.getAttribute("aria-pressed")).toBe("false");
  expect(screen.queryByRole("button", { name: /docs/ })).toBeNull();
  expect(screen.getByRole("button", { name: /native/ })).toBeTruthy();
  fireEvent.change(screen.getByRole("textbox", { name: "搜索技能" }), { target: { value: "missing" } });
  expect(screen.queryByRole("button", { name: /native/ })).toBeNull();
  expect(screen.getByText("显示 0 / 共 1 项")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "清除筛选" }));
  expect(unmanaged.getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByRole("button", { name: /native/ })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /docs/ })).toBeNull();

  fireEvent.click(managed);
  expect(screen.getByRole("button", { name: /docs/ })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /native/ })).toBeNull();
  expect(api.listSkillCatalog).toHaveBeenCalledTimes(1);
});

it.each([true, false])("registers a chosen source with migration=%s", async migrateSource => {
  const paths = ["one", "two"].map(id => ({ id, canonical_path: `/work/${id}`, project_id: "p", exists: true } as WorkspacePath));
  vi.mocked(api.getProjectDetail).mockResolvedValue({ workspace_paths: paths } as ProjectDetailData);
  vi.mocked(api.listProjects).mockResolvedValue([{ id: "p", name: "Demo", visibility: "normal" } as never]);
  const projectLocation: SkillLocation = { ...loc, id: "shared-one", agent: null, scope: "project", path: "/work/one/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", name: "review", locations: [projectLocation.id], entry_paths: ["/work/one/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [projectLocation], resources: [skill], warnings: [] };
  vi.mocked(api.registerSkillSource).mockResolvedValue({ message: "Registered" });
  render(<SkillTestApp initialProjectId="p" />);
  expect(await screen.findByRole("button", { name: /review/ })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "托管 Skill 源" }));
  fireEvent.change(screen.getByLabelText("源 Skill 目录"), { target: { value: "/tmp/review" } });
  const migrate = screen.getByRole("checkbox", { name: "迁移源目录到 NoEnding" }) as HTMLInputElement;
  expect(migrate.checked).toBe(false);
  if (migrateSource) fireEvent.click(migrate);
  fireEvent.click(screen.getByRole("button", { name: "托管源" }));
  await waitFor(() => expect(api.registerSkillSource).toHaveBeenCalledWith("/tmp/review", migrateSource));
});

it.each(["卡片视图", "列表视图"])("opens the same detail from %s without a back link", async mode => {
  render(<SkillTestApp />);
  fireEvent.click(screen.getByRole("button", { name: mode }));
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  await screen.findByLabelText("技能详情");
  expect(navigation).toHaveBeenLastCalledWith(expect.objectContaining({ view: "skill", resourceId: "server" }));
});

it("keeps the resource title and hides actions when an old detail route is no longer available", async () => {
  inventory.resources = [];
  render(<SkillDetailView resourceId="removed" initialTitle="docs" navigate={navigation} />);
  expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("docs");
  await screen.findByRole("alert");
  expect(api.getSkillDetail).toHaveBeenCalledWith("removed");
});

it("renders detail view directly without flashing an intermediate loading screen", async () => {
  let finishDetail!: (val: any) => void;
  const detailPromise = new Promise(resolve => { finishDetail = resolve; });
  vi.mocked(api.getSkillDetail).mockReturnValue(detailPromise as any);

  render(<SkillDetailView resourceId="server" initialTitle="docs" navigate={navigation} />);

  // The header renders from the route title while the detail request is pending.
  expect(screen.getByRole("heading", { name: "docs" })).toBeTruthy();

  // Resolve detail
  finishDetail(bundle(resource(), "# Documentation workflow"));
  expect(await screen.findByLabelText("技能详情")).toBeTruthy();
});

it("renders skills board directly without flashing an intermediate loading screen", async () => {
  let finishCatalog!: (val: any) => void;
  const catalogPromise = new Promise(resolve => { finishCatalog = resolve; });
  vi.mocked(api.listSkillCatalog).mockReturnValue(catalogPromise as any);

  render(<SkillsView navigate={navigation} />);

  // The board shell renders while the catalog request is pending.
  expect(screen.queryByText("没有匹配的技能。")).toBeNull();
  expect(screen.getByRole("region", { name: "技能" })).toBeTruthy();

  // Resolve catalog
  await act(async () => {
    finishCatalog(inventory);
  });
  expect(await screen.findByRole("button", { name: /docs/ })).toBeTruthy();
});
