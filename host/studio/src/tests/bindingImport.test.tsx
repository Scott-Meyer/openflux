import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { BindingLayout, V2ModuleRecord } from "../api/v2Client";
import { ModuleView } from "../components/ModuleView";

const MODULE: V2ModuleRecord = {
  id: "wow-key-faces",
  manifest: {
    id: "wow-key-faces",
    name: "WoW Key Faces",
    version: "1.0.0",
    description: "Shows imported WoW key faces",
    author: "Test",
    category: "companion",
    stages: [],
    host_requirement: "optional",
    slot_capable: false,
    stackable: true,
    entrypoints: {},
    channels: { subscribes: [], publishes: [] },
    settings: [],
    ui: { builtin_view: "binding-import" },
  },
  tier: "local",
  path: "/modules/wow-key-faces",
  healthy: true,
  state: { enabled: true, config: {} },
};
const ENDPOINT = "/v2/modules/wow-key-faces/binding-layout";

function moduleView(mod: V2ModuleRecord) {
  return <ModuleView mod={mod} keys={[]} onUpdated={async () => {}} onOpenKeymap={() => {}} />;
}

function chooseExport(name: string, contents: string) {
  fireEvent.change(screen.getByLabelText("Myslot export file"), {
    target: { files: [new File([contents], name, { type: "text/plain" })] },
  });
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

it("offers the binding importer only for modules that opt in", async () => {
  let layout: BindingLayout = { schema: 1, faces: [{ key: "KC_R", mods: 1, name: "Polymorph" }] };
  const fetch = vi.fn(async () => Response.json({ layout }));
  vi.stubGlobal("fetch", fetch);
  const { rerender } = render(moduleView({ ...MODULE, manifest: { ...MODULE.manifest, ui: undefined } }));
  expect(screen.queryByRole("region", { name: "Import bindings" })).toBeNull();
  expect(fetch).not.toHaveBeenCalled();

  rerender(moduleView(MODULE));
  expect(screen.getByRole("region", { name: "Import bindings" })).toBeTruthy();
  await screen.findByText("Polymorph");
  expect(fetch).toHaveBeenCalledWith(ENDPOINT, expect.objectContaining({ method: "GET" }));

  layout = { schema: 1, faces: [{ key: "KC_R", mods: 1, name: "Arcane Blast" }] };
  fireEvent.click(screen.getByRole("button", { name: "Refresh bindings" }));
  await screen.findByText("Arcane Blast");
  expect(screen.queryByText("Polymorph")).toBeNull();

  layout = { schema: 1, faces: [{ key: "KC_Q", mods: 2, name: "Frostbolt" }] };
  // The shell delivers the latest module records after another client imports.
  rerender(moduleView({ ...MODULE }));
  await screen.findByText("Frostbolt");
  expect(screen.queryByText("Arcane Blast")).toBeNull();
});

it("waits for Import, shows what was saved, and keeps it when a replacement fails", async () => {
  let rejectImport = false;
  const layout = {
    schema: 1,
    faces: [
      { key: "KC_Q", mods: 0, name: "Frostbolt" },
      { key: "KC_LEFT_BRACKET", mods: 15, name: "Éclair" },
    ],
    flavor: "retail",
    skipped: 2,
  };
  const fetch = vi.fn(async (url: string, request: RequestInit) => {
    if (url !== ENDPOINT) return Response.json({ error: "Unexpected endpoint" }, { status: 404 });
    if (request.method === "GET") return Response.json({ layout: null });
    if (rejectImport) return Response.json({ error: "Unrecognized Myslot export" }, { status: 400 });
    return Response.json({ layout });
  });
  vi.stubGlobal("fetch", fetch);
  render(moduleView(MODULE));
  await screen.findByText("No bindings have been imported yet.");
  const importButton = screen.getByRole<HTMLButtonElement>("button", { name: "Import" });
  expect(importButton.disabled).toBe(true);

  const contents = "Example Myslot export containing Éclair\r\n";
  chooseExport("mage.txt", contents);
  expect(screen.getByText("Selected: mage.txt")).toBeTruthy();
  expect(fetch).toHaveBeenCalledTimes(1);
  fireEvent.click(importButton);
  expect(importButton.disabled).toBe(true);
  expect(screen.getByRole("status").textContent).toBe("Importing…");
  await screen.findByText("Imported mage.txt on this computer.");
  expect(fetch).toHaveBeenLastCalledWith(
    ENDPOINT,
    expect.objectContaining({ method: "POST", body: JSON.stringify({ contents }) }),
  );
  expect(screen.getByText("Sync to Keyboard")).toBeTruthy();
  expect(screen.getByText("2 bindings · retail · 2 skipped")).toBeTruthy();
  const [first, second] = within(screen.getByRole("list", { name: "Imported bindings" })).getAllByRole("listitem");
  expect(within(first).getByText("Q")).toBeTruthy();
  expect(within(first).getByText("Frostbolt")).toBeTruthy();
  expect(within(second).getByText("Ctrl + Shift + Alt + Cmd + [")).toBeTruthy();
  expect(within(second).getByText("Éclair")).toBeTruthy();

  rejectImport = true;
  chooseExport("broken.txt", "not a Myslot export");
  fireEvent.click(importButton);
  expect((await screen.findByRole("alert")).textContent).toContain("Unrecognized Myslot export");
  expect(screen.getByText("Selected: broken.txt")).toBeTruthy();
  expect(screen.getByText("Frostbolt")).toBeTruthy();
  await waitFor(() => expect(importButton.disabled).toBe(false));

  chooseExport("too-large.txt", "a".repeat(512 * 1024 + 1));
  fireEvent.click(importButton);
  await screen.findByText(/The export is larger than 512 KiB/);
  expect(screen.getByText("Frostbolt")).toBeTruthy();
  expect(fetch.mock.calls.map(([url]) => url)).toEqual([ENDPOINT, ENDPOINT, ENDPOINT]);
});
