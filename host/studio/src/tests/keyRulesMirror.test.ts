/// <reference types="vite/client" />
import { expect, it } from "vitest";
import lua from "../../../../modules/keys-renderer/script.lua?raw";
import { MIRRORED_TABLES } from "../components/keyRules/keyNames";

// Studio writes key names the renderer resolves back to the picked physical keys. That depends
// on mirroring the renderer's own name tables, so a change there has to land here too.

function luaTable(name: string): Record<string, string> {
  const m = new RegExp(`local ${name} = \\{([\\s\\S]*?)\\n\\}`).exec(lua);
  if (!m) throw new Error(`${name} not found in keys-renderer`);
  const out: Record<string, string> = {};
  for (const e of m[1].matchAll(/(?:\["((?:\\.|[^"])*)"\]|([A-Za-z_][A-Za-z0-9_]*))\s*=\s*("?[A-Za-z0-9_]+"?)/g)) {
    const key = e[1] !== undefined ? e[1].replace(/\\(.)/g, "$1") : e[2];
    out[key] = e[3].replace(/"/g, "");
  }
  return out;
}

it("mirrors the renderer's key name table", () => {
  const want = Object.fromEntries(Object.entries(luaTable("KEYCODE_TO_INDEX")).map(([k, v]) => [k, Number(v)]));
  expect(MIRRORED_TABLES.keycodeToIndex).toEqual(want);
});

it("mirrors the renderer's key name aliases", () => {
  expect(MIRRORED_TABLES.alias).toEqual(luaTable("KEYCODE_ALIAS"));
});
