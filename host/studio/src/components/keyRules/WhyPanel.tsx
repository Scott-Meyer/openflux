import type React from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ExplainContext, ExplainedStep, InspectResult } from "./inspect";
import { describeFace, explainSteps, inspectKey, testPulse } from "./inspect";

export interface WhyPanelProps {
  keyIndex: number;
  keyName: string;
  context: ExplainContext;
  /** Bumped when something that changes faces was saved, to ask again. */
  refreshToken?: number;
  /** Keys the Test pulse button lights up (the selection, or just this key). */
  pulseKeys: number[];
  onOpenRule?: (rowIndex: number) => void;
}

/** Skipped steps folded by provider and reason, so 40 shortcut legends read as one line. */
function foldSkipped(steps: ExplainedStep[]): Array<{ who: string; why: string; count: number; step: ExplainedStep }> {
  const out: Array<{ who: string; why: string; count: number; step: ExplainedStep }> = [];
  for (const s of steps) {
    const why = s.skipped ?? "";
    const prev =
      s.rowIndex === undefined
        ? out.find((o) => o.who === s.who && o.why === why && o.step.rowIndex === undefined)
        : undefined;
    if (prev) prev.count += 1;
    else out.push({ who: s.who, why, count: 1, step: s });
  }
  return out;
}

/** "Why does this key look like this?" from the renderer's own answer. */
export const WhyPanel: React.FC<WhyPanelProps> = ({
  keyIndex,
  keyName,
  context,
  refreshToken,
  pulseKeys,
  onOpenRule,
}) => {
  const [result, setResult] = useState<InspectResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [pulseMsg, setPulseMsg] = useState<string | null>(null);
  const [showSkipped, setShowSkipped] = useState(false);
  const seq = useRef(0);

  const ask = useCallback(async () => {
    const my = ++seq.current;
    setLoading(true);
    setError(null);
    try {
      const r = await inspectKey(keyIndex);
      if (my === seq.current) setResult(r);
    } catch (err) {
      if (my === seq.current) {
        setResult(null);
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (my === seq.current) setLoading(false);
    }
  }, [keyIndex]);

  // A new key starts fresh; a save elsewhere asks again (after the module reloads) and keeps
  // showing the last answer meanwhile.
  useEffect(() => {
    setResult(null);
    setPulseMsg(null);
    const t = window.setTimeout(() => void ask(), 150);
    return () => window.clearTimeout(t);
  }, [ask]);
  useEffect(() => {
    if (!refreshToken) return;
    const t = window.setTimeout(() => void ask(), 900);
    return () => window.clearTimeout(t);
  }, [ask, refreshToken]);

  const steps = result ? explainSteps(result, context) : [];
  const applied = steps.filter((s) => s.matched);
  const skipped = foldSkipped(steps.filter((s) => !s.matched));

  const pulse = async () => {
    setPulseMsg(null);
    try {
      await testPulse(pulseKeys);
      setPulseMsg(
        `Pulsing ${pulseKeys.length === 1 ? keyName : `${pulseKeys.length} keys`} on the keyboard for 3 seconds.`,
      );
    } catch (err) {
      setPulseMsg(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <section className="card kr-why" aria-label={`Why ${keyName} looks like this`}>
      <div className="kr-why-head">
        <h3>Why does {keyName} look like this?</h3>
        <div className="btn-group">
          <button type="button" className="btn-secondary btn-compact" onClick={() => void pulse()}>
            Test pulse
          </button>
          <button type="button" className="btn-secondary btn-compact" onClick={() => void ask()} disabled={loading}>
            {loading ? "Asking…" : "Ask again"}
          </button>
        </div>
      </div>
      {pulseMsg && <small className="muted">{pulseMsg}</small>}
      {error && <div className="notice notice-warn">{error}</div>}
      {result && (
        <>
          <p className="kr-why-face">
            {describeFace(result.face)}
            {result.signal && typeof result.signal.until_ms === "number" && (
              <> A signal is lighting it for {(result.signal.until_ms / 1000).toFixed(1)} s more.</>
            )}
          </p>
          <small className="muted">
            On layer {result.layer ?? context.activeLayer}
            {result.mods ? " with modifiers held" : ""}. Read top to bottom: the first line has the last word.
          </small>
          {applied.length === 0 ? (
            <p className="muted">No rule targets this key right now.</p>
          ) : (
            <ol className="kr-steps">
              {applied.map((s, i) => (
                <li key={s.id} className={i === 0 ? "is-top" : ""}>
                  <span className="kr-step-who">
                    {s.rowIndex !== undefined && onOpenRule ? (
                      <button type="button" className="link-btn" onClick={() => onOpenRule(s.rowIndex as number)}>
                        {s.who}
                      </button>
                    ) : (
                      s.who
                    )}
                  </span>
                  <span className="kr-step-what"> {s.what}.</span>
                  <span className="kr-step-meta">
                    {s.mode ?? "replace"} · priority {s.priority ?? "?"}
                  </span>
                </li>
              ))}
            </ol>
          )}
          {skipped.length > 0 && (
            <div className="kr-skipped">
              <button
                type="button"
                className="link-btn"
                aria-expanded={showSkipped}
                onClick={() => setShowSkipped((v) => !v)}
              >
                {showSkipped ? "▾" : "▸"} Not applying right now ({steps.length - applied.length})
              </button>
              {showSkipped && (
                <ul>
                  {skipped.map((s) => (
                    <li key={s.step.id}>
                      {s.step.rowIndex !== undefined && onOpenRule ? (
                        <button
                          type="button"
                          className="link-btn"
                          onClick={() => onOpenRule(s.step.rowIndex as number)}
                        >
                          {s.who}
                        </button>
                      ) : (
                        <strong>{s.who}</strong>
                      )}
                      {s.count > 1 ? ` (${s.count} rules)` : ""}: {s.why}.
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </>
      )}
    </section>
  );
};
