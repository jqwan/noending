import { useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import PathListEditor, { type PathEntryDraft } from "./PathListEditor";
import type { CreateWorkstreamReport, Workstream } from "../../types";

/**
 * 新建 Workstream（方案 §22）：标题 / 描述 / **初始工作路径列表**（后两项可选）。
 *
 * Project 选择器已经删除 —— v0.2 里 Project 是从工作目录派生出来的，
 * 用户既不能挑也不能造（§0）。这里唯一和物理世界有关的输入就是那组路径：
 * 它们按提交顺序成为 WorkstreamPath 列表，第 1 条被接受的即主工作路径，
 * Project 由它们自动出现。
 *
 * `api.createWorkstream` 接受 title / description / initialPaths 并逐条回报
 * 结果：被拒绝的路径（写错形式、NoEnding 自留目录等）在这里说破，而不是
 * 静默丢掉（§42.3 不能确定就不猜）。没有 Project 形参，也不存在为旧 bridge
 * 留的过渡位。
 */
export default function NewWorkstreamModal({ onClose, onCreated }: {
  onClose: () => void;
  onCreated?: (w: Workstream) => void;
}) {
  const [title, setTitle] = useState("");
  const [desc, setDesc] = useState("");
  const [entries, setEntries] = useState<PathEntryDraft[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  /**
   * 已经建好、但有路径没有被接受：Workstream 照建（§41 不能确定就不猜），
   * 但「你以为带上了、其实没有」的部分必须逐条说破，而不是静默跳走。
   */
  const [report, setReport] = useState<CreateWorkstreamReport | null>(null);
  const busyRef = useRef(false);

  const create = async () => {
    const trimmedTitle = title.trim();
    if (trimmedTitle === "" || busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError("");
    try {
      const r = await api.createWorkstream(
        trimmedTitle,
        desc,
        entries.map((e) => e.raw),
      );
      if (r.paths.some((p) => !p.accepted)) {
        setReport(r);
        return;
      }
      onCreated?.(r.workstream);
      onClose();
    } catch (e) {
      // 失败时留在弹窗里、把原因说出来：静默关闭会让用户以为已经建好了。
      console.error(e);
      setError(String(e));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const leave = () => {
    if (report) onCreated?.(report.workstream);
    onClose();
  };

  if (report) {
    const allRejected = report.paths.length > 0 && report.paths.every((p) => !p.accepted);
    return (
      <Modal
        title={allRejected ? "任务已创建（没有工作路径）" : "任务已创建（部分路径未接受）"}
        onClose={leave}
      >
        <p style={{ margin: "0 0 8px", maxWidth: "72ch" }}>
          <b>{report.workstream.title}</b> 已经建好了。提交的 {report.paths.length} 条路径里，
          {report.paths.filter((p) => p.accepted).length} 条被接受
          {allRejected ? "，0 条被接受" : ""}。
        </p>
        <div style={{ marginBottom: 10 }}>
          {report.paths.map((p, i) => (
            <div key={`${p.raw}-${i}`} className="list-row" style={{ cursor: "default" }}>
              <div className="grow" style={{ minWidth: 0 }}>
                <span className="mono" style={{ overflowWrap: "anywhere" }} title={p.raw}>
                  {p.raw}
                </span>
              </div>
              <div className="side">
                {p.accepted ? (
                  <span className="badge accent">
                    第 {(p.position ?? 0) + 1} 条{p.project_name ? ` · 项目 ${p.project_name}` : ""}
                  </span>
                ) : (
                  <span className="badge warn" style={{ maxWidth: 280, overflowWrap: "anywhere" }}>
                    {p.reason ?? "未接受"}
                  </span>
                )}
              </div>
            </div>
          ))}
        </div>
        <p className="small muted" style={{ marginBottom: 12 }}>
          {allRejected
            ? "没有工作路径的任务依然有效：新建会话会从 NoEnding 的默认工作目录启动，也不归属任何项目。你可以在详情页随时添加工作路径。"
            : "被拒绝的路径不影响其余路径：任务按提交顺序带上了被接受的部分。"}
        </p>
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn primary" onClick={leave}>知道了</button>
        </div>
      </Modal>
    );
  }

  return (
    <Modal title="新建任务" onClose={onClose}>
      <label className="field"><span>标题</span>
        <input type="text" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus
          placeholder="例如：接口设计 / 行程规划 / 预算整理"
          onKeyDown={(e) => e.key === "Enter" && create()} /></label>
      <label className="field"><span>描述（可选）</span>
        <textarea value={desc} onChange={(e) => setDesc(e.target.value)} /></label>
      <div className="field"><span>工作路径（可选，可多条）</span>
        <PathListEditor entries={entries} onChange={setEntries} /></div>
      {error && (
        <div className="badge warn" style={{ marginBottom: 10, overflowWrap: "anywhere" }}>{error}</div>
      )}
      <div className="row" style={{ justifyContent: "flex-end" }}>
        <button className="btn" onClick={onClose} disabled={busy}>取消</button>
        <button className="btn primary" disabled={busy || !title.trim()} onClick={create}>
          {busy ? "创建中…" : "创建"}
        </button>
      </div>
    </Modal>
  );
}
