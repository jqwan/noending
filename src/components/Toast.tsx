import { useEffect, useState } from "react";

/**
 * 轻量全局 Toast：New / Resume 成功后不再强制弹 LaunchResultModal，
 * 只提示结果，「查看详情」作为可选入口打开完整启动信息。
 */
export interface ToastAction {
  label: string;
  onClick: () => void;
}

interface Toast {
  id: number;
  text: string;
  action?: ToastAction;
}

const TOAST_MS = 5000;

let nextId = 1;
let listeners: ((t: Toast) => void)[] = [];

export function showToast(text: string, action?: ToastAction) {
  const t: Toast = { id: nextId++, text, action };
  for (const l of listeners) l(t);
}

export default function ToastHost() {
  const [toasts, setToasts] = useState<Toast[]>([]);

  useEffect(() => {
    const l = (t: Toast) => {
      setToasts((ts) => [...ts, t]);
      setTimeout(() => {
        setToasts((ts) => ts.filter((x) => x.id !== t.id));
      }, TOAST_MS);
    };
    listeners.push(l);
    return () => {
      listeners = listeners.filter((x) => x !== l);
    };
  }, []);

  if (toasts.length === 0) return null;

  return (
    <div className="toast-host">
      {toasts.map((t) => (
        <div key={t.id} className="toast">
          <span>{t.text}</span>
          {t.action && (
            <button className="link" onClick={t.action.onClick}>
              {t.action.label}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
