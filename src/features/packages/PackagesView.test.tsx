import { useState } from "react";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../../api";
import PackagesView from "./PackagesView";
import PackageDetailView from "./PackageDetailView";
import AgentPackagesSection from "./AgentPackagesSection";
import type { Route } from "../../app/routes";
import { viewState } from "../../hooks/useViewState";
import type { PackageDetail, PackageEntry, PackageInventory } from "../../types";

vi.mock("../../api", () => ({ api: {
  listPackages: vi.fn(), getPackageDetail: vi.fn(),
  previewImportPackage: vi.fn(), uninstallPackage: vi.fn(), enablePackage: vi.fn(), disablePackage: vi.fn(),
  installLocalPackage: vi.fn(), installLocalMarketplace: vi.fn(), openPath: vi.fn(),
} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../../components/Toast", () => ({ showToast: vi.fn() }));
import { showToast } from "../../components/Toast";

const entry = (overrides: Partial<PackageEntry> = {}): PackageEntry => ({
  id: "pkg-codex-review",
  agent: "codex",
  name: "review",
  version: "1.2.0",
  description: "Code review helpers",
  origin: "market",
  marketplace: "openai-curated",
  install_path: "/home/.codex/plugins/cache/openai-curated/review/1.2.0",
  scopes: [{ scope: "user", state: "enabled", config_path: "/home/.codex/config.toml", config_exists: true, project_path: null }],
  capabilities: { uninstall: true, toggle: true },
  revision: "r1",
  error: null,
  ...overrides,
});

let inventory: PackageInventory;
const navigation = vi.fn();

function PackageTestApp() {
  const [route, setRoute] = useState<Route>({ view: "packages" });
  const navigate = (next: Route) => { navigation(next); setRoute(next); };
  if (route.view === "package") return <PackageDetailView packageId={route.packageId} initialTitle={route.initialTitle} agent={route.agent} navigate={navigate} />;
  return <PackagesView navigate={navigate} />;
}

const detailOf = (row: PackageEntry): PackageDetail => ({
  entry: row,
  contents: { skills: ["pr-check"], mcp_files: [".mcp.json"] },
  root: row.install_path,
});

beforeEach(() => {
  vi.clearAllMocks();
  viewState.clear(); localStorage.clear();
  inventory = { packages: [entry()], warnings: [] };
  vi.mocked(api.listPackages).mockImplementation(async () => inventory);
  vi.mocked(api.getPackageDetail).mockImplementation(async id => {
    const row = inventory.packages.find(p => p.id === id);
    return row ? detailOf(row) : null;
  });
  vi.mocked(api.openPath).mockResolvedValue();
  vi.mocked(api.uninstallPackage).mockResolvedValue({ message: "插件包已卸载" });
  vi.mocked(api.enablePackage).mockResolvedValue({ message: "启停配置已保存" });
  vi.mocked(api.disablePackage).mockResolvedValue({ message: "启停配置已保存" });
  vi.mocked(api.installLocalPackage).mockResolvedValue({ message: "已安装到 1 个 Agent" });
  vi.mocked(api.previewImportPackage).mockImplementation(async path => ({
    path, kind: "package" as const,
    options: [{ agent: "pi" as const, manifest: "plugin.json", name: "toolkit", description: "Imported toolkit", plugins: [] }],
  }));
  vi.mocked(api.installLocalMarketplace).mockResolvedValue({ message: "已安装 2 个插件" });
});
afterEach(cleanup);

it("renders managed packages with agent, market and state, and opens the detail page", async () => {
  inventory = { packages: [
    entry(),
    entry({ id: "pkg-wb-find", agent: "workbuddy", name: "find-skills", origin: "market", marketplace: "codebuddy-plugins-official", capabilities: { uninstall: false, toggle: false }, scopes: [{ scope: "user", state: "unknown", config_path: "s", config_exists: true, project_path: null }] }),
  ], warnings: [] };
  render(<PackageTestApp />);
  expect(await screen.findByText("review")).toBeTruthy();
  // Enablement is a badge on the card; the codex fixture is enabled.
  expect(screen.getByText("启用")).toBeTruthy();
  // Read-only entries surface their unknown enablement on the other tab.
  expect(screen.queryByText("find-skills")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "不可管理" }));
  expect(await screen.findByText("find-skills")).toBeTruthy();
  expect(screen.getByText("未知")).toBeTruthy();
  expect(screen.queryByText("review")).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "可管理" }));
  fireEvent.click(await screen.findByRole("button", { name: /review/ }));
  expect(await screen.findByRole("heading", { name: "描述" })).toBeTruthy();
  expect(screen.getByText("Code review helpers")).toBeTruthy();
  expect(screen.getByText("市场 · openai-curated")).toBeTruthy();
  // The user-level toggle lives in the page header; the config file is a
  // property now.
  expect(screen.getByRole("button", { name: "停用" })).toBeTruthy();
  expect(screen.getByText(/config.toml/)).toBeTruthy();
  // Package-provided resources stay inside this page; no skill projections.
  expect(screen.getByText("Skill · pr-check")).toBeTruthy();
  expect(screen.getByText("MCP · .mcp.json")).toBeTruthy();
});

it("treats NoEnding built-ins by the general rules: unmanageable, no buttons", async () => {
  inventory = { packages: [entry({
    id: "pkg-agy-identity", agent: "antigravity", name: "noending-terminal-identity",
    origin: "noending_managed", marketplace: null,
    capabilities: { uninstall: false, toggle: false },
    scopes: [{ scope: "user", state: "enabled", config_path: "s", config_exists: true, project_path: null }],
  })], warnings: [] };
  render(<PackagesView navigate={navigation} />);
  // Neither capability → 可管理 tab is empty; the built-in lives with the
  // read-only entries, no toggle or uninstall shortcut anywhere.
  expect(await screen.findByText("没有可管理的插件包。")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "不可管理" }));
  expect(await screen.findByText("noending-terminal-identity")).toBeTruthy();
  expect(screen.getByText("NoEnding 内置")).toBeTruthy();
  // Its enablement (ships enabled) still reads through the badge.
  expect(screen.getByText("启用")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "停用" })).toBeNull();
  expect(screen.queryByRole("button", { name: "启用" })).toBeNull();
  expect(screen.queryByRole("button", { name: "卸载插件包" })).toBeNull();
});

it("filters by agent and query on the board", async () => {
  inventory = { packages: [entry(), entry({ id: "pkg-pi-tools", agent: "pi", name: "team-tools", origin: "market", marketplace: null, scopes: [{ scope: "user", state: "enabled", config_path: "s", config_exists: true, project_path: null }] })], warnings: [] };
  render(<PackagesView navigate={navigation} />);
  expect(await screen.findByText("review")).toBeTruthy();
  expect(screen.getByText("team-tools")).toBeTruthy();

  fireEvent.change(screen.getByRole("combobox", { name: "筛选 Agent" }), { target: { value: "pi" } });
  expect(await screen.findByText("team-tools")).toBeTruthy();
  expect(screen.queryByText("review")).toBeNull();
});

it("keeps read-only surfaces for entries whose Agent exposes no management", async () => {
  inventory = { packages: [
    entry({ id: "pkg-wb-builtin", agent: "workbuddy", name: "welcome", origin: "bundled", marketplace: "workbuddy-builtin", capabilities: { uninstall: false, toggle: false } }),
    entry({ id: "pkg-pi-quiet", agent: "pi", name: "quiet", origin: "market", marketplace: null, capabilities: { uninstall: true, toggle: false }, scopes: [{ scope: "user", state: "enabled", config_path: "s", config_exists: true, project_path: null }] }),
  ], warnings: [] };
  const { unmount } = render(<PackageDetailView packageId="pkg-wb-builtin" initialTitle="welcome" navigate={navigation} />);
  await screen.findByRole("region", { name: "插件包详情" });
  expect(screen.getByText("Agent 内置")).toBeTruthy();
  // Bundled packages get no toggle or uninstall at all.
  expect(screen.queryByRole("button", { name: "停用" })).toBeNull();
  expect(screen.queryByRole("button", { name: "卸载插件包" })).toBeNull();
  unmount();

  render(<PackageDetailView packageId="pkg-pi-quiet" initialTitle="quiet" navigate={navigation} />);
  await screen.findByRole("region", { name: "插件包详情" });
  expect(screen.getByRole("button", { name: "卸载插件包" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "停用" })).toBeNull();
  expect(screen.queryByRole("button", { name: "启用" })).toBeNull();
});

it("installs a single-format directory straight to its matched agent without registering", async () => {
  render(<PackagesView navigate={navigation} />);
  fireEvent.click(await screen.findByRole("button", { name: "导入插件包" }));
  const dialog = await screen.findByRole("dialog", { name: "导入插件包" });
  fireEvent.change(within(dialog).getByLabelText("插件包目录"), { target: { value: "/external/toolkit" } });
  // The preview derives the only installable target from the manifest; there
  // is no selection control for a single-format directory.
  expect(await within(dialog).findByText("toolkit")).toBeTruthy();
  expect(within(dialog).getByText("plugin.json")).toBeTruthy();
  expect(within(dialog).getByText("将安装到 Pi")).toBeTruthy();
  expect(within(dialog).queryByRole("checkbox")).toBeNull();
  fireEvent.click(within(dialog).getByRole("button", { name: "安装" }));
  await vi.waitFor(() => expect(api.installLocalPackage).toHaveBeenCalledWith("/external/toolkit", ["pi"]));
  // The install is the whole flow — the dialog closes and the board refresh
  // toast carries the result. Nothing is registered or rolled back.
  await vi.waitFor(() => expect(within(dialog).queryByRole("dialog", { name: "导入插件包" })).toBeNull());
  expect(showToast).toHaveBeenCalledWith("已安装到 1 个 Agent");
});

it("offers every format of a dual-manifest directory and installs only the checked ones", async () => {
  vi.mocked(api.previewImportPackage).mockResolvedValue({
    path: "/external/dual", kind: "package",
    options: [
      { agent: "antigravity", manifest: "plugin.json", name: "a-plugin", description: "", plugins: [] },
      { agent: "pi", manifest: "package.json", name: "a-package", description: "", plugins: [] },
    ],
  });
  vi.mocked(api.installLocalPackage).mockResolvedValue({ message: "已安装到 1 个 Agent" });
  render(<PackagesView navigate={navigation} />);
  fireEvent.click(await screen.findByRole("button", { name: "导入插件包" }));
  const dialog = await screen.findByRole("dialog", { name: "导入插件包" });
  fireEvent.change(within(dialog).getByLabelText("插件包目录"), { target: { value: "/external/dual" } });
  // Wait for the debounced preview before querying the option rows.
  expect(await within(dialog).findByText(/多种清单格式/)).toBeTruthy();
  // Both formats are pre-selected.
  const boxes = within(dialog).getAllByRole("checkbox");
  expect(boxes).toHaveLength(2);
  expect(boxes.every(box => (box as HTMLInputElement).checked)).toBe(true);
  // Uncheck one — only the other agent is installed.
  fireEvent.click(within(dialog).getByRole("checkbox", { name: /Pi/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "安装" }));
  await vi.waitFor(() => expect(api.installLocalPackage).toHaveBeenCalledWith("/external/dual", ["antigravity"]));
});

it("installs selected plugins from a local marketplace directory", async () => {
  vi.mocked(api.previewImportPackage).mockResolvedValue({
    path: "/external/my-market", kind: "marketplace",
    options: [{
      agent: "claude_code", manifest: ".claude-plugin/marketplace.json",
      name: "my-market", description: "Local collection",
      plugins: [
        { name: "review", description: "Review helpers" },
        { name: "docs", description: "" },
      ],
    }],
  });
  render(<PackagesView navigate={navigation} />);
  fireEvent.click(await screen.findByRole("button", { name: "导入插件包" }));
  const dialog = await screen.findByRole("dialog", { name: "导入插件包" });
  fireEvent.change(within(dialog).getByLabelText("插件包目录"), { target: { value: "/external/my-market" } });
  // The marketplace's plugins enumerate with everything pre-selected, and
  // the hint explains the directory must stay in place.
  expect(await within(dialog).findByText("my-market")).toBeTruthy();
  expect(within(dialog).getByText(/需保留在原处/)).toBeTruthy();
  const boxes = within(dialog).getAllByRole("checkbox");
  expect(boxes).toHaveLength(2);
  expect(boxes.every(box => (box as HTMLInputElement).checked)).toBe(true);
  fireEvent.click(within(dialog).getByRole("checkbox", { name: /docs/ }));
  fireEvent.click(within(dialog).getByRole("button", { name: "安装" }));
  await vi.waitFor(() => expect(api.installLocalMarketplace).toHaveBeenCalledWith("/external/my-market", [
    { agent: "claude_code", plugins: ["review"] },
  ]));
  expect(api.installLocalPackage).not.toHaveBeenCalled();
});

it("rejects non-package directories in the preview and keeps submit disabled", async () => {
  vi.mocked(api.previewImportPackage).mockRejectedValue("该目录根目录没有插件清单（plugin.json / package.json），不是插件包");
  render(<PackagesView navigate={navigation} />);
  fireEvent.click(await screen.findByRole("button", { name: "导入插件包" }));
  const dialog = await screen.findByRole("dialog", { name: "导入插件包" });
  fireEvent.change(within(dialog).getByLabelText("插件包目录"), { target: { value: "/not/a/package" } });
  expect(await within(dialog).findByRole("alert")).toBeTruthy();
  expect(within(dialog).getByRole("button", { name: "安装" })).toHaveProperty("disabled", true);
  expect(api.installLocalPackage).not.toHaveBeenCalled();
});

it("uninstalls through the confirm dialog and returns to the board when the entry disappears", async () => {
  render(<PackageDetailView packageId="pkg-codex-review" initialTitle="review" navigate={navigation} />);
  fireEvent.click(await screen.findByRole("button", { name: "卸载插件包" }));
  const dialog = await screen.findByRole("dialog", { name: "卸载插件包" });
  expect(within(dialog).getByText(/原生命令卸载/)).toBeTruthy();
  fireEvent.click(within(dialog).getByRole("button", { name: "卸载" }));
  await vi.waitFor(() => expect(api.uninstallPackage).toHaveBeenCalledWith("pkg-codex-review"));
  await vi.waitFor(() => expect(navigation).toHaveBeenCalledWith({ view: "packages" }));
});

it("toggles user level directly from the header icon button", async () => {
  const { unmount } = render(<PackageDetailView packageId="pkg-codex-review" initialTitle="review" navigate={navigation} />);
  fireEvent.click(await screen.findByRole("button", { name: "停用" }));
  await vi.waitFor(() => expect(api.disablePackage).toHaveBeenCalledWith("pkg-codex-review", "r1"));
  unmount();

  inventory = { packages: [entry({ scopes: [{ scope: "user", state: "disabled", config_path: "c", config_exists: true, project_path: null }] })], warnings: [] };
  render(<PackageDetailView packageId="pkg-codex-review" initialTitle="review" navigate={navigation} />);
  fireEvent.click(await screen.findByRole("button", { name: "启用" }));
  await vi.waitFor(() => expect(api.enablePackage).toHaveBeenCalledWith("pkg-codex-review", "r1"));
});

it("shows the empty and unreadable states without pretending data exists", async () => {
  inventory = { packages: [], warnings: ["~/.codex/config.toml：无法解析"] };
  render(<PackagesView navigate={navigation} />);
  expect(await screen.findByText("没有可管理的插件包。")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "不可管理" }));
  expect(await screen.findByText("没有不可管理的插件包。")).toBeTruthy();
  expect(screen.getByText(/部分来源无法读取/)).toBeTruthy();
});

it("treats a never-written switch as off: gray button, click enables", async () => {
  inventory = { packages: [entry({ scopes: [{ scope: "user", state: "unset", config_path: "c", config_exists: false, project_path: null }] })], warnings: [] };
  render(<PackageTestApp />);
  fireEvent.click(await screen.findByRole("button", { name: "启用" }));
  await vi.waitFor(() => expect(api.enablePackage).toHaveBeenCalledWith("pkg-codex-review", "r1"));
  expect(api.disablePackage).not.toHaveBeenCalled();
});

it("keeps build-metadata versions off the card while the full string stays reachable", async () => {
  const longVersion = "5.6.2-wb.39298511.g37a65c0b.hbf00e3f3d64d";
  inventory = { packages: [entry({ id: "pkg-wb-long", agent: "workbuddy", name: "longver", origin: "bundled", marketplace: "workbuddy-builtin", version: longVersion })], warnings: [] };
  render(<PackageTestApp />);
  expect(await screen.findByText("longver")).toBeTruthy();
  // Card shows the shortened version; the full string is only the tooltip.
  expect(screen.getByText("v5.6.2")).toBeTruthy();
  expect(screen.queryByText(`v${longVersion}`)).toBeNull();
  expect(screen.getByTitle(longVersion)).toBeTruthy();
  // Footer carries the origin class, not the marketplace name; no dates.
  expect(screen.getByText("Agent 内置")).toBeTruthy();
  expect(screen.queryByText(/2026/)).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: /longver/ }));
  expect(await screen.findByText("Agent 内置")).toBeTruthy();
});

it("renders never-written config files as non-clickable and marked", async () => {
  inventory = { packages: [entry({ scopes: [
    { scope: "user", state: "enabled", config_path: "/home/.codex/config.toml", config_exists: false, project_path: null },
  ] })], warnings: [] };
  render(<PackageDetailView packageId="pkg-codex-review" initialTitle="review" navigate={navigation} />);
  const props = await screen.findByRole("region", { name: "属性" });
  // A derived-but-never-created file is muted text and cannot be opened.
  const missing = within(props).getByText(/config.toml（尚未创建）/);
  expect(missing.closest("button")).toBeNull();
  expect(within(props).queryByRole("button", { name: "复制配置文件路径" })).toBeNull();
  expect(api.openPath).not.toHaveBeenCalled();
});

it("toggles from the card footer; uninstall-only entries get no switch but a trash button", async () => {
  inventory = { packages: [
    entry(),
    entry({ id: "pkg-pi-tools", agent: "pi", name: "team-tools", capabilities: { uninstall: true, toggle: false } }),
  ], warnings: [] };
  render(<PackageTestApp />);
  expect(await screen.findByText("review")).toBeTruthy();
  // The footer button toggles user level in place.
  fireEvent.click(screen.getByRole("button", { name: "停用" }));
  await vi.waitFor(() => expect(api.disablePackage).toHaveBeenCalledWith("pkg-codex-review", "r1"));
  // Pi can only uninstall: still manageable, so it stays on this tab with a
  // trash shortcut and no power switch.
  expect(await screen.findByText("team-tools")).toBeTruthy();
  const piCard = screen.getByText("team-tools").closest("article");
  expect(within(piCard as HTMLElement).queryByRole("button", { name: "停用" })).toBeNull();
  expect(within(piCard as HTMLElement).getByRole("button", { name: "卸载插件包" })).toBeTruthy();
});

it("uninstalls from the card through a confirm dialog", async () => {
  inventory = { packages: [entry()], warnings: [] };
  render(<PackageTestApp />);
  expect(await screen.findByText("review")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "卸载插件包" }));
  const dialog = await screen.findByRole("dialog", { name: "卸载插件包" });
  expect(within(dialog).getByText(/原生命令卸载/)).toBeTruthy();
  fireEvent.click(within(dialog).getByRole("button", { name: "卸载" }));
  await vi.waitFor(() => expect(api.uninstallPackage).toHaveBeenCalledWith("pkg-codex-review"));
  await vi.waitFor(() => expect(screen.queryByRole("dialog", { name: "卸载插件包" })).toBeNull());
});

it("exposes the same toggle on list rows", async () => {
  inventory = { packages: [entry()], warnings: [] };
  render(<PackageTestApp />);
  expect(await screen.findByText("review")).toBeTruthy();
  // The view preference persists across mounts; the row carries the toggle.
  fireEvent.click(screen.getByRole("button", { name: "列表视图" }));
  const row = await screen.findByText("review").then(node => node.closest("article"));
  fireEvent.click(within(row as HTMLElement).getByRole("button", { name: "停用" }));
  await vi.waitFor(() => expect(api.disablePackage).toHaveBeenCalledWith("pkg-codex-review", "r1"));
});

it("shows the desktop-only dsh bundle as manageable with unknown enablement", async () => {
  inventory = { packages: [entry({
    id: "pkg-dsh-base", agent: "dsh", name: "@deepseek-ai/dsh-base", origin: "profile",
    marketplace: "desktop", capabilities: { uninstall: true, toggle: false },
    scopes: [
      { scope: "user", state: "unknown", config_path: "/home/.dsh/profiles/desktop/package.json", config_exists: true, project_path: null },
    ],
  })], warnings: [] };
  render(<PackageTestApp />);
  // Uninstall capability puts the bundle on the manageable tab with a trash
  // shortcut and no power switch.
  expect(await screen.findByText("@deepseek-ai/dsh-base")).toBeTruthy();
  expect(screen.getByText(/配置档 · desktop/)).toBeTruthy();
  const dshCard = screen.getByText("@deepseek-ai/dsh-base").closest("article");
  expect(within(dshCard as HTMLElement).getByRole("button", { name: "卸载插件包" })).toBeTruthy();
  expect(within(dshCard as HTMLElement).queryByRole("button", { name: "停用" })).toBeNull();
  // Enablement lives in the unread Cordis patch layer -> 未知 badge.
  expect(within(dshCard as HTMLElement).getByText("未知")).toBeTruthy();
});

it("gives the Agent detail package block the same direct toggle", async () => {
  inventory = { packages: [entry({ id: "pkg-codex-review", agent: "codex", name: "review" })], warnings: [] };
  render(<AgentPackagesSection agent="codex" navigate={navigation} />);
  const row = await screen.findByRole("button", { name: /review/ });
  expect(screen.queryByText("已启用")).toBeNull();
  // Same direct user-level toggle as the board cards.
  fireEvent.click(screen.getByRole("button", { name: "停用" }));
  await vi.waitFor(() => expect(api.disablePackage).toHaveBeenCalledWith("pkg-codex-review", "r1"));
  expect(navigation).not.toHaveBeenCalled();
  fireEvent.click(row);
  expect(navigation).toHaveBeenCalledWith(expect.objectContaining({ view: "package", packageId: "pkg-codex-review" }));
});
