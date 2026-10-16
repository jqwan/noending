import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import PluginsView from "./PluginsView";
import type { PluginInventory, PluginLocation, PluginResource, ProjectDetailData, WorkspacePath } from "../../types";

vi.mock("../../api", () => ({ api: {
  listProjects: vi.fn(), getProjectDetail: vi.fn(), listPluginResources: vi.fn(),
  getPluginResource: vi.fn(), pluginLocationRevision: vi.fn(), savePluginMcp: vi.fn(),
  installPluginSkill: vi.fn(), removePluginSkill: vi.fn(), removePluginMcp: vi.fn(),
  managePluginPackage: vi.fn(), setPluginSkillState: vi.fn(), openPath: vi.fn(),
} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../../components/Toast", () => ({ showToast: vi.fn() }));
const loc: PluginLocation = { id: "pi-user", kind: "mcp", agent: "pi", scope: "user", path: "/home/test/.pi/agent/mcp.json", label: "Pi", writable: true, package: null, package_enabled: null };
const resource = (overrides: Partial<PluginResource> = {}): PluginResource => ({ id: "server", kind: "mcp", name: "docs", description: "Project documentation", path: loc.path, locations: [loc.id], entry_paths: [loc.path], enabled: true, revision: "original-revision", managed: false, error: null, skill_controls: [], ...overrides });
let inventory: PluginInventory;
beforeEach(() => {
  vi.clearAllMocks();
  inventory = { locations: [loc], resources: [resource()], warnings: [] };
  vi.mocked(api.listProjects).mockResolvedValue([]);
  vi.mocked(api.listPluginResources).mockImplementation(async () => inventory);
  vi.mocked(api.getPluginResource).mockImplementation(async (_scope, id) => ({ resource: inventory.resources.find(r => r.id === id)!, content: '{"url":"https://example/mcp","headers":{"X-Team":"keep"},"exposure":"deferred"}', config: { url: "https://example/mcp", headers: { "X-Team": "keep" }, exposure: "deferred" } }));
  vi.mocked(api.openPath).mockResolvedValue();
  vi.mocked(api.pluginLocationRevision).mockResolvedValue("new-file-revision");
  vi.mocked(api.savePluginMcp).mockResolvedValue({ message: "Saved" });
});
afterEach(cleanup);

it("reads Skill content and loading entries without exposing a Skill editor", async () => {
  inventory = { locations: [{ ...loc, id: "shared", kind: "skill", agent: null, path: "/home/test/.agents/skills" }], resources: [resource({ id: "review", kind: "skill", name: "review", locations: ["shared"], enabled: null })], warnings: [] };
  vi.mocked(api.getPluginResource).mockResolvedValue({ resource: inventory.resources[0], content: "# Review workflow\n\nRead source changes.", config: null });
  render(<PluginsView />);
  fireEvent.click(await screen.findByRole("button", { name: /review/ }));
  expect(await screen.findByRole("heading", { name: "Review workflow" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "编辑配置" })).toBeNull();
  expect(screen.queryByRole("textbox", { name: /Skill/ })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "打开所在目录" }));
  expect(api.openPath).toHaveBeenCalledWith(inventory.resources[0].path);
});

it("MCP editing sends one server with its original revision and preserves advanced fields", async () => {
  render(<PluginsView initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  fireEvent.click(await screen.findByRole("button", { name: "编辑配置" }));
  fireEvent.change(screen.getByLabelText("MCP 服务地址"), { target: { value: "https://new.example/mcp" } });
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.savePluginMcp).toHaveBeenCalledWith({}, loc.id, "original-revision", "docs", { url: "https://new.example/mcp", headers: { "X-Team": "keep" }, exposure: "deferred" }, false));
});

it("package-owned MCP is inspectable but cannot be edited or deleted independently", async () => {
  inventory.locations[0] = { ...loc, package: "team-tools", writable: false };
  inventory.resources[0] = resource({ managed: true });
  render(<PluginsView initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  await screen.findByLabelText("资源详情");
  expect(screen.queryByRole("button", { name: "编辑配置" })).toBeNull();
  expect(screen.queryByRole("button", { name: "删除配置" })).toBeNull();
});

it("explicit project targets report partial success and retry only failed directories", async () => {
  const paths = ["one", "two"].map(id => ({ id, canonical_path: `/work/${id}`, project_id: "p", exists: true } as WorkspacePath));
  vi.mocked(api.getProjectDetail).mockResolvedValue({ workspace_paths: paths } as ProjectDetailData);
  vi.mocked(api.listProjects).mockResolvedValue([{ id: "p", name: "Demo", visibility: "normal" } as never]);
  vi.mocked(api.listPluginResources).mockImplementation(async scope => ({ resources: [], warnings: [], locations: [{ ...loc, id: `pi-${scope.workspace_path_id}`, scope: "project", path: `/work/${scope.workspace_path_id}/.pi/mcp.json` }] }));
  vi.mocked(api.savePluginMcp).mockImplementation(async scope => {
    if (scope.workspace_path_id === "two" && vi.mocked(api.savePluginMcp).mock.calls.length === 2) throw new Error("directory became read-only");
    return { message: "Saved" };
  });
  render(<PluginsView initialTab="mcp" initialProjectId="p" />);
  await waitFor(() => expect((screen.getByLabelText("配置工作目录") as HTMLSelectElement).value).toBe("one"));
  await waitFor(() => expect(screen.getByRole("button", { name: "新增 MCP" }).hasAttribute("disabled")).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "新增 MCP" }));
  fireEvent.change(screen.getByLabelText("MCP 名称"), { target: { value: "tools" } });
  fireEvent.change(screen.getByLabelText("MCP 启动程序"), { target: { value: "tools-server" } });
  fireEvent.click(screen.getByLabelText("/work/two"));
  await waitFor(() => expect(screen.getByRole("button", { name: "保存" }).hasAttribute("disabled")).toBe(false));
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await screen.findByText(/部分目录操作失败/);
  expect(api.savePluginMcp).toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByRole("button", { name: "保存" }));
  await waitFor(() => expect(api.savePluginMcp).toHaveBeenCalledTimes(3));
  expect(vi.mocked(api.savePluginMcp).mock.calls.map(call => call[0].workspace_path_id)).toEqual(["one", "two", "two"]);
});

it.each(["pi", "antigravity"] as const)("uses %s's native MCP switch without dropping server options", async agent => {
  inventory.locations[0] = { ...loc, agent };
  const config = { command: "tools-server", env: { TOKEN: "${TOOLS_TOKEN}" } };
  vi.mocked(api.getPluginResource).mockResolvedValue({ resource: inventory.resources[0], content: JSON.stringify(config), config });
  render(<PluginsView initialTab="mcp" />);
  fireEvent.click(await screen.findByRole("button", { name: /docs/ }));
  fireEvent.click(await screen.findByRole("button", { name: "禁用" }));
  await waitFor(() => expect(api.savePluginMcp).toHaveBeenCalledWith({}, loc.id, "original-revision", "docs", { ...config, [agent === "antigravity" ? "disabled" : "enabled"]: agent === "antigravity" }, false));
});
