import type React from "react";
import { useState } from "react";
import { type DaemonError, toDaemonError } from "../../api/daemonError";
import {
  COMPONENT_LABELS,
  type ComponentStatus,
  type ConversionStatus,
  type KeyboardStatus,
  type UpdatesClient,
} from "../../api/updates";
import { ConfirmDialog, DaemonErrorNotice } from "./common";

const KIND_LABELS: Record<ComponentStatus["kind"], string> = {
  openflux: "OpenFlux",
  preRelease: "OpenFlux development build",
  factory: "Factory software",
  unknown: "Unrecognized software",
};

/**
 * Converting a keyboard that doesn't run OpenFlux on every component. OpenFlux replaces the
 * firmware, bridge and display together and then imports the keyboard's settings.
 */
export const ConversionWizard: React.FC<{
  updates: UpdatesClient;
  keyboard: KeyboardStatus;
  conversion: ConversionStatus | null;
  jobRunning: boolean;
  onJobStarted: (jobId: string) => void;
}> = ({ updates, keyboard, conversion, jobRunning, onJobStarted }) => {
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<DaemonError | null>(null);
  const found = keyboard.conversion?.found?.length ? keyboard.conversion.found : keyboard.components;
  const install = keyboard.conversion?.install ?? [];
  const canConvert = conversion ? conversion.canConvert : true;

  const convert = async () => {
    setConfirming(false);
    setError(null);
    try {
      const { jobId } = await updates.startConversion("convert");
      onJobStarted(jobId);
    } catch (e) {
      setError(toDaemonError(e));
    }
  };

  return (
    <section className="wizard" aria-label="Convert to OpenFlux">
      <h4>Convert this keyboard to OpenFlux</h4>
      <p>
        This keyboard runs software that isn't OpenFlux. OpenFlux works only with a keyboard that runs OpenFlux on all
        of its parts, so converting replaces the keyboard's firmware, bridge and display software together.
      </p>
      <ul className="kv-list compact">
        {found.map((c) => (
          <li key={c.component}>
            <span>{COMPONENT_LABELS[c.component]}</span>
            <strong>
              {KIND_LABELS[c.kind]}
              {c.installed ? ` ${c.installed}` : ""}
            </strong>
          </li>
        ))}
      </ul>
      <ul className="wizard-facts">
        <li>Your themes, keymaps, profiles, modules and key calibration are kept and imported into OpenFlux.</li>
        <li>The keyboard can't type while it converts. This takes a few minutes; keep it connected.</li>
        <li>You can reset the keyboard to its factory software later, under Advanced.</li>
      </ul>
      {install.length > 0 && (
        <p className="muted">
          Installs {install.map((i) => `${COMPONENT_LABELS[i.component].toLowerCase()} ${i.version}`).join(", ")}.
        </p>
      )}
      {!canConvert && conversion?.reason && <p className="muted">{conversion.reason}</p>}
      <button
        type="button"
        className="btn-primary"
        disabled={jobRunning || !canConvert}
        onClick={() => setConfirming(true)}
      >
        Convert to OpenFlux…
      </button>
      {error && <DaemonErrorNotice title="The conversion didn't start" error={error} />}
      {confirming && (
        <ConfirmDialog
          title="Convert to OpenFlux?"
          confirmLabel="Convert"
          onConfirm={() => void convert()}
          onCancel={() => setConfirming(false)}
        >
          <p>
            OpenFlux replaces the software on the keyboard and then imports your settings. The keyboard can't type until
            this finishes, which takes a few minutes. Keep it connected, and use another keyboard if you need to type in
            the meantime.
          </p>
        </ConfirmDialog>
      )}
    </section>
  );
};

/**
 * Returning the keyboard to its factory software, which the daemon downloads from its official
 * source. Two confirmations, because afterwards OpenFlux can't change the keyboard.
 */
export const FactoryReset: React.FC<{
  updates: UpdatesClient;
  conversion: ConversionStatus | null;
  jobRunning: boolean;
  onJobStarted: (jobId: string) => void;
}> = ({ updates, conversion, jobRunning, onJobStarted }) => {
  const [stage, setStage] = useState<0 | 1 | 2>(0);
  const [error, setError] = useState<DaemonError | null>(null);
  const available = conversion?.canRestore ?? false;

  const reset = async () => {
    setStage(0);
    setError(null);
    try {
      const { jobId } = await updates.startConversion("restore");
      onJobStarted(jobId);
    } catch (e) {
      setError(toDaemonError(e));
    }
  };

  const unavailable = error?.status === 501;
  return (
    <section className="danger-zone" aria-label="Reset to factory software">
      <span className="danger-label">Reset to factory software</span>
      <p className="muted">
        Removes OpenFlux from the keyboard and installs the keyboard's factory software, downloaded from its official
        source. Studio can't change the keyboard's settings afterwards unless you convert it again.
      </p>
      {!available && (
        <p className="muted" role="status">
          Not available: {conversion?.reason ?? "OpenFlux can't reset this keyboard right now."}
        </p>
      )}
      {unavailable ? (
        <p className="muted" role="status">
          Not available: {error?.message}
        </p>
      ) : (
        error && <DaemonErrorNotice title="The reset didn't start" error={error} />
      )}
      <button
        type="button"
        className="btn-danger"
        disabled={!available || jobRunning || unavailable}
        onClick={() => setStage(1)}
      >
        Reset to factory software…
      </button>
      {stage === 1 && (
        <ConfirmDialog
          title="Reset to factory software?"
          confirmLabel="Continue"
          danger
          onConfirm={() => setStage(2)}
          onCancel={() => setStage(0)}
        >
          <p>
            OpenFlux removes its software from the keyboard and installs the factory software. The keyboard can't type
            while this runs. Afterwards, Studio can't change the keyboard until you convert it to OpenFlux again.
          </p>
        </ConfirmDialog>
      )}
      {stage === 2 && (
        <ConfirmDialog
          title="Reset the keyboard now?"
          confirmLabel="Reset now"
          danger
          onConfirm={() => void reset()}
          onCancel={() => setStage(0)}
        >
          <p>This is the last confirmation. Keep the keyboard connected until the reset finishes.</p>
        </ConfirmDialog>
      )}
    </section>
  );
};
