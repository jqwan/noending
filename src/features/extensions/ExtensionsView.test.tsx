import { useState } from "react";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import ExtensionsView from "./ExtensionsView";
import ExtensionDetailView from "./ExtensionDetailView";
import type { Route } from "../../app/routes";
import { viewState } from "../../hooks/useViewState";
import type { ExtensionInventory, ExtensionLocation, ExtensionResource, ProjectDetailData, WorkspacePath } from "../../types";

vi.mock("../../api", () => ({ api: {
  listProjects: vi.fn(), getProjectDetail: vi.fn(), listExtensionResources: vi.fn(),
  getExtensionResource: vi.fn(), extensionLocationRevision: vi.fn(), saveExtensionMcp: vi.fn(),
  installExtensionSkill: vi.fn(), removeExtensionSkill: vi.fn(), removeExtensionMcp: vi.fn(),
  managePluginPackage: vi.fn(), setExtensionSkillState: vi.fn(), openPath: vi.fn(),
} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../../components/Toast", () => ({ showToast: vi.fn() }));
const loc: ExtensionLocation = { id: "pi-user", kind: "mcp", agent: "pi", scope: "user", path: "/home/test/.pi/agent/mcp.json", label: "Pi", writable: true, package: null, package_enabled: null };
const resource = (overrides: Partial<ExtensionResource> = {}): ExtensionResource => ({ id: "server", kind: "mcp", name: "docs", description: "Project documentation", path: loc.path, locations: [loc.id], entry_paths: [loc.path], enabled: true, revision: "original-revision", managed: false, error: null, skill_controls: [], ...overrides });
let inventory: ExtensionInventory;
const navigation = vi.fn();
function ExtensionTestApp({ initialTab = "skill", initialProjectId }: { initialTab?: "skill" | "mcp" | "package"; initialProjectId?: string }) {
  const [route, setRoute] = useState<Route>({ view: "extensions", tab: initialTab, projectId: initialProjectId });
  const navigate = (next: Route) => { navigation(next); setRoute(next); };
  if (route.view === "extension") return <ExtensionDetailView key={route.resourceId} {...route} navigate={navigate} goBack={fallback => navigate(fallback ?? { view: "extensions" })} />;
  return <ExtensionsView initialTab={initialTab} initialProjectId={initialProjectId} navigate={navigate} />;
}
beforeEach(() => {
  vi.clearAllMocks();
  viewState.clear(); localStorage.clear();
  inventory = { locations: [loc], resources: [resource()], warnings: [] };
  vi.mocked(api.listProjects).mockResolvedValue([]);
  vi.mocked(api.listExtensionResources).mockImplementation(async () => inventory);
  vi.mocked(api.getExtensionResource).mockImplementation(async (_scope, id) => ({ resource: inventory.resources.find(r => r.id === id)!, content: '{"url":"https://example/mcp","headers":{"X-Team":"keep"},"exposure":"deferred"}', config: { url: "https://example/mcp", headers: { "X-Team": "keep" }, exposure: "deferred" } }));
  vi.mocked(api.openPath).mockResolvedValue();
  vi.mocked(api.extensionLocationRevision).mockResolvedValue("new-file-revision");
  vi.mocked(api.saveExtensionMcp).mockResolvedValue({ message: "Saved" });
});
afterEach(cleanup);

it("reads Skill content and loading entries without exposing a Skill editor", async () => {
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], enabled: null })], warnings: [] };
  vi.mocked(api.getExtensionResource).mockResolvedValue({ resource: inventory.resources[0], content: "# Review workflow\n\nRead source changes.", config: null });
  render(<ExtensionTestApp />);
  fireEvent.click(await screen.findByRole("button", { name: /review/ }));
  expect(await screen.findByRole("heading", { name: "Review workflow" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "编辑配置" })).toBeNull();
  expect(screen.queryByRole("textbox", { name: /Skill/ })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "打开所在目录" }));
  expect(api.openPath).toHaveBeenCalledWith(inventory.resources[0].path);
  expect(navigation).toHaveBeenCalledWith(expect.objectContaining({ view: "extension", resourceId: "review", kind: "skill", initialTitle: "review" }));
  expect(screen.queryByRole("button", { name: "卡片视图" })).toBeNull();
});

it("MCP editing sends one server with its original revision and preserves advanced fields", async () => {
  render(<ExtensionTestApp initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  fireEvent.click(await screen.findByRole("button", { name: "编辑配置" }));
  fireEvent.change(screen.getByLabelText("MCP 服务地址"), { target: { value: "https://new.example/mcp" } });
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.saveExtensionMcp).toHaveBeenCalledWith({}, loc.id, "original-revision", "docs", { url: "https://new.example/mcp", headers: { "X-Team": "keep" }, exposure: "deferred" }, false));
});

it("package-owned MCP is inspectable but cannot be edited or deleted independently", async () => {
  inventory.locations[0] = { ...loc, package: "team-tools", writable: false };
  inventory.resources[0] = resource({ managed: true });
  render(<ExtensionTestApp initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  await screen.findByLabelText("资源详情");
  expect(screen.queryByRole("button", { name: "编辑配置" })).toBeNull();
  expect(screen.queryByRole("button", { name: "删除配置" })).toBeNull();
});

it("explicit project targets report partial success and retry only failed directories", async () => {
  const paths = ["one", "two"].map(id => ({ id, canonical_path: `/work/${id}`, project_id: "p", exists: true } as WorkspacePath));
  vi.mocked(api.getProjectDetail).mockResolvedValue({ workspace_paths: paths } as ProjectDetailData);
  vi.mocked(api.listProjects).mockResolvedValue([{ id: "p", name: "Demo", visibility: "normal" } as never]);
  vi.mocked(api.listExtensionResources).mockImplementation(async scope => ({ resources: [], warnings: [], locations: [{ ...loc, id: `pi-${scope.workspace_path_id}`, scope: "project", path: `/work/${scope.workspace_path_id}/.pi/mcp.json` }] }));
  vi.mocked(api.saveExtensionMcp).mockImplementation(async scope => {
    if (scope.workspace_path_id === "two" && vi.mocked(api.saveExtensionMcp).mock.calls.length === 2) throw new Error("directory became read-only");
    return { message: "Saved" };
  });
  render(<ExtensionTestApp initialTab="mcp" initialProjectId="p" />);
  await waitFor(() => expect((screen.getByLabelText("配置工作目录") as HTMLSelectElement).value).toBe("one"));
  await waitFor(() => expect(screen.getByRole("button", { name: "新增 MCP" }).hasAttribute("disabled")).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "新增 MCP" }));
  fireEvent.change(screen.getByLabelText("MCP 名称"), { target: { value: "tools" } });
  fireEvent.change(screen.getByLabelText("MCP 启动程序"), { target: { value: "tools-server" } });
  fireEvent.click(screen.getByLabelText("/work/two"));
  await waitFor(() => expect(screen.getByRole("button", { name: "保存" }).hasAttribute("disabled")).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await screen.findByText(/部分目录操作失败/);
  expect(api.saveExtensionMcp).toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.saveExtensionMcp).toHaveBeenCalledTimes(3));
  expect(vi.mocked(api.saveExtensionMcp).mock.calls.map(call => call[0].workspace_path_id)).toEqual(["one", "two", "two"]);
});

it.each(["pi", "antigravity"] as const)("uses %s's native MCP switch without dropping server options", async agent => {
  inventory.locations[0] = { ...loc, agent };
  const config = { command: "tools-server", env: { TOKEN: "${TOOLS_TOKEN}" } };
  vi.mocked(api.getExtensionResource).mockResolvedValue({ resource: inventory.resources[0], content: JSON.stringify(config), config });
  render(<ExtensionTestApp initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  fireEvent.click(await screen.findByRole("button", { name: "禁用" }));
  await waitFor(() => expect(api.saveExtensionMcp).toHaveBeenCalledWith({}, loc.id, "original-revision", "docs", { ...config, [agent === "antigravity" ? "disabled" : "enabled"]: agent === "antigravity" }, false));
});

it.each(["卡片视图", "列表视图"])("opens the same detail from %s and retains the display preference after returning", async mode => {
  render(<ExtensionTestApp initialTab="mcp" />);
  fireEvent.click(screen.getByRole("button", { name: mode }));
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  await screen.findByLabelText("资源详情");
  expect(navigation).toHaveBeenLastCalledWith(expect.objectContaining({ view: "extension", resourceId: "server", kind: "mcp" }));
  fireEvent.click(screen.getByRole("button", { name: /← 扩展/ }));
  await screen.findByRole("button", { name: /docs/ });
  expect(screen.getByRole("button", { name: mode }).getAttribute("aria-pressed")).toBe("true");
});

it("only changes user Skill switches even when opened from a project context", async () => {
  const skill = resource({ id: "review", kind: "skill", name: "review", enabled: null, locations: ["shared"], skill_controls: [
    { agent: "codex", scope: "user", enabled: false, revision: "user-config" },
    { agent: "codex", scope: "project", enabled: true, revision: "project-config" },
  ] });
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null }], resources: [skill], warnings: [] };
  vi.mocked(api.setExtensionSkillState).mockResolvedValue({ message: "Saved" });
  render(<ExtensionDetailView resourceId="review" kind="skill" initialTitle="review" projectId="p" workspacePathId="one" navigate={navigation} goBack={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: "启用" }));
  await waitFor(() => expect(api.setExtensionSkillState).toHaveBeenCalledWith({}, "review", "user", "user-config", true));
  expect(within(screen.getByLabelText("Agent 配置")).getByText("Pi")).toBeTruthy();
  expect(screen.queryByText(/项目级已启用/)).toBeNull();
});

it("project-only resources remain inspectable without user mutation actions", async () => {
  inventory.locations[0] = { ...loc, scope: "project" };
  render(<ExtensionDetailView resourceId="server" kind="mcp" initialTitle="docs" projectId="p" workspacePathId="one" navigate={navigation} goBack={vi.fn()} />);
  await screen.findByLabelText("资源详情");
  for (const name of ["编辑配置", "删除配置", "启用", "禁用"]) expect(screen.queryByRole("button", { name })).toBeNull();
});

it("keeps the resource title and hides actions when an old detail route is no longer available", async () => {
  inventory.resources = [];
  render(<ExtensionDetailView resourceId="removed" kind="mcp" initialTitle="docs" navigate={navigation} goBack={vi.fn()} />);
  expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("docs");
  await screen.findByRole("alert");
  expect(api.getExtensionResource).not.toHaveBeenCalled();
  for (const name of ["编辑配置", "删除配置", "启用", "禁用"]) expect(screen.queryByRole("button", { name })).toBeNull();
});

it("removing a user MCP returns to the board while a rejected removal retains the detail", async () => {
  vi.mocked(api.removeExtensionMcp).mockRejectedValueOnce(new Error("配置已被修改"))
    .mockImplementationOnce(async () => { inventory.resources = []; return { message: "Removed" }; });
  render(<ExtensionTestApp initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  fireEvent.click(await screen.findByRole("button", { name: "删除配置" }));
  fireEvent.click(screen.getByRole("button", { name: "确认移除" }));
  await screen.findAllByText("Error: 配置已被修改");
  expect(navigation).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole("button", { name: "确认移除" }));
  await waitFor(() => expect(navigation).toHaveBeenLastCalledWith(expect.objectContaining({ view: "extensions", tab: "mcp" })));
  expect(api.removeExtensionMcp).toHaveBeenLastCalledWith({}, "server", "original-revision");
  expect(screen.queryByLabelText("资源详情")).toBeNull();
});

it("package resources link to their parent and only the package exposes uninstall", async () => {
  inventory.locations[0] = { ...loc, package: "team-tools", writable: false };
  inventory.locations.push({ ...loc, id: "package-location", kind: "package" });
  inventory.resources = [resource({ managed: true }), resource({ id: "package", name: "team-tools", kind: "package", locations: ["package-location"], enabled: null })];
  vi.mocked(api.managePluginPackage).mockImplementation(async () => { inventory.resources = []; return { message: "Removed" }; });
  render(<ExtensionTestApp initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  fireEvent.click(await screen.findByRole("button", { name: "team-tools" }));
  expect(navigation).toHaveBeenLastCalledWith(expect.objectContaining({ view: "extension", resourceId: "package" }));
  fireEvent.click(await screen.findByRole("button", { name: "卸载" }));
  expect(screen.queryByRole("button", { name: "禁用" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "确认移除" }));
  await waitFor(() => expect(api.managePluginPackage).toHaveBeenCalledWith({}, "pi", "remove", "team-tools", "package", "original-revision"));
});
