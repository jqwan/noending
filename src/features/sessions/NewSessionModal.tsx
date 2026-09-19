import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { useBaseExperience } from "../../app/experience";
import { announceLaunch } from "../launcher/LaunchResultModal";
import ContextPreviewModal from "../launcher/ContextPreviewModal";
import {
  AgentRow,
  CwdRow,
  RuntimeRow,
  deliveryLevelLabel,
} from "../launcher/LaunchPreviewRows";
import type { Agent, PreparedLaunch, Workstream } from "../../types";

/**
 * 全局新建 Session（方案 §15，跨边界契约 §8.1.1）。
 *
 * 启动路径唯一：`prepareNewSession → launchPrepared`。Modal 一打开就 Prepare，
 * 所以「工作目录 / Agent / Runtime」显示的就是这次启动真正使用的解析结果
 * （`resolve_new_session_cwd` 的三级优先级在后端完成，前端不重算）。点击启动时
 * 后端重算状态指纹，任何变化都以 stale 中止——绝不用没预览过的参数启动
 * （Preview-Launch Identity / Launch Preparation Integrity）。
 *
 * `workstreamId` 是 Commit 0 冻结的可选预置入参：省略或 `"none"` 即
 * standalone（0 绑定完全合法）。预置后用户仍然可以改。
 */
export type NewSessionModalProps = {
  onClose: () => void;
  /** 预置选中的 Workstream；省略或 "none" = standalone。 */
  workstreamId?: string | null;
};

const STANDALONE = "none";

export default function NewSessionModal({
  onClose,
  workstreamId,
}: NewSessionModalProps) {
  const [workstreams, setWorkstreams] = useState<Workstream[]>([]);
  const [wsId, setWsId] = useState(
    workstreamId && workstreamId !== STANDALONE ? workstreamId : STANDALONE
  );
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [agentResolved, setAgentResolved] = useState(false);
  const [prepared, setPrepared] = useState<PreparedLaunch | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const { deliveryLevel } = useBaseExperience();
  const deliveryOff = deliveryLevel === "off";

  /**
   * `preparedRef` is the single held capability. `seqRef` makes adoption
   * race-free: a prepare superseded by a later one (or by unmount) cancels
   * itself instead of overwriting the current selection's token.
   */
  const preparedRef = useRef<PreparedLaunch | null>(null);
  const wsIdRef = useRef(wsId);
  wsIdRef.current = wsId;
  const seqRef = useRef(0);

  /**
   * Stop holding the capability. `alreadyReleased` is for the two paths where
   * someone else already destroyed it: the preview modal cancels on close, and
   * `launchPrepared` consumes the token itself — cancelling in those cases
   * would either double-release or destroy the launch that is in flight.
   */
  const releasePrepared = useCallback((alreadyReleased = false) => {
    const held = preparedRef.current;
    preparedRef.current = null;
    setPrepared(null);
    if (held && !alreadyReleased) {
      api.cancelPrepared(held.id).catch(console.error);
    }
  }, []);

  useEffect(() => {
    api
      .listWorkstreams()
      .then((ws) =>
        setWorkstreams(ws.filter((w) => w.visibility === "normal"))
      )
      .catch(console.error);
    api
      .getDefaultAgent()
      .then(setDefaultAgent)
      .catch(console.error)
      .finally(() => setAgentResolved(true));
    return () => {
      seqRef.current += 1;
      if (preparedRef.current) {
        api.cancelPrepared(preparedRef.current.id).catch(console.error);
        preparedRef.current = null;
      }
    };
  }, []);

  /**
   * Prepare once: returns the token it adopted, or null. Every caller goes
   * through here so there is exactly one place that can hold a capability.
   */
  const prepare = useCallback(async (): Promise<PreparedLaunch | null> => {
    if (!defaultAgent) return null;
    const mine = ++seqRef.current;
    setPreparing(true);
    try {
      const p = await api.prepareNewSession(
        defaultAgent,
        wsIdRef.current === STANDALONE ? [] : [wsIdRef.current]
      );
      if (seqRef.current !== mine) {
        api.cancelPrepared(p.id).catch(console.error);
        return null;
      }
      preparedRef.current = p;
      setPrepared(p);
      setError("");
      return p;
    } catch (e: unknown) {
      if (seqRef.current === mine) setError(String(e));
      return null;
    } finally {
      if (seqRef.current === mine) setPreparing(false);
    }
  }, [defaultAgent]);

  // Open (and every re-selection) prepares: 工作目录与 Runtime 都取自这份结果。
  useEffect(() => {
    void prepare();
  }, [prepare, wsId]);

  const handleWsChange = (next: string) => {
    releasePrepared();
    setWsId(next);
  };

  const handleClose = () => {
    releasePrepared();
    onClose();
  };

  /** 唯一启动路径：消费 PreparedLaunch，绝不复用（single-use capability）。 */
  const start = async () => {
    const current = preparedRef.current;
    if (busy || !current) return;
    // launchPrepared 自己消费这份令牌，这里只能松手不能再 cancel
    releasePrepared(true);
    setBusy(true);
    setError("");
    try {
      const r = await api.launchPrepared(current.id);
      announceLaunch("启动", r);
      onClose();
    } catch (e: unknown) {
      setBusy(false);
      // 令牌已消费：重新 Prepare 一份，用户再次确认才真正启动。
      const fresh = await prepare();
      if (fresh) {
        setError("状态已变化，启动计划已刷新，请再次确认。");
      } else {
        setError(`启动失败：${String(e)}`);
      }
    }
  };

  // Title of the current selection; null while standalone or still loading.
  const selectedTitle =
    wsId === STANDALONE ? null : workstreams.find((w) => w.id === wsId)?.title;

  return (
    <>
      <Modal title="新建 Session" onClose={handleClose}>
        {workstreamId && workstreamId !== STANDALONE && selectedTitle && (
          <div className="settings-row-hint" style={{ marginBottom: 6 }}>
            已预置 Workstream：{selectedTitle}，可以再改
          </div>
        )}
        <label className="field">
          <span>Workstream（可选；也可以之后为 Session 关联）</span>
          <select value={wsId} onChange={(e) => handleWsChange(e.target.value)}>
            <option value={STANDALONE}>无（直接开始）</option>
            {workstreams.map((w) => (
              <option key={w.id} value={w.id}>
                {w.title}
              </option>
            ))}
          </select>
        </label>

        {defaultAgent ? (
          <AgentRow agent={defaultAgent} hint="设置中的默认 Agent" first />
        ) : (
          <div className="row-line" style={{ borderTop: 0 }}>
            <div>
              <div className="settings-row-label">Agent</div>
              <div className="settings-row-hint">
                {agentResolved
                  ? "未检测到可用的 Agent CLI — 请先到「设置 → Agent」配置"
                  : "加载中…"}
              </div>
            </div>
            <span className="badge">{agentResolved ? "未检测" : "—"}</span>
          </div>
        )}
        <CwdRow cwd={prepared?.cwd} pending={preparing} resolution={prepared?.cwd_resolution} />
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
              onClick={() => setPreviewOpen(true)}
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

        <div className="row" style={{ justifyContent: "flex-end", marginTop: 14 }}>
          <button className="btn" onClick={handleClose} disabled={busy}>
            取消
          </button>
          <button
            className="btn primary"
            disabled={busy || preparing || !defaultAgent || !prepared}
            onClick={start}
          >
            {busy ? "启动中…" : preparing ? "准备中…" : "启动"}
          </button>
        </div>
      </Modal>

      {previewOpen && prepared && (
        <ContextPreviewModal
          prepared={prepared}
          onClose={() => {
            setPreviewOpen(false);
            // 预览内部已回收这个令牌，这里只重新备好一份，保持就绪。
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
