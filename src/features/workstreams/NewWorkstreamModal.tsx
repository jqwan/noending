import React, { useRef, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import type { Workstream } from "../../types";

/**
 * 新建 Workstream（方案 §22）：标题 / 描述 / **初始工作路径**（后两项可选）。
 *
 * Project 选择器已经删除 —— v0.2 里 Project 是从工作目录派生出来的，
 * 用户既不能挑也不能造（§0）。这里唯一和物理世界有关的输入就是那条初始路径：
 * 它成为 WorkstreamPath 列表的 position 0，Project 由它自动出现。
 *
 * `api.createWorkstream` 的第一个参数仍是 projectId，那是 bridge 留的过渡形参，
 * 这里恒为 `null`。
 */
export default function NewWorkstreamModal({ onClose, onCreated }: {
  onClose: () => void;
  onCreated?: (w: Workstream) => void;
}) {
  const [title, setTitle] = useState("");
  const [desc, setDesc] = useState("");
  const [path, setPath] = useState("");
  const [pathHint, setPathHint] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  /**
   * 已经建好、但初始路径没有被接受：`create_workstream` 对解析不了的路径是
   * 「照样建、只是不带路径」（§41 不能确定就不猜），返回值里看不出差别。
   * 所以这里补一次读，把「你以为带上了、其实没有」说破，而不是静默跳走。
   */
  const [createdWithoutPath, setCreatedWithoutPath] = useState<Workstream | null>(null);
  const busyRef = useRef(false);

  const create = async () => {
    const trimmedTitle = title.trim();
    const rawPath = path.trim();
    if (trimmedTitle === "" || busyRef.current) return;
    if (rawPath !== "" && !/^([~/\\]|[A-Za-z]:[\\/])/.test(rawPath)) {
      // 只挡明显写错的形式；「能不能当工作路径」的最终判定在 Rust 侧。
      setPathHint("请输入绝对路径（例如 /Users/… 、C:\\Users\\…），或写成 ~ 开头的形式。");
      return;
    }
    setPathHint("");
    busyRef.current = true;
    setBusy(true);
    setError("");
    try {
      const w = await api.createWorkstream(trimmedTitle, desc, rawPath);
      if (rawPath !== "") {
        const paths = await api.listWorkstreamPaths(w.id).catch(() => null);
        if (paths !== null && paths.length === 0) {
          setCreatedWithoutPath(w);
          return;
        }
      }
      onCreated?.(w);
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
    if (createdWithoutPath) onCreated?.(createdWithoutPath);
    onClose();
  };

  if (createdWithoutPath) {
    return (
      <Modal title="Workstream 已创建（没有工作路径）" onClose={leave}>
        <p style={{ margin: "0 0 8px", maxWidth: "72ch" }}>
          <b>{createdWithoutPath.title}</b> 已经建好了，但你填的路径没有被接受：
          工作路径需要一个可解析的绝对路径，并且不能是 NoEnding 的自留目录。
        </p>
        <p className="small muted" style={{ marginBottom: 12 }}>
          没有工作路径的 Workstream 依然有效：新建 Session 会从 NoEnding 的默认工作目录启动，
          也不归属任何 Project。你可以在详情页随时添加工作路径。
        </p>
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn primary" onClick={leave}>知道了</button>
        </div>
      </Modal>
    );
  }

  return (
    <Modal title="新建 Workstream" onClose={onClose}>
      <label className="field"><span>标题</span>
        <input type="text" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus
          placeholder="例如：接口设计 / 行程规划 / 预算整理"
          onKeyDown={(e) => e.key === "Enter" && create()} /></label>
      <label className="field"><span>描述（可选）</span>
        <textarea value={desc} onChange={(e) => setDesc(e.target.value)} /></label>
      <label className="field"><span>初始工作路径（可选）</span>
        <input type="text" className="mono" value={path}
          onChange={(e) => { setPath(e.target.value); setPathHint(""); }}
          placeholder="/path/to/目录 — 这条 Workstream 的第 1 条工作路径"
          onKeyDown={(e) => e.key === "Enter" && create()} /></label>
      {pathHint !== "" && (
        <div className="badge warn" style={{ marginBottom: 10 }}>{pathHint}</div>
      )}
      <div className="small muted" style={{ marginBottom: 12 }}>
        这条路径会成为该 Workstream 的<b>主工作路径</b>（新建 Session 默认在这里启动），
        Project 由 NoEnding 从它自动派生 —— 不需要、也不能手工指定。
        留空也完全可以：一条没有路径的 Workstream 是合法状态。
      </div>
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
