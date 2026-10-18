import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import ExtensionsView from "./ExtensionsView";
import ExtensionDetailView from "./ExtensionDetailView";
import type { Route } from "../../app/routes";
import { viewState } from "../../hooks/useViewState";
import type { ExtensionInventory, ExtensionLocation, ExtensionProjectSkillScope, ExtensionResource, ExtensionSkillSourceKind, ProjectCardData, ProjectDetailData, WorkspacePath } from "../../types";

vi.mock("../../api", () => ({ api: {
  listProjects: vi.fn(), listProjectCards: vi.fn(), getProjectDetail: vi.fn(), listExtensionResources: vi.fn(), listExtensionCatalog: vi.fn(),
  getExtensionDetail: vi.fn(),
  registerExtensionSkillSource: vi.fn(), uninstallExtensionSkill: vi.fn(), extensionSkillUninstallPreview: vi.fn(), extensionSkillMigrationPreview: vi.fn(), migrateExtensionSkill: vi.fn(), disableExtensionSkillAll: vi.fn(), enableExtensionSkillAll: vi.fn(), enableExtensionSkillProjects: vi.fn(), disableExtensionSkillProjects: vi.fn(),
  saveMcpDefinition: vi.fn(), enableMcpDefinitionUser: vi.fn(), disableMcpDefinitionUser: vi.fn(), enableMcpDefinitionProjects: vi.fn(), disableMcpDefinitionProjects: vi.fn(), uninstallMcpDefinition: vi.fn(),
  openPath: vi.fn(),
} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../../components/Toast", () => ({ showToast: vi.fn() }));
const loc: ExtensionLocation = { id: "pi-user", kind: "mcp", agent: "pi", scope: "user", path: "/home/test/.pi/agent/mcp.json", label: "Pi" };
const resource = (overrides: Partial<ExtensionResource> = {}): ExtensionResource => {
  const kind = overrides.kind ?? "mcp";
  return { id: "server", kind, name: "docs", description: "Project documentation", path: loc.path, locations: [loc.id], entry_paths: [loc.path], entry_links: [], revision: "original-revision", registered: kind === "skill", error: null, ...overrides };
};
const projectScope = (overrides: Partial<ExtensionProjectSkillScope> = {}): ExtensionProjectSkillScope => ({ project_id: "p1", project_name: "Alpha", workspaces: [], enabled: false, full: false, ...overrides });
const projectCard = (overrides: Partial<ProjectCardData> = {}): ProjectCardData => ({ id: "p1", name: "Alpha", name_customized: false, visibility: "normal", kind: "single_directory", path_count: 1, missing_path_count: 0, workstream_count: 0, session_count: 0, representative_paths: ["/work/one"], search_paths: ["/work/one"], last_activity_at: null, updated_at: "", ...overrides });
const bundle = (row: ExtensionResource, content: string, config: Record<string, unknown> | null = null, scopes: ExtensionProjectSkillScope[] = [], sourceKind?: ExtensionSkillSourceKind) =>
  ({ detail: { resource: row, content, config }, locations: inventory.locations, scopes, source_kind: sourceKind ?? (row.kind === "mcp" ? null : row.entry_links.some(link => !link) ? "agent" : "external") });
let inventory: ExtensionInventory;
const navigation = vi.fn();
function ExtensionTestApp({ initialTab = "skill", initialProjectId }: { initialTab?: "skill" | "mcp"; initialProjectId?: string }) {
  const [route, setRoute] = useState<Route>({ view: "extensions", tab: initialTab, projectId: initialProjectId });
  const navigate = (next: Route) => { navigation(next); setRoute(next); };
  if (route.view === "extension") return <ExtensionDetailView key={route.resourceId} {...route} navigate={navigate} />;
  return <ExtensionsView initialTab={initialTab} initialProjectId={initialProjectId} navigate={navigate} />;
}
beforeEach(() => {
  vi.clearAllMocks();
  viewState.clear(); localStorage.clear();
  inventory = { locations: [loc], resources: [resource()], warnings: [] };
  vi.mocked(api.listProjects).mockResolvedValue([]);
  vi.mocked(api.listProjectCards).mockResolvedValue([]);
  vi.mocked(api.listExtensionResources).mockImplementation(async () => inventory);
  vi.mocked(api.listExtensionCatalog).mockImplementation(async () => inventory);
  vi.mocked(api.getExtensionDetail).mockImplementation(async id => {
    const row = inventory.resources.find(r => r.id === id);
    return row ? bundle(row, '{"url":"https://example/mcp","headers":{"X-Team":"keep"},"exposure":"deferred"}', { url: "https://example/mcp", headers: { "X-Team": "keep" }, exposure: "deferred" }) : null;
  });
  vi.mocked(api.openPath).mockResolvedValue();
});
afterEach(cleanup);

it("reads Skill content and enablement without exposing a Skill editor", async () => {
  const sourcePath = "/home/test/skills/review/SKILL.md";
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [resource({ id: "review", kind: "skill", name: "review", path: sourcePath, locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] })], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(inventory.resources[0], "# Review workflow\n\nRead source changes."));
  render(<ExtensionTestApp />);
  fireEvent.click(await screen.findByRole("button", { name: /review/ }));
  expect(await screen.findByRole("heading", { name: "Review workflow" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "编辑配置" })).toBeNull();
  expect(screen.queryByRole("textbox", { name: /Skill/ })).toBeNull();
  expect(screen.queryByRole("button", { name: "打开所在目录" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: sourcePath }));
  expect(api.openPath).toHaveBeenCalledWith(sourcePath);
  expect(navigation).toHaveBeenCalledWith(expect.objectContaining({ view: "extension", resourceId: "review", kind: "skill", initialTitle: "review" }));
  expect(screen.queryByRole("button", { name: "卡片视图" })).toBeNull();
});

it("uninstalls a Skill and locks the keep-source option when the source is in an Agent directory", async () => {
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.extensionSkillUninstallPreview).mockResolvedValue({ delete_source_required: true, entry_count: 2, source_path: "/home/test/skills/review" });
  vi.mocked(api.uninstallExtensionSkill).mockResolvedValue({ message: "Removed" });
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" navigate={navigation} />);
  fireEvent.click(await screen.findByRole("button", { name: "卸载 Skill" }));
  const confirm = await screen.findByRole("button", { name: "删除并卸载" }) as HTMLButtonElement;
  await waitFor(() => expect(confirm.disabled).toBe(false));
  const keepSource = screen.getByRole("checkbox") as HTMLInputElement;
  expect(keepSource.checked).toBe(true);
  expect(keepSource.disabled).toBe(true);
  expect(screen.getByText("/home/test/skills/review")).toBeTruthy();
  fireEvent.click(confirm);
  await waitFor(() => expect(api.uninstallExtensionSkill).toHaveBeenCalledWith("review", "original-revision", true));
});

it("removes a linked Skill from NoEnding without deleting its source", async () => {
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.extensionSkillUninstallPreview).mockResolvedValue({ delete_source_required: false, entry_count: 1, source_path: "/home/test/skills/review" });
  vi.mocked(api.uninstallExtensionSkill).mockResolvedValue({ message: "Removed" });
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" navigate={navigation} />);
  fireEvent.click(await screen.findByRole("button", { name: "卸载 Skill" }));
  const keepSource = screen.getByRole("checkbox") as HTMLInputElement;
  const confirm = await screen.findByRole("button", { name: "仅移除" }) as HTMLButtonElement;
  await waitFor(() => expect(confirm.disabled).toBe(false));
  expect(keepSource.checked).toBe(false);
  expect(keepSource.disabled).toBe(false);
  fireEvent.click(confirm);
  await waitFor(() => expect(api.uninstallExtensionSkill).toHaveBeenCalledWith("review", "original-revision", false));
});

it("shows user-level coverage and disables it by clearing the select-all scope", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: [entry], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.disableExtensionSkillAll).mockResolvedValue({ message: "Disabled" });
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  expect(within(section).getByText("用户级")).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Codex" })).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Pi" })).toBeTruthy();
  expect(within(section).queryByText(entry)).toBeNull();
  fireEvent.click(within(section).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置扩展" });
  const master = await within(dialog).findByRole("checkbox", { name: /全选/ }) as HTMLInputElement;
  expect(master.checked).toBe(true);
  fireEvent.click(master);
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.disableExtensionSkillAll).toHaveBeenCalledWith("review"));
});

it("keeps unrelated detail actions available while a scope change runs", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: [entry], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  let finish!: () => void;
  vi.mocked(api.disableExtensionSkillAll).mockImplementationOnce(() => new Promise(resolve => { finish = () => resolve({ message: "Disabled" }); }));
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  fireEvent.click(within(section).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置扩展" });
  fireEvent.click(await within(dialog).findByRole("checkbox", { name: /全选/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.disableExtensionSkillAll).toHaveBeenCalled());
  expect((within(dialog).getByRole("button", { name: "保存中…" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "卸载 Skill" }) as HTMLButtonElement).disabled).toBe(false);
  expect((screen.getByRole("button", { name: "转移到 NoEnding" }) as HTMLButtonElement).disabled).toBe(false);
  await act(async () => { finish(); });
});

it("adds project-level configuration from the detail page", async () => {
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: [], entry_paths: [], entry_links: [] });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.listProjectCards).mockResolvedValue([
    projectCard(),
    projectCard({ id: "p2", name: "Beta", representative_paths: ["/work/two"], search_paths: ["/work/two"] }),
  ]);
  vi.mocked(api.enableExtensionSkillProjects).mockResolvedValue({ message: "Enabled" });
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  fireEvent.click(within(section).getByRole("button", { name: "配置" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /Alpha/ }));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableExtensionSkillProjects).toHaveBeenCalledWith(["p1"], "review"));
});

it("marks a physical source scope without offering disable", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: [entry], entry_links: [false] });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  expect(within(section).getAllByText("物理入口").length).toBeGreaterThan(0);
  expect(within(section).queryByRole("button", { name: "停用" })).toBeNull();
});

it("registers a physical Skill source and deletes it after confirming", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: [entry], entry_links: [false], registered: false });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.extensionSkillMigrationPreview).mockResolvedValue({ source_path: entry, target_path: "/home/test/.noending/skills/review", entry_count: 1, physical_entries: 1 });
  vi.mocked(api.migrateExtensionSkill).mockResolvedValue({ message: "Migrated", source_path: "/home/test/.noending/skills/review/SKILL.md", resource_id: "migrated-review", name: "review", description: "Review" });
  render(<ExtensionTestApp />);
  fireEvent.click(await screen.findByRole("button", { name: /review/ }));
  const section = await screen.findByLabelText("源目录");
  expect(screen.queryByRole("button", { name: "配置" })).toBeNull();
  fireEvent.click(within(section).getByRole("button", { name: "转移到 NoEnding" }));
  const dialog = await screen.findByRole("dialog", { name: "转移到 NoEnding" });
  expect(await within(dialog).findByText(/1 个（其中 1 个物理入口）/)).toBeTruthy();
  const removeSource = within(dialog).getByRole("checkbox", { name: "转移后删除源目录" }) as HTMLInputElement;
  expect(removeSource.checked).toBe(true);
  expect(removeSource.disabled).toBe(true);
  expect(within(dialog).getByText(/源目录位于 Agent 配置目录，转移后会删除源目录/)).toBeTruthy();
  fireEvent.click(within(dialog).getByRole("button", { name: "转移" }));
  await waitFor(() => expect(api.migrateExtensionSkill).toHaveBeenCalledWith("review", true));
  await waitFor(() => expect(navigation).toHaveBeenLastCalledWith(expect.objectContaining({ view: "extension", resourceId: "migrated-review", kind: "skill", initialTitle: "review" })));
});

it("offers registration for a linked source and can keep the original directory", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", kind: "skill", name: "review", path: "/home/test/external/review/SKILL.md", locations: ["shared"], entry_paths: [entry], entry_links: [true], registered: false });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.extensionSkillMigrationPreview).mockResolvedValue({ source_path: "/home/test/external/review/SKILL.md", target_path: "/home/test/.noending/skills/review", entry_count: 1, physical_entries: 0 });
  vi.mocked(api.migrateExtensionSkill).mockResolvedValue({ message: "Copied", source_path: "/home/test/.noending/skills/review/SKILL.md", resource_id: "migrated-review", name: "review", description: "Review" });
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" navigate={navigation} />);
  expect(await screen.findByText("外部目录")).toBeTruthy();
  const section = await screen.findByLabelText("源目录");
  fireEvent.click(within(section).getByRole("button", { name: "转移到 NoEnding" }));
  const dialog = await screen.findByRole("dialog", { name: "转移到 NoEnding" });
  expect(await within(dialog).findByText("1 个链接入口")).toBeTruthy();
  const removeSource = within(dialog).getByRole("checkbox", { name: "转移后删除源目录" }) as HTMLInputElement;
  expect(removeSource.checked).toBe(true);
  expect(removeSource.disabled).toBe(false);
  fireEvent.click(removeSource);
  fireEvent.click(within(dialog).getByRole("button", { name: "转移" }));
  await waitFor(() => expect(api.migrateExtensionSkill).toHaveBeenCalledWith("review", false));
});

it("registers a discovered Skill from its card", async () => {
  const entry = "/home/test/.agents/skills/review/SKILL.md";
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: [entry], entry_links: [true], registered: false });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.extensionSkillMigrationPreview).mockResolvedValue({ source_path: "/home/test/external/review/SKILL.md", target_path: "/home/test/.noending/skills/review", entry_count: 1, physical_entries: 0 });
  vi.mocked(api.migrateExtensionSkill).mockResolvedValue({ message: "Registered", source_path: "/home/test/.noending/skills/review/SKILL.md", resource_id: "migrated-review", name: "review", description: "Review" });
  render(<ExtensionTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  expect(within(card).queryByRole("button", { name: "配置" })).toBeNull();
  fireEvent.click(within(card).getByRole("button", { name: "转移" }));
  const dialog = await screen.findByRole("dialog", { name: "转移到 NoEnding" });
  await within(dialog).findByText("1 个链接入口");
  fireEvent.click(within(dialog).getByRole("button", { name: "转移" }));
  await waitFor(() => expect(api.migrateExtensionSkill).toHaveBeenCalledWith("review", true));
});

it("keeps a disabled registered Skill listed and enables it from the detail", async () => {
  const skillLocation: ExtensionLocation = { ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", kind: "skill", name: "review", path: "/home/test/skills/review/SKILL.md", locations: [], entry_paths: [], entry_links: [], registered: true });
  inventory = { locations: [skillLocation], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  vi.mocked(api.enableExtensionSkillAll).mockResolvedValue({ message: "Enabled" });
  render(<ExtensionTestApp />);
  const row = await screen.findByRole("button", { name: /review/ });
  expect(within(row).getByText("未启用")).toBeTruthy();
  expect(within(row).queryByRole("button", { name: "转移" })).toBeNull();
  fireEvent.click(row);
  const section = await screen.findByLabelText("启用范围");
  expect(within(section).getByText("当前未在任何作用域启用。")).toBeTruthy();
  fireEvent.click(within(section).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置扩展" });
  fireEvent.click(await within(dialog).findByRole("checkbox", { name: /全选/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableExtensionSkillAll).toHaveBeenCalledWith("review"));
});

it("enables a Skill at user level from its card", async () => {
  const userLocation: ExtensionLocation = { ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: [], entry_paths: [], entry_links: [] });
  inventory = { locations: [userLocation], resources: [skill], warnings: [] };
  vi.mocked(api.enableExtensionSkillAll).mockResolvedValue({ message: "Enabled" });
  render(<ExtensionTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置扩展" });
  fireEvent.click(await within(dialog).findByRole("checkbox", { name: /全选/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableExtensionSkillAll).toHaveBeenCalledWith("review"));
  expect(navigation).not.toHaveBeenCalled();
});

it("offers only the fill action for a physical user entry", async () => {
  const userLocation: ExtensionLocation = { ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [false] });
  inventory = { locations: [userLocation], resources: [skill], warnings: [] };
  vi.mocked(api.enableExtensionSkillAll).mockResolvedValue({ message: "Enabled" });
  render(<ExtensionTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置扩展" });
  const master = await within(dialog).findByRole("checkbox", { name: /全选/ }) as HTMLInputElement;
  expect(master.checked).toBe(true);
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableExtensionSkillAll).toHaveBeenCalledWith("review"));
});

it("applies project-level changes from a card, including removals", async () => {
  const userLocation: ExtensionLocation = { ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const projectLocation: ExtensionLocation = { ...loc, id: "project-skill", kind: "skill", agent: null, scope: "project", path: "/work/one/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["project-skill"], entry_paths: ["/work/one/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [userLocation, projectLocation], resources: [skill], warnings: [] };
  vi.mocked(api.listProjectCards).mockResolvedValue([
    projectCard({ id: "p1", name: "Alpha", search_paths: ["/work/one"] }),
    projectCard({ id: "p2", name: "Beta", representative_paths: ["/work/two"], search_paths: ["/work/two"] }),
    projectCard({ id: "p3", name: "Empty", path_count: 0, missing_path_count: 0, representative_paths: [], search_paths: [] }),
  ]);
  vi.mocked(api.enableExtensionSkillProjects).mockResolvedValue({ message: "Enabled" });
  vi.mocked(api.disableExtensionSkillProjects).mockResolvedValue({ message: "Disabled" });
  render(<ExtensionTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const alpha = await screen.findByRole("checkbox", { name: /Alpha/ }) as HTMLInputElement;
  expect(alpha.checked).toBe(true);
  expect(alpha.disabled).toBe(false);
  expect((screen.getByRole("checkbox", { name: /Empty/ }) as HTMLInputElement).disabled).toBe(true);
  fireEvent.click(alpha);
  fireEvent.click(screen.getByRole("checkbox", { name: /Beta/ }));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableExtensionSkillProjects).toHaveBeenCalledWith(["p2"], "review"));
  expect(api.disableExtensionSkillProjects).toHaveBeenCalledWith(["p1"], "review");
});

it("adds a registered MCP definition to selected projects from its card", async () => {
  const definition = resource({ id: "mcp-docs", kind: "mcp", name: "docs", locations: [], entry_paths: [], entry_links: [], registered: true });
  inventory = { locations: [], resources: [definition], warnings: [] };
  vi.mocked(api.listProjectCards).mockResolvedValue([
    projectCard(),
    projectCard({ id: "p2", name: "Beta", representative_paths: ["/work/two"], search_paths: ["/work/two"] }),
  ]);
  vi.mocked(api.enableMcpDefinitionProjects).mockResolvedValue({ message: "Enabled" });
  render(<ExtensionTestApp initialTab="mcp" />);
  const card = await screen.findByRole("button", { name: /docs/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /Alpha/ }));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableMcpDefinitionProjects).toHaveBeenCalledWith(["p1"], "mcp-docs"));
});

it("converts user level to the remaining projects when one is unchecked", async () => {
  const userLocation: ExtensionLocation = { ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [userLocation], resources: [skill], warnings: [] };
  vi.mocked(api.listProjectCards).mockResolvedValue([
    projectCard(),
    projectCard({ id: "p2", name: "Beta", representative_paths: ["/work/two"], search_paths: ["/work/two"] }),
  ]);
  vi.mocked(api.disableExtensionSkillAll).mockResolvedValue({ message: "Disabled" });
  vi.mocked(api.enableExtensionSkillProjects).mockResolvedValue({ message: "Enabled" });
  render(<ExtensionTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置扩展" });
  const master = await within(dialog).findByRole("checkbox", { name: /全选/ }) as HTMLInputElement;
  const beta = within(dialog).getByRole("checkbox", { name: /Beta/ }) as HTMLInputElement;
  expect(master.checked).toBe(true);
  expect(beta.checked).toBe(true);
  fireEvent.click(beta);
  expect(master.checked).toBe(false);
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableExtensionSkillProjects).toHaveBeenCalledWith(["p1"], "review"));
  expect(api.disableExtensionSkillAll).toHaveBeenCalledWith("review");
  expect(api.enableExtensionSkillAll).not.toHaveBeenCalled();
  expect(vi.mocked(api.disableExtensionSkillAll).mock.invocationCallOrder[0])
    .toBeLessThan(vi.mocked(api.enableExtensionSkillProjects).mock.invocationCallOrder[0]);
});

it("refreshes the board silently after saving a scope change", async () => {
  const userLocation: ExtensionLocation = { ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: [], entry_paths: [], entry_links: [] });
  const other = resource({ id: "lint", kind: "skill", name: "lint", locations: [], entry_paths: [], entry_links: [] });
  inventory = { locations: [userLocation], resources: [skill, other], warnings: [] };
  let finish: (value: ExtensionInventory) => void = () => {};
  vi.mocked(api.listExtensionCatalog)
    .mockImplementationOnce(async () => inventory)
    .mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  vi.mocked(api.enableExtensionSkillAll).mockResolvedValue({ message: "Enabled" });
  render(<ExtensionTestApp />);
  const card = await screen.findByRole("button", { name: /review/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置扩展" });
  fireEvent.click(await within(dialog).findByRole("checkbox", { name: /全选/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.listExtensionCatalog).toHaveBeenCalledTimes(2));
  expect(screen.queryByText("正在读取本地资源…")).toBeNull();
  expect(screen.getByRole("button", { name: /lint/ })).toBeTruthy();
  expect(within(dialog).getByRole("button", { name: "保存中…" })).toBeTruthy();
  await act(async () => { finish(inventory); });
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
});

it("registers a discovered MCP definition from its card", async () => {
  vi.mocked(api.saveMcpDefinition).mockResolvedValue({ message: "Created" });
  render(<ExtensionTestApp initialTab="mcp" />);
  const card = await screen.findByRole("button", { name: /docs/ });
  fireEvent.click(within(card).getByRole("button", { name: "登记" }));
  expect(await screen.findByText(/https:\/\/example\/mcp/)).toBeTruthy();
  fireEvent.click(within(screen.getByRole("dialog", { name: "登记到 NoEnding" })).getByRole("button", { name: "登记" }));
  await waitFor(() => expect(api.saveMcpDefinition).toHaveBeenCalledWith("docs", "Project documentation", { url: "https://example/mcp", headers: { "X-Team": "keep" }, exposure: "deferred" }));
});

it("registers a discovered MCP definition from the detail page", async () => {
  vi.mocked(api.saveMcpDefinition).mockResolvedValue({ message: "Created" });
  render(<ExtensionDetailView resourceId="server" kind="mcp" initialTitle="docs" navigate={navigation} />);
  const section = await screen.findByLabelText("配置文件");
  fireEvent.click(within(section).getByRole("button", { name: "登记" }));
  fireEvent.click(within(await screen.findByRole("dialog", { name: "登记到 NoEnding" })).getByRole("button", { name: "登记" }));
  await waitFor(() => expect(api.saveMcpDefinition).toHaveBeenCalledWith("docs", "Project documentation", { url: "https://example/mcp", headers: { "X-Team": "keep" }, exposure: "deferred" }));
});

it("enables a registered MCP definition at user level from its card", async () => {
  const definition = resource({ id: "mcp-docs", kind: "mcp", name: "docs", locations: [], entry_paths: [], entry_links: [], registered: true });
  inventory = { locations: [], resources: [definition], warnings: [] };
  vi.mocked(api.enableMcpDefinitionUser).mockResolvedValue({ message: "Enabled" });
  render(<ExtensionTestApp initialTab="mcp" />);
  const card = await screen.findByRole("button", { name: /docs/ });
  fireEvent.click(within(card).getByRole("button", { name: "配置" }));
  const dialog = await screen.findByRole("dialog", { name: "配置扩展" });
  fireEvent.click(await within(dialog).findByRole("checkbox", { name: /全选/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.enableMcpDefinitionUser).toHaveBeenCalledWith("mcp-docs"));
});

it("does not mark a project-enabled Skill as disabled", async () => {
  const projectLocation: ExtensionLocation = { ...loc, id: "project-skill", kind: "skill", agent: null, scope: "project", path: "/work/one/.agents/skills", label: "共享 Skills" };
  inventory = {
    locations: [projectLocation],
    resources: [resource({ id: "zeta", kind: "skill", name: "zeta", locations: ["project-skill"], entry_paths: ["/work/one/.agents/skills/zeta/SKILL.md"], entry_links: [true] })],
    warnings: [],
  };
  render(<ExtensionTestApp initialTab="skill" />);
  const card = await screen.findByRole("button", { name: /zeta/ });
  expect(within(card).queryByText("未启用")).toBeNull();
});

it("sorts by name by default and by enablement scope on request", async () => {
  const userLocation: ExtensionLocation = { ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" };
  const projectLocation: ExtensionLocation = { ...loc, id: "project-skill", kind: "skill", agent: null, scope: "project", path: "/work/one/.agents/skills" };
  inventory = {
    locations: [userLocation, projectLocation],
    resources: [
      resource({ id: "zeta", kind: "skill", name: "zeta", locations: ["project-skill"], entry_paths: ["/work/one/.agents/skills/zeta/SKILL.md"], entry_links: [true] }),
      resource({ id: "alpha", kind: "skill", name: "alpha", locations: [], entry_paths: [], entry_links: [] }),
      resource({ id: "beta", kind: "skill", name: "beta", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/beta/SKILL.md"], entry_links: [true] }),
    ],
    warnings: [],
  };
  render(<ExtensionTestApp initialTab="skill" />);
  const section = await screen.findByRole("region", { name: "Skills" });
  const names = () => within(section).getAllByRole("button").filter(button => button.classList.contains("extension-row")).map(button => button.querySelector("strong")?.textContent);
  expect(names()).toEqual(["alpha", "beta", "zeta"]);
  fireEvent.change(screen.getByRole("combobox", { name: "排序" }), { target: { value: "scope" } });
  expect(names()).toEqual(["beta", "zeta", "alpha"]);
});

it("summarizes user-level Agent coverage read-only when every Agent is covered", async () => {
  const locations: ExtensionLocation[] = [
    { ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" },
    { ...loc, id: "claude", kind: "skill", agent: "claude_code", path: "/home/test/.claude/skills", label: "Claude Code Skills" },
    { ...loc, id: "antigravity", kind: "skill", agent: "antigravity", path: "/home/test/.gemini/antigravity-cli/skills", label: "Antigravity Skills" },
  ];
  const entries = [
    "/home/test/.agents/skills/review/SKILL.md",
    "/home/test/.claude/skills/review/SKILL.md",
    "/home/test/.gemini/antigravity-cli/skills/review/SKILL.md",
  ];
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: locations.map(l => l.id), entry_paths: entries, entry_links: [true, true, true] });
  inventory = { locations, resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes."));
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  expect(within(section).getByText("用户级")).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Codex" })).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Pi" })).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Antigravity" })).toBeTruthy();
  expect(within(section).queryByText(/Codex、Pi/)).toBeNull();
  expect(within(section).queryByRole("button", { name: /^(启用|停用|补齐)$/ })).toBeNull();
});

it("shows project enablement read-only without a project association", async () => {
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes.", null, [projectScope({
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
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  expect(api.getExtensionDetail).toHaveBeenCalledWith("review");
  expect(api.listExtensionCatalog).not.toHaveBeenCalled();
  expect(within(section).getByText("Alpha")).toBeTruthy();
  expect(within(section).queryByText("/work/alpha/.agents/skills/review/SKILL.md")).toBeNull();
  expect(within(section).getByRole("img", { name: "Antigravity" })).toBeTruthy();
  expect(within(section).queryByText("Beta")).toBeNull();
  expect(within(section).queryByRole("button", { name: /^(启用|停用|补齐)$/ })).toBeNull();
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("marks a physical project scope as a source directory and keeps the detail read-only", async () => {
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], entry_paths: ["/home/test/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills", label: "共享 Skills" }], resources: [skill], warnings: [] };
  vi.mocked(api.getExtensionDetail).mockResolvedValue(bundle(skill, "# Review\n\nRead changes.", null, [projectScope({
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
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" navigate={navigation} />);
  const section = await screen.findByLabelText("启用范围");
  expect(within(section).getByText("Beta")).toBeTruthy();
  expect(within(section).getAllByText("物理入口").length).toBeGreaterThan(0);
  expect(within(section).queryByRole("button", { name: /^(启用|停用|补齐)$/ })).toBeNull();
});

it("hides the native config file card for a registered MCP", async () => {
  const definition = resource({ id: "server", kind: "mcp", name: "docs", registered: true });
  inventory = { locations: [loc], resources: [definition], warnings: [] };
  render(<ExtensionDetailView resourceId="server" kind="mcp" initialTitle="docs" navigate={navigation} />);
  expect(await screen.findByText("MCP 配置")).toBeTruthy();
  expect(screen.queryByLabelText("配置文件")).toBeNull();
  expect(screen.getByLabelText("启用范围")).toBeTruthy();
});

it("filters cards by registration state", async () => {
  const registered = resource({ id: "registered", kind: "mcp", name: "docs", registered: true, locations: [], entry_paths: [], entry_links: [] });
  const discovered = resource({ id: "discovered", kind: "mcp", name: "native" });
  inventory = { locations: [loc], resources: [registered, discovered], warnings: [] };
  render(<ExtensionTestApp initialTab="mcp" />);
  await screen.findByRole("button", { name: /docs/ });
  fireEvent.change(screen.getByRole("combobox", { name: "筛选登记状态" }), { target: { value: "registered" } });
  expect(screen.getByRole("button", { name: /docs/ })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /native/ })).toBeNull();
  fireEvent.change(screen.getByRole("combobox", { name: "筛选登记状态" }), { target: { value: "unregistered" } });
  expect(screen.queryByRole("button", { name: /docs/ })).toBeNull();
  expect(screen.getByRole("button", { name: /native/ })).toBeTruthy();
});

it("shows MCP enablement scope read-only", async () => {
  render(<ExtensionTestApp initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  await screen.findByLabelText("资源详情");
  const section = screen.getByLabelText("启用范围");
  expect(within(section).getByText("用户级")).toBeTruthy();
  expect(within(section).getByRole("img", { name: "Pi" })).toBeTruthy();
  expect(within(section).queryByText(loc.path)).toBeNull();
  expect(within(section).queryByRole("button", { name: /^(启用|停用|补齐|编辑配置|删除配置|登记)$/ })).toBeNull();
  expect(within(screen.getByLabelText("配置文件")).getByRole("button", { name: "登记" })).toBeTruthy();
});

it("creates an MCP definition from the board without choosing a scope", async () => {
  vi.mocked(api.saveMcpDefinition).mockResolvedValue({ message: "Created" });
  render(<ExtensionTestApp initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: "新增 MCP 定义" }));
  fireEvent.change(screen.getByLabelText("MCP 名称"), { target: { value: "tools" } });
  fireEvent.change(screen.getByLabelText("MCP 启动程序"), { target: { value: "tools-server" } });
  fireEvent.click(screen.getByRole("button", { name: "创建定义" }));
  await waitFor(() => expect(api.saveMcpDefinition).toHaveBeenCalledWith("tools", "", { command: "tools-server", args: [] }));
});

it("browses all Skills and registers a source without writing an Agent entry", async () => {
  const paths = ["one", "two"].map(id => ({ id, canonical_path: `/work/${id}`, project_id: "p", exists: true } as WorkspacePath));
  vi.mocked(api.getProjectDetail).mockResolvedValue({ workspace_paths: paths } as ProjectDetailData);
  vi.mocked(api.listProjects).mockResolvedValue([{ id: "p", name: "Demo", visibility: "normal" } as never]);
  const projectLocation: ExtensionLocation = { ...loc, id: "shared-one", kind: "skill", agent: null, scope: "project", path: "/work/one/.agents/skills", label: "共享 Skills" };
  const skill = resource({ id: "review", kind: "skill", name: "review", locations: [projectLocation.id], entry_paths: ["/work/one/.agents/skills/review/SKILL.md"], entry_links: [true] });
  inventory = { locations: [projectLocation], resources: [skill], warnings: [] };
  vi.mocked(api.registerExtensionSkillSource).mockResolvedValue({ message: "Registered" });
  render(<ExtensionTestApp initialTab="skill" initialProjectId="p" />);
  expect(await screen.findByRole("button", { name: /review/ })).toBeTruthy();
  expect(screen.queryByLabelText("配置工作目录")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "转移 Skill 源" }));
  fireEvent.change(screen.getByLabelText("源 Skill 目录"), { target: { value: "/tmp/review" } });
  fireEvent.click(screen.getByRole("button", { name: "转移源" }));
  await waitFor(() => expect(api.registerExtensionSkillSource).toHaveBeenCalledWith("/tmp/review"));
});

it.each(["卡片视图", "列表视图"])("opens the same detail from %s without a back link", async mode => {
  render(<ExtensionTestApp initialTab="mcp" />);
  fireEvent.click(screen.getByRole("button", { name: mode }));
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  await screen.findByLabelText("资源详情");
  expect(navigation).toHaveBeenLastCalledWith(expect.objectContaining({ view: "extension", resourceId: "server", kind: "mcp" }));
  expect(screen.queryByRole("button", { name: /← 扩展/ })).toBeNull();
});

it("project-only resources remain inspectable without user mutation actions", async () => {
  inventory.locations[0] = { ...loc, scope: "project" };
  render(<ExtensionDetailView resourceId="server" kind="mcp" initialTitle="docs" navigate={navigation} />);
  await screen.findByLabelText("资源详情");
  for (const name of ["编辑配置", "删除配置", "启用", "禁用"]) expect(screen.queryByRole("button", { name })).toBeNull();
});

it("keeps the resource title and hides actions when an old detail route is no longer available", async () => {
  inventory.resources = [];
  render(<ExtensionDetailView resourceId="removed" kind="mcp" initialTitle="docs" navigate={navigation} />);
  expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("docs");
  await screen.findByRole("alert");
  expect(api.getExtensionDetail).toHaveBeenCalledWith("removed");
  for (const name of ["编辑配置", "删除配置", "启用", "禁用"]) expect(screen.queryByRole("button", { name })).toBeNull();
});

it("removing an MCP definition returns to the board while a rejected removal retains the detail", async () => {
  vi.mocked(api.uninstallMcpDefinition).mockRejectedValueOnce(new Error("配置文件被占用"))
    .mockImplementationOnce(async () => { inventory.resources = []; return { message: "Removed" }; });
  render(<ExtensionTestApp initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  fireEvent.click(await screen.findByRole("button", { name: "卸载 MCP" }));
  fireEvent.click(screen.getByRole("button", { name: "删除定义并移除" }));
  await screen.findAllByText("Error: 配置文件被占用");
  expect(navigation).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole("button", { name: "删除定义并移除" }));
  await waitFor(() => expect(navigation).toHaveBeenLastCalledWith(expect.objectContaining({ view: "extensions", tab: "mcp" })));
  expect(api.uninstallMcpDefinition).toHaveBeenLastCalledWith("server");
  expect(screen.queryByLabelText("资源详情")).toBeNull();
});

it("only exposes Skill and MCP categories", async () => {
  render(<ExtensionTestApp />);
  const group = await screen.findByRole("group", { name: "扩展类别" });
  expect(within(group).getAllByRole("button").map(button => button.getAttribute("aria-label"))).toEqual(["Skills", "MCP"]);
});
