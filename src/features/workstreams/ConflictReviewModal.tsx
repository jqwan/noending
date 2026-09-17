import React, { useState } from "react";
import { api } from "../../api";
import { Modal, timeAgo } from "../../components/common";
import {
  AUTHORITY_LABELS,
  type ContextConflict,
  type ContextItem,
  type ContextItemRevision,
} from "../../types";

interface Props {
  conflicts: ContextConflict[];
  items: [ContextItem, ContextItemRevision][];
  onClose: () => void;
  onChanged: () => void;
}

export default function ConflictReviewModal({
  conflicts,
  items,
  onClose,
  onChanged,
}: Props) {
  const openConflicts = conflicts.filter((c) => c.status === "open");
  const [currentIndex, setCurrentIndex] = useState(0);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  // Inline editing state for left (current) item
  const [editingLeft, setEditingLeft] = useState(false);
  const [editTitle, setEditTitle] = useState("");
  const [editContent, setEditContent] = useState("");

  const itemsById = new Map(items.map(([item, rev]) => [item.id, [item, rev] as const]));

  const currentConflict = openConflicts[currentIndex];
  const leftPair = currentConflict ? itemsById.get(currentConflict.left_item_id) : null;
  const leftItem = leftPair ? leftPair[0] : null;
  const leftRev = leftPair ? leftPair[1] : null;

  const rightPair = currentConflict?.right_item_id
    ? itemsById.get(currentConflict.right_item_id)
    : null;
  const rightItem = rightPair ? rightPair[0] : null;
  const rightRev = rightPair ? rightPair[1] : null;

  let candidateSnapshot: {
    title?: string;
    content?: string;
    authority?: string;
    source_refs?: string[];
  } | null = null;
  if (currentConflict?.candidate_snapshot_json) {
    try {
      candidateSnapshot = JSON.parse(currentConflict.candidate_snapshot_json);
    } catch {
      // ignore
    }
  }

  const startEditLeft = () => {
    if (!leftRev) return;
    setEditTitle(leftRev.title);
    setEditContent(leftRev.content);
    setEditingLeft(true);
  };

  const handleResolve = async (status: "resolved" | "dismissed") => {
    if (!currentConflict || busy) return;
    setBusy(true);
    setError("");
    try {
      const edit =
        editingLeft && status === "resolved"
          ? {
              title: editTitle.trim(),
              content: editContent,
            }
          : undefined;

      if (editingLeft && status === "resolved" && !editTitle.trim()) {
        setError("标题不能为空");
        setBusy(false);
        return;
      }

      await api.resolveConflictWithEdit(
        currentConflict.id,
        status,
        note.trim() || undefined,
        edit,
      );
      setNote("");
      setEditingLeft(false);
      onChanged();
      if (currentIndex >= openConflicts.length - 1) {
        setCurrentIndex(Math.max(0, openConflicts.length - 2));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="Context 冲突审查 (Conflict Review)" onClose={onClose}>
      {openConflicts.length === 0 ? (
        <div style={{ textAlign: "center", padding: "24px 0" }}>
          <div style={{ fontSize: 24, marginBottom: 8 }}>✓</div>
          <div style={{ fontWeight: 550, marginBottom: 4 }}>所有冲突已处理完毕</div>
          <div className="muted small" style={{ marginBottom: 20 }}>
            当前事实与会话提取保持一致。
          </div>
          <button className="btn" onClick={onClose}>
            关闭
          </button>
        </div>
      ) : (
        <div>
          {/* Conflict switcher if more than one */}
          {openConflicts.length > 1 && (
            <div
              className="row"
              style={{
                gap: 6,
                marginBottom: 16,
                overflowX: "auto",
                paddingBottom: 4,
              }}
            >
              {openConflicts.map((c, idx) => (
                <button
                  key={c.id}
                  className={`btn small ${idx === currentIndex ? "primary" : "ghost"}`}
                  onClick={() => {
                    setCurrentIndex(idx);
                    setEditingLeft(false);
                    setError("");
                  }}
                >
                  冲突 #{idx + 1}
                </button>
              ))}
            </div>
          )}

          {error && <div className="badge warn" style={{ marginBottom: 12 }}>{error}</div>}

          {/* Side-by-side comparison */}
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "1fr 1fr",
              gap: 16,
              marginBottom: 16,
            }}
          >
            {/* Left: Current Fact */}
            <div
              className="card"
              style={{
                padding: "14px",
                borderColor: "var(--border-default)",
                background: "var(--bg-app)",
              }}
            >
              <div className="row between" style={{ marginBottom: 8 }}>
                <span className="section-label" style={{ margin: 0 }}>当前事实 (Current Fact)</span>
                {leftItem && (
                  <span className="badge accent" style={{ fontSize: 10 }}>
                    {AUTHORITY_LABELS[leftItem.authority] ?? leftItem.authority}
                  </span>
                )}
              </div>

              {editingLeft ? (
                <div>
                  <label className="field" style={{ marginBottom: 6 }}>
                    <span>标题</span>
                    <input
                      type="text"
                      value={editTitle}
                      autoFocus
                      onChange={(e) => setEditTitle(e.target.value)}
                    />
                  </label>
                  <label className="field" style={{ marginBottom: 8 }}>
                    <span>内容</span>
                    <textarea
                      value={editContent}
                      rows={4}
                      onChange={(e) => setEditContent(e.target.value)}
                    />
                  </label>
                  <button
                    className="link small"
                    onClick={() => setEditingLeft(false)}
                  >
                    取消编辑
                  </button>
                </div>
              ) : (
                <div>
                  <div style={{ fontWeight: 600, fontSize: 13.5, marginBottom: 4 }}>
                    {leftRev?.title ?? currentConflict.left_item_id}
                  </div>
                  {leftRev?.content && (
                    <div
                      className="small"
                      style={{
                        whiteSpace: "pre-wrap",
                        color: "var(--text-secondary)",
                        marginBottom: 10,
                        lineHeight: 1.45,
                      }}
                    >
                      {leftRev.content}
                    </div>
                  )}
                  <div className="row between" style={{ marginTop: 8 }}>
                    <span className="small muted">
                      {leftItem ? timeAgo(leftItem.updated_at) : ""}
                    </span>
                    <button className="link small" onClick={startEditLeft}>
                      ✎ 修正内容
                    </button>
                  </div>
                </div>
              )}
            </div>

            {/* Right: Conflicting Evidence */}
            <div
              className="card"
              style={{
                padding: "14px",
                borderColor: "var(--warning, #eab308)",
                background: "var(--bg-app)",
              }}
            >
              <div className="row between" style={{ marginBottom: 8 }}>
                <span className="section-label" style={{ margin: 0, color: "var(--warning, #eab308)" }}>
                  冲突证据 (Agent Evidence)
                </span>
                {rightItem ? (
                  <span className="badge" style={{ fontSize: 10 }}>
                    {AUTHORITY_LABELS[rightItem.authority] ?? rightItem.authority}
                  </span>
                ) : candidateSnapshot?.authority ? (
                  <span className="badge" style={{ fontSize: 10 }}>
                    提议: {AUTHORITY_LABELS[candidateSnapshot.authority] ?? candidateSnapshot.authority}
                  </span>
                ) : null}
              </div>

              {rightRev ? (
                <div>
                  <div style={{ fontWeight: 600, fontSize: 13.5, marginBottom: 4 }}>
                    {rightRev.title}
                  </div>
                  <div
                    className="small"
                    style={{
                      whiteSpace: "pre-wrap",
                      color: "var(--text-secondary)",
                      marginBottom: 10,
                      lineHeight: 1.45,
                    }}
                  >
                    {rightRev.content}
                  </div>
                  <div className="small muted">
                    {timeAgo(rightRev.created_at)}
                    {rightRev.source_ref ? ` · ${rightRev.source_ref}` : ""}
                  </div>
                </div>
              ) : candidateSnapshot ? (
                <div>
                  <div style={{ fontWeight: 600, fontSize: 13.5, marginBottom: 4 }}>
                    {candidateSnapshot.title ?? "未命名提议"}
                  </div>
                  {candidateSnapshot.content && (
                    <div
                      className="small"
                      style={{
                        whiteSpace: "pre-wrap",
                        color: "var(--text-secondary)",
                        marginBottom: 10,
                        lineHeight: 1.45,
                      }}
                    >
                      {candidateSnapshot.content}
                    </div>
                  )}
                  <div className="small muted">
                    Agent 提取提议（已被权威策略拦截未生效）
                    {candidateSnapshot.source_refs?.length
                      ? ` · ${candidateSnapshot.source_refs.join(", ")}`
                      : ""}
                  </div>
                </div>
              ) : (
                <div className="muted small">
                  {currentConflict.conflict_type === "authority"
                    ? "Agent 尝试修改或完成已被用户保护的事实。"
                    : "未找到右侧条目详情。"}
                </div>
              )}
            </div>
          </div>

          {/* Audit Resolution Form */}
          <div style={{ marginTop: 14 }}>
            <label className="field" style={{ marginBottom: 12 }}>
              <span>裁决说明 (Audit Resolution Note)</span>
              <input
                type="text"
                value={note}
                placeholder="填写处理理由（如：已与团队确认按当前事实执行 / 采纳 Agent 新发现）"
                onChange={(e) => setNote(e.target.value)}
              />
            </label>

            <div className="row between" style={{ marginTop: 16 }}>
              <button className="btn ghost" onClick={onClose}>
                稍后处理
              </button>
              <div className="row" style={{ gap: 8 }}>
                <button
                  className="btn"
                  disabled={busy}
                  title="维持当前事实，忽略该冲突提取"
                  onClick={() => handleResolve("dismissed")}
                >
                  忽略冲突（维持现状）
                </button>
                <button
                  className="btn primary"
                  disabled={busy}
                  title="标记此冲突已解决并形成审计记录"
                  onClick={() => handleResolve("resolved")}
                >
                  {editingLeft ? "保存修正并解决" : "标记为已解决"}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </Modal>
  );
}
