import { expect, it } from "vitest";
import { applyRule, resolveFaces } from "../components/keyRules/faces";
import { explainSteps } from "../components/keyRules/inspect";
import { buildKeyResolver, formatKeySpec, parseKeySpec } from "../components/keyRules/keyNames";
import { customProviderRules, describeRule, partState, setPartState } from "../components/keyRules/rules";

// The physical ANSI layout (85 keys), as layer 0 of `keyboard/keymap`.
const ANSI = [
  "ESCAPE F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12 F13 F14 DELETE",
  "GRAVE 1 2 3 4 5 6 7 8 9 0 MINUS EQUAL BACKSPACE HOME",
  "TAB Q W E R T Y U I O P LEFT_BRACKET RIGHT_BRACKET BACKSLASH END",
  "CAPS_LOCK A S D F G H J K L SEMICOLON QUOTE ENTER PAGE_UP",
  "LEFT_SHIFT Z X C V B N M COMMA DOT SLASH RIGHT_SHIFT UP PAGE_DOWN",
  "LEFT_CTRL LEFT_ALT LEFT_GUI SPACE RIGHT_GUI FN RIGHT_ALT RIGHT_CTRL LEFT DOWN RIGHT",
]
  .join(" ")
  .split(" ");
const keymap = { layers: [{ keyActions: ANSI.map((c) => ({ keyCode: `KC_${c}` })) }] };
const keys = ANSI.map((_, i) => ({
  index: i,
  row: i < 16 ? 0 : i < 31 ? 1 : i < 46 ? 2 : i < 60 ? 3 : i < 74 ? 4 : 5,
}));

const live = buildKeyResolver({ keys, keymap });
const noKeymap = buildKeyResolver({ keys });

it("writes names that resolve back to the picked physical keys", () => {
  for (const r of [live, noKeymap]) {
    for (const k of keys) {
      const spec = formatKeySpec([k.index], r);
      expect(parseKeySpec(spec, r).indices).toEqual([k.index]);
    }
  }
  // Readable where the name is trustworthy...
  expect(formatKeySpec([1, 2, 3, 4], live)).toBe("F1-F4");
  expect(formatKeySpec([0, 47, 48], live)).toBe("Esc, A, S");
  // ...and not misleading where the static short name doesn't match the key's binding:
  // physical 13 is F13 on this layout, though the static table calls it PRT.
  expect(formatKeySpec([13], live)).toBe("KC_F13");
  expect(formatKeySpec([13], noKeymap)).toBe("Prt");
  // Digits are digit keys, the comma key is "comma", and an index is "#N".
  expect(formatKeySpec([17, 68], live)).toBe("1, comma");
  expect(formatKeySpec([77], noKeymap)).toBe("#77");
  // A whole group is written by name.
  expect(formatKeySpec(live.group("home_row"), live, "home_row")).toBe("home_row");
});

it("reads keys specs the way keys-custom splits them", () => {
  const p = parseKeySpec("F1-F3, esc, home_row, row:5, nope", live);
  expect(p.groups).toEqual(["home_row", "row:5"]);
  expect(p.unknown).toEqual(["nope"]);
  expect(p.indices.slice(0, 4)).toEqual([1, 2, 3, 0]);
  expect(p.indices).toContain(47);
  expect(p.indices).toContain(84);
  expect(parseKeySpec("1, #1, comma, 47", live).indices).toEqual([17, 1, 68, 47]);
});

it("switches a part between inherit, set and clear", () => {
  let row: Record<string, unknown> = { keys: "F2", glow: "none" };
  expect(partState(row, "glow")).toBe("inherit");
  row = setPartState(row, "glow", "set");
  expect(row.glow).toBe("soft");
  expect(partState(row, "glow")).toBe("set");
  row = setPartState(row, "glow", "clear");
  expect(row).toMatchObject({ glow: "none", clear: "glow" });
  expect(partState(row, "glow")).toBe("clear");
  row = setPartState(row, "symbol", "clear");
  expect(row.clear).toBe("glow,icon,image");
  row = setPartState(row, "glow", "inherit");
  expect(row.clear).toBe("icon,image");
  row = setPartState(row, "corner_tr", "clear");
  expect(row.corner_tr).toBe("none");
});

it("describes a row in a sentence", () => {
  const ctx = { resolver: live, keyName: (i: number) => (i === 2 ? "F2" : `#${i}`) };
  expect(
    describeRule(
      {
        keys: "F2",
        condition_topic: "home/bedroom",
        condition_path: "scene",
        condition_op: "eq",
        condition_value: "Dim",
        glow: "pulse",
        glow_color: "#ffd166",
      },
      ctx,
    ),
  ).toBe('On F2, when home/bedroom → scene is "Dim": pulse a glow in #ffd166.');
  expect(describeRule({ keys: "letters", layer: "0", hide: "legend", corner_tr: "none" }, ctx)).toBe(
    "On letters, on layer 0: hide the legend (markers stay) and remove the top-right corner.",
  );
});

it("previews hidden legends, keeping binding markers", () => {
  const providers = [
    {
      topic: "keys/providers/keymap",
      payload: {
        priority: 10,
        mode: "replace",
        rules: [
          { key: 46, label: "Caps" },
          { key: 47, label: "a" },
        ],
      },
    },
    {
      topic: "keys/providers/keymap-bindings",
      payload: { priority: 90, mode: "patch", rules: [{ key: 46, corners: { tr: { text: "L2" } } }] },
    },
  ];
  const custom = customProviderRules(
    { rules: [{ keys: "Caps, A", hide: "legend", glow: "soft", layer: "0" }] },
    noKeymap,
  );
  const faces = resolveFaces(providers, {
    resolver: noKeymap,
    activeLayer: 0,
    overrideTopic: { topic: "keys/providers/custom", priority: 100, mode: "patch", rules: custom },
  });
  expect(faces.get(46)).toMatchObject({ corners: { tr: { text: "L2" } }, glow: { strength: 1 } });
  expect(faces.get(46)?.label).toBeUndefined();
  // On another layer the rule doesn't hold, unless it's the rule being previewed.
  const other = resolveFaces(providers, {
    resolver: noKeymap,
    activeLayer: 1,
    overrideTopic: { topic: "keys/providers/custom", priority: 100, mode: "patch", rules: custom },
  });
  expect(other.get(46)?.label).toBe("Caps");
  const previewed = resolveFaces(providers, {
    resolver: noKeymap,
    activeLayer: 1,
    overrideTopic: { topic: "keys/providers/custom", priority: 100, mode: "patch", rules: custom },
    previewSource: "rules:1",
  });
  expect(previewed.get(46)?.label).toBeUndefined();
  // A patch's false clears; a replace resets.
  expect(applyRule({ label: "x", glow: {} }, { glow: false }, "patch")).toEqual({ label: "x" });
  expect(applyRule({ label: "x" }, { icon: "star" }, "replace")).toEqual({ icon: "star" });
});

it("explains renderer steps in words, newest word first", () => {
  const steps = explainSteps(
    {
      key: 47,
      layer: 0,
      steps: [
        { provider: "keys/providers/keymap", rule: 1, priority: 10, mode: "replace", matched: true },
        {
          provider: "keys/providers/shortcuts",
          rule: 1,
          priority: 30,
          mode: "replace",
          matched: false,
          skipped: "modifiers",
        },
        {
          provider: "keys/providers/custom",
          rule: 1,
          priority: 111,
          mode: "patch",
          matched: true,
          source: "rules:1",
          name: "Clean",
        },
        {
          provider: "keys/providers/custom",
          rule: 2,
          priority: 112,
          mode: "patch",
          matched: false,
          skipped: "layer",
          source: "rules:2",
        },
      ],
    },
    {
      providers: {
        "keys/providers/keymap": { rules: [{ key: 47, label: "a" }] },
        "keys/providers/shortcuts": { rules: [{ key: 47, label: "All", mods: 8 }] },
        "keys/providers/custom": {
          rules: [
            { keys: [47], hide: "legend" },
            { keys: [47], glow: {}, when: { layer: 1 } },
          ],
        },
      },
      rows: [{ name: "Clean" }, {}],
      activeLayer: 0,
    },
  );
  expect(steps.map((s) => [s.who, s.what, s.skipped])).toEqual([
    ["Your rule 2", "glows", "it's for layer 1, and layer 0 is active"],
    ["Your rule 1 “Clean”", "hides the legend", undefined],
    ["Shortcut legends", "sets the face: shows “All”", "it only applies while Cmd is held"],
    ["Keymap legends", "sets the face: shows “a”", undefined],
  ]);
  expect(steps[0].rowIndex).toBe(1);
});
