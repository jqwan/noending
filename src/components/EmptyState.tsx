import React from "react";

/** 统一空状态（整体设计方案 §77）：说明 + 具体下一步动作。 */
export default function EmptyState({ title, hint, actions }: {
  title: string;
  hint?: string;
  actions?: React.ReactNode;
}) {
  return (
    <div className="empty" style={{ padding: "40px 8px" }}>
      <div style={{ fontWeight: 550, color: "var(--text-secondary)", marginBottom: 4 }}>{title}</div>
      {hint && <div className="small">{hint}</div>}
      {actions && <div className="invite">{actions}</div>}
    </div>
  );
}
