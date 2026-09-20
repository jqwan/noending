import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import { showToast } from "../../components/Toast";
import { agentDisplayLabel, sessionDisplayTitle } from "./SessionTable";
import type { PermanentDeletionPreview } from "../../types";

/**
 * Session 永久删除（Session Lifecycle & Deletion v0.1 §19/§37/§38）。
 *
 * 流程复用 PreparedLaunch 的思想：What you confirm is what gets deleted。
 * 父级只提交 sessionId；这里先 prepare 拿到**冻结**的删除预览（后端持有
 * 全部 path / target，前端永不提交它们），确认后以 job_id 执行。
 *
 * execute 不用 rejection 表达运营失败（源文件删除失败 / stale）——
 * 调用正常返回 `{ purged, job, error }`，必须看返回值决定下一步：
 *   • purged            → 成功：提示（含脱敏条数）、关闭、父级刷新。
 *   • job.state=stale   → 源会话在删除前变化：展示中止说明，可重新准备或取消任务。
 *   • job.state=failed  → 源会话删除失败：可用同一 job_id 重试，或取消任务。
 *
 * Adapter 不支持安全源会话删除时（prepare 拒绝），按 §38 渲染
 * 「永久删除暂不可用」状态——绝不提供「仅删除 NoEnding 数据」的旁路。
 */

type Phase =
  | "preparing"    // prepare 在路上
  | "unsupported"  // Adapter 不支持安全源会话删除（§38）
  | "prepareError" // 其他 prepare 失败（例如 Session 不在回收站）
  | "preview"      // 冻结预览已就绪，等确认
  | "executing"    // execute 在路上（按钮禁用，禁止关闭）
  | "stale"        // execute 返回：源会话在删除前发生了变化
  | "failed";      // execute 返回：源会话删除失败（可重试）

/** 后端用这条消息表达「Adapter 没有安全删除能力」（§38）——按关键词分流。 */
const UNSUPPORTED_MARKER = "暂不支持安全的源会话删除";

/** bytes → 人类可读大小。file_identity / sha256 不展示：那是机器对账字段。 */
function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "未知大小";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes;
  let i = -1;
  do {
    v /= 1024;
    i += 1;
  } while (v >= 1024 && i < units.length - 1);
  return `${v >= 100 ? Math.round(v) : Math.round(v * 10) / 10} ${units[i]}`;
}

/** 大数加千位分隔（§20 示例的「2,431 个 Events」）。 */
const fmtCount = (n: number) => n.toLocaleString("en-US");

export default function PermanentDeleteModal({ sessionId, onClose, onDeleted }: {
  sessionId: string;
  onClose: () => void;
  /** 永久删除成功后调用（此时弹窗不再走 onClose）：父级负责收尾与刷新。 */
  onDeleted?: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("preparing");
  const [preview, setPreview] = useState<PermanentDeletionPreview | null>(null);
  /** prepare 失败（非 unsupported）或 execute failed 时的错误文本。 */
  const [errorText, setErrorText] = useState<string | null>(null);
  /** stale / failed 状态下仍然有效的 job：[取消永久删除] 与 [重试] 都以它为准。 */
  const [liveJobId, setLiveJobId] = useState<string | null>(null);

  /** 重新走一遍 prepare → 冻结预览。stale 后的「重新准备」也回到这里。 */
  const prepare = useCallback(() => {
    setPhase("preparing");
    setPreview(null);
    setErrorText(null);
    setLiveJobId(null);
    api.prepareSessionPermanentDelete(sessionId)
      .then((p) => {
        setPreview(p);
        setPhase("preview");
      })
      .catch((e) => {
        const msg = String(e);
        if (msg.includes(UNSUPPORTED_MARKER)) {
          setErrorText(msg);
          setPhase("unsupported");
        } else {
          setErrorText(msg);
          setPhase("prepareError");
        }
      });
  }, [sessionId]);
  useEffect(() => { prepare(); }, [prepare]);

  const execute = async (jobId: string) => {
    setPhase("executing");
    try {
      const r = await api.executeSessionPermanentDelete(jobId);
      if (r.purged) {
        showToast(
          r.redacted_revisions > 0
            ? `已永久删除该 Session，并脱敏了 ${fmtCount(r.redacted_revisions)} 条 Context 来源`
            : "已永久删除该 Session",
        );
        onDeleted?.();
        onClose();
        return;
      }
      // 运营失败不走 rejection：按 job.state 分流（§21/§22）。
      const state = r.job?.state ?? null;
      setErrorText(r.error ?? r.job?.last_error ?? "未知错误");
      setLiveJobId(r.job?.id ?? jobId);
      if (state === "stale") setPhase("stale");
      else setPhase("failed"); // failed（含删除被中断的恢复路径）与无法判定的形态都走可重试分支
    } catch (e) {
      // 理论上不该发生（job 缺失等）；照样给出可重试出口，绝不静默。
      setErrorText(String(e));
      setLiveJobId(jobId);
      setPhase("failed");
    }
  };

  /** 取消遗留的 deletion job 再关窗。取消失败只提示，不阻拦离开——
   *  Session 反正仍在回收站，任务之后仍可取消/重试。 */
  const cancelAndClose = async () => {
    const jobId = liveJobId;
    onClose();
    if (!jobId) return;
    try {
      await api.cancelSessionPermanentDelete(jobId);
    } catch (e) {
      console.error(e);
      showToast(`取消删除任务失败：${String(e)}`);
    }
  };

  /** 执行期间禁止 Escape / 背板关闭：删除正在进行，中途消失会让人不知道结果。 */
  const requestClose = () => {
    if (phase === "executing") return;
    onClose();
  };

  if (phase === "unsupported") {
    // §38：不提供「仅删除 NoEnding 数据」。留在回收站是唯一推荐动作。
    return (
      <Modal title="永久删除暂不可用" onClose={onClose}>
        <p style={{ margin: "0 0 8px", maxWidth: "72ch", overflowWrap: "anywhere" }}>
          {errorText}
        </p>
        <p className="muted small" style={{ margin: "0 0 14px" }}>
          你仍可以把它保留在回收站。
        </p>
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn" onClick={onClose}>知道了</button>
        </div>
      </Modal>
    );
  }

  if (phase === "preparing" || phase === "prepareError") {
    return (
      <Modal title="永久删除" onClose={requestClose}>
        {phase === "preparing" && (
          <div className="muted" style={{ padding: "12px 0" }}>
            正在准备删除预览，冻结将要删除的源文件清单…
          </div>
        )}
        {phase === "prepareError" && (
          <>
            <div className="badge warn" style={{ display: "inline-block", marginBottom: 10, overflowWrap: "anywhere" }}>
              无法准备永久删除
            </div>
            <p className="small" style={{ margin: "0 0 14px", maxWidth: "72ch", overflowWrap: "anywhere" }}>
              {errorText}
            </p>
            <div className="row" style={{ justifyContent: "flex-end" }}>
              <button className="btn" onClick={onClose}>关闭</button>
              <button className="btn primary" onClick={prepare}>重试</button>
            </div>
          </>
        )}
      </Modal>
    );
  }

  // ---- 以下状态都持有 preview（冻结计划）----
  const busy = phase === "executing";

  return (
    <Modal title="永久删除" onClose={requestClose}>
      {preview && phase !== "stale" && phase !== "failed" && (
        <>
          <p style={{ margin: "0 0 10px", maxWidth: "72ch" }}>
            将永久删除 <b>{sessionDisplayTitle(preview.session_title)}</b>
            <span className="muted">（{agentDisplayLabel(preview.agent)}）</span>：
          </p>

          {preview.source_state === "confirmed_absent" ? (
            <>
              {/* 加固 §2：源文件在 prepare 时已确认不存在——只删 NoEnding 侧 */}
              <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
                Agent 源文件已不存在（可能已在应用外被删除）。
              </div>
              <div className="small" style={{ margin: "0 0 12px", maxWidth: "72ch" }}>
                本次将只删除 NoEnding 中的该会话数据；不会尝试删除任何文件。
              </div>
            </>
          ) : (
            <>
              {/* §37 关键警告，逐字保留 */}
              <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
                此操作会同时删除 Agent 保存的原始会话。
              </div>
              <div className="small" style={{ margin: "0 0 12px", maxWidth: "72ch" }}>
                删除完成后：
                <ul className="purge-list">
                  <li>NoEnding 中该 Session 的事件、绑定和摄入历史都会消失。</li>
                  <li>Workstream Context 内容不会因此删除。</li>
                  <li>
                    如果你以后从备份恢复原始 Agent 会话，NoEnding
                    会将它作为一个新的 Session 再次收录。
                  </li>
                </ul>
              </div>
            </>
          )}

          <div className="section-label" style={{ margin: "14px 0 2px" }}>将永久删除</div>
          <ul className="purge-list">
            {preview.source_targets.map((t) => (
              <li key={`${t.kind}:${t.path}`}>
                <span className="mono" style={{ overflowWrap: "anywhere" }}>{t.path}</span>
                <span className="muted small">
                  {" "}（{agentDisplayLabel(preview.agent)} · {formatSize(t.size)}）
                </span>
              </li>
            ))}
            {preview.source_state === "confirmed_absent" && (
              <li className="muted">Agent 源文件（已确认不存在，无需删除）</li>
            )}
            <li>1 个 Session</li>
            <li>{fmtCount(preview.event_count)} 个 Events</li>
            <li>{fmtCount(preview.binding_count)} 个 Workstream 绑定</li>
            <li>{fmtCount(preview.sync_run_count)} 个 Sync 运行</li>
            <li>{fmtCount(preview.context_delivery_count)} 个 Context 交付记录</li>
            <li>{fmtCount(preview.context_revision_redaction_count)} 条 Context 来源将被脱敏</li>
            <li>{fmtCount(preview.launch_intent_count)} 条启动记录</li>
          </ul>

          <div className="section-label" style={{ margin: "14px 0 2px" }}>保留</div>
          <ul className="purge-list">
            <li>Workstream、WorkstreamPath、WorkspacePath、Project</li>
            <li>Workstream Context 内容</li>
          </ul>
          <p className="muted small" style={{ margin: "6px 0 0", maxWidth: "72ch" }}>
            部分 Context 的「来源」将显示为「来源会话已永久删除」。
          </p>

          <div className="row" style={{ justifyContent: "flex-end", marginTop: 16 }}>
            <button className="btn" disabled={busy} onClick={onClose}>取消</button>
            {/* 不以 source_targets 为空为由禁用：源文件可能早已不在（§23），
                execute → AlreadyAbsent → 正常进入 NoEnding purge，这是合法路径。 */}
            <button
              className="btn danger"
              disabled={busy}
              onClick={() => execute(preview.job_id)}
            >
              {busy ? "删除中…" : "永久删除"}
            </button>
          </div>
        </>
      )}

      {phase === "stale" && (
        <>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
            删除已中止
          </div>
          <p style={{ margin: "0 0 14px", maxWidth: "72ch" }}>
            源会话在删除前发生了变化，本次删除已中止，NoEnding 数据完整保留。
          </p>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" disabled={busy} onClick={cancelAndClose}>取消永久删除</button>
            <button className="btn primary" disabled={busy} onClick={prepare}>重新准备</button>
          </div>
        </>
      )}

      {phase === "failed" && (
        <>
          <div className="badge warn" style={{ display: "inline-block", marginBottom: 10 }}>
            源会话删除失败
          </div>
          <p style={{ margin: "0 0 6px", maxWidth: "72ch", overflowWrap: "anywhere" }}>
            源会话删除失败：{errorText}，Session 仍保留在回收站，NoEnding 数据完整保留。
          </p>
          <p className="muted small" style={{ margin: "0 0 14px", maxWidth: "72ch" }}>
            常见原因是文件被占用（例如 Windows 文件共享冲突）或权限不足；处理后可直接重试。
          </p>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn" disabled={busy} onClick={cancelAndClose}>取消永久删除</button>
            <button
              className="btn primary"
              disabled={busy || liveJobId === null}
              onClick={() => liveJobId && execute(liveJobId)}
            >
              {busy ? "删除中…" : "重试"}
            </button>
          </div>
        </>
      )}
    </Modal>
  );
}
