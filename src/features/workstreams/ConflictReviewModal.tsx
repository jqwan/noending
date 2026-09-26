import React, { useState } from "react";
import { api } from "../../api";
import { Modal, timeAgo } from "../../components/common";
import {
  AUTHORITY_LABELS,
  type ConflictReviewCase,
} from "../../types";

interface Props {
  cases: ConflictReviewCase[];
  initialConflictId?: string;
  onClose: () => void;
  onChanged: () => void;
}

export default function ConflictReviewModal({
  cases,
  initialConflictId,
  onClose,
  onChanged,
}: Props) {
  const openCases = cases.filter((c) => c.conflict.status === "open");
  const [currentIndex, setCurrentIndex] = useState(() => {
    if (initialConflictId) {
      const idx = openCases.findIndex((c) => c.conflict.id === initialConflictId);
      if (idx >= 0) return idx;
    }
    return 0;
  });

  React.useEffect(() => {
    if (initialConflictId) {
      const idx = openCases.findIndex((c) => c.conflict.id === initialConflictId);
      if (idx >= 0) setCurrentIndex(idx);
    }
  }, [initialConflictId]);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const [editingLeft, setEditingLeft] = useState(false);
  const [editTitle, setEditTitle] = useState("");
  const [editContent, setEditContent] = useState("");

  const currentCase = openCases[currentIndex];

  const startEditLeft = () => {
    const base = currentCase?.current_left ?? currentCase?.left_at_conflict;
    if (!base) return;
    setEditTitle(base.title);
    setEditContent(base.content);
    setEditingLeft(true);
  };

  const handleResolve = async (status: "resolved" | "dismissed") => {
    if (!currentCase || busy) return;
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
        currentCase.conflict.id,
        status,
        note.trim() || undefined,
        edit,
      );
      setNote("");
      setEditingLeft(false);
      onChanged();
      if (currentIndex >= openCases.length - 1) {
        setCurrentIndex(Math.max(0, openCases.length - 2));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="Context 冲突审查 (Conflict Review)" onClose={onClose}>
      {openCases.length === 0 ? (
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
          {openCases.length > 1 && (
            <div
              className="row"
              style={{
                gap: 6,
                marginBottom: 16,
                overflowX: "auto",
                paddingBottom: 4,
              }}
            >
              {openCases.map((c, idx) => (
                <button
                  key={c.conflict.id}
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
            {/* Left: Fact at Conflict */}
            <div
              className="card"
              style={{
                padding: "14px",
                borderColor: "var(--border-default)",
                background: "var(--bg-app)",
              }}
            >
              <div className="row between" style={{ marginBottom: 8 }}>
                <span className="section-label" style={{ margin: 0 }}>
                  冲突时事实 (Fact at Conflict)
                </span>
                {currentCase.left_at_conflict && (
                  <span className="badge accent" style={{ fontSize: 10 }}>
                    {AUTHORITY_LABELS[currentCase.left_at_conflict.authority] ??
                      currentCase.left_at_conflict.authority}
                  </span>
                )}
              </div>

              {editingLeft ? (
                <div>
                  <label className="field" style={{ marginBottom: 6 }}>
                    <span>标题 (修正当前事实)</span>
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
                    {currentCase.left_at_conflict?.title ?? currentCase.conflict.left_item_id}
                  </div>
                  {currentCase.left_at_conflict?.content && (
                    <div
                      className="small"
                      style={{
                        whiteSpace: "pre-wrap",
                        color: "var(--text-secondary)",
                        marginBottom: 10,
                        lineHeight: 1.45,
                      }}
                    >
                      {currentCase.left_at_conflict.content}
                    </div>
                  )}
                  <div className="row between" style={{ marginTop: 8 }}>
                    <span className="small muted">
                      {currentCase.left_at_conflict
                        ? timeAgo(currentCase.left_at_conflict.created_at)
                        : ""}
                    </span>
                    <button className="link small" onClick={startEditLeft}>
                      ✎ 修正当前事实
                    </button>
                  </div>

                  {/* Warning if current fact evolved since conflict */}
                  {currentCase.left_changed_since_conflict && currentCase.current_left && (
                    <div
                      style={{
                        marginTop: 12,
                        padding: "8px 10px",
                        borderRadius: 6,
                        background: "rgba(59, 130, 246, 0.08)",
                        border: "1px solid rgba(59, 130, 246, 0.25)",
                      }}
                    >
                      <div
                        className="small"
                        style={{
                          fontWeight: 600,
                          color: "var(--accent, #3b82f6)",
                          marginBottom: 4,
                        }}
                      >
                        ⚠ 当前事实在此冲突后已发生变化
                      </div>
                      <div className="small" style={{ fontWeight: 550, marginBottom: 2 }}>
                        {currentCase.current_left.title}
                      </div>
                      {currentCase.current_left.content && (
                        <div
                          className="small muted"
                          style={{
                            whiteSpace: "pre-wrap",
                            marginBottom: 4,
                            fontSize: 11.5,
                          }}
                        >
                          {currentCase.current_left.content}
                        </div>
                      )}
                      <div className="small muted" style={{ fontSize: 11 }}>
                        最新修订: {timeAgo(currentCase.current_left.created_at)} · 权威:{" "}
                        {AUTHORITY_LABELS[currentCase.current_left.authority] ??
                          currentCase.current_left.authority}
                      </div>
                    </div>
                  )}
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
                <span
                  className="section-label"
                  style={{ margin: 0, color: "var(--warning, #eab308)" }}
                >
                  冲突证据 (Conflicting Evidence)
                </span>
                {currentCase.right_at_conflict ? (
                  <span className="badge" style={{ fontSize: 10 }}>
                    {AUTHORITY_LABELS[currentCase.right_at_conflict.authority] ??
                      currentCase.right_at_conflict.authority}
                  </span>
                ) : currentCase.candidate_at_conflict ? (
                  <span className="badge" style={{ fontSize: 10 }}>
                    提议:{" "}
                    {AUTHORITY_LABELS[currentCase.candidate_at_conflict.authority] ??
                      currentCase.candidate_at_conflict.authority}
                  </span>
                ) : null}
              </div>

              {currentCase.right_at_conflict ? (
                <div>
                  <div style={{ fontWeight: 600, fontSize: 13.5, marginBottom: 4 }}>
                    {currentCase.right_at_conflict.title}
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
                    {currentCase.right_at_conflict.content}
                  </div>
                  <div className="small muted">
                    {timeAgo(currentCase.right_at_conflict.created_at)}
                    {currentCase.right_at_conflict.source_ref
                      ? ` · ${currentCase.right_at_conflict.source_ref}`
                      : ""}
                  </div>

                  {currentCase.right_changed_since_conflict && currentCase.current_right && (
                    <div
                      style={{
                        marginTop: 12,
                        padding: "8px 10px",
                        borderRadius: 6,
                        background: "rgba(234, 179, 8, 0.08)",
                        border: "1px solid rgba(234, 179, 8, 0.25)",
                      }}
                    >
                      <div
                        className="small"
                        style={{
                          fontWeight: 600,
                          color: "var(--warning, #eab308)",
                          marginBottom: 4,
                        }}
                      >
                        ⚠ 右侧条目在此冲突后也已演进
                      </div>
                      <div className="small" style={{ fontWeight: 550, marginBottom: 2 }}>
                        {currentCase.current_right.title}
                      </div>
                      <div className="small muted" style={{ fontSize: 11 }}>
                        最新修订: {timeAgo(currentCase.current_right.created_at)}
                      </div>
                    </div>
                  )}
                </div>
              ) : currentCase.candidate_at_conflict ? (
                <div>
                  <div style={{ fontWeight: 600, fontSize: 13.5, marginBottom: 4 }}>
                    {currentCase.candidate_at_conflict.title ?? "未命名提议"}
                  </div>
                  {currentCase.candidate_at_conflict.content && (
                    <div
                      className="small"
                      style={{
                        whiteSpace: "pre-wrap",
                        color: "var(--text-secondary)",
                        marginBottom: 10,
                        lineHeight: 1.45,
                      }}
                    >
                      {currentCase.candidate_at_conflict.content}
                    </div>
                  )}
                  <div className="small muted">
                    Agent 提取提议（已被权威策略拦截未生效）
                    {currentCase.candidate_at_conflict.source_refs?.length
                      ? ` · ${currentCase.candidate_at_conflict.source_refs.join(", ")}`
                      : ""}
                  </div>
                </div>
              ) : (
                <div className="muted small">
                  {currentCase.conflict.conflict_type === "authority"
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
