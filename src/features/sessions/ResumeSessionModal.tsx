import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { announceLaunch } from "../launcher/LaunchResultModal";
import { usePreparedLaunch } from "../launcher/usePreparedLaunch";
import {
  AgentRow,
  CwdRow,
  PreviewRow,
  DesktopOpenRow,
} from "../launcher/LaunchPreviewRows";
import type { AgentStatusEntry, PreparedLaunch, SessionDetail } from "../../types";
import type { Route } from "../../app/routes";

/** SessionDetailView 通过此 Modal 继续已有 Session。 */
export type ResumeSessionModalProps = {
  sessionId: string;
  onClose: () => void;
  /** 内嵌终端启动成功后导航到该会话的终端子页；不传则只提示。 */
  navigate?: (r: Route) => void;
};

/** 继续方式的选项：有 TUI CLI 才有终端两式，接了桌面端且在场才有桌面式。 */
type MethodOption = { value: "terminal" | "desktop" | "embedded"; label: string };

function methodOptionsOf(status: AgentStatusEntry | null): MethodOption[] {
  if (!status) return [];
  const options: MethodOption[] = [];
  if (status.terminal_cli) {
    options.push({ value: "terminal", label: "外部终端" });
    options.push({ value: "embedded", label: "内嵌终端" });
  }
  if (status.desktop_app && status.desktop_app_present) {
    options.push({ value: "desktop", label: "桌面应用" });
  }
  return options;
}

/**
 * 打开即准备（后端会先摄入这个 Session 的最新消息），预览显示 Agent / 工作目录 /
 * 所属任务 / Runtime；「继续」消费这份 PreparedLaunch，不要求用户重选已确定的参数。
 * PreparedLaunch 是 single-use 能力令牌；关闭预览后会重新准备，卸载时会回收。
 */
export default function ResumeSessionModal({
  sessionId,
  onClose,
  navigate,
}: ResumeSessionModalProps) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [busy, setBusy] = useState(false);
  const [agentStatus, setAgentStatus] = useState<AgentStatusEntry | null>(null);
  const detailRequest = useRef(0);

  const refreshDetail = useCallback(async (isCurrent: () => boolean = () => true) => {
    const request = ++detailRequest.current;
    try {
      const fresh = await api.getSessionDetail(sessionId);
      if (isCurrent() && detailRequest.current === request) setDetail(fresh);
    } catch (e) {
      console.error(e);
    }
  }, [sessionId]);

  const prepareLaunch = useCallback(async (isCurrent: () => boolean): Promise<PreparedLaunch> => {
    const prepared = await api.prepareResumeSession(sessionId);
    if (isCurrent()) await refreshDetail(isCurrent);
    return prepared;
  }, [refreshDetail, sessionId]);
  const { prepared, preparedRef, preparing, error, setError, prepare, release: releasePrepared } =
    usePreparedLaunch(prepareLaunch);

  useEffect(() => {
    void refreshDetail();
    return () => {
      detailRequest.current += 1;
    };
  }, [refreshDetail]);

  // 继续方式的选项是适配器静态事实（terminal_cli / desktop_app）+ 桌面端在场；
  // 当前值是存储的偏好。读不到时不出这一行，预览的其余事实照常。
  useEffect(() => {
    let live = true;
    api.getAgentStatus()
      .then((status) => {
        if (live) setAgentStatus(status[detail?.session.agent ?? ""] ?? null);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [detail?.session.agent]);

  const methodOptions = methodOptionsOf(agentStatus);
  const method = agentStatus?.resume_open_method ?? "terminal";
  const changeMethod = async (next: MethodOption["value"]) => {
    try {
      await api.setResumeOpenMethod(detail!.session.agent, next);
      setAgentStatus((prev) => (prev ? { ...prev, resume_open_method: next } : prev));
      releasePrepared();
      void prepare();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleClose = () => {
    releasePrepared();
    onClose();
  };

  const handleResume = async () => {
    const current = preparedRef.current;
    if (busy || !current) return;
    // single-use：launchPrepared 自己消费这份令牌，这里只能松手不能再 cancel
    releasePrepared(true);
    setBusy(true);
    setError("");
    try {
      const res = await api.launchPrepared(current.id);
      announceLaunch("继续", res);
      // 内嵌启动把用户带到终端子页：这就是这次「继续」的对话现场。
      if (res?.terminal_id && navigate) {
        navigate({ view: "session", sessionId, entry: "terminal" });
      }
      onClose();
    } catch (e: unknown) {
      setBusy(false);
      const fresh = await prepare();
      if (fresh) {
        setError("状态已变化，启动计划已刷新，请再次确认「继续」。");
      } else {
        setError(`启动失败：${String(e)}`);
      }
    }
  };

  if (!detail) {
    return (
      <Modal title="继续会话" onClose={handleClose}>
        <div style={{ padding: "20px 0", color: "var(--text-muted)" }}>
          {preparing ? "准备中…" : "加载中…"}
        </div>
      </Modal>
    );
  }

  const { session, owner_workstream } = detail;
  // 这次继续真正生效的所属任务：PreparedLaunch 冻结的那一个；未就绪时退回详情读到的 Owner。
  const ownerWorkstreamId = prepared
    ? prepared.owner_workstream_id
    : session.owner_workstream_id;
  // 详情里的名字只在这条任务确实就是本次生效的那一个时才拿来用（Preview-Launch Identity）。
  const ownerTitle = owner_workstream && owner_workstream.id === ownerWorkstreamId
    ? owner_workstream.title.trim() || "未命名任务"
    : null;
  // 一次只有零个或一个任务，不再出现多任务计数。
  const ownerDisplay = ownerWorkstreamId === null
    ? "未归属任务"
    : ownerTitle ?? "未命名任务";

  return (
    <Modal title="继续会话" onClose={handleClose}>
      <AgentRow
        agent={session.agent}
        hint="原会话 Agent"
        first
      />
      <CwdRow
        cwd={prepared?.cwd ?? session.cwd}
        pending={preparing && !prepared}
        resolution={prepared?.cwd_resolution}
      />

      {methodOptions.length > 1 && (
        <PreviewRow
          label="继续方式"
          hint="会记住，作为该 Agent 之后的默认方式；内嵌 = 在 NoEnding 里的终端子页运行"
        >
          <div className="settings-seg" role="group" aria-label="继续方式">
            {methodOptions.map((o) => (
              <button
                key={o.value}
                type="button"
                className={method === o.value ? "on" : ""}
                disabled={busy}
                onClick={() => void changeMethod(o.value)}
              >
                {o.label}
              </button>
            ))}
          </div>
        </PreviewRow>
      )}

      <PreviewRow
        label="所属任务"
        hint={ownerDisplay}
      >
        <span className="badge">
          {ownerWorkstreamId === null ? "无" : "1 个"}
        </span>
      </PreviewRow>

      {prepared?.desktop_open && (
        <DesktopOpenRow desktopOpen={prepared.desktop_open} />
      )}

      {error && (
        <div
          className="badge warn"
          style={{ marginTop: 8, display: "flex", gap: 8, alignItems: "center" }}
        >
          <span style={{ flex: 1, overflowWrap: "anywhere" }}>{error}</span>
          {!preparing && !busy && (
            <button
              type="button"
              className="btn small"
              onClick={() => {
                releasePrepared();
                void prepare();
              }}
            >
              重试
            </button>
          )}
        </div>
      )}

      <div className="row" style={{ justifyContent: "flex-end", marginTop: 18 }}>
        <button className="btn" onClick={handleClose} disabled={busy}>
          取消
        </button>
        <button
          className="btn primary"
          disabled={busy || preparing || !prepared}
          onClick={handleResume}
        >
          {busy
            ? "继续中…"
            : preparing
              ? "准备中…"
              : method === "embedded"
                ? "内嵌继续"
                : method === "desktop"
                  ? "在桌面应用中继续"
                  : "继续"}
        </button>
      </div>
    </Modal>
  );
}
