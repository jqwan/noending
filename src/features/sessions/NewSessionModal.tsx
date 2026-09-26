import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { announceLaunch } from "../launcher/LaunchResultModal";
import { usePreparedLaunch } from "../launcher/usePreparedLaunch";
import {
  AgentRow,
  CwdRow,
  RuntimeRow,
} from "../launcher/LaunchPreviewRows";
import type { Agent, PreparedLaunch, WorkstreamCardData } from "../../types";

/**
 * 全局新建 Session。启动路径唯一：`prepareNewSession → launchPrepared`。Modal 一打开
 * 就 Prepare，所以预览显示的就是这次启动真正使用的解析结果（cwd 三级优先级在后端算）。
 * 点击启动时后端重算状态指纹，任何变化都以 stale 中止——绝不用没预览过的参数启动。
 *
 * `workstreamId` 是可选预置入参：省略或 `"none"` 即 standalone（0 个所属任务完全合法）。
 * 预置后用户仍可改。下拉读 Workstream 卡片投影：标题相同的靠主路径才能分清，而启动
 * 目录恰由主路径决定，所以选项里必须能看到它。
 */
export type NewSessionModalProps = {
  onClose: () => void;
  /** 预置选中的所属任务；省略或 "none" = standalone。 */
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
  const [ownerWorkstreamId, setOwnerWorkstreamId] = useState(
    workstreamId && workstreamId !== STANDALONE ? workstreamId : STANDALONE
  );
  const [defaultAgent, setDefaultAgent] = useState<Agent | null>(null);
  const [agentResolved, setAgentResolved] = useState(false);
  const [busy, setBusy] = useState(false);

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
      ownerWorkstreamId === STANDALONE ? null : ownerWorkstreamId,
    );
  }, [defaultAgent, ownerWorkstreamId]);
  const { prepared, preparedRef, preparing, error, setError, prepare, release: releasePrepared } =
    usePreparedLaunch(prepareLaunch);

  const handleWsChange = (next: string) => {
    releasePrepared();
    setOwnerWorkstreamId(next);
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

  return (
    <Modal title="新建会话" onClose={handleClose}>
      <label className="field">
        <span>所属任务（可选）</span>
        <select value={ownerWorkstreamId} onChange={(e) => handleWsChange(e.target.value)}>
          <option value={STANDALONE}>无（直接开始）</option>
          {workstreams.map((w) => (
            <option key={w.id} value={w.id}>
              {workstreamLabel(w)}
            </option>
          ))}
        </select>
      </label>

      {defaultAgent ? (
        <AgentRow agent={defaultAgent} hint="默认 Agent" first />
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
  );
}
