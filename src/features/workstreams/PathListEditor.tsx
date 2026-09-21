import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import WorkspacePathField, { absolutePathHint, usePathProbe } from "../../components/WorkspacePathField";
import type { PathProbe } from "../../types";

/**
 * 有序工作路径列表的「创建时编辑」形态（方案 §1.5）：第 1 条是主工作路径，
 * 角色完全由位置表达。与详情页 `WorkstreamPathList` 共享同一套语义——
 * 设为主路径 = 移到第 1 位，上下移 = 整表 swap，移除 = 剩余条目位置前移——
 * 但这里是**提交前的草稿**，不动任何数据库行；权威判定仍发生在创建那一刻。
 *
 * 除了手输，还接受两类快捷输入：
 * - 「浏览…」/ 快选列表（`WorkspacePathField` 内部）；
 * - 从 Finder / 资源管理器把文件夹拖进来（Tauri webview drag-drop）。
 */

export interface PathEntryDraft {
  raw: string;
}

export type PathListEditorProps = {
  entries: PathEntryDraft[];
  onChange: (entries: PathEntryDraft[]) => void;
  addPlaceholder?: string;
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
      <span className="mono" style={{ overflowWrap: "anywhere", flex: 1, minWidth: 120 }} title={raw}>
        {raw}
      </span>
      <RowFeedback probe={probe} />
    </div>
  );
}

export default function PathListEditor({ entries, onChange, addPlaceholder }: PathListEditorProps) {
  const [addValue, setAddValue] = useState("");
  const [warn, setWarn] = useState("");
  const [dragOver, setDragOver] = useState(false);
  // 拖拽回调是订阅一次的闭包，读 entries 要走 ref，否则永远看到第一帧。
  const entriesRef = useRef(entries);
  entriesRef.current = entries;

  /** 追加若干条（浏览/快选/拖拽/手输共用）；已存在的直接指出，不静默吞掉。 */
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

  const append = () => {
    const raw = addValue.trim();
    const hint = absolutePathHint(raw);
    if (raw !== "" && hint !== "") {
      setWarn(hint);
      return;
    }
    setWarn("");
    if (raw === "") return;
    addRawPaths([raw]);
    setAddValue("");
  };

  const remove = (index: number) => {
    onChange(entriesRef.current.filter((_, i) => i !== index));
  };

  const move = (index: number, delta: -1 | 1) => {
    const list = [...entriesRef.current];
    const to = index + delta;
    if (to < 0 || to >= list.length) return;
    const [moved] = list.splice(index, 1);
    list.splice(to, 0, moved);
    onChange(list);
  };

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter") {
          setDragOver(true);
        } else if (event.payload.type === "leave") {
          setDragOver(false);
        } else if (event.payload.type === "drop") {
          setDragOver(false);
          if (event.payload.paths.length > 0) addRawPaths(event.payload.paths);
        }
      })
      .then((u) => {
        if (disposed) u();
        else unlisten = u;
      })
      .catch(() => {
        // 非 Tauri 环境（单测/纯浏览器）没有拖拽事件，输入框仍然完整可用。
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div
      style={{
        border: dragOver ? "1px dashed var(--accent, #888)" : "1px dashed transparent",
        borderRadius: 6,
        padding: 2,
        margin: -3,
      }}
    >
      {entries.length > 0 && (
        <div style={{ marginBottom: 8 }}>
          {entries.map((entry, i) => (
            <div
              key={`${entry.raw}-${i}`}
              className="list-row"
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
              <div className="side">
                <button className="btn small" disabled={i === 0} title="上移一位"
                  onClick={() => move(i, -1)}>↑</button>
                <button className="btn small" disabled={i === entries.length - 1} title="下移一位"
                  onClick={() => move(i, 1)}>↓</button>
                <button className="btn small" title="从列表里拿掉这一条"
                  onClick={() => remove(i)}>移除</button>
              </div>
            </div>
          ))}
        </div>
      )}

      <WorkspacePathField
        value={addValue}
        onChange={setAddValue}
        onSubmit={append}
        placeholder={addPlaceholder ?? "/path/to/目录 — 回车或点「添加」加入列表"}
        exclude={entries.map((e) => e.raw)}
      />
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 6 }}>
        <button className="btn small" disabled={addValue.trim() === ""} onClick={append}>
          添加
        </button>
      </div>
      {warn !== "" && (
        <div className="badge warn" style={{ marginTop: 6, display: "block" }}>{warn}</div>
      )}
      <div className="small muted" style={{ marginTop: 6 }}>
        第 1 条是<b>主工作路径</b>：新建会话默认在这里启动，项目也由路径派生
        —— 不需要、也不能手工指定。可以把文件夹直接拖进来。全部留空也合法：
        一条没有路径的任务依然有效。
      </div>
    </div>
  );
}
