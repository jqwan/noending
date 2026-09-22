import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import WorkspacePathField, { absolutePathHint } from "./WorkspacePathField";
import { api } from "../api";
import type { PathProbe, RecentWorkspacePath } from "../types";

vi.mock("../api", () => ({
  api: {
    probeWorkspacePath: vi.fn(),
    listRecentWorkspacePaths: vi.fn(),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

function okProbe(raw: string, overrides: Partial<PathProbe> = {}): PathProbe {
  return {
    raw,
    status: "ok",
    canonical_path: raw,
    exists: true,
    git_state: "detected",
    git_kind: "main",
    project: { id: "p1", name: "Foo", known: true },
    ...overrides,
  };
}

function recent(paths: string[]): RecentWorkspacePath[] {
  return paths.map((p) => ({
    path: p,
    known: true,
    exists: true,
    project_name: null,
    git_state: null,
    git_kind: null,
    last_used_at: null,
  }));
}

/** 组件契约是受控 value/onChange，测试也照契约来：由 Harness 持有真实 state。 */
function Harness({ onChange, ...rest }: {
  exclude?: string[];
  onSubmit?: () => void;
  enableProbe?: boolean;
  enableRecent?: boolean;
  onChange?: (value: string) => void;
}) {
  const [value, setValue] = useState("");
  const handle = (v: string) => {
    setValue(v);
    onChange?.(v);
  };
  return (
    <WorkspacePathField
      value={value}
      onChange={handle}
      exclude={rest.exclude}
      onSubmit={rest.onSubmit}
      enableProbe={rest.enableProbe}
      enableRecent={rest.enableRecent}
    />
  );
}

async function type(value: string) {
  await act(async () => {
    fireEvent.change(screen.getByRole("textbox"), { target: { value } });
  });
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("WorkspacePathField", () => {
  it("debounces the probe and renders the project the path would land in", async () => {
    vi.mocked(api.probeWorkspacePath).mockResolvedValue(okProbe("/repo/foo"));
    render(<Harness />);

    await type("/repo/foo");
    expect(api.probeWorkspacePath).not.toHaveBeenCalled();

    await waitFor(() => expect(api.probeWorkspacePath).toHaveBeenCalledWith("/repo/foo"));
    screen.getByText(/归属于项目/);
  });

  it("warns for a reserved path from the probe verdict", async () => {
    vi.mocked(api.probeWorkspacePath).mockResolvedValue({
      raw: "/home/me/.noending/data",
      status: "reserved",
      canonical_path: null,
      exists: false,
      git_state: null,
      git_kind: null,
      project: null,
    });
    render(<Harness />);
    await type("/home/me/.noending/data");
    await screen.findByText(/自留目录/);
  });

  it("shows the absolute-path hint instantly and skips the probe", async () => {
    render(<Harness />);
    await type("relative/dir");
    await screen.findByText(/绝对路径/);
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 450));
    });
    expect(api.probeWorkspacePath).not.toHaveBeenCalled();
  });

  it("opens recent paths on demand, honours exclude, and fills on pick", async () => {
    vi.mocked(api.listRecentWorkspacePaths).mockResolvedValue(
      recent(["/repo/foo", "/repo/bar"])
    );
    const onChange = vi.fn();
    render(<Harness onChange={onChange} exclude={["/repo/bar"]} />);

    fireEvent.click(screen.getByRole("button", { name: "最近使用" }));
    await screen.findByText("/repo/foo");
    expect(screen.queryByText("/repo/bar")).toBeNull();

    await act(async () => {
      fireEvent.mouseDown(screen.getByText("/repo/foo"));
    });
    expect(onChange).toHaveBeenCalledWith("/repo/foo");
  });

  it("fills the path from the native folder dialog", async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    vi.mocked(open).mockResolvedValue("/picked/dir");
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "浏览…" }));
    });
    expect(open).toHaveBeenCalledWith(expect.objectContaining({ directory: true }));
    expect(onChange).toHaveBeenCalledWith("/picked/dir");
  });

  it("lets Enter submit the form", () => {
    const onSubmit = vi.fn();
    render(<WorkspacePathField value="/repo/foo" onChange={vi.fn()} onSubmit={onSubmit} />);
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it("does not submit while an IME is composing", () => {
    const onSubmit = vi.fn();
    render(<WorkspacePathField value="/repo/foo" onChange={vi.fn()} onSubmit={onSubmit} />);
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter", isComposing: true });
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("does not submit for the IME Enter key code", () => {
    const onSubmit = vi.fn();
    render(<WorkspacePathField value="/repo/foo" onChange={vi.fn()} onSubmit={onSubmit} />);
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter", keyCode: 229 });
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("keeps the absolute-path hint wording shared", () => {
    expect(absolutePathHint("x/y")).toContain("绝对路径");
    expect(absolutePathHint("/repo")).toBe("");
    expect(absolutePathHint("~/repo")).toBe("");
    expect(absolutePathHint("C:\\repo")).toBe("");
  });
});
