import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { ActuationPanel } from "../components/ActuationPanel";
import {
  releaseAfterActuationMove,
  releasePointOf,
  releasePointProblem,
  validReleasePoint,
} from "../components/actuationTravel";
import type { ActuationMode, KeyglassConfig } from "../types/keyglass";
import { TEST_CONFIG } from "./fixtures";

afterEach(() => {
  cleanup();
});

/** The panel over `modes` as the app holds it, sending every change to `sent`. */
function Harness({ modes, sent }: { modes: ActuationMode[]; sent: ActuationMode[][] }) {
  const [config, setConfig] = useState<KeyglassConfig>({ ...structuredClone(TEST_CONFIG), actuationModes: modes });
  const [selected, setSelected] = useState<number | null>(0);
  return (
    <ActuationPanel
      config={config}
      selectedKeyIndex={selected}
      onSelectKey={setSelected}
      onChangeActuation={(next) => {
        sent.push(next);
        setConfig((c) => ({ ...c, actuationModes: next }));
      }}
    />
  );
}

/** Renders the panel over `modes`, recording every change it sends. */
function renderPanel(modes: ActuationMode[]): { sent: ActuationMode[][] } {
  const sent: ActuationMode[][] = [];
  render(<Harness modes={modes} sent={sent} />);
  return { sent };
}

const typing = (patch: Partial<ActuationMode> = {}): ActuationMode => ({
  id: "typing",
  name: "Typing",
  type: 1,
  actuationPointMm: 2.0,
  rapidTrigger: false,
  ...patch,
});

const gaming: ActuationMode = {
  id: "gaming",
  name: "Gaming",
  type: 2,
  actuationPointMm: 1.2,
  rapidTrigger: true,
  rapidTriggerSensitivityMm: 0.3,
};

/** The mode card titled `name`. */
function modeCard(name: string): HTMLElement {
  const card = screen.getByRole("button", { name }).closest(".card");
  if (!(card instanceof HTMLElement)) throw new Error(`no card for ${name}`);
  return card;
}

describe("fixed actuation travel", () => {
  it("defaults the release point 0.2 mm above the actuation point", () => {
    expect(releasePointOf({ actuationPointMm: 2.0 })).toBe(1.8);
    expect(releasePointOf({ actuationPointMm: 0.1 })).toBe(0);
    expect(releasePointOf({ actuationPointMm: 2.0, releasePointMm: 0.5 })).toBe(0.5);
  });

  it("accepts only a release point above the actuation point", () => {
    expect(releasePointProblem(1.2, 1.1)).toBeNull();
    expect(releasePointProblem(0.1, 0)).toBeNull();
    expect(releasePointProblem(1.2, 1.2)).toMatch(/must be above the actuation point \(1\.2 mm\)/);
    expect(releasePointProblem(1.2, 1.5)).toMatch(/less than 1\.2 mm/);
    // Floating-point sums on the 0.1 mm grid compare as steps.
    expect(releasePointProblem(0.1 + 0.2, 0.3)).not.toBeNull();
  });

  it("keeps the gap when the actuation point moves", () => {
    expect(releaseAfterActuationMove(2.0, 1.5, 1.2)).toBe(0.7);
    expect(releaseAfterActuationMove(2.0, 1.5, 0.3)).toBe(0);
    // A release point that wasn't above moves to the default gap.
    expect(releaseAfterActuationMove(1.0, 1.4, 2.0)).toBe(1.8);
    expect(validReleasePoint(1.0, 1.4)).toBe(0.8);
    expect(validReleasePoint(1.0, 0.4)).toBe(0.4);
  });
});

describe("ActuationPanel release point", () => {
  it("shows a fixed mode's release point and what it does", () => {
    renderPanel([typing({ releasePointMm: 1.5 }), gaming]);
    const card = modeCard("Typing");
    expect(within(card).getByLabelText(/^Release Point \(1\.5 mm\)/)).toBeTruthy();
    expect(within(card).getByText(/sits above the actuation\s+point/)).toBeTruthy();
    expect(within(modeCard("Gaming")).queryByLabelText(/Release Point/)).toBeNull();
  });

  it("sends a release point above the actuation point", () => {
    const { sent } = renderPanel([typing(), gaming]);
    fireEvent.change(within(modeCard("Typing")).getByLabelText(/^Release Point/), { target: { value: "1.3" } });
    expect(sent.at(-1)?.[0]).toMatchObject({ actuationPointMm: 2.0, releasePointMm: 1.3, touched: true });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("refuses a release point at or below the actuation point and explains why", () => {
    const { sent } = renderPanel([typing({ releasePointMm: 1.5 }), gaming]);
    const card = modeCard("Typing");
    for (const value of ["2", "2.4"]) {
      fireEvent.change(within(card).getByLabelText(/^Release Point/), { target: { value } });
      expect(sent).toHaveLength(0);
      expect(within(card).getByRole("alert").textContent).toMatch(/must be above the actuation point \(2\.0 mm\)/);
      expect(within(card).getByLabelText(/^Release Point \(1\.5 mm\)/)).toBeTruthy();
    }
    // A valid choice clears the explanation.
    fireEvent.change(within(card).getByLabelText(/^Release Point/), { target: { value: "1.9" } });
    expect(sent.at(-1)?.[0].releasePointMm).toBe(1.9);
    expect(within(card).queryByRole("alert")).toBeNull();
  });

  it("moves the release point with the actuation point", () => {
    const { sent } = renderPanel([typing({ releasePointMm: 1.5 }), gaming]);
    fireEvent.change(within(modeCard("Typing")).getByLabelText(/^Actuation Point/), { target: { value: "1.2" } });
    expect(sent.at(-1)?.[0]).toMatchObject({ actuationPointMm: 1.2, releasePointMm: 0.7 });
  });

  it("explains a stored release point that isn't above the actuation point", () => {
    renderPanel([typing({ releasePointMm: 2.2 }), gaming]);
    expect(within(modeCard("Typing")).getByRole("alert").textContent).toMatch(
      /release point \(2\.2 mm\) must be above the actuation point \(2\.0 mm\)/,
    );
  });

  it("gives a mode leaving rapid trigger a valid release point", () => {
    const { sent } = renderPanel([typing(), { ...gaming, releasePointMm: 1.6 }]);
    fireEvent.click(within(modeCard("Gaming")).getByRole("checkbox"));
    expect(sent.at(-1)?.[1]).toMatchObject({ rapidTrigger: false, type: 1, releasePointMm: 1.0 });
  });

  it("marks a mode the keyboard left at its default and drops the mark once edited", () => {
    const { sent } = renderPanel([typing({ firmwareDefault: true, firmwareWord: 0 }), gaming]);
    expect(within(modeCard("Typing")).getByText(/Keyboard Default/)).toBeTruthy();
    fireEvent.change(within(modeCard("Typing")).getByLabelText(/^Release Point/), { target: { value: "1.0" } });
    expect(sent.at(-1)?.[0].firmwareDefault).toBeUndefined();
    expect(within(modeCard("Typing")).queryByText(/Keyboard Default/)).toBeNull();
  });

  it("validates a key's own release point", () => {
    const { sent } = renderPanel([typing({ releasePointMm: 1.5 }), gaming]);
    const keySlider = () => screen.getByLabelText(/\S Release Point \(/);
    expect(keySlider()).toBeTruthy();

    fireEvent.change(keySlider(), { target: { value: "2.5" } });
    expect(sent).toHaveLength(0);
    expect(screen.getByRole("alert").textContent).toMatch(/must be above the actuation point \(2\.0 mm\)/);

    fireEvent.change(keySlider(), { target: { value: "0.6" } });
    expect(sent.at(-1)?.[0].perKeyOverrides?.ansi?.["0"]).toMatchObject({
      actuationPointMm: 2.0,
      rapidTrigger: false,
      releasePointMm: 0.6,
      touched: true,
    });
    expect(screen.queryByRole("alert")).toBeNull();

    // The key's actuation point carries its own release point along.
    fireEvent.change(screen.getByLabelText(/\S Actuation \(/), { target: { value: "1.0" } });
    expect(sent.at(-1)?.[0].perKeyOverrides?.ansi?.["0"]).toMatchObject({
      actuationPointMm: 1.0,
      releasePointMm: 0,
    });
  });
});
