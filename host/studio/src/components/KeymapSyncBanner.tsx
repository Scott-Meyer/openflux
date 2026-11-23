import type React from "react";
import { useState } from "react";
import { type DaemonError, toDaemonError } from "../api/daemonError";
import type { KeymapSyncNotice } from "../types/openflux";

interface KeymapSyncBannerProps {
  notice: KeymapSyncNotice;
  /** What the banner says while the daemon reads the keymap from the keyboard. */
  syncingText: string;
  /** Rebuilds the keymap file from the keyboard's firmware keymap; rejects with the daemon's refusal. */
  onAdoptFirmware: () => Promise<void>;
}

function withReason(text: string, reason: string | null): string {
  return reason ? `${text}: ${reason}` : `${text}.`;
}

/** Explains why keymap editing waits for the keyboard, and offers a way out when it can't load. */
export const KeymapSyncBanner: React.FC<KeymapSyncBannerProps> = ({ notice, syncingText, onAdoptFirmware }) => {
  const [adopting, setAdopting] = useState(false);
  const [adoptError, setAdoptError] = useState<DaemonError | null>(null);

  if (notice.state === "pulling") {
    return (
      <div className="topbar-banner" style={{ marginBottom: 10, borderRadius: 8 }} aria-live="polite">
        {syncingText}
      </div>
    );
  }

  if (notice.state === "disconnected") {
    return (
      <div className="notice notice-warn" aria-live="polite">
        The keyboard isn't connected. Its keymap loads when you connect it.
      </div>
    );
  }

  const adopt = async () => {
    setAdopting(true);
    setAdoptError(null);
    try {
      await onAdoptFirmware();
    } catch (e) {
      setAdoptError(toDaemonError(e));
    } finally {
      setAdopting(false);
    }
  };

  return (
    <div className="notice notice-warn" aria-live="polite">
      <strong>
        {notice.state === "mismatch"
          ? withReason("The keyboard's keymap file and its firmware keymap don't match", notice.reason)
          : withReason("Keyglass couldn't load the keyboard's keymap", notice.reason)}
      </strong>
      <p>
        Keymap editing stays locked until this is resolved. Using the firmware keymap rebuilds the keymap file from what
        the keyboard actually does.
      </p>
      <p>
        <button type="button" className="btn-secondary" disabled={adopting} onClick={() => void adopt()}>
          {adopting ? "Using the keyboard's firmware keymap…" : "Use the keyboard's firmware keymap"}
        </button>
      </p>
      {adoptError && (
        <div className="notice notice-error" role="alert">
          <p>{adoptError.message}</p>
          {adoptError.recovery && <p className="notice-recovery">{adoptError.recovery}</p>}
        </div>
      )}
    </div>
  );
};
