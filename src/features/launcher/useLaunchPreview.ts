import { useCallback, useEffect, useRef, useState } from "react";
import type { CwdResolution } from "../../types";

export function useLaunchPreview(loadPreview: () => Promise<CwdResolution | null>) {
  const [preview, setPreview] = useState<CwdResolution | null>(null);
  const [preparing, setPreparing] = useState(false);
  const [error, setError] = useState("");
  const sequence = useRef(0);

  const clear = useCallback(() => {
    sequence.current += 1;
    setPreview(null);
    setPreparing(false);
  }, []);

  const refresh = useCallback(async () => {
    const mine = ++sequence.current;
    setPreview(null);
    setPreparing(true);
    setError("");
    try {
      const next = await loadPreview();
      if (sequence.current === mine) setPreview(next);
    } catch (e: unknown) {
      if (sequence.current === mine) setError(String(e));
    } finally {
      if (sequence.current === mine) setPreparing(false);
    }
  }, [loadPreview]);

  useEffect(() => {
    void refresh();
    return () => {
      sequence.current += 1;
    };
  }, [refresh]);

  return { preview, preparing, error, setError, refresh, clear };
}
