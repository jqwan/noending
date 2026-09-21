import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import ResumeSessionModal from "./ResumeSessionModal";
import { api } from "../../api";
import type { PreparedLaunch, SessionDetail } from "../../types";

vi.mock("../../api", () => ({
  api: {
    getSessionDetail: vi.fn(),
    prepareResumeSession: vi.fn(),
    cancelPrepared: vi.fn().mockResolvedValue(undefined),
    launchPrepared: vi.fn(),
  },
}));

vi.mock("../../app/experience", () => ({
  useBaseExperience: vi.fn(() => ({
    intelligenceEnabled: false,
    deliveryLevel: "balanced",
  })),
  useDeliveryOff: vi.fn(() => false),
}));

vi.mock("../launcher/LaunchResultModal", () => ({ announceLaunch: vi.fn() }));

afterEach(() => {
  cleanup();
  vi.mocked(api.getSessionDetail).mockReset();
  vi.mocked(api.prepareResumeSession).mockReset();
  vi.mocked(api.cancelPrepared).mockReset();
  vi.mocked(api.cancelPrepared).mockResolvedValue(undefined);
  vi.mocked(api.launchPrepared).mockReset();
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function detail(id: string): SessionDetail {
  return {
    session: {
      id,
      agent: "codex",
      agent_session_id: `${id}-agent`,
      title: null,
      cwd: `/${id}`,
      project_id: null,
      workspace_path_id: null,
      raw_path: `/${id}.jsonl`,
      parent_agent_session_id: null,
      started_at: null,
      last_activity_at: null,
      trashed_at: null,
    },
    events: [],
    bindings: [],
    cursor: 0,
    processed_cursor: 0,
    classification: "",
    raw_path_status: "present",
    workspace_path: null,
  };
}

function prepared(id: string, cwd: string | null = null): PreparedLaunch {
  return {
    id: `prepared-${id}`,
    mode: "resume",
    agent: "codex",
    session_id: id,
    workstream_ids: [],
    extra_workstream_ids: [],
    cwd,
    cwd_resolution: {
      source: "session_cwd",
      cwd,
      fallback: false,
      workstream_id: null,
      path_position: null,
      note: null,
    },
    delivery_level: "off",
    bundle: {
      mode: "resume",
      delivery_level: "off",
      workstream_ids: [],
      sections: [],
      markdown: "",
      approx_tokens: 0,
    },
    runtime: { model: null, provider: null, effort: null },
    state_fingerprint: `fingerprint-${id}`,
    prepared_at: "2026-09-20T00:00:00Z",
  };
}

function previewCwd(dialog: HTMLElement, cwd: string) {
  return within(dialog).queryByText(
    (_, element) => element?.tagName === "SPAN" && element.textContent?.includes(cwd) === true,
  );
}

describe("ResumeSessionModal preparation races", () => {
  it.each([
    ["succeeds", true],
    ["fails", false],
  ])("does not let stale A refresh details after switching to B and B %s", async (_label, bSucceeds) => {
    const prepareA = deferred<PreparedLaunch>();
    const prepareB = deferred<PreparedLaunch>();
    vi.mocked(api.getSessionDetail).mockImplementation(async (id) => detail(id));
    vi.mocked(api.prepareResumeSession).mockImplementation((id) =>
      id === "a" ? prepareA.promise : prepareB.promise,
    );

    const view = render(<ResumeSessionModal sessionId="a" onClose={vi.fn()} />);
    await waitFor(() => expect(api.prepareResumeSession).toHaveBeenCalledWith("a", []));

    view.rerender(<ResumeSessionModal sessionId="b" onClose={vi.fn()} />);
    await waitFor(() => expect(api.prepareResumeSession).toHaveBeenCalledWith("b", []));
    await screen.findByText("/b");

    if (bSucceeds) {
      await act(async () => {
        prepareB.resolve(prepared("b", "/prepared-b"));
      });
      await screen.findByText("/prepared-b");
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "继续" }));
      });
      await waitFor(() => expect(api.launchPrepared).toHaveBeenCalledWith("prepared-b"));
    } else {
      await act(async () => {
        prepareB.reject(new Error("B failed"));
      });
      await screen.findByText("Error: B failed");
      expect((screen.getByRole("button", { name: "继续" }) as HTMLButtonElement).disabled).toBe(true);
    }

    await act(async () => {
      prepareA.resolve(prepared("a"));
    });
    await waitFor(() => expect(api.cancelPrepared).toHaveBeenCalledWith("prepared-a"));
    await screen.findByText("/b");
  });

  it("reclaims an existing A token when switching to B and B fails", async () => {
    const prepareB = deferred<PreparedLaunch>();
    vi.mocked(api.getSessionDetail).mockImplementation(async (id) => detail(id));
    vi.mocked(api.prepareResumeSession).mockImplementation((id) =>
      id === "a" ? Promise.resolve(prepared("a", "/prepared-a")) : prepareB.promise,
    );

    const view = render(<ResumeSessionModal sessionId="a" onClose={vi.fn()} />);
    await screen.findByText("/prepared-a");

    view.rerender(<ResumeSessionModal sessionId="b" onClose={vi.fn()} />);
    await screen.findByText("/b");
    await waitFor(() => expect(api.cancelPrepared).toHaveBeenCalledWith("prepared-a"));

    await act(async () => {
      prepareB.reject(new Error("B failed"));
    });
    await screen.findByText("Error: B failed");
    expect((screen.getByRole("button", { name: "继续" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("refreshes the preview by replacing the token after the new one succeeds", async () => {
    const refresh = deferred<PreparedLaunch>();
    vi.mocked(api.getSessionDetail).mockImplementation(async (id) => detail(id));
    vi.mocked(api.prepareResumeSession)
      .mockResolvedValueOnce(prepared("initial", "/prepared-initial"))
      .mockImplementationOnce(() => refresh.promise);

    render(<ResumeSessionModal sessionId="b" onClose={vi.fn()} />);
    await screen.findByRole("button", { name: "预览 Context" });
    fireEvent.click(screen.getByRole("button", { name: "预览 Context" }));
    const previewDialog = await screen.findByRole("dialog", { name: "Context 注入预览" });
    expect(previewCwd(previewDialog, "/prepared-initial")).not.toBeNull();

    await act(async () => {
      fireEvent.click(within(previewDialog).getByRole("button", { name: "刷新预览" }));
    });
    await waitFor(() => expect(api.prepareResumeSession).toHaveBeenCalledTimes(2));
    expect(api.cancelPrepared).not.toHaveBeenCalledWith("prepared-initial");
    expect(previewCwd(previewDialog, "/prepared-initial")).not.toBeNull();

    await act(async () => {
      refresh.resolve(prepared("refreshed", "/prepared-refreshed"));
    });
    await waitFor(() =>
      expect(previewCwd(previewDialog, "/prepared-refreshed")).not.toBeNull(),
    );
    expect(previewCwd(previewDialog, "/prepared-initial")).toBeNull();
    expect(api.cancelPrepared).toHaveBeenCalledWith("prepared-initial");
  });

  it("keeps the old preview token when refreshing fails", async () => {
    vi.mocked(api.getSessionDetail).mockImplementation(async (id) => detail(id));
    vi.mocked(api.prepareResumeSession)
      .mockResolvedValueOnce(prepared("initial", "/prepared-initial"))
      .mockRejectedValueOnce(new Error("refresh failed"));

    render(<ResumeSessionModal sessionId="b" onClose={vi.fn()} />);
    await screen.findByRole("button", { name: "预览 Context" });
    fireEvent.click(screen.getByRole("button", { name: "预览 Context" }));
    const previewDialog = await screen.findByRole("dialog", { name: "Context 注入预览" });

    await act(async () => {
      fireEvent.click(within(previewDialog).getByRole("button", { name: "刷新预览" }));
    });
    await screen.findByText("Error: refresh failed");
    expect(previewCwd(previewDialog, "/prepared-initial")).not.toBeNull();
    expect(api.cancelPrepared).not.toHaveBeenCalledWith("prepared-initial");

    vi.mocked(api.launchPrepared).mockResolvedValue(undefined as never);
    await act(async () => {
      fireEvent.click(
        within(previewDialog).getByRole("button", { name: "确认并立即启动" }),
      );
    });
    await waitFor(() => expect(api.launchPrepared).toHaveBeenCalledWith("prepared-initial"));
  });
});
