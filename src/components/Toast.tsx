import { useEffect, useState } from "react";

/** 轻量全局 Toast：New / Resume 成功后只提示结果，「查看详情」作为可选入口。 */
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
    const timers = new Map<number, ReturnType<typeof setTimeout>>();

    const scheduleDismiss = (id: number) => {
      const existingTimer = timers.get(id);
      if (existingTimer) clearTimeout(existingTimer);
      const timer = setTimeout(() => {
        setToasts((curr) => curr.filter((x) => x.id !== id));
        timers.delete(id);
      }, TOAST_MS);
      timers.set(id, timer);
    };

    const l = (t: Toast) => {
      setToasts((ts) => {
        const existing = ts.find(
          (x) => x.text === t.text && x.action?.label === t.action?.label
        );
        if (existing) {
          scheduleDismiss(existing.id);
          return ts;
        }
        scheduleDismiss(t.id);
        return [...ts, t];
      });
    };
    listeners.push(l);
    return () => {
      listeners = listeners.filter((x) => x !== l);
      for (const timer of timers.values()) clearTimeout(timer);
      timers.clear();
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
