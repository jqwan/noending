import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { showToast } from "../../components/Toast";
import type { SummaryDocument, SummaryEditResult, SummaryItem } from "../../types";

type Target = { kind: "session" | "task" | "project"; id: string; revision: number };
type Draft = {
  key: string;
  target: Target;
  base: SummaryDocument;
  index: number | null;
  item: SummaryItem;
};

export function useSummaryEditor({ target, summary, onChange, disabled = false }: {
  target?: Target;
  summary?: SummaryDocument | null;
  onChange: (result: SummaryEditResult, removedIndex?: number) => void;
  disabled?: boolean;
}) {
  const key = target ? `${target.kind}:${target.id}` : "";
  const currentKey = useRef(key);
  const epoch = useRef(0);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [state, setState] = useState<Draft | null>(null);
  const [pending, setPending] = useState("");
  const busy = useRef("");
  const [error, setError] = useState("");
  if (currentKey.current !== key) {
    currentKey.current = key;
    ++epoch.current;
    busy.current = "";
    setState(null);
    setPending("");
    setError("");
  }
  const draft = state?.key === key ? state : null;
  const saving = pending === key && key !== "";
  const available = Boolean(target) && !disabled && !saving;
  const stale = Boolean(draft && target && draft.target.revision !== target.revision);

  const start = (index: number | null) => {
    if (!target || !available || draft) return;
    const base = structuredClone(summary ?? { items: [] });
    const item = index === null ? { title: "", description: "", details: [], title_locked: false } : structuredClone(base.items[index]);
    setState({ key, target: { ...target }, base, index, item });
    setError("");
  };
  const cancel = () => { if (!saving) { setState(null); setError(""); } };
  const save = async (remove = false) => {
    if (!draft || !available || stale || busy.current === key) return;
    const item = {
      ...draft.item, title: draft.item.title.trim(), description: draft.item.description.trim(),
      details: draft.item.details.map(detail => detail.trim()).filter(Boolean),
    };
    if (!remove && (!item.title || /[\r\n]/.test(item.title))) {
      setError("请填写事项标题，标题不能换行。");
      return;
    }
    if (remove && (draft.index === null || draft.item.title_locked)) return;
    const items = [...draft.base.items];
    if (remove) items.splice(draft.index!, 1);
    else if (draft.index === null) items.push(item);
    else items[draft.index] = item;
    const operation = epoch.current;
    busy.current = key;
    setPending(key);
    setError("");
    try {
      const result = await api.saveSummary(draft.target.kind, draft.target.id, draft.target.revision, { items });
      if (!mounted.current || operation !== epoch.current) return;
      onChange(result, remove ? draft.index! : undefined);
      setState(null);
      showToast(remove ? "已删除事项" : "已保存事项");
      window.dispatchEvent(new Event("noending:sync"));
    } catch (failure) {
      if (mounted.current && operation === epoch.current) {
        setError(`保存失败，草稿已保留：${String(failure)}`);
        window.dispatchEvent(new Event("noending:sync"));
      }
    } finally {
      if (operation === epoch.current) { busy.current = ""; if (mounted.current) setPending(""); }
    }
  };
  const toggleTitleLock = async (index: number) => {
    if (!target || !summary || !available || draft || busy.current === key) return;
    const operation = epoch.current;
    busy.current = key;
    setPending(key);
    try {
      const result = await api.setSummaryTitleLock(target.kind, target.id, target.revision, index + 1, !summary.items[index].title_locked);
      if (!mounted.current || operation !== epoch.current) return;
      onChange(result);
      window.dispatchEvent(new Event("noending:sync"));
    } catch (failure) {
      if (mounted.current && operation === epoch.current) {
        showToast(`标题锁定操作失败：${String(failure)}`);
        window.dispatchEvent(new Event("noending:sync"));
      }
    } finally {
      if (operation === epoch.current) { busy.current = ""; if (mounted.current) setPending(""); }
    }
  };
  return {
    draft, saving, available, stale, error, editing: Boolean(draft), start, cancel, save, toggleTitleLock,
    change(item: SummaryItem) { if (!saving) setState(previous => previous?.key === key ? { ...previous, item } : previous); },
  };
}

export type SummaryEditor = ReturnType<typeof useSummaryEditor>;
