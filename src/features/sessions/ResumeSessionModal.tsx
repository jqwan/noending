import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { useBaseExperience } from "../../app/experience";
import { announceLaunch } from "../launcher/LaunchResultModal";
import ContextPreviewModal from "../launcher/ContextPreviewModal";
import { usePreparedLaunch } from "../launcher/usePreparedLaunch";
import {
  AgentRow,
  CwdRow,
  PreviewRow,
  RuntimeRow,
  deliveryLevelLabel,
} from "../launcher/LaunchPreviewRows";
import type { PreparedLaunch, SessionDetail } from "../../types";

/** SessionDetailView 通过此 Modal 继续已有 Session。 */
export type ResumeSessionModalProps = {
  sessionId: string;
  onClose: () => void;
};

/**
 * 打开即准备（后端会先摄入这个 Session 自己的最新消息），预览显示
 * Agent / 工作目录 / 所属任务 / Runtime；「继续」消费这份 PreparedLaunch。
 * 不要求用户重新选择任何已经确定的参数，也不再有额外的 Workstream 入参（§16）。
 *
 * PreparedLaunch 是 single-use 能力令牌；关闭预览后会重新准备，卸载时会回收。
 */
export default function ResumeSessionModal({
  sessionId,
  onClose,
}: ResumeSessionModalProps) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const detailRequest = useRef(0);

  const { deliveryLevel } = useBaseExperience();
  const deliveryOff = deliveryLevel === "off";

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
  // §33：一次只有零个或一个任务，不再出现多任务计数。
  const ownerDisplay = ownerWorkstreamId === null
    ? "未归属任务"
    : ownerTitle ?? "未命名任务";

  return (
    <>
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

        <PreviewRow
          label="所属任务"
          hint={ownerDisplay}
        >
          <span className="badge">
            {ownerWorkstreamId === null ? "无" : "1 个"}
          </span>
        </PreviewRow>

        {prepared && (
          <RuntimeRow agent={prepared.agent} runtime={prepared.runtime} />
        )}

        {/* Context 关闭时不挂载预览，也不显示 token 计数。 */}
        {!deliveryOff && (
          <div className="row-line">
            <div>
              <div className="settings-row-label">Context 注入</div>
              <div className="settings-row-hint">
                等级：{deliveryLevelLabel(deliveryLevel)} ·
                预览显示的就是本次真正注入的 Context
              </div>
            </div>
            <button
              type="button"
              className="btn small"
              disabled={preparing || busy || !prepared}
              onClick={() => {
                if (prepared) setPreviewOpen(true);
              }}
            >
              预览 Context
            </button>
          </div>
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
            {busy ? "继续中…" : preparing ? "准备中…" : "继续"}
          </button>
        </div>
      </Modal>

      {previewOpen && prepared && (
        <ContextPreviewModal
          prepared={prepared}
          onClose={() => {
            setPreviewOpen(false);
            // 预览内部已回收这个令牌：重新准备一份，保持「继续」随时可用
            releasePrepared(true);
            void prepare();
          }}
          onRefresh={async () => {
            // 刷新当前预览时保留旧令牌；预览拿到新令牌后再回收旧的那个。
            return await prepare({ preserveCurrent: true });
          }}
          onLaunched={() => {
            // 预览内已经启动成功：令牌是被消费掉的，不再 cancel
            releasePrepared(true);
            onClose();
          }}
        />
      )}
    </>
  );
}
