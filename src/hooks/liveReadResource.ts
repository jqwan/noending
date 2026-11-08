import { useEffect, useRef, useState } from "react";
import { createReadResource } from "./readResource";

type RefreshEvent = { name: string; affects?: (event: Event) => boolean };

/** 多个界面共享事件读取：合并短时间通知，串行补读，隐藏时只记脏。 */
export function createLiveReadResource<T>(read: () => Promise<T>, events: RefreshEvent[]) {
  const stale = Symbol("superseded read");
  const observers = new Map<(failed: boolean) => void, () => boolean>();
  let version = 0;
  let readingVersion = 0;
  let dirty = true;
  let running = false;
  let failed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopEvents: (() => void)[] = [];
  const resource = createReadResource(async () => {
    const expected = readingVersion;
    const value = await read();
    if (expected !== version) throw stale;
    return value;
  });
  const active = () => [...observers.values()].some(enabled => enabled());
  const report = (value: boolean) => {
    failed = value;
    observers.forEach((_, notify) => notify(value));
  };
  const schedule = () => {
    if (timer !== undefined || running || !active() || !dirty) return;
    timer = setTimeout(() => { timer = undefined; refresh(); }, 80);
  };
  const refresh = () => {
    if (running || !active() || !dirty) return;
    dirty = false;
    running = true;
    readingVersion = version;
    const expected = version;
    void resource.refresh("live").then(() => report(false)).catch(error => {
      if (expected !== version || error === stale) return;
      report(true);
      console.error(error);
    }).finally(() => {
      running = false;
      schedule();
    });
  };
  const invalidate = () => {
    ++version;
    dirty = true;
    schedule();
  };
  const observe = (enabled: () => boolean, notify: (failed: boolean) => void) => {
    if (!observers.size) {
      dirty = true;
      failed = false;
      stopEvents = events.map(({ name, affects }) => {
        const listener = (event: Event) => { if (!affects || affects(event)) invalidate(); };
        window.addEventListener(name, listener);
        return () => window.removeEventListener(name, listener);
      });
    }
    observers.set(notify, enabled);
    notify(failed);
    if (enabled()) {
      if (timer !== undefined) { clearTimeout(timer); timer = undefined; }
      refresh();
    }
    return () => {
      observers.delete(notify);
      if (!active() && timer !== undefined) { clearTimeout(timer); timer = undefined; }
      if (!observers.size) {
        stopEvents.forEach(stop => stop());
        stopEvents = [];
        ++version;
        dirty = true;
      }
    };
  };
  function useValue(enabled = true) {
    const value = resource.useValue("live");
    const [failedRead, setFailed] = useState(failed);
    const enabledRef = useRef(enabled);
    useEffect(() => observe(() => enabledRef.current, setFailed), []);
    useEffect(() => {
      enabledRef.current = enabled;
      if (!active() && timer !== undefined) { clearTimeout(timer); timer = undefined; }
      if (enabled) {
        if (timer !== undefined) { clearTimeout(timer); timer = undefined; }
        refresh();
      }
    }, [enabled]);
    return { value, loading: value === undefined && !failedRead, failed: failedRead };
  }
  return { useValue };
}
