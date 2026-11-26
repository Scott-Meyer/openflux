import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { KeycapsTab } from "../components/KeycapsTab";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const ANSI = [
  "ESCAPE F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12 F13 F14 DELETE",
  "GRAVE 1 2 3 4 5 6 7 8 9 0 MINUS EQUAL BACKSPACE HOME",
  "TAB Q W E R T Y U I O P LEFT_BRACKET RIGHT_BRACKET BACKSLASH END",
  "CAPS_LOCK A S D F G H J K L SEMICOLON QUOTE ENTER PAGE_UP",
  "LEFT_SHIFT Z X C V B N M COMMA DOT SLASH RIGHT_SHIFT UP PAGE_DOWN",
  "LEFT_CTRL LEFT_ALT LEFT_GUI SPACE RIGHT_GUI FN RIGHT_ALT RIGHT_CTRL LEFT DOWN RIGHT",
].map((r) => r.split(" "));

const geometry = {
  keys: ANSI.flatMap((row, r) =>
    row.map((_, c) => ({ row: r, col: c, rect: { x: c * 100, y: r * 100, w: 95, h: 95 } })),
  ).map((k, i) => ({ ...k, key_index: i })),
};
const codes = ANSI.flat();
const LABELS: Record<string, string> = { ESCAPE: "Esc", CAPS_LOCK: "Caps", DELETE: "Del" };
const keymapProvider = {
  priority: 10,
  mode: "replace",
  rules: codes.map((c, i) => ({ key: i, label: LABELS[c] ?? (c.length === 1 ? c.toLowerCase() : c) })),
};
// Caps is a layer-tap key: its "L2" marker is a separate patch, so hiding the legend keeps it.
const bindings = { priority: 90, mode: "patch", rules: [{ key: 46, corners: { tr: { text: "L2" } } }] };

interface Published {
  topic: string;
  payload: Record<string, unknown>;
}

/** A daemon with Key Rules and a renderer that answers inspect requests. PUT /state merges config keys like the real one. */
function stubDaemon(config: Record<string, unknown>) {
  const state = { enabled: true, config: structuredClone(config) };
  const published: Published[] = [];
  const events: Array<{ seq: number; topic: string; payload: unknown }> = [];
  let seq = 100;
  let inspectAnswer: (req: Record<string, unknown>) => Record<string, unknown> = (req) => ({
    key: req.key,
    layer: 0,
    steps: [],
    face: {},
  });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      const u = new URL(url, "http://studio.test");
      const retained = (payload: unknown, topic = u.searchParams.get("pattern") ?? "") =>
        Response.json({ messages: [{ topic, payload }] });
      if (u.pathname === "/v2/bus/retained") {
        const pattern = u.searchParams.get("pattern");
        if (pattern === "keyboard/geometry") return retained(geometry);
        if (pattern === "keyboard/resolved_keys")
          return retained({ active_layer: 0, keys: codes.map((_, i) => ({ keycode: 4 + i, source_layer: 0 })) });
        if (pattern === "system/actions") return retained({ slots: {} });
        if (pattern === "keyboard/keymap")
          return retained({ layers: [{ keyActions: codes.map((c) => ({ keyCode: `KC_${c}` })) }] });
        if (pattern === "keys/providers/#") {
          return Response.json({
            messages: [
              { topic: "keys/providers/keymap", payload: keymapProvider },
              { topic: "keys/providers/keymap-bindings", payload: bindings },
            ],
          });
        }
        return Response.json({ messages: [] });
      }
      if (u.pathname === "/v2/modules") {
        return Response.json({
          modules: [
            {
              id: "keys-custom",
              manifest: { id: "keys-custom", name: "Key Rules", settings: [] },
              state: structuredClone(state),
            },
          ],
          modules_rev: 1,
        });
      }
      if (u.pathname === "/v2/modules/keys-custom/state" && init?.method === "PUT") {
        const patch = JSON.parse(String(init.body)) as { enabled?: boolean; config?: Record<string, unknown> };
        state.config = { ...state.config, ...patch.config };
        if (patch.enabled !== undefined) state.enabled = patch.enabled;
        return Response.json({ ok: true });
      }
      if (u.pathname === "/v2/status") return Response.json({ pid: 1, modules_rev: 1, latest_bus_seq: seq });
      if (u.pathname === "/v2/bus/pub") {
        const body = JSON.parse(String(init?.body)) as Published;
        published.push(body);
        if (body.topic === "keys/inspect/request") {
          events.push({
            seq: ++seq,
            topic: "keys/inspect/result",
            payload: { ...inspectAnswer(body.payload), request_id: body.payload.request_id },
          });
        }
        return Response.json({ ok: true });
      }
      if (u.pathname === "/v2/bus/events") {
        const since = Number(u.searchParams.get("since_seq"));
        return Response.json({ events: events.filter((e) => e.seq > since), latest_seq: seq });
      }
      if (u.pathname === "/v2/keyboard/sync") return Response.json({ ok: true, uploaded_files: [], uploaded_bytes: 0 });
      return Response.json({ error: "Not available in this test" }, { status: 404 });
    }),
  );
  return {
    rules: () => state.config.rules as Array<Record<string, unknown>>,
    config: () => state.config,
    published,
    answerInspect: (fn: typeof inspectAnswer) => {
      inspectAnswer = fn;
    },
    /** A change made outside Studio, like `fluxctl keys rules add`. */
    setFromCli: (rules: unknown[]) => {
      state.config = { ...state.config, rules };
    },
  };
}

async function openTab() {
  await act(async () => {
    render(<KeycapsTab />);
  });
  return screen.getByRole("group", { name: "Keyboard" });
}

const keyButton = (board: HTMLElement, name: string) => within(board).getByRole("button", { name });

it("builds a rule from keys picked on the keyboard", async () => {
  const daemon = stubDaemon({ rules: [] });
  const board = await openTab();
  await waitFor(() => expect(keyButton(board, "Caps")).toBeTruthy());

  fireEvent.click(keyButton(board, "Caps"));
  fireEvent.click(keyButton(board, "A"), { shiftKey: true });
  fireEvent.click(screen.getByLabelText(/only on layer 0/));
  fireEvent.click(screen.getByRole("button", { name: "Hide legend" }));

  await waitFor(() => expect(daemon.rules()).toHaveLength(1));
  expect(daemon.rules()[0]).toMatchObject({ keys: "KC_CAPS_LOCK, A", layer: "0", hide: "legend", enabled: true });
  // Said back in words, in the list and in the editor.
  expect(screen.getAllByText("On Caps and A, on layer 0: hide the legend (markers stay).").length).toBeGreaterThan(0);
  // The map shows the legend gone and the layer-tap marker kept.
  expect(keyButton(board, "Caps").textContent).toBe("L2");
  expect(keyButton(board, "A").textContent).toBe("");
});

it("sets, clears and inherits parts, and keeps fields Studio doesn't know", async () => {
  const daemon = stubDaemon({ rules: [{ keys: "fkeys", glow: "soft", hide: "nothing", from_cli: 7 }] });
  await openTab();
  fireEvent.click(await screen.findByRole("button", { name: /^Rule 1: On the F-keys: glow\./ }));

  const glow = screen.getByRole("group", { name: "Glow" });
  fireEvent.click(within(glow).getByRole("button", { name: "Clear" }));
  await waitFor(() => expect(daemon.rules()[0]).toMatchObject({ glow: "none", clear: "glow", from_cli: 7 }));

  const outline = screen.getByRole("group", { name: "Outline" });
  fireEvent.click(within(outline).getByRole("button", { name: "Set" }));
  await waitFor(() => expect(daemon.rules()[0].outline).toBe("ring"));
  // Edge outlines show parallax, so they aren't offered.
  expect(within(screen.getByLabelText("Outline shape")).queryByRole("option", { name: /Edge/ })).toBeNull();

  fireEvent.change(screen.getByLabelText("Outline color"), { target: { value: "#4de1ff" } });
  await waitFor(() => expect(daemon.rules()[0].outline_color).toBe("#4de1ff"));
  expect(screen.getAllByText("On the F-keys: draw a ring in #4de1ff and remove the glow.").length).toBeGreaterThan(0);

  fireEvent.click(within(glow).getByRole("button", { name: "Inherit" }));
  await waitFor(() => expect(daemon.rules()[0].clear).toBe(""));
});

it("adds a bus condition in plain words", async () => {
  const daemon = stubDaemon({ rules: [] });
  const board = await openTab();
  await waitFor(() => expect(keyButton(board, "F2")).toBeTruthy());
  fireEvent.click(keyButton(board, "F2"));
  fireEvent.click(screen.getByRole("button", { name: "Glow" }));
  await waitFor(() => expect(daemon.rules()).toHaveLength(1));

  fireEvent.click(screen.getByRole("button", { name: "+ Bus value" }));
  fireEvent.change(screen.getByLabelText("Bus topic"), { target: { value: "home/bedroom" } });
  fireEvent.change(screen.getByLabelText("Value path"), { target: { value: "scene" } });
  fireEvent.change(screen.getByLabelText("Value"), { target: { value: "Dim" } });
  await waitFor(() =>
    expect(daemon.rules()[0]).toMatchObject({
      keys: "F2",
      condition_topic: "home/bedroom",
      condition_path: "scene",
      condition_op: "eq",
      condition_value: "Dim",
    }),
  );
  expect(screen.getAllByText(/^On F2, when home\/bedroom → scene is "Dim": glow in #ffd166\.$/).length).toBeGreaterThan(
    0,
  );
});

it("reorders, disables and deletes rules; later rows win", async () => {
  const daemon = stubDaemon({
    rules: [
      { name: "Dim", keys: "Esc", label: "Dim" },
      { name: "Bright", keys: "Esc", label: "Bright" },
    ],
  });
  const board = await openTab();
  // The later row wins on the map.
  await waitFor(() => expect(keyButton(board, "Esc").textContent).toBe("Bright"));

  fireEvent.click(screen.getByRole("button", { name: "Move Bright up" }));
  await waitFor(() => expect(daemon.rules().map((r) => r.name)).toEqual(["Bright", "Dim"]));
  expect(keyButton(board, "Esc").textContent).toBe("Dim");

  fireEvent.click(screen.getByRole("checkbox", { name: "Enable Dim" }));
  await waitFor(() => expect(daemon.rules()[1].enabled).toBe(false));
  expect(keyButton(board, "Esc").textContent).toBe("Bright");

  fireEvent.click(screen.getByRole("button", { name: "Delete Bright" }));
  expect(daemon.rules()).toHaveLength(2); // asks first
  fireEvent.click(screen.getByRole("button", { name: "Confirm deleting Bright" }));
  await waitFor(() => expect(daemon.rules().map((r) => r.name)).toEqual(["Dim"]));
});

it("shows rules changed elsewhere, e.g. by fluxctl", async () => {
  const daemon = stubDaemon({ rules: [] });
  await openTab();
  daemon.setFromCli([{ keys: "home_row", hide: "legend" }]);
  expect(
    await screen.findByRole("button", { name: /^Rule 1: On the home row: hide the legend/ }, { timeout: 4000 }),
  ).toBeTruthy();
});

it("explains the focused key from the renderer's answer, and test-pulses it", async () => {
  const daemon = stubDaemon({ rules: [{ name: "Clean", keys: "A", hide: "legend" }] });
  daemon.answerInspect((req) => ({
    key: req.key,
    layer: 0,
    steps: [
      { provider: "keys/providers/keymap", rule: 48, priority: 10, mode: "replace", matched: true },
      {
        provider: "keys/providers/custom",
        rule: 1,
        priority: 111,
        mode: "patch",
        matched: true,
        source: "rules:1",
        name: "Clean",
      },
    ],
    face: {},
  }));
  const board = await openTab();
  await waitFor(() => expect(keyButton(board, "A")).toBeTruthy());
  fireEvent.click(keyButton(board, "A"));

  const why = await screen.findByRole("region", { name: "Why A looks like this" });
  await within(why).findByText("Your rule 1 “Clean”");
  expect(within(why).getByText(/hides the legend\./)).toBeTruthy();
  expect(daemon.published.find((p) => p.topic === "keys/inspect/request")?.payload).toMatchObject({
    key: 47,
    request_id: expect.any(String),
  });

  fireEvent.click(within(why).getByRole("button", { name: "Test pulse" }));
  await waitFor(() =>
    expect(daemon.published.find((p) => p.topic === "keys/signal")?.payload).toEqual({
      id: "studio-test-pulse",
      keys: [47],
      set: { outline: { pulse: true } },
      duration_ms: 3000,
    }),
  );
});

it("keeps the quick per-key override, as a patch that hides only the legend", async () => {
  const daemon = stubDaemon({ rules: [{ keys: "Esc", glow: "soft" }] });
  const board = await openTab();
  await waitFor(() => expect(keyButton(board, "Caps")).toBeTruthy());
  fireEvent.click(keyButton(board, "Caps"));
  const panel = screen.getByRole("region", { name: "Quick override" });
  fireEvent.change(within(panel).getByLabelText("Backdrop (inset tint)"), { target: { value: "#123456" } });
  fireEvent.click(within(panel).getByLabelText("Hide the legend (layer and modifier markers stay)"));
  fireEvent.click(within(panel).getByRole("button", { name: "Apply override" }));
  await waitFor(() => expect(daemon.config().by_key).toEqual({ "0:46": { bg: "#123456", hide: true } }));
  expect(daemon.rules()).toEqual([{ keys: "Esc", glow: "soft" }]); // rules untouched
  await waitFor(() => expect(keyButton(board, "Caps").textContent).toBe("L2"));
});
