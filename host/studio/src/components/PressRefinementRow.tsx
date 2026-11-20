import type React from "react";
import { useCallback, useEffect, useState } from "react";
import type { DaemonClient, PressRefinementState, PressRefinementStatus } from "../api/daemonClient";
import { toDaemonError } from "../api/daemonError";
import type { Job } from "../api/updates";
import { useMountedRef } from "../hooks/useMountedRef";

/** What a keyboard without press-refinement data detects, and what the data is. */
export const PRESS_REFINEMENT_NOT_INSTALLED =
  "Your keyboard's key detection matches the keyboard maker's original 1.4 software. The press-refinement data is the detection refinement of the maker's 1.5 software. OpenFlux can store it on the keyboard; once it's installed, this row shows whether the firmware uses it.";

/** Installed data that this firmware stores without applying. */
export const PRESS_REFINEMENT_INACTIVE = "Installed, but this firmware doesn't use it.";

const STATE_LABELS: Record<PressRefinementState, string> = {
  installed: "Installed",
  absent: "Not installed",
  invalid: "Not installed (the stored data is damaged)",
  storageUnavailable: "Keyboard storage unavailable",
  unknown: "Unknown",
};

/** The calls the row makes. */
export type PressRefinementRowClient = Pick<DaemonClient, "getPressRefinement" | "installPressRefinement" | "pickFile">;

/**
 * The keyboard's press-refinement data: whether it is installed and used, and, when it isn't installed, buttons that
 * install it from the keyboard maker's published software or from a file the user already has.
 * `firmwareConnected` changing reads the state again.
 */
export const PressRefinementRow: React.FC<{ client: PressRefinementRowClient; firmwareConnected: boolean }> = ({
  client,
  firmwareConnected,
}) => {
  const mounted = useMountedRef();
  const [status, setStatus] = useState<PressRefinementStatus | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const s = await client.getPressRefinement();
      if (!mounted.current) return;
      setStatus(s);
      setRefusal(null);
    } catch (e) {
      if (!mounted.current) return;
      setStatus(null);
      setRefusal(toDaemonError(e).message);
    }
  }, [client, mounted]);

  useEffect(() => {
    void firmwareConnected;
    void refresh();
  }, [refresh, firmwareConnected]);

  const install = async (path?: string) => {
    setInstalling(true);
    setResult(null);
    try {
      const done = await client.installPressRefinement(path);
      if (!mounted.current) return;
      setStatus({ state: done.state, crc32: done.crc32, active: done.active });
      setRefusal(null);
      setResult(done.alreadyInstalled ? "The keyboard already had this data." : "Installed and verified.");
    } catch (e) {
      if (!mounted.current) return;
      setResult(`Not installed: ${toDaemonError(e).message}`);
      void refresh();
    } finally {
      if (mounted.current) setInstalling(false);
    }
  };

  const installFromFile = async () => {
    const path = await client.pickFile("Choose the keyboard maker's software package (.msix) or firmware file (.uf2)", [
      "msix",
      "uf2",
    ]);
    if (!path?.trim()) return;
    await install(path.trim());
  };

  const state = status?.state;
  const inactive = state === "installed" && status?.active === false;
  const label = inactive
    ? PRESS_REFINEMENT_INACTIVE
    : state
      ? STATE_LABELS[state]
      : refusal
        ? "Unavailable"
        : "Checking…";
  const canInstall = !!status && state !== "installed" && state !== "storageUnavailable";

  return (
    <section className="press-refinement" aria-label="Press refinement">
      <div className="kv-list" style={{ marginBottom: 6 }}>
        <div>
          <span>Press refinement</span>
          <strong>{label}</strong>
        </div>
      </div>
      {canInstall && <p style={{ fontSize: 12, marginTop: 0 }}>{PRESS_REFINEMENT_NOT_INSTALLED}</p>}
      <p className="muted" style={{ fontSize: 12, marginTop: 0 }}>
        OpenFlux installs press-refinement data from the keyboard maker's published software. Download it or choose a
        copy on your computer.
      </p>
      {refusal && <p className="muted">{refusal}</p>}
      {state === "unknown" && status?.message && <p className="muted">{status.message}</p>}
      {state === "storageUnavailable" && (
        <p className="muted">The keyboard can't store press-refinement data right now; reconnect it and try again.</p>
      )}
      {canInstall && (
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <button type="button" className="btn-secondary" disabled={installing} onClick={() => void install()}>
            Install press-refinement data
          </button>
          <button type="button" className="btn-secondary" disabled={installing} onClick={() => void installFromFile()}>
            Use a file I already have…
          </button>
        </div>
      )}
      {installing && <p className="status-msg">Installing…</p>}
      {result && <p className="status-msg">{result}</p>}
    </section>
  );
};

/** What a finished conversion did about press-refinement data. */
export const PressRefinementOutcome: React.FC<{ outcome: Job["pressRefinement"] }> = ({ outcome }) => {
  if (outcome?.installed && outcome.active === false) {
    return <p>Press-refinement data: {PRESS_REFINEMENT_INACTIVE}</p>;
  }
  if (outcome?.installed) return <p>Press-refinement data is installed.</p>;
  return <p>Press-refinement data isn't installed. {PRESS_REFINEMENT_NOT_INSTALLED} You can install it from Device.</p>;
};
