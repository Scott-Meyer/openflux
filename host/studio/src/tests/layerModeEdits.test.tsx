import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { ActuationPanel } from "../components/ActuationPanel";
import {
  addActuationMode,
  addLayer,
  deleteActuationMode,
  deleteLayer,
  layerCount,
  renameActuationMode,
  renameLayer,
} from "../components/layerModeEdits";
import type { KeymapConfig, OpenFluxConfig } from "../types/openflux";
import { TEST_CONFIG } from "./fixtures";

afterEach(() => {
  cleanup();
});

const accepted = (edit: ReturnType<typeof addLayer>) => {
  if (!edit.ok) throw new Error(`refused: ${edit.reason}`);
  return edit;
};

/** TEST_CONFIG with a third layer L2 that the FN layer switches to with its first key. */
function threeLayers(): OpenFluxConfig {
  const config = structuredClone(TEST_CONFIG);
  const withL2 = accepted(addLayer(config, "Numpad")).patch.keymap as KeymapConfig;
  for (const ldata of Object.values(withL2.keymaps)) {
    ldata.layers[1].keyActions[0] = { type: 2, layerControlType: "toggleLayer", targetLayer: 2 };
    ldata.layers[0].keyActions[1] = { type: 2, layerControlType: "momentaryLayer", targetLayer: 2 };
  }
  return { ...config, keymap: withL2 };
}

describe("layer edits", () => {
  it("adds a fall-through layer to every layout, up to the keyboard's limit", () => {
    const config = structuredClone(TEST_CONFIG);
    const km = accepted(addLayer(config, " Numpad ")).patch.keymap as KeymapConfig;
    for (const ldata of Object.values(km.keymaps)) {
      expect(ldata.layers).toHaveLength(3);
      expect(ldata.layers[2].name).toBe("Numpad");
      expect(ldata.layers[2].keyActions).toHaveLength(ldata.layers[0].keyActions.length);
      expect(ldata.layers[2].keyActions.every((ka) => ka.keyCode === "KC_TRANSPARENT")).toBe(true);
    }
    const full = { ...config, keymap: km, keymapLimits: { maxLayers: 3, maxActuationModes: 8 } };
    const refused = addLayer(full, "One more");
    expect(refused.ok).toBe(false);
    expect(!refused.ok && refused.reason).toMatch(/at most 3 layers/);
  });

  it("renames a layer in every layout", () => {
    const km = accepted(renameLayer(structuredClone(TEST_CONFIG), 1, "Media")).patch.keymap as KeymapConfig;
    expect(Object.values(km.keymaps).map((l) => l.layers[1].name)).toEqual(["Media", "Media"]);
    expect(renameLayer(TEST_CONFIG, 1, "  ").ok).toBe(false);
  });

  it("refuses to delete the base layer", () => {
    const edit = deleteLayer(threeLayers(), 0);
    expect(edit.ok).toBe(false);
    expect(!edit.ok && edit.reason).toMatch(/base layer/);
  });

  it("deleting a layer clears keys that switch to it, moves later layers down and asks first", () => {
    const config = threeLayers();
    config.profiles = [
      { ...config.profiles![0], default_layer: 1 },
      { ...config.profiles![1], default_layer: 2 },
    ];
    config.appearance.defaultTheme.stylemaps = {
      "1": { styles: { "4": { label: "FN key" } } },
      "2": { styles: { "5": { label: "Num" } } },
    } as OpenFluxConfig["appearance"]["defaultTheme"]["stylemaps"];

    // Deleting L1: L2 becomes L1, and keys that switch to L2 follow it.
    const edit = accepted(deleteLayer(config, 1));
    const km = edit.patch.keymap as KeymapConfig;
    for (const ldata of Object.values(km.keymaps)) {
      expect(ldata.layers.map((l) => l.name)).toEqual(["Base", "Numpad"]);
      expect(ldata.layers[0].keyActions[1]).toMatchObject({ type: 2, targetLayer: 1 });
    }
    expect(edit.patch.profiles?.map((p) => p.default_layer)).toEqual([0, 1]);
    expect(Object.keys(edit.patch.appearance?.defaultTheme.stylemaps ?? {})).toEqual(["1"]);
    expect(edit.patch.appearance?.defaultTheme.stylemaps["1"].styles).toEqual({ "5": { label: "Num" } });
    expect(edit.consequences.join("\n")).toMatch(/Profile “Default” starts on FN; it will start on the base layer/);
    expect(edit.consequences.join("\n")).toMatch(/Key styles set for FN/);

    // Deleting L2: the keys that switch to it are cleared.
    const del2 = accepted(deleteLayer(config, 2));
    const ansi = (del2.patch.keymap as KeymapConfig).keymaps.ansi;
    expect(ansi.layers[0].keyActions[1]).toEqual({ type: 0, keyCode: "KC_NO" });
    expect(ansi.layers[1].keyActions[0]).toEqual({ type: 0, keyCode: "KC_TRANSPARENT" });
    expect(del2.consequences[0]).toBe("2 keys that switch to Numpad will be cleared.");
    expect(layerCount(del2.patch.keymap as KeymapConfig)).toBe(2);
  });
});

describe("actuation mode edits", () => {
  it("adds, duplicates and renames modes; untouched modes stay untouched", () => {
    const config = structuredClone(TEST_CONFIG);
    const added = accepted(addActuationMode(config, "Writing")).patch.actuationModes ?? [];
    expect(added.map((m) => m.name)).toEqual(["Typing/General", "FPS Gaming", "Writing"]);
    expect(added[0].touched).toBe(false);
    expect(added[2]).toMatchObject({ type: 1, rapidTrigger: false, touched: true, globalTouched: true });
    expect(new Set(added.map((m) => m.id)).size).toBe(3);

    const copy = (accepted(addActuationMode(config, "Gaming 2", 1)).patch.actuationModes ?? [])[2];
    expect(copy).toMatchObject({ name: "Gaming 2", rapidTrigger: true, rapidTriggerSensitivityMm: 0.3, touched: true });
    expect(copy.id).not.toBe("gaming");

    const renamed = accepted(renameActuationMode(config, 1, "Shooter")).patch.actuationModes ?? [];
    expect(renamed.map((m) => m.name)).toEqual(["Typing/General", "Shooter"]);

    const full = { ...config, keymapLimits: { maxLayers: 8, maxActuationModes: 2 } };
    expect(addActuationMode(full, "Third").ok).toBe(false);
  });

  it("refuses to delete the last mode", () => {
    const config = { ...structuredClone(TEST_CONFIG), actuationModes: [TEST_CONFIG.actuationModes![0]] };
    const edit = deleteActuationMode(config, 0);
    expect(!edit.ok && edit.reason).toMatch(/at least one/);
  });

  it("deleting a mode in use moves its profiles to the first remaining mode", () => {
    const config = structuredClone(TEST_CONFIG);
    config.actuationModes = [
      ...(config.actuationModes ?? []),
      { ...config.actuationModes![0], id: "third", name: "Third" },
    ];
    config.profiles = [
      { ...config.profiles![0], actuation_mode_index: 0 },
      { ...config.profiles![1], actuation_mode_index: 2 },
    ];
    const edit = accepted(deleteActuationMode(config, 0));
    expect(edit.patch.actuationModes?.map((m) => m.id)).toEqual(["gaming", "third"]);
    expect(edit.patch.profiles?.map((p) => p.actuation_mode_index)).toEqual([0, 1]);
    expect(edit.consequences).toEqual(["Profile “Default” uses “Typing/General”; it will use “FPS Gaming”."]);
  });
});

/** The actuation panel with mode management, applying every edit to its config. */
function ManagedPanel({ initial, patches }: { initial: OpenFluxConfig; patches: Partial<OpenFluxConfig>[] }) {
  const [config, setConfig] = useState(initial);
  return (
    <ActuationPanel
      config={config}
      selectedKeyIndex={0}
      onSelectKey={() => {}}
      onChangeActuation={(actuationModes) => setConfig((c) => ({ ...c, actuationModes }))}
      onApplyConfigEdit={(patch) => {
        patches.push(patch);
        setConfig((c) => ({ ...c, ...patch }));
      }}
    />
  );
}

describe("managing actuation modes in the panel", () => {
  it("asks before deleting a mode a profile uses, and applies modes and profiles together", () => {
    const patches: Partial<OpenFluxConfig>[] = [];
    render(<ManagedPanel initial={structuredClone(TEST_CONFIG)} patches={patches} />);
    fireEvent.click(screen.getByRole("button", { name: "Delete Typing/General" }));
    const dialog = screen.getByRole("dialog", { name: "Delete Typing/General" });
    expect(dialog.textContent).toMatch(/Profile “Default” uses “Typing\/General”; it will use “FPS Gaming”/);
    expect(patches).toHaveLength(0);
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete Typing/General" }));
    expect(patches).toHaveLength(1);
    expect(patches[0].actuationModes?.map((m) => m.name)).toEqual(["FPS Gaming"]);
    expect(patches[0].profiles?.every((p) => p.actuation_mode_index === 0)).toBe(true);
    expect(screen.queryByRole("heading", { name: "Typing/General" })).toBeNull();
    expect((screen.getByRole("button", { name: "Delete FPS Gaming" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("adds and renames modes", () => {
    const patches: Partial<OpenFluxConfig>[] = [];
    render(<ManagedPanel initial={structuredClone(TEST_CONFIG)} patches={patches} />);
    fireEvent.change(screen.getByLabelText("New mode name"), { target: { value: "Writing" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Mode" }));
    expect(screen.getByRole("heading", { name: "Writing" })).toBeTruthy();
    expect(screen.getByText("3 of 8 actuation modes")).toBeTruthy();
    const card = screen.getByRole("heading", { name: "Writing" }).closest(".card") as HTMLElement;
    fireEvent.click(within(card).getByRole("button", { name: "Rename" }));
    fireEvent.change(within(card).getByLabelText("Rename Writing"), { target: { value: "Prose" } });
    fireEvent.click(within(card).getByRole("button", { name: "Rename" }));
    expect(screen.getByRole("heading", { name: "Prose" })).toBeTruthy();
    expect(patches.map((p) => p.actuationModes?.length)).toEqual([3, 3]);
  });
});
