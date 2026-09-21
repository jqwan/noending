import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { useBaseExperience } from "../../app/experience";
import { announceLaunch } from "../launcher/LaunchResultModal";
import ContextPreviewModal from "../launcher/ContextPreviewModal";
import { usePreparedLaunch } from "../launcher/usePreparedLaunch";
import {
  AgentRow,
  CwdRow,
  RuntimeRow,
  deliveryLevelLabel,
} from "../launcher/LaunchPreviewRows";
import type { Agent, PreparedLaunch, WorkstreamCardData } from "../../types";

/**
 * 全局新建 Session。
 *
 * 启动路径唯一：`prepareNewSession → launchPrepared`。Modal 一打开就 Prepare，
 * 所以「工作目录 / Agent / Runtime」显示的就是这次启动真正使用的解析结果
 * （`resolve_new_session_cwd` 的三级优先级在后端完成，前端不重算）。点击启动时
 * 后端重算状态指纹，任何变化都以 stale 中止——绝不用没预览过的参数启动
 * 后端在启动时会再次校验状态指纹。
 *
 * `workstreamId` 是可选预置入参：省略或 `"none"` 即
 * standalone（0 绑定完全合法）。预置后用户仍然可以改。
 *
 * 下拉读的是 Workstream 卡片投影：标题相同的 Workstream 靠主路径才能分清，
 * 而 Session 的实际启动目录恰恰由主路径决定——选项里必须能看到它。
 */
export type NewSessionModalProps = {
  onClose: () => void;
  /** 预置选中的 Workstream；省略或 "none" = standalone。 */
  workstreamId?: string | null;
};

const STANDALONE = "none";

/** 下拉选项里路径的紧凑形态：末段才是识别信息，整条路径留给 title。 */
function pathTail(path: string): string {
  const segs = path.split(/[\\/]/).filter(Boolean);
  const tail = segs.slice(-2).join("/");
  return segs.length > 2 ? `…/${tail}` : tail;
}

function workstreamLabel(w: WorkstreamCardData): string {
  if (w.primary_path) return `${w.title} · ${pathTail(w.primary_path)}`;
  return `${w.title} · 无工作路径`;
}

export default function NewSessionModal({
  onClose,
  workstreamId,
}: NewSessionModalProps) {
  const [workstreams, setWorkstreams] = useState<WorkstreamCardData[]>([]);
  const [wsId, setWsId] = useState(
    workstreamId && workstreamId !== STANDALONE ? workstreamId : STANDALONE
  );
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [agentResolved, setAgentResolved] = useState(false);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [busy, setBusy] = useState(false);

  const { deliveryLevel } = useBaseExperience();
  const deliveryOff = deliveryLevel === "off";

  useEffect(() => {
    api
      .listWorkstreamCards()
      .then((ws) =>
        setWorkstreams(ws.filter((w) => w.visibility === "normal"))
      )
      .catch(console.error);
    api
      .getDefaultAgent()
      .then(setDefaultAgent)
      .catch(console.error)
      .finally(() => setAgentResolved(true));
  }, []);

  const prepareLaunch = useCallback(async (): Promise<PreparedLaunch | null> => {
    if (!defaultAgent) return null;
    return api.prepareNewSession(
      defaultAgent,
      wsId === STANDALONE ? [] : [wsId],
    );
  }, [defaultAgent, wsId]);
  const { prepared, preparedRef, preparing, error, setError, prepare, release: releasePrepared } =
    usePreparedLaunch(prepareLaunch);

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
      <Modal title="新建会话" onClose={handleClose}>
        {workstreamId && workstreamId !== STANDALONE && selectedTitle && (
          <div className="settings-row-hint" style={{ marginBottom: 6 }}>
            已预置任务：{selectedTitle}，可以再改
          </div>
        )}
        <label className="field">
          <span>任务（可选；也可以之后为会话关联）</span>
          <select value={wsId} onChange={(e) => handleWsChange(e.target.value)}>
            <option value={STANDALONE}>无（直接开始）</option>
            {workstreams.map((w) => (
              <option key={w.id} value={w.id}>
                {workstreamLabel(w)}
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
