import { useEffect, useLayoutEffect, useRef, useState, type SetStateAction } from "react";

export const viewState = new Map<string, unknown>();

type Validator<T> = (value: unknown) => value is T;

export function preferenceValues<T extends string>(...values: T[]): Validator<T> {
  return (value): value is T => values.some(option => option === value);
}

export const booleanPreference: Validator<boolean> = (value): value is boolean => typeof value === "boolean";
export const booleanRecordPreference: Validator<Record<string, boolean>> = (value): value is Record<string, boolean> =>
  typeof value === "object" && value !== null && !Array.isArray(value) && Object.values(value).every(booleanPreference);

export function readViewPreference<T>(key: string, initial: T, validate: Validator<T>): T {
  const cached = viewState.get(key);
  if (validate(cached)) return cached;
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(`noending.view.${key}`) ?? "null");
    if (validate(saved)) return saved;
  } catch { /* Unavailable storage or obsolete preferences use the default. */ }
  return initial;
}

export function writeViewPreference<T>(key: string, value: T) {
  viewState.set(key, value);
  try {
    localStorage.setItem(`noending.view.${key}`, JSON.stringify(value));
  } catch { /* Keep preferences usable in memory when storage is unavailable. */ }
}

function useStateValue<T>(key: string, initial: T, validate?: Validator<T>) {
  const [value, setValue] = useState<T>(() => validate ? readViewPreference(key, initial, validate)
    : viewState.has(key) ? viewState.get(key) as T : initial);
  const persistent = Boolean(validate);
  useEffect(() => {
    if (!persistent) return;
    writeViewPreference(key, value);
  }, [key, value, persistent]);
  const update = (next: SetStateAction<T>) => setValue(current => {
    const value = typeof next === "function" ? (next as (v: T) => T)(current) : next;
    viewState.set(key, value);
    return value;
  });
  return [value, update] as const;
}

export function useViewState<T>(key: string, initial: T) {
  return useStateValue(key, initial);
}

/** Opt in durable preferences explicitly; transient view state stays in memory. */
export function useViewPreference<T>(key: string, initial: T, validate: Validator<T>) {
  return useStateValue(key, initial, validate);
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
