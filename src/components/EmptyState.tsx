
/** 统一空状态（整体设计）：说明 + 具体下一步动作。 */
export default function EmptyState({ title, hint, actions }: {
  title: string;
  hint?: string;
  actions?: React.ReactNode;
}) {
  return (
    <div className="empty">
      <div className="empty-title">{title}</div>
      {hint && <div className="small">{hint}</div>}
      {actions && <div className="invite">{actions}</div>}
    </div>
  );
}
