import type React from "react";
import { useState } from "react";
import type { DaemonClient } from "../api/daemonClient";
import type { AutoProfileSwitch } from "../types/openflux";

/** What the button says about automatic profile switching. */
function describe(state: AutoProfileSwitch): { label: string; title: string } {
  if (state.locked) {
    return {
      label: "Auto-switch locked",
      title: "App rules don't switch the profile. Click to let them switch again.",
    };
  }
  if (state.heldUntilAppChange) {
    return {
      label: "Auto-switch held",
      title:
        "A profile was picked on the keyboard, so app rules wait until the app in front changes. Click to lock automatic switching.",
    };
  }
  return { label: "Auto-switch on", title: "App rules switch the profile. Click to lock it." };
}

/**
 * Shows whether app rules may switch the profile (on, held until the app changes, or locked) and
 * locks or unlocks automatic switching when clicked. `onChange` gets the daemon's new state;
 * `onError` the reason a change failed. Renders nothing for daemons that don't report the lock.
 */
export const AutoProfileLockButton: React.FC<{
  client: Pick<DaemonClient, "setAutoProfileLock">;
  state: AutoProfileSwitch | undefined;
  onChange: (next: AutoProfileSwitch) => void;
  onError: (message: string) => void;
}> = ({ client, state, onChange, onError }) => {
  const [busy, setBusy] = useState(false);
  if (!state) return null;
  const { label, title } = describe(state);
  const toggle = async () => {
    setBusy(true);
    try {
      onChange(await client.setAutoProfileLock(!state.locked));
    } catch (e) {
      onError(`Couldn't change the profile lock: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };
  return (
    <button
      type="button"
      className={`auto-lock-btn ${state.locked ? "is-locked" : state.heldUntilAppChange ? "is-held" : ""}`}
      aria-pressed={state.locked}
      title={title}
      disabled={busy}
      onClick={() => void toggle()}
    >
      {label}
    </button>
  );
};
