import { useEffect, useRef } from "react";

/**
 * A ref that is true while the component is mounted.
 *
 * Asset loads that are deduplicated across effect runs outlive the run that
 * started them, so they check this instead of a per-run cancelled flag before
 * storing their result.
 */
export function useMountedRef() {
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);
  return mountedRef;
}
