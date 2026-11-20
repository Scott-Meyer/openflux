import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import fixture from "../../../daemon/tests/fixtures/keycodes.json";
import { KeycodePickerModal } from "../components/KeycodePickerModal";
import { KEYCODE_ALIASES, KEYCODE_CATEGORIES, keycodeInfo, keycodeLegend } from "../components/keycodes";

afterEach(() => {
  cleanup();
});

const catalogue = KEYCODE_CATEGORIES.flatMap((c) => c.keys);

describe("key code catalogue", () => {
  it("has exactly the names the daemon encodes, each once", () => {
    const daemonNames = [
      "KC_TRANSPARENT",
      "KC_NO",
      ...fixture.keyboard.map((k) => k.name),
      ...fixture.consumer.map((k) => k.name),
      ...fixture.openflux.map((k) => k.name),
    ];
    const codes = catalogue.map((k) => k.code);
    expect(new Set(codes).size).toBe(codes.length);
    expect([...codes].sort()).toEqual([...daemonNames].sort());
  });

  it("accepts the daemon's aliases and shows them with their name's legend", () => {
    expect(KEYCODE_ALIASES).toEqual(fixture.aliases);
    expect(keycodeLegend("KC_INS")).toBe("INS");
    expect(keycodeLegend("KC_LCTL")).toBe("LCTRL");
    expect(keycodeInfo("KC_LNG1")?.code).toBe("KC_LANGUAGE_1");
  });

  it("gives every key a legend that fits a key cap", () => {
    for (const key of catalogue) {
      expect(key.legend.length, key.code).toBeGreaterThan(0);
      expect(key.legend.length, key.code).toBeLessThanOrEqual(6);
    }
    expect(keycodeLegend("KC_SOMETHING_NEW")).toBe("SOMETHING_NEW");
  });
});

describe("KeycodePickerModal", () => {
  it("binds a key outside the basic set, with modifiers", () => {
    const onApply = vi.fn();
    render(
      <KeycodePickerModal
        action={{ type: 0, keyCode: "KC_LCTL" }}
        macros={[]}
        layerCount={2}
        onApply={onApply}
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByTitle("Left Control (KC_LEFT_CTRL)").className).toContain("selected");
    fireEvent.click(screen.getByTitle("Print Screen (KC_PRINT_SCREEN)"));
    fireEvent.click(screen.getByLabelText(/Shift/));
    fireEvent.click(screen.getByRole("button", { name: "Apply Binding" }));
    expect(onApply).toHaveBeenCalledWith({ type: 0, keyCode: "KC_PRINT_SCREEN", modifiers: 0x02 });
  });

  it("offers every consumer control and function key", () => {
    render(
      <KeycodePickerModal
        action={{ type: 0, keyCode: "KC_A" }}
        macros={[]}
        layerCount={2}
        onApply={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    for (const name of [
      "KC_F24",
      "KC_KP_ENTER",
      "KC_INTERNATIONAL_9",
      "KC_WWW_HOME",
      "KC_CALCULATOR",
      "KC_MEDIA_EJECT",
    ]) {
      expect(screen.getByTitle(new RegExp(`\\(${name}\\)$`))).toBeTruthy();
    }
  });
});
