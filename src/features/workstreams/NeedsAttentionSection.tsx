import React from "react";
import type { ContextConflict } from "../../types";

interface Props {
  conflicts: ContextConflict[];
  onReview: () => void;
}

export default function NeedsAttentionSection({ conflicts, onReview }: Props) {
  const openConflicts = (conflicts ?? []).filter((c) => c.status === "open");
  if (openConflicts.length === 0) return null;

  return (
    <div
      className="card"
      style={{
        marginBottom: 20,
        borderColor: "var(--warning, #eab308)",
        backgroundColor: "rgba(234, 179, 8, 0.06)",
        padding: "12px 14px",
      }}
    >
      <div className="row between" style={{ alignItems: "center" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <span style={{ fontSize: 18, lineHeight: 1, color: "var(--warning, #eab308)" }}>⚠</span>
          <div>
            <div style={{ fontWeight: 600, fontSize: 13, color: "var(--text-primary)" }}>
              {openConflicts.length} 个 Context 冲突待处理
            </div>
            <div className="small muted" style={{ fontSize: 11.5, marginTop: 1 }}>
              存在与当前事实不一致的提取或观察结果
            </div>
          </div>
        </div>
        <button className="btn small primary" onClick={onReview}>
          审查冲突
        </button>
      </div>
    </div>
  );
}
