import { useLayoutEffect, useRef, useState, type SetStateAction } from "react";

export const viewState = new Map<string, unknown>();

export function useViewState<T>(key: string, initial: T) {
  const [value, setValue] = useState<T>(() => viewState.has(key) ? viewState.get(key) as T : initial);
  const update = (next: SetStateAction<T>) => setValue(current => {
    const value = typeof next === "function" ? (next as (v: T) => T)(current) : next;
    viewState.set(key, value);
    return value;
  });
  return [value, update] as const;
}

export function useViewScroll(key: string, ready: boolean) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const node = ref.current;
    if (!node || !ready) return;
    node.scrollTop = Number(viewState.get(key) ?? 0);
    const save = () => viewState.set(key, node.scrollTop);
    node.addEventListener("scroll", save);
    return () => node.removeEventListener("scroll", save);
  }, [key, ready]);
  return ref;
}
