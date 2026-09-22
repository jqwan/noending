import { useEffect, useState } from "react";
import { api } from "../../api";
import { Modal } from "../../components/common";
import type { RecentWorkspacePath } from "../../types";

export default function ExistingPathPicker({ exclude, onClose, onSelect }: {
  exclude: string[];
  onClose: () => void;
  onSelect: (paths: string[]) => void;
}) {
  const [paths, setPaths] = useState<RecentWorkspacePath[] | null>(null);
  const [error, setError] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    setError(false);
    api.listRecentWorkspacePaths().then(rows => { if (active) setPaths(rows); })
      .catch(() => { if (active) setError(true); });
    return () => { active = false; };
  }, [attempt]);
  return (
    <Modal title="选择已有目录" onClose={onClose}>
      <div className="existing-path-list" role="group" aria-label="已有目录">
        {error ? <div role="alert">读取目录失败 <button className="btn small" onClick={() => setAttempt(n => n + 1)}>重试</button></div>
          : !paths ? <div role="status">加载中…</div>
          : paths.length === 0 ? <div className="muted">暂无已有目录</div>
          : paths.map(p => {
            const added = exclude.includes(p.path);
            return <label className={`existing-path-option${added ? " is-added" : ""}`} key={p.path}>
              <input type="checkbox" disabled={added} checked={added || selected.includes(p.path)} onChange={e => setSelected(old => e.target.checked ? [...old, p.path] : old.filter(path => path !== p.path))} />
              <span className="existing-path-info mono" title={p.path}>{p.path}</span>
            </label>;
          })}
      </div>
      <div className="row" style={{ justifyContent: "flex-end" }}>
        <button className="btn" onClick={onClose}>取消</button>
        <button className="btn primary" disabled={!selected.length} onClick={() => onSelect(selected)}>添加{selected.length ? ` (${selected.length})` : ""}</button>
      </div>
    </Modal>
  );
}
