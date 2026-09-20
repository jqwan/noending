import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { PreparedLaunch } from "../../types";

type Prepare = (isCurrent: () => boolean) => Promise<PreparedLaunch | null>;
type PrepareOptions = { preserveCurrent?: boolean };

/** Owns one single-use launch preparation and all stale-token cleanup. */
export function usePreparedLaunch(prepareLaunch: Prepare) {
  const [prepared, setPrepared] = useState<PreparedLaunch | null>(null);
  const [preparing, setPreparing] = useState(false);
  const [error, setError] = useState("");
  const preparedRef = useRef<PreparedLaunch | null>(null);
  const sequence = useRef(0);

  const release = useCallback((alreadyReleased = false) => {
    const held = preparedRef.current;
    preparedRef.current = null;
    setPrepared(null);
    if (held && !alreadyReleased) api.cancelPrepared(held.id).catch(console.error);
  }, []);

  const prepare = useCallback(async ({ preserveCurrent = false }: PrepareOptions = {}) => {
    const mine = ++sequence.current;
    if (!preserveCurrent) {
      const held = preparedRef.current;
      preparedRef.current = null;
      setPrepared(null);
      if (held) api.cancelPrepared(held.id).catch(console.error);
    }
    setPreparing(true);
    setError("");
    try {
      const next = await prepareLaunch(() => sequence.current === mine);
      if (!next) return null;
      if (sequence.current !== mine) {
        api.cancelPrepared(next.id).catch(console.error);
        return null;
      }
      const held = preparedRef.current;
      preparedRef.current = next;
      setPrepared(next);
      if (held && held.id !== next.id) {
        api.cancelPrepared(held.id).catch(console.error);
      }
      setError("");
      return next;
    } catch (e: unknown) {
      if (sequence.current === mine) setError(String(e));
      return null;
    } finally {
      if (sequence.current === mine) setPreparing(false);
    }
  }, [prepareLaunch]);

  useEffect(() => {
    void prepare();
    return () => {
      sequence.current += 1;
      const held = preparedRef.current;
      preparedRef.current = null;
      if (held) api.cancelPrepared(held.id).catch(console.error);
    };
  }, [prepare]);

  return { prepared, preparedRef, preparing, error, setError, prepare, release };
}
