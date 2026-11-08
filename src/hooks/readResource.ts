import { useCallback, useSyncExternalStore } from "react";

const resources = new Set<(key?: string) => void>();

/** 只缓存读取快照；重新读取期间保留已有内容，错误由调用方展示。 */
export function createReadResource<T>(read: (key: string) => Promise<T>) {
  const values = new Map<string, T>();
  const listeners = new Map<string, Set<() => void>>();
  const pending = new Map<string, Promise<T>>();
  const notify = (key: string) => listeners.get(key)?.forEach(listener => listener());
  const get = (key: string) => values.get(key);
  const set = (key: string, value: T) => {
    pending.delete(key);
    values.set(key, value);
    notify(key);
  };
  const clear = (key?: string) => {
    if (key !== undefined) {
      pending.delete(key); values.delete(key); notify(key);
    } else {
      pending.clear(); values.clear(); listeners.forEach((_, id) => notify(id));
    }
  };
  resources.add(clear);
  const refresh = (key: string): Promise<T> => {
    const request = Promise.resolve().then(() => read(key)).then(value => {
      if (pending.get(key) === request) {
        values.set(key, value);
        notify(key);
      }
      return value;
    }).finally(() => { if (pending.get(key) === request) pending.delete(key); });
    pending.set(key, request);
    return request;
  };
  const ensure = (key: string): Promise<void> | undefined => {
    if (values.has(key)) return;
    return (pending.get(key) ?? refresh(key)).then(() => {});
  };
  const revalidate = (key: string): Promise<T> => pending.get(key) ?? refresh(key);
  function useValue(key: string) {
    return useSyncExternalStore(useCallback(listener => {
      const subscribers = listeners.get(key) ?? new Set();
      subscribers.add(listener); listeners.set(key, subscribers);
      return () => { subscribers.delete(listener); if (!subscribers.size) listeners.delete(key); };
    }, [key]), useCallback(() => get(key), [key]));
  }
  return { get, set, clear, refresh, revalidate, ensure, useValue };
}

/** 删除对象或清理工作区缓存时，同时使迟到的读取失效。 */
export function clearReadResources(keys?: string[]) {
  for (const clear of resources) {
    if (keys) keys.forEach(key => clear(key));
    else clear();
  }
}
