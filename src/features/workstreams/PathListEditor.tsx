import { useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import Icon from "../../components/Icon";
import { usePathProbe } from "../../components/WorkspacePathField";
import ExistingPathPicker from "./ExistingPathPicker";
import type { PathProbe } from "../../types";

/** 创建前的路径草稿，第一项为主路径。 */
export interface PathEntryDraft {
  raw: string;
}

export type PathListEditorProps = {
  entries: PathEntryDraft[];
  onChange: (entries: PathEntryDraft[]) => void;
};

/** 一行草稿路径的探测反馈（紧凑版：只有异常与归属，不给整段解释）。 */
function RowFeedback({ probe }: { probe: PathProbe | null }) {
  if (!probe) return null;
  if (probe.status !== "ok") {
    const text =
      probe.status === "reserved"
        ? "NoEnding 自留目录，不能作为工作路径"
        : probe.status === "home"
          ? "不能是用户主目录本身"
          : "无法解析为绝对路径";
    return <span className="badge warn">{text}</span>;
  }
  return (
    <>
      {!probe.exists && <span className="badge warn">目录不存在</span>}
      {probe.git_state === "detected" && (
        <span className="badge">{probe.git_kind === "linked" ? "Git worktree" : "Git 仓库"}</span>
      )}
      {probe.project && (
        <span className="small muted">
          {probe.project.known
            ? <>项目 {probe.project.name ?? probe.project.id}</>
            : <>将创建项目 {probe.project.name}</>}
        </span>
      )}
    </>
  );
}

/** 每行一个组件实例，探测 hook 才有稳定的调用位置（不能在 map 回调里调）。 */
function DraftRow({ raw }: { raw: string }) {
  const probe = usePathProbe(raw, true);
  return (
    <div className="row" style={{ gap: 8, alignItems: "center", flexWrap: "wrap" }}>
      <div className="path-draft-name">
        <strong>{raw.split(/[\\/]/).filter(Boolean).pop() ?? raw}</strong>
        <div className="mono small muted" title={raw}>{raw}</div>
      </div>
      <RowFeedback probe={probe} />
    </div>
  );
}

export default function PathListEditor({ entries, onChange }: PathListEditorProps) {
  const [picking, setPicking] = useState(false);
  const [browsing, setBrowsing] = useState(false);
  const [warn, setWarn] = useState("");
  const editorRef = useRef<HTMLDivElement>(null);
  // 目录选择结束后使用最新列表。
  const entriesRef = useRef(entries);
  entriesRef.current = entries;

  /** 追加目录并跳过重复项。 */
  const addRawPaths = (raws: string[]) => {
    const current = entriesRef.current;
    const known = new Set(current.map((e) => e.raw));
    const fresh: PathEntryDraft[] = [];
    let duplicate = false;
    for (const raw of raws) {
      const trimmed = raw.trim();
      if (trimmed === "") continue;
      if (known.has(trimmed)) {
        duplicate = true;
        const row = Array.from(editorRef.current?.querySelectorAll<HTMLElement>("[data-path]") ?? []).find(el => el.dataset.path === trimmed);
        row?.scrollIntoView?.({ block: "nearest" });
        row?.focus();
        continue;
      }
      known.add(trimmed);
      fresh.push({ raw: trimmed });
    }
    if (fresh.length > 0) {
      onChange([...current, ...fresh]);
      setWarn(duplicate ? "部分路径已在列表里，重复的没有再加。" : "");
    } else if (duplicate) {
      setWarn("这条路径已经在列表里了。");
    }
  };

  const browse = async () => {
    setBrowsing(true);
    setWarn("");
    try {
      const picked = await open({ directory: true, multiple: true, title: "选择工作目录" });
      if (picked) addRawPaths(typeof picked === "string" ? [picked] : picked);
    } catch (error) {
      setWarn(`无法选择目录：${String(error)}。请重试。`);
    } finally {
      setBrowsing(false);
    }
  };

  const remove = (index: number) => {
    onChange(entriesRef.current.filter((_, i) => i !== index));
  };

  const makePrimary = (index: number) => {
    if (index === 0) return;
    const list = [...entriesRef.current];
    const [moved] = list.splice(index, 1);
    list.unshift(moved);
    onChange(list);
  };

  return (
    <div ref={editorRef} className="path-list-editor">
      {entries.length > 0 && (
        <div className="path-draft-list">
          {entries.map((entry, i) => (
            <div
              key={`${entry.raw}-${i}`}
              className="list-row"
              data-path={entry.raw}
              tabIndex={-1}
              style={{ cursor: "default", padding: "6px 4px" }}
            >
              <div className="grow" style={{ minWidth: 0 }}>
                <div className="row" style={{ gap: 6, alignItems: "center", flexWrap: "wrap" }}>
                  {i === 0 ? (
                    <span className="badge accent" title="新建会话默认从这里启动，也决定项目归属">
                      主路径
                    </span>
                  ) : (
                    <span className="badge">第 {i + 1} 条</span>
                  )}
                  <DraftRow raw={entry.raw} />
                </div>
              </div>
              <div className="side row">
                {i > 0 && <button type="button" className="btn small ghost" onClick={() => makePrimary(i)}>设为主要</button>}
                <button type="button" className="btn small ghost icon-only" title="移除路径" aria-label={`移除 ${entry.raw}`} onClick={() => remove(i)}><Icon name="close" /></button>

              </div>
            </div>
          ))}
        </div>
      )}

      <div className="row path-add-actions">
        <button type="button" className="btn small" disabled={browsing} onClick={browse}><Icon name="plus" />{browsing ? "选择中…" : "新增目录"}</button>
        <button type="button" className="btn small" onClick={() => setPicking(true)}><Icon name="folder" />选择已有目录</button>
      </div>
      {picking && <ExistingPathPicker exclude={entries.map(e => e.raw)} onClose={() => setPicking(false)} onSelect={paths => { addRawPaths(paths); setPicking(false); }} />}
      {warn !== "" && (
        <div className="badge warn" style={{ marginTop: 6, display: "block" }}>{warn}</div>
      )}
    </div>
  );
}
