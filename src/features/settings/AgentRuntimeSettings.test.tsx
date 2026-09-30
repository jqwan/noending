// Agent 设置行的契约：只读安装状态（可执行文件 / 版本 / 检测徽标），一次读取、
// 不做任何 discovery 或写入。模型与思考强度的 override 已移除——行内不该再出现
// 它们的痕迹。api 模块整体被 mock：测试不依赖 Tauri，只断言调用模式。

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import AgentRuntimeRow from "./AgentRuntimeSettings";
import { api } from "../../api";
import type { Agent, AgentRuntimeSettings } from "../../types";

vi.mock("../../api", () => ({
  api: {
    getAgentRuntimeSettings: vi.fn(),
  },
}));

vi.mocked(api.getAgentRuntimeSettings);

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

beforeEach(() => {
  vi.mocked(api.getAgentRuntimeSettings).mockReset();
});

afterEach(cleanup);

describe("AgentRuntimeRow install state", () => {
  it("renders_detection_from_a_single_settings_read", async () => {
    vi.mocked(api.getAgentRuntimeSettings).mockResolvedValue(settings());

    render(<AgentRuntimeRow agent="codex" />);
    expect(await screen.findByText("/usr/local/bin/codex")).toBeTruthy();
    expect(screen.getByText("已检测")).toBeTruthy();
    // 挂载只读一次；没有任何 discovery 入口。
    await new Promise((r) => setTimeout(r, 0));
    expect(api.getAgentRuntimeSettings).toHaveBeenCalledTimes(1);
  });

  it("an_undetected_agent_says_so", async () => {
    vi.mocked(api.getAgentRuntimeSettings).mockResolvedValue(
      settings({ detected: false, executable: null, version: null }),
    );

    render(<AgentRuntimeRow agent="codex" />);
    expect(await screen.findByText("未找到可执行文件")).toBeTruthy();
    expect(screen.getByText("未检测")).toBeTruthy();
  });
});
