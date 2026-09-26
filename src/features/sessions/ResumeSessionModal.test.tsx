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
      root_agent_session_id: `${id}-agent`,
      title: null,
      cwd: `/${id}`,
      project_id: null,
      workspace_path_id: null,
      owner_workstream_id: null,
      forked_from_session_id: null,
      started_at: null,
      last_activity_at: null,
      last_conversation_at: null,
      trashed_at: null,
    },
    messages: [],
    owner_workstream: null,
    workspace_path: null,
    members: [],
    stats: {
      member_count: 0,
      child_count: 0,
      side_count: 0,
      max_depth: 0,
      tool_call_count: 0,
      tool_error_count: 0,
      compaction_count: 0,
      side_activity_count: 0,
      input_tokens: null,
      output_tokens: null,
      cached_tokens: null,
      reasoning_tokens: null,
      cost: null,
    },
    ingested_message_sequence: 0,
    processed_message_sequence: 0,
    root_source_status: "present",
    can_resume: true,
    can_permanently_delete: false,
    forked_from: null,
  };
}

function prepared(
  id: string,
  cwd: string | null = null,
  ownerWorkstreamId: string | null = null,
): PreparedLaunch {
  return {
    id: `prepared-${id}`,
    mode: "resume",
    agent: "codex",
    session_id: id,
    owner_workstream_id: ownerWorkstreamId,
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
      workstream_id: null,
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
    await waitFor(() => expect(api.prepareResumeSession).toHaveBeenCalledWith("a"));

    view.rerender(<ResumeSessionModal sessionId="b" onClose={vi.fn()} />);
    await waitFor(() => expect(api.prepareResumeSession).toHaveBeenCalledWith("b"));
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

describe("ResumeSessionModal owner display", () => {
  it("shows the single owner task instead of a count of tasks", async () => {
    vi.mocked(api.getSessionDetail).mockResolvedValue({
      ...detail("s1"),
      session: { ...detail("s1").session, owner_workstream_id: "w1" },
      owner_workstream: {
        id: "w1",
        title: "NoEnding 会话模型重构",
        description: "",
        lifecycle: "active",
        visibility: "normal",
        created_at: "2026-09-21T00:00:00+00:00",
        updated_at: "2026-09-21T00:00:00+00:00",
      },
    });
    vi.mocked(api.prepareResumeSession).mockResolvedValue(prepared("s1", null, "w1"));

    render(<ResumeSessionModal sessionId="s1" onClose={vi.fn()} />);

    await screen.findByText("NoEnding 会话模型重构");
    // §33：不再出现「X 个任务」这种多任务计数，只显示这一个任务的名字。
    screen.getByText("1 个");
    expect(screen.queryByText(/个任务/)).toBeNull();
  });

  it("says 未归属任务 for a session with no owner", async () => {
    vi.mocked(api.getSessionDetail).mockResolvedValue(detail("s1"));
    vi.mocked(api.prepareResumeSession).mockResolvedValue(prepared("s1"));

    render(<ResumeSessionModal sessionId="s1" onClose={vi.fn()} />);

    await screen.findByText("未归属任务");
  });
});
