import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { useBaseExperience } from "../../app/experience";
import { announceLaunch } from "../launcher/LaunchResultModal";
import ContextPreviewModal from "../launcher/ContextPreviewModal";
import {
  AgentRow,
  CwdRow,
  PreviewRow,
  RuntimeRow,
  deliveryLevelLabel,
} from "../launcher/LaunchPreviewRows";
import type { PreparedLaunch, SessionDetail } from "../../types";

/** 冻结契约（§8.1.1）：形状保持不变，B 的 SessionDetailView 正按此调用。 */
export type ResumeSessionModalProps = {
  sessionId: string;
  onClose: () => void;
};

/**
 * 继续 Session（方案 §15）。
 *
 * 打开即 Prepare（后端会先摄入这个 Session 自己的最新消息），预览显示
 * Agent / 工作目录 / Workstream / Runtime；「继续」消费这份 PreparedLaunch。
 * 不要求用户重新选择任何已经确定的参数。
 *
 * ⚠️ Context Preview 的隐藏只是**可见性**改动：eager prepare、关闭预览后的
 * 重 prepare、卸载时的 cancelPrepared 全部保留——PreparedLaunch 是 single-use
 * 能力令牌，少一次 prepare 就没有可启动的令牌（§15「UX 简化不能绕开 integrity」）。
 */
export default function ResumeSessionModal({
  sessionId,
  onClose,
}: ResumeSessionModalProps) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [prepared, setPrepared] = useState<PreparedLaunch | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const { deliveryLevel } = useBaseExperience();
  const deliveryOff = deliveryLevel === "off";

  const preparedRef = useRef<PreparedLaunch | null>(null);
  const seqRef = useRef(0);

  /**
   * Stop holding the capability. `alreadyReleased` marks the paths where the
   * token was destroyed elsewhere: the preview cancels on close, and
   * `launchPrepared` consumes it. Cancelling again there would either
   * double-release or kill the launch that is in flight.
   */
  const releasePrepared = useCallback((alreadyReleased = false) => {
    const held = preparedRef.current;
    preparedRef.current = null;
    setPrepared(null);
    if (held && !alreadyReleased) {
      api.cancelPrepared(held.id).catch(console.error);
    }
  }, []);

  /** mount 时 eager prepare，出错时同样从这里重来；这是唯一持有令牌的地方。 */
  const prepare = useCallback(async (): Promise<PreparedLaunch | null> => {
    const mine = ++seqRef.current;
    setPreparing(true);
    try {
      const p = await api.prepareResumeSession(sessionId, []);
      if (seqRef.current !== mine) {
        // 已被更晚的 prepare 取代或组件已卸载（cleanup 递增了 seq）：立刻回收，不留孤儿 preparation
        api.cancelPrepared(p.id).catch(console.error);
        return null;
      }
      preparedRef.current = p;
      setPrepared(p);
      setError("");
      // Prepare 内部会摄入 Session 新消息，刷新 detail 让绑定显示与之一致
      try {
        const fresh = await api.getSessionDetail(sessionId);
        if (seqRef.current === mine) setDetail(fresh);
      } catch (e) {
        console.error(e);
      }
      return p;
    } catch (e: unknown) {
      if (seqRef.current === mine) setError(String(e));
      return null;
    } finally {
      if (seqRef.current === mine) setPreparing(false);
    }
  }, [sessionId]);

  useEffect(() => {
    let cancelled = false;
    api
      .getSessionDetail(sessionId)
      .then((d) => {
        if (!cancelled) setDetail(d);
      })
      .catch(console.error);
    void prepare();
    return () => {
      cancelled = true;
      seqRef.current += 1;
      // 卸载时释放持有的 preparation
      if (preparedRef.current) {
        api.cancelPrepared(preparedRef.current.id).catch(console.error);
        preparedRef.current = null;
      }
    };
  }, [prepare, sessionId]);

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
      <Modal title="继续 Session" onClose={handleClose}>
        <div style={{ padding: "20px 0", color: "var(--text-muted)" }}>
          {preparing ? "准备中…" : "加载中…"}
        </div>
      </Modal>
    );
  }

  const { session, bindings } = detail;
  // PreparedLaunch 里的是这次启动真正生效的 Workstream 集合；未就绪时退回绑定列表
  const effectiveWsIds = prepared
    ? prepared.workstream_ids
    : bindings.map(([b]) => b.workstream_id);
  const wsDisplay =
    effectiveWsIds.length === 0
      ? "未关联 Workstream"
      : bindings
          .filter(([b]) => effectiveWsIds.includes(b.workstream_id))
          .map(([, title]) => title)
          .filter(Boolean)
          .join(" · ") || `${effectiveWsIds.length} 个关联 Workstream`;

  return (
    <>
      <Modal title="继续 Session" onClose={handleClose}>
        <AgentRow
          agent={session.agent}
          hint="这个 Session 原本使用的 Agent"
          first
        />
        <CwdRow cwd={prepared?.cwd ?? session.cwd} pending={preparing && !prepared} />

        <PreviewRow
          label="Workstream"
          hint={wsDisplay}
        >
          <span className="badge">
            {effectiveWsIds.length > 0 ? `${effectiveWsIds.length} 个` : "无"}
          </span>
        </PreviewRow>

        {prepared && (
          <RuntimeRow agent={prepared.agent} runtime={prepared.runtime} />
        )}

        {/* Context Preview 在注入关闭时整块不挂载，也不出现任何 token 计数（§15、§24）。 */}
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
            // 预览在拿到新令牌后自行回收旧的那个。失败时 prepare 返回 null，
            // 旧令牌仍在手上且依然有效，所以预览保持打开。
            return await prepare();
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
