//  — Agent Settings 的 discovery 行为契约：
//
//   1. mount 只读已保存设置，绝不 spawn Agent CLI（进入页面 ≠ 刷新模型）；
//   2. 「刷新模型」按钮是唯一 discovery 入口；
//   3. 已保存 override 在没有任何 model catalog 时照常显示；
//   4. discovery 失败只降级 catalog，绝不覆盖已保存的 override。
//
// api 模块整体被 mock：测试不依赖 Tauri，只断言调用模式。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import AgentRuntimeRow from "./AgentRuntimeSettings";
import { api } from "../../api";
import type { Agent, AgentRuntimeDiscovery, AgentRuntimeSettings } from "../../types";

vi.mock("../../api", () => ({
  api: {
    getAgentRuntimeSettings: vi.fn(),
    refreshAgentRuntimeOptions: vi.fn(),
    setAgentRuntimeOverrides: vi.fn(),
  },
}));

vi.mocked(api.getAgentRuntimeSettings);
vi.mocked(api.refreshAgentRuntimeOptions);

function settings(over: Partial<AgentRuntimeSettings> = {}): AgentRuntimeSettings {
  return {
    agent: "codex" as Agent,
    detected: true,
    executable: "/usr/local/bin/codex",
    version: "1.2.3",
    overrides: { model: null, provider: null, effort: null },
    capabilities: { provider: "unsupported", model: "discoverable", effort: "discoverable" },
    models: [],
    model_source: "not_loaded",
    effort_levels: ["low", "medium", "high"],
    warnings: [],
    ...over,
  };
}

function discovery(over: Partial<AgentRuntimeDiscovery> = {}): AgentRuntimeDiscovery {
  return {
    capabilities: { provider: "unsupported", model: "discoverable", effort: "discoverable" },
    models: [],
    model_source: "dynamic",
    effort_levels: ["low", "medium", "high"],
    warnings: [],
    ...over,
  };
}

beforeEach(() => {
  vi.mocked(api.getAgentRuntimeSettings).mockReset();
  vi.mocked(api.refreshAgentRuntimeOptions).mockReset();
});

afterEach(cleanup);

describe("AgentRuntimeRow model discovery", () => {
  it("agent_settings_mount_does_not_refresh_models", async () => {
    vi.mocked(api.getAgentRuntimeSettings).mockResolvedValue(settings());
    vi.mocked(api.refreshAgentRuntimeOptions).mockResolvedValue(discovery());

    render(<AgentRuntimeRow agent="codex" />);
    await screen.findByText("刷新模型"); // 设置已加载，行已完整渲染
    // 再让若干个宏任务跑完：即便实现里藏着自动 refresh，这里也必然已触发
    await new Promise((r) => setTimeout(r, 0));

    expect(api.getAgentRuntimeSettings).toHaveBeenCalledTimes(1);
    expect(api.refreshAgentRuntimeOptions).not.toHaveBeenCalled();
  });

  it("manual_refresh_calls_model_discovery", async () => {
    vi.mocked(api.getAgentRuntimeSettings).mockResolvedValue(settings());
    vi.mocked(api.refreshAgentRuntimeOptions).mockResolvedValue(
      discovery({
        models: [{ id: "gpt-5.6-sol", display_name: null, provider: null, supported_efforts: [] }],
      }),
    );

    render(<AgentRuntimeRow agent="codex" />);
    fireEvent.click(await screen.findByText("刷新模型"));

    await waitFor(() => {
      expect(api.refreshAgentRuntimeOptions).toHaveBeenCalledTimes(1);
    });
    // catalog 到位后 footer 显示动态来源计数
    await screen.findByText("1 个模型来自 Codex CLI");
  });

  it("saved_override_renders_without_model_catalog", async () => {
    vi.mocked(api.getAgentRuntimeSettings).mockResolvedValue(
      settings({ overrides: { model: "gpt-5.6-sol", provider: null, effort: null } }),
    );

    render(<AgentRuntimeRow agent="codex" />);
    // models=[] 且从未刷新：自定义值仍原样显示，且不触发任何 discovery
    expect(await screen.findByDisplayValue("gpt-5.6-sol")).toBeTruthy();
    await new Promise((r) => setTimeout(r, 0));
    expect(api.refreshAgentRuntimeOptions).not.toHaveBeenCalled();
  });

  it("discovery_failure_preserves_saved_override", async () => {
    vi.mocked(api.getAgentRuntimeSettings).mockResolvedValue(
      settings({
        overrides: { model: "gpt-5.6-sol", provider: null, effort: "high" },
      }),
    );
    vi.mocked(api.refreshAgentRuntimeOptions).mockRejectedValue("codex CLI probe 失败");

    render(<AgentRuntimeRow agent="codex" />);
    fireEvent.click(await screen.findByText("刷新模型"));

    // 失败提示出现（unavailable 说明 + warning），但 override 纹丝不动
    await screen.findByText(/无法从 Agent 获取模型列表/);
    expect(screen.getByDisplayValue("gpt-5.6-sol")).toBeTruthy();
    expect(screen.getByDisplayValue("high")).toBeTruthy();
    expect(api.setAgentRuntimeOverrides).not.toHaveBeenCalled();
  });
});
