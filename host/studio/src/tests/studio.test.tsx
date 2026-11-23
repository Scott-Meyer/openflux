import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import React from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import type { DaemonClient } from "../api/daemonClient";
import { DaemonError } from "../api/daemonError";
import { computeCenterKeyParallax } from "../components/ParallaxDiagram";
import type { DeviceStatus, OpenFluxConfig, PreviewKeyRect } from "../types/openflux";
import { TEST_CONFIG } from "./fixtures";
import { FACTORY_KEYBOARD, MockDaemon, mockServices } from "./mockDaemon";

afterEach(() => {
  cleanup();
});

/** The rendered key with `data-key-index`; throws when the key is not on screen. */
function keyElement(index: number | string): Element {
  const el = document.querySelector(`[data-key-index="${index}"]`);
  if (!el) throw new Error(`key ${index} is not rendered`);
  return el;
}

/** The mock daemon's keyboard: its status, and how it answers a request to adopt the firmware keymap. */
interface MockKeyboard {
  status: DeviceStatus;
  /** When set, `POST /v1/keymap/adopt-firmware` is refused with this error. */
  adoptRefusal: DaemonError | null;
}

function createMockClient(initial: OpenFluxConfig = structuredClone(TEST_CONFIG)): {
  client: DaemonClient;
  calls: string[];
  state: OpenFluxConfig;
  keyboard: MockKeyboard;
} {
  const state = structuredClone(initial);
  const calls: string[] = [];
  const status: DeviceStatus = {
    connected: { bridge: true, firmware: true },
    versions: {
      firmware: "openflux-firmware 1.0.0",
      bridge: "openflux-bridge 1.0.0",
      display: "openflux-display 1.0.0",
      os: "2.1.0",
    },
    keymapSync: { state: "ok", reason: null },
    desiredProfileIndex: 0,
    appliedProfileIndex: 0,
  };
  const keyboard: MockKeyboard = { status, adoptRefusal: null };
  const rects = Array.from({ length: 85 }, (_, i) => ({
    keyIndex: i,
    x: 20 + (i % 16) * 115,
    y: 280 + Math.floor(i / 16) * 120,
    w: 105,
    h: 105,
  }));

  const client: DaemonClient = {
    getStatus: vi.fn(async () => {
      calls.push("getStatus");
      return structuredClone(status);
    }),
    setAutoProfileLock: vi.fn(async (locked) => {
      calls.push(`setAutoProfileLock:${locked}`);
      status.autoProfileSwitch = { locked, heldUntilAppChange: false };
      return structuredClone(status.autoProfileSwitch);
    }),
    getConfig: vi.fn(async () => {
      calls.push("getConfig");
      return structuredClone(state);
    }),
    getDefaultConfig: vi.fn(async () => {
      calls.push("getDefaultConfig");
      return structuredClone(TEST_CONFIG);
    }),
    updateConfig: vi.fn(async (patch) => {
      calls.push(`updateConfig:${Object.keys(patch).join(",")}`);
      Object.assign(state, patch);
    }),
    saveToKeyboard: vi.fn(async () => {
      calls.push("saveToKeyboard");
      return { message: "Successfully saved to keyboard" };
    }),
    adoptFirmwareKeymap: vi.fn(async () => {
      calls.push("adoptFirmwareKeymap");
      if (keyboard.adoptRefusal) throw keyboard.adoptRefusal;
      state.hydrated = { appearance: true, settings: true, mosaic: true, ...state.hydrated, keymap: true };
      status.keymapSync = { state: "ok", reason: null };
      return structuredClone(state);
    }),
    importBundle: vi.fn(async (p) => {
      calls.push(`importBundle:${p}`);
      return { message: "Imported", profiles: ["Gaming"] };
    }),
    exportBundle: vi.fn(async (outPath) => {
      calls.push(`exportBundle:${outPath}`);
      return { message: "Exported", outPath };
    }),
    addIcon: vi.fn(async (p) => {
      calls.push(`addIcon:${p}`);
      return { message: "1 Icons imported successfully", iconPaths: ["abc123.png"] };
    }),
    importMedia: vi.fn(async () => {
      throw new Error("no media in this test");
    }),
    getAerialVideos: vi.fn(async () => []),
    importAerialVideo: vi.fn(async () => {
      throw new Error("no aerial videos in this test");
    }),
    getAssetsCatalog: vi.fn(async () => {
      throw new Error("no asset catalog in this test");
    }),
    getShortcutPack: vi.fn(async () => {
      throw new Error("no shortcut packs in this test");
    }),
    getAssetDataUrl: vi.fn(async () => {
      throw new Error("no assets in this test");
    }),
    getInstalledApps: vi.fn(async () => []),
    getPreviewRects: vi.fn(async () => {
      calls.push("getPreviewRects");
      return rects;
    }),
    renderPreviewRgba: vi.fn(async (opts) => {
      calls.push(`renderPreview:${JSON.stringify(opts?.liveJson ?? {})}`);
      return new Uint8Array(1920 * 1080 * 4);
    }),
    closePreview: vi.fn(async () => {
      calls.push("closePreview");
    }),
    backupDevice: vi.fn(async (dir) => {
      calls.push(`backupDevice:${dir}`);
      return { message: "Backup complete" };
    }),
    flashFirmwareUf2: vi.fn(async (p) => {
      calls.push(`flashFirmwareUf2:${p}`);
      return { message: "Flashed & verified" };
    }),
    sideloadIpk: vi.fn(async (p) => {
      calls.push(`sideloadIpk:${p}`);
      return { message: "Sideloaded" };
    }),
    pomodoroAction: vi.fn(async (action) => {
      calls.push(`pomodoroAction:${action}`);
      return { message: `Pomodoro ${action}`, pomodoro: { running: action === "toggle", remainingS: 1800, rev: 1 } };
    }),
    getSocStats: vi.fn(async () => ({
      cpuPct: 18.5,
      gpuPct: 24.0,
      memUsedMb: 412,
      memTotalMb: 2048,
      renderMs: 8,
      rkmpp: true,
    })),
    pickFile: vi.fn(async () => "/tmp/picked_file.openflux"),
    pickSaveFile: vi.fn(async () => "/tmp/export.openflux"),
    pickFolder: vi.fn(async () => "/tmp/backup"),
    getPressRefinement: vi.fn(async () => ({ state: "installed" as const, crc32: 0x1234abcd })),
    installPressRefinement: vi.fn(async () => ({
      state: "installed" as const,
      crc32: 0x1234abcd,
      alreadyInstalled: true,
      fromCache: true,
    })),
  };
  return { client, calls, state, keyboard };
}

describe("Keyglass Studio", () => {
  it("shows the daemon's config, renders the live preview, and closes the preview on unmount", async () => {
    const { client, calls } = createMockClient();
    const { unmount } = render(<App client={client} initialTab="themes" />);

    await waitFor(() => {
      expect(calls).toContain("getConfig");
      expect(calls).toContain("getPreviewRects");
    });

    expect(screen.getByDisplayValue("Test Theme")).toBeTruthy();
    expect((screen.getByLabelText(/Font Family/i) as HTMLSelectElement).value).toBe("Inter");
    expect(screen.getByText(/^Connected$/i)).toBeTruthy();

    unmount();
    expect(calls).toContain("closePreview");
  });

  it("edits modules, key legends, rapid trigger and app rules, and restores the daemon's defaults", async () => {
    const { client, calls, state } = createMockClient();
    render(<App client={client} />);
    await waitFor(() => expect(calls).toContain("getConfig"));

    // Modules: assign the Virtual Pet tile to slot 1
    fireEvent.click(screen.getByRole("button", { name: "Modules" }));
    fireEvent.click(screen.getByText("Virtual Pet"));
    await waitFor(() => {
      expect(state.mosaic.tileTypes[0]).toBe(17);
    });

    // Keymap: give key #16 a custom legend
    fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
    fireEvent.click(keyElement(16));
    const legendInput = screen.getByPlaceholderText("Uses the key's own label");
    fireEvent.change(legendInput, { target: { value: "GRAVE" } });
    await waitFor(() => {
      expect(state.appearance.defaultTheme.stylemaps["0"]?.styles["16"]?.label).toBe("GRAVE");
    });

    // Actuation: turn on rapid trigger for the first mode
    fireEvent.click(screen.getByRole("button", { name: "Actuation" }));
    const checkboxes = screen.getAllByRole("checkbox");
    fireEvent.click(checkboxes[0]);
    await waitFor(() => {
      expect(state.actuationModes?.[0].rapidTrigger).toBe(true);
    });

    // Shortcuts: add an app rule
    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    fireEvent.change(screen.getByPlaceholderText("com.microsoft.VSCode"), {
      target: { value: "com.apple.Terminal" },
    });
    fireEvent.click(screen.getByText("Add App Rule"));
    await waitFor(() => {
      expect(state.appRules.length).toBe(1);
      expect(state.appRules[0].matcher).toBe("com.apple.Terminal");
    });

    // Device: restoring defaults writes the daemon's defaults with explicitReset, then Save reaches the keyboard
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    fireEvent.click(screen.getByRole("button", { name: "Device" }));
    fireEvent.click(screen.getByText(/Restore Default Settings/i));
    confirmSpy.mockRestore();
    await waitFor(() => {
      expect(state.explicitReset).toBe(true);
      expect(state.keymap.keymaps.ansi.layers[0].keyActions[3].keyCode).toBe("KC_F3");
    });
    fireEvent.click(screen.getByText(/Save Changes to Keyboard/i));
    await waitFor(() => {
      expect(calls).toContain("saveToKeyboard");
    });
  });

  it("queues Save as a barrier, retains dirty on concurrent newer edits, and blocks Save after prior patch failure", async () => {
    const { client, calls } = createMockClient();
    let releaseFirstPatch!: () => void;
    let releaseSave!: () => void;
    let patchCount = 0;

    client.updateConfig = vi.fn(async (patch) => {
      patchCount++;
      const idx = patchCount;
      if (idx === 1) {
        await new Promise<void>((resolve) => {
          releaseFirstPatch = resolve;
        });
      }
      calls.push(`flushed#${idx}:${Object.keys(patch).join(",")}`);
    });
    client.saveToKeyboard = vi.fn(async () => {
      calls.push("saveToKeyboard:start");
      await new Promise<void>((resolve) => {
        releaseSave = resolve;
      });
      calls.push("saveToKeyboard:done");
      return { message: "Saved" };
    });

    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(calls).toContain("getConfig"));
    const initialRenders = calls.filter((c) => c.startsWith("renderPreview")).length;

    // Edit A is held in flight, Save is queued behind it, and edit B arrives while Save runs.
    fireEvent.change(screen.getByDisplayValue("Test Theme"), { target: { value: "EditA" } });
    await waitFor(() => expect(releaseFirstPatch).toBeDefined());
    fireEvent.click(screen.getByText(/Save Changes to Keyboard/i));
    expect(calls).not.toContain("saveToKeyboard:start");
    expect(calls.filter((c) => c.startsWith("renderPreview")).length).toBeGreaterThanOrEqual(initialRenders);

    releaseFirstPatch();
    await waitFor(() => expect(releaseSave).toBeDefined());
    // Edit B while Save is still waiting for its response.
    fireEvent.change(screen.getByDisplayValue("EditA"), { target: { value: "EditB" } });
    releaseSave();
    await waitFor(() => {
      expect(calls).toContain("flushed#1:appearance");
      expect(calls).toContain("saveToKeyboard:done");
      expect(calls).toContain("flushed#2:appearance");
    });
    // Edit B came after the revision Save covered, so the app stays dirty.
    expect(screen.getByText(/Save Changes to Keyboard/i)).toBeTruthy();

    // A failed edit C followed by a successful unrelated edit D still blocks Save.
    let failNext = true;
    client.updateConfig = vi.fn(async () => {
      if (failNext) {
        failNext = false;
        throw new Error("Simulated patch failure C");
      }
    });
    fireEvent.change(screen.getByDisplayValue("EditB"), { target: { value: "EditC" } });
    await waitFor(() => expect(screen.getByText(/Simulated patch failure C/i)).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Modules" }));
    fireEvent.click(screen.getByText("Virtual Pet"));
    await waitFor(() => expect(client.updateConfig).toHaveBeenCalledTimes(2));

    const saveCallsBefore = (client.saveToKeyboard as ReturnType<typeof vi.fn>).mock.calls.length;
    fireEvent.click(screen.getByText(/Save Changes to Keyboard/i));
    await waitFor(() => {
      expect(screen.getByText(/Cannot save: earlier config update failed/i)).toBeTruthy();
    });
    expect((client.saveToKeyboard as ReturnType<typeof vi.fn>).mock.calls.length).toBe(saveCallsBefore);
  });

  it("paces live preview frames while idle without edits and stops completely on unmount", async () => {
    const { client, calls } = createMockClient();
    const { unmount } = render(<App client={client} />);
    await waitFor(
      () => {
        expect(calls.filter((c) => c.startsWith("renderPreview")).length).toBeGreaterThanOrEqual(2);
      },
      { timeout: 1000 },
    );
    const countBeforeUnmount = calls.filter((c) => c.startsWith("renderPreview")).length;
    unmount();
    await new Promise((r) => setTimeout(r, 250));
    expect(calls.filter((c) => c.startsWith("renderPreview")).length).toBe(countBeforeUnmount);
  });

  it("blocks edits until the first config arrives, commits typed keycodes on Enter, recovers from a failed keymap write, and drops layer fields when a Fn key is remapped", async () => {
    const { client } = createMockClient();
    let releaseInitialGet!: (cfg: OpenFluxConfig) => void;
    client.getConfig = vi.fn(
      () =>
        new Promise<OpenFluxConfig>((resolve) => {
          releaseInitialGet = resolve;
        }),
    );

    const savedPayloads: Partial<OpenFluxConfig>[] = [];
    let failKeymapOnce = true;
    client.updateConfig = vi.fn(async (patch) => {
      if (patch.keymap && failKeymapOnce) {
        failKeymapOnce = false;
        throw new Error("Simulated invalid partial keycode rejection");
      }
      savedPayloads.push(patch);
    });

    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(releaseInitialGet).toBeDefined());

    // An appearance edit before the first config arrives is not sent.
    fireEvent.change(screen.getByDisplayValue("Default"), { target: { value: "PrematureEdit" } });
    expect(client.updateConfig).not.toHaveBeenCalled();

    // The delayed first config carries a custom theme, which appears untouched.
    const realDaemonCfg: OpenFluxConfig = structuredClone(TEST_CONFIG);
    realDaemonCfg.appearance.customThemes = [
      {
        ...realDaemonCfg.appearance.defaultTheme,
        id: "user_custom_1",
        name: "My Real Custom Theme",
      },
    ];
    releaseInitialGet(realDaemonCfg);
    await waitFor(() => {
      expect(screen.getByText("My Real Custom Theme")).toBeTruthy();
    });
    expect(savedPayloads.some((p) => p.appearance?.defaultTheme.name === "PrematureEdit")).toBe(false);

    // Typing a keycode character by character doesn't write intermediate values.
    fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
    const input = screen.getByLabelText("Key Action Code") as HTMLInputElement;
    const callsBeforeType = (client.updateConfig as ReturnType<typeof vi.fn>).mock.calls.length;
    fireEvent.change(input, { target: { value: "KC_ES" } });
    fireEvent.change(input, { target: { value: "KC_E" } });
    expect((client.updateConfig as ReturnType<typeof vi.fn>).mock.calls.length).toBe(callsBeforeType);

    // Enter commits KC_E, which the mock rejects once.
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(screen.getByText(/Simulated invalid partial keycode rejection/i)).toBeTruthy());

    // Remapping key #64 to KC_A writes a plain key action without layer fields.
    fireEvent.click(keyElement(64));
    const fnInput = screen.getByLabelText("Key Action Code") as HTMLInputElement;
    fireEvent.change(fnInput, { target: { value: "KC_A" } });
    fireEvent.keyDown(fnInput, { key: "Enter" });

    await waitFor(() => {
      const lastKm = savedPayloads.filter((p) => p.keymap).pop()?.keymap;
      if (!lastKm) throw new Error("no keymap write recorded yet");
      const ka64 = lastKm.keymaps.ansi.layers[0].keyActions[64];
      expect(ka64.type).toBe(0);
      expect(ka64.keyCode).toBe("KC_A");
      expect(ka64.layerControlType).toBeUndefined();
      expect(ka64.targetLayer).toBeUndefined();
    });

    // The later valid keymap write supersedes the failure, so Save goes through.
    fireEvent.click(screen.getByText(/Save Changes to Keyboard/i));
    await waitFor(() => expect(client.saveToKeyboard).toHaveBeenCalled());
  });

  it("focusing and blurring toggleLayer and switchToLayer keys without typing writes nothing", async () => {
    const { client } = createMockClient();
    const cfgWithFnKinds: OpenFluxConfig = structuredClone(TEST_CONFIG);
    cfgWithFnKinds.keymap.keymaps.ansi.layers[0].keyActions[64] = {
      type: 2,
      layerControlType: "toggleLayer",
      targetLayer: 1,
    };
    cfgWithFnKinds.keymap.keymaps.ansi.layers[0].keyActions[65] = {
      type: 2,
      layerControlType: "switchToLayer",
      targetLayer: 2,
    };
    client.getConfig = vi.fn(async () => cfgWithFnKinds);
    client.updateConfig = vi.fn(async () => {});

    render(<App client={client} />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Keymap" }));

    for (const kIdx of ["64", "65"]) {
      fireEvent.click(keyElement(kIdx));
      const inp = screen.getByLabelText("Key Action Code") as HTMLInputElement;
      fireEvent.focus(inp);
      fireEvent.blur(inp);
    }
    expect(client.updateConfig).not.toHaveBeenCalled();
  });

  it("runs a bundle import after an in-flight edit and keeps a later edit along with the imported themes", async () => {
    const { client, calls } = createMockClient();
    let releasePatchA!: () => void;
    let patchIdx = 0;
    let daemonThemeName = "Test Theme";
    let daemonCustomThemes: typeof TEST_CONFIG.appearance.customThemes = [];

    client.updateConfig = vi.fn(async (patch) => {
      const idx = ++patchIdx;
      calls.push(`updateConfig#${idx}:start`);
      if (idx === 1) {
        await new Promise<void>((resolve) => {
          releasePatchA = resolve;
        });
      }
      if (patch.appearance) {
        daemonThemeName = patch.appearance.defaultTheme.name;
        daemonCustomThemes = patch.appearance.customThemes ?? daemonCustomThemes;
      }
      calls.push(`updateConfig#${idx}:done`);
    });
    client.getConfig = vi.fn(async () => {
      calls.push(`getConfig:${daemonThemeName}:${daemonCustomThemes.length}`);
      const c = structuredClone(TEST_CONFIG);
      c.appearance.defaultTheme.name = daemonThemeName;
      c.appearance.customThemes = daemonCustomThemes;
      return c;
    });
    client.importBundle = vi.fn(async () => {
      calls.push("importBundle");
      daemonCustomThemes = [structuredClone(TEST_CONFIG.appearance.defaultTheme)];
      return { message: "Imported", profiles: ["Work"] };
    });

    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(calls).toContain("getConfig:Test Theme:0"));

    // Hold edit A in flight.
    fireEvent.change(screen.getByDisplayValue("Test Theme"), { target: { value: "PatchA" } });
    await waitFor(() => expect(releasePatchA).toBeDefined());

    // Queue an import; it waits for edit A so A cannot overwrite the imported themes.
    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    client.pickFile = vi.fn(async () => "/tmp/test.openflux");
    fireEvent.click(screen.getByText("Import Bundle or .flux File..."));
    await waitFor(() => expect(client.pickFile).toHaveBeenCalled());
    await Promise.resolve();
    expect(calls).not.toContain("importBundle");

    // Edit B is made after the import was queued.
    fireEvent.click(screen.getByRole("button", { name: "Themes" }));
    fireEvent.change(screen.getByDisplayValue("PatchA"), { target: { value: "EditB" } });

    // Releasing A lets the import and refresh run; the result keeps both edit B and the imported theme.
    releasePatchA();
    await waitFor(() => expect(calls).toContain("updateConfig#2:done"));
    expect(calls.indexOf("updateConfig#1:done")).toBeLessThan(calls.indexOf("importBundle"));
    expect(calls.indexOf("importBundle")).toBeLessThan(calls.indexOf("getConfig:PatchA:1"));
    expect(daemonThemeName).toBe("EditB");
    expect(daemonCustomThemes.length).toBe(1);
    expect(screen.getByDisplayValue("EditB")).toBeTruthy();
  });

  it("ignores a slow startup config that arrives after a newer post-import config", async () => {
    const { client } = createMockClient();
    let resolveStartupGet!: (cfg: OpenFluxConfig) => void;
    let getCount = 0;

    client.getConfig = vi.fn(async () => {
      getCount++;
      if (getCount === 1) {
        return new Promise<OpenFluxConfig>((resolve) => {
          resolveStartupGet = resolve;
        });
      }
      const imported = structuredClone(TEST_CONFIG);
      imported.appearance.defaultTheme.name = "ImportedThemeFromRefresh";
      return imported;
    });
    client.importBundle = vi.fn(async () => ({ message: "Imported", profiles: ["Work"] }));

    render(<App client={client} />);
    await waitFor(() => expect(resolveStartupGet).toBeDefined());

    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    client.pickFile = vi.fn(async () => "/tmp/new.openflux");
    fireEvent.click(screen.getByText("Import Bundle or .flux File..."));
    await waitFor(() => expect(getCount).toBe(2));

    // The imported config is shown.
    fireEvent.click(screen.getByRole("button", { name: "Themes" }));
    await waitFor(() => {
      expect(screen.getByDisplayValue("ImportedThemeFromRefresh")).toBeTruthy();
    });

    // The slow startup response arrives last and is ignored.
    const staleStartup = structuredClone(TEST_CONFIG);
    staleStartup.appearance.defaultTheme.name = "StaleStartupSnapshot";
    resolveStartupGet(staleStartup);
    await new Promise((r) => setTimeout(r, 30));
    expect(screen.getByDisplayValue("ImportedThemeFromRefresh")).toBeTruthy();
  });

  it("imports a .flux file as well as a Keyglass bundle", async () => {
    const { client, calls } = createMockClient();
    render(<App client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: "Shortcuts" }));
    client.pickFile = vi.fn(async () => "/tmp/exported.flux");
    fireEvent.click(screen.getByText("Import Bundle or .flux File..."));
    await waitFor(() => expect(calls).toContain("importBundle:/tmp/exported.flux"));
    expect(client.pickFile).toHaveBeenCalledWith(expect.any(String), ["openflux", "flux"]);
  });

  it("restoring defaults after an import clears custom themes instead of merging the imported ones back", async () => {
    const { client } = createMockClient();
    let backendCustom = [structuredClone(TEST_CONFIG.appearance.defaultTheme)];
    client.getConfig = vi.fn(async () => {
      const c = structuredClone(TEST_CONFIG);
      c.appearance.customThemes = backendCustom;
      return c;
    });
    client.importBundle = vi.fn(async () => {
      backendCustom = [
        { ...structuredClone(TEST_CONFIG.appearance.defaultTheme), id: "imported_custom_1", name: "Imported" },
      ];
      return { message: "Imported", profiles: ["Imported"] };
    });
    client.updateConfig = vi.fn(async (patch) => {
      if (patch.appearance) {
        backendCustom = patch.appearance.customThemes;
      }
    });

    render(<App client={client} />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());

    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    client.pickFile = vi.fn(async () => "/tmp/bundle.openflux");
    fireEvent.click(screen.getByText("Import Bundle or .flux File..."));
    await waitFor(() => expect(client.importBundle).toHaveBeenCalled());

    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    fireEvent.click(screen.getByRole("button", { name: "Device" }));
    fireEvent.click(screen.getByText(/Restore Default Settings/i));
    confirmSpy.mockRestore();
    fireEvent.click(await screen.findByText(/Save Changes to Keyboard/i));

    await waitFor(() => expect(client.saveToKeyboard).toHaveBeenCalled());
    expect(client.getDefaultConfig).toHaveBeenCalled();
    expect(backendCustom).toEqual([]);
  });

  it("keeps imported themes when an appearance edit is made while the post-import refresh is in flight", async () => {
    const { client } = createMockClient();
    let releasePostImportGet!: (cfg: OpenFluxConfig) => void;
    let getCalls = 0;
    let backendCustom: typeof TEST_CONFIG.appearance.customThemes = [];

    client.getConfig = vi.fn(async () => {
      getCalls++;
      if (getCalls === 1) {
        return structuredClone(TEST_CONFIG);
      }
      return new Promise<OpenFluxConfig>((resolve) => {
        releasePostImportGet = resolve;
      });
    });
    client.importBundle = vi.fn(async () => {
      backendCustom = [
        { ...structuredClone(TEST_CONFIG.appearance.defaultTheme), id: "imp_delayed", name: "ImportedDelayed" },
      ];
      return { message: "Imported", profiles: ["Work"] };
    });
    client.updateConfig = vi.fn(async (patch) => {
      if (patch.appearance) {
        backendCustom = patch.appearance.customThemes;
      }
    });

    render(<App client={client} />);
    await waitFor(() => expect(getCalls).toBe(1));

    // The import finishes and its follow-up config refresh is held.
    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    client.pickFile = vi.fn(async () => "/tmp/delayed.openflux");
    fireEvent.click(screen.getByText("Import Bundle or .flux File..."));
    await waitFor(() => expect(releasePostImportGet).toBeDefined());

    // Edit appearance while the UI still has the pre-import theme list.
    fireEvent.click(screen.getByRole("button", { name: "Themes" }));
    fireEvent.change(screen.getByDisplayValue("Test Theme"), { target: { value: "EditedDuringImportGet" } });

    // After the refresh and Save, both the edit and the imported theme are kept.
    const importedCfg = structuredClone(TEST_CONFIG);
    importedCfg.appearance.customThemes = [...backendCustom];
    releasePostImportGet(importedCfg);

    await waitFor(() => expect(client.updateConfig).toHaveBeenCalled());
    fireEvent.click(screen.getByText(/Save Changes to Keyboard/i));
    await waitFor(() => expect(client.saveToKeyboard).toHaveBeenCalled());
    expect(backendCustom.map((t) => t.id)).toContain("imp_delayed");
    expect(screen.getByDisplayValue("EditedDuringImportGet")).toBeTruthy();
  });

  it("blocks appearance writes and Save when the refresh after an import fails", async () => {
    const { client } = createMockClient();
    let getCalls = 0;
    let backendCustom: typeof TEST_CONFIG.appearance.customThemes = [];

    client.getConfig = vi.fn(async () => {
      getCalls++;
      if (getCalls === 1) return structuredClone(TEST_CONFIG);
      throw new Error("Simulated post-import getConfig timeout");
    });
    client.importBundle = vi.fn(async () => {
      backendCustom = [{ ...structuredClone(TEST_CONFIG.appearance.defaultTheme), id: "imp_keep", name: "Keep" }];
      return { message: "Imported", profiles: ["Work"] };
    });
    client.updateConfig = vi.fn(async (patch) => {
      if (patch.appearance) {
        backendCustom = patch.appearance.customThemes;
      }
    });

    render(<App client={client} />);
    await waitFor(() => expect(getCalls).toBe(1));

    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    client.pickFile = vi.fn(async () => "/tmp/fail_get.openflux");
    fireEvent.click(screen.getByText("Import Bundle or .flux File..."));
    await waitFor(() => expect(screen.getByText(/failed to refresh config/i)).toBeTruthy());

    // A stale appearance edit would overwrite the imported themes, so it and Save are refused.
    fireEvent.click(screen.getByRole("button", { name: "Themes" }));
    fireEvent.change(screen.getByDisplayValue("Test Theme"), { target: { value: "StaleOverwrite" } });
    fireEvent.click(screen.getByText(/Save Changes to Keyboard/i));

    // The edit is held back, and Save then refuses because of it.
    await waitFor(() => expect(screen.getByText("Cannot save: earlier config update failed")).toBeTruthy());
    expect(client.updateConfig).not.toHaveBeenCalled();
    expect(client.saveToKeyboard).not.toHaveBeenCalled();
    expect(backendCustom.map((t) => t.id)).toEqual(["imp_keep"]);
  });

  it("keeps edits blocked after a failed post-import refresh even when the slow startup config arrives", async () => {
    const { client } = createMockClient();
    let resolveStartupGet!: (cfg: OpenFluxConfig) => void;
    let getCalls = 0;
    let backendCustom: typeof TEST_CONFIG.appearance.customThemes = [];

    client.getConfig = vi.fn(async () => {
      getCalls++;
      if (getCalls === 1) {
        return new Promise<OpenFluxConfig>((resolve) => {
          resolveStartupGet = resolve;
        });
      }
      throw new Error("Post-import getConfig rejected");
    });
    client.importBundle = vi.fn(async () => {
      backendCustom = [{ ...structuredClone(TEST_CONFIG.appearance.defaultTheme), id: "imp_locked", name: "Locked" }];
      return { message: "Imported", profiles: ["Work"] };
    });
    client.updateConfig = vi.fn(async (patch) => {
      if (patch.appearance) backendCustom = patch.appearance.customThemes;
    });

    render(<App client={client} />);
    await waitFor(() => expect(resolveStartupGet).toBeDefined());

    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    client.pickFile = vi.fn(async () => "/tmp/locked.openflux");
    fireEvent.click(screen.getByText("Import Bundle or .flux File..."));
    await waitFor(() => expect(screen.getByText(/failed to refresh config/i)).toBeTruthy());

    // The startup config predates the import, so it does not clear the failure.
    resolveStartupGet(structuredClone(TEST_CONFIG));
    await new Promise((r) => setTimeout(r, 30));

    fireEvent.click(screen.getByRole("button", { name: "Themes" }));
    fireEvent.change(screen.getByDisplayValue("Default"), { target: { value: "StaleAfterStartup" } });
    fireEvent.click(screen.getByText("Save to Keyboard"));

    await waitFor(() => expect(screen.getByText("Cannot save: earlier config update failed")).toBeTruthy());
    expect(client.updateConfig).not.toHaveBeenCalled();
    expect(client.saveToKeyboard).not.toHaveBeenCalled();
    expect(backendCustom.map((t) => t.id)).toEqual(["imp_locked"]);
  });

  it("refetches preview key rects once a layout change is written and drops stale ISO rects when switching back", async () => {
    const { client } = createMockClient();
    let curLayout: "ansi" | "iso" = "ansi";
    let releasePut!: () => void;
    // The mock answers with the committed layout, ignoring the requested one.
    client.getPreviewRects = vi.fn(async () =>
      Array.from({ length: curLayout === "iso" ? 87 : 85 }, (_, i) => ({
        keyIndex: i,
        x: (i % 16) * 110 + 40,
        y: Math.floor(i / 16) * 110 + 250,
        w: 96,
        h: 96,
      })),
    );
    client.updateConfig = vi.fn(async (patch) => {
      await new Promise<void>((resolve) => {
        releasePut = () => {
          if (patch.keymap?.selectedFrameLayoutType === "iso") curLayout = "iso";
          resolve();
        };
      });
    });

    render(<App client={client} />);
    await waitFor(() => expect(client.getPreviewRects).toHaveBeenCalledTimes(1));

    fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
    fireEvent.click(screen.getByText("ISO"));
    await waitFor(() => expect(releasePut).toBeDefined());
    const callsBeforePutCommit = (client.getPreviewRects as ReturnType<typeof vi.fn>).mock.calls.length;
    releasePut();
    await waitFor(() =>
      expect((client.getPreviewRects as ReturnType<typeof vi.fn>).mock.calls.length).toBeGreaterThan(
        callsBeforePutCommit,
      ),
    );
    const lastResult = await (client.getPreviewRects as ReturnType<typeof vi.fn>).mock.results.at(-1)?.value;
    expect(lastResult).toHaveLength(87);

    // Switching back to ANSI while the rect fetch is pending drops the 87 ISO rects,
    // so keys #85 and #86 never appear on ANSI.
    let rejectAnsiRects!: (e: Error) => void;
    client.getPreviewRects = vi.fn(
      () =>
        new Promise<PreviewKeyRect[]>((_res, rej) => {
          rejectAnsiRects = rej;
        }),
    );
    fireEvent.click(screen.getByText("ANSI"));
    await waitFor(() => expect(rejectAnsiRects).toBeDefined());
    expect(screen.queryByText("#85")).toBeNull();
    expect(screen.queryByText("85")).toBeNull();
    rejectAnsiRects(new Error("rect fetch failed"));
  });

  it("keeps refreshing the preview for a Keyboard State tile over a static background", async () => {
    vi.useFakeTimers();
    try {
      const cfgStatic = structuredClone(TEST_CONFIG);
      cfgStatic.appearance.defaultTheme.backgroundImagePath = "solid.png";
      cfgStatic.mosaic.tileTypes = [9, 0, 0, 0];
      const { client } = createMockClient(cfgStatic);
      render(<App client={client} />);
      await vi.advanceTimersByTimeAsync(100);
      const callsAfterInit = (client.renderPreviewRgba as ReturnType<typeof vi.fn>).mock.calls.length;
      await vi.advanceTimersByTimeAsync(1100);
      expect((client.renderPreviewRgba as ReturnType<typeof vi.fn>).mock.calls.length).toBeGreaterThan(callsAfterInit);
    } finally {
      vi.useRealTimers();
    }
  });

  it("applies presets, keycodes, recorded macros, per-key rapid trigger and shortcuts, and exports a bundle", async () => {
    const { client } = createMockClient();
    const updates: Partial<OpenFluxConfig>[] = [];
    const origUpdate = client.updateConfig;
    client.updateConfig = vi.fn(async (patch: Partial<OpenFluxConfig>) => {
      updates.push(patch);
      return origUpdate(patch);
    });
    client.getAssetsCatalog = vi.fn(async () => ({
      fonts: [{ family: "Inter", files: ["Inter.ttf"], hasItalic: true, isVariable: true }],
      keyIcons: [{ id: "save.png", category: "key_icons", name: "save" }],
      media: [
        { name: "loop.mp4", path: "loop.mp4", fullPath: "/tmp/loop.mp4", kind: "video" as const, thumbnail: null },
      ],
      presets: [
        {
          id: "p1",
          name: "Midnight Cyber Pulse",
          fontFamily: "RobotoMono",
          fontWeight: 500,
          fontItalic: false,
          backgroundImagePath: "068a.jpg",
          backgroundImageBlurAmount: 1,
          backgroundImageDimming: 25,
          moduleBackgroundDimming: 65,
          selectedKeyEffect: "ripple" as const,
          ringGlowColor: 0xff38bdf8,
          legendColor: 0xffe0f2fe,
          blendingModePreset: "lightGlow" as const,
          thumbnail: "",
        },
      ],
      shortcutPacks: [{ id: "Excel.csv", name: "Excel", rowCount: 1 }],
    }));
    client.getShortcutPack = vi.fn(async () => ({
      pack: "Excel.csv",
      application: "Excel",
      rows: [
        {
          context: "File",
          shortcut: "Save As",
          label: "Save As",
          icon: "",
          macString: "Cmd+Shift+S",
          winString: "F12",
          macKeyCode: "KC_S",
          macKeyCode2: null,
          macHasSecondKey: false,
          macModifiers: 0x0a,
          winKeyCode: "KC_F12",
          winKeyCode2: null,
          winHasSecondKey: false,
          winModifiers: 0,
        },
      ],
    }));
    client.exportBundle = vi.fn(async (outPath: string) => ({
      message: `Exported to ${outPath}`,
      outPath,
    }));

    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(client.getAssetsCatalog).toHaveBeenCalled());

    // Preset browser
    fireEvent.click(screen.getByRole("button", { name: "Presets" }));
    fireEvent.click(screen.getByText("Midnight Cyber Pulse"));
    await waitFor(() => expect(updates.some((u) => u.appearance?.defaultTheme.fontFamily === "RobotoMono")).toBe(true));

    // Keycode picker and macro recording with a modifier chord
    fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
    fireEvent.click(screen.getByText("All Keycodes..."));
    fireEvent.click(screen.getByText("Apply Binding"));
    fireEvent.click(screen.getByText("+ New Macro"));
    fireEvent.click(screen.getByText("Record Keystrokes"));
    fireEvent.keyDown(window, { code: "MetaLeft" });
    fireEvent.keyDown(window, { code: "MetaLeft", repeat: true }); // key repeat is not recorded
    await new Promise((r) => setTimeout(r, 25));
    fireEvent.keyDown(window, { code: "KeyC" });
    fireEvent.keyUp(window, { code: "KeyC" });
    fireEvent.keyUp(window, { code: "MetaLeft" });
    fireEvent.click(screen.getByText("Stop Recording"));
    fireEvent.click(screen.getByText(/Assign #0 to Selected Key/i));
    await waitFor(() =>
      expect(updates.some((u) => Array.isArray(u.keymap?.macros) && u.keymap.macros.length === 1)).toBe(true),
    );
    const latestMacros = updates.filter((u) => Array.isArray(u.keymap?.macros)).at(-1)?.keymap?.macros;
    const recSteps = Array.isArray(latestMacros) ? (latestMacros[0].steps ?? []) : [];
    expect(recSteps.map((s) => `${s.keyCode}:${s.isDown ? "down" : "up"}`)).toEqual([
      "KC_LGUI:down",
      "KC_C:down",
      "KC_C:up",
      "KC_LGUI:up",
    ]);
    expect(recSteps[1].delayMs).toBeGreaterThanOrEqual(15);

    // Deleting macro #0 clears keys bound to it and renumbers keys bound to #1.
    fireEvent.click(screen.getByText("+ New Macro"));
    fireEvent.click(keyElement(2));
    fireEvent.click(screen.getByText(/Assign #1 to Selected Key/i));
    fireEvent.click(screen.getByText("#0: Macro 1"));
    fireEvent.click(screen.getByText("Delete"));
    await waitFor(() => {
      const last = updates.at(-1)?.keymap;
      expect(last?.keymaps.ansi.layers[0].keyActions[0].type).toBe(0);
    });
    const postDelKm = updates.at(-1)?.keymap;
    expect(postDelKm?.keymaps.ansi.layers[0].keyActions[0].keyCode).toBe("KC_NO");
    expect(postDelKm?.keymaps.ansi.layers[0].keyActions[2].type).toBe(1);
    expect(postDelKm?.keymaps.ansi.layers[0].keyActions[2].macroId).toBe(0);

    // Per-key rapid trigger override, then reset to the mode default
    fireEvent.click(screen.getByRole("button", { name: "Actuation" }));
    fireEvent.click(screen.getByLabelText("Per-Key Rapid Trigger"));
    await waitFor(() =>
      expect(
        updates.some(
          (u) =>
            u.actuationModes?.[0].globalTouched === false &&
            u.actuationModes?.[0].perKeyOverrides?.ansi?.["2"]?.rapidTrigger === true,
        ),
      ).toBe(true),
    );
    fireEvent.click(screen.getByText(/Reset M0 to Mode Default/i));
    await waitFor(() =>
      expect(updates.at(-1)?.actuationModes?.[0].perKeyOverrides?.ansi?.["2"]?.resetToDefault).toBe(true),
    );

    // Shortcut pack row applied to a key, then bundle export
    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    await waitFor(() => expect(screen.getByText("Save As")).toBeTruthy());
    fireEvent.click(screen.getByText(/Apply → M0/i));
    await waitFor(() =>
      expect(updates.some((u) => u.keymap?.keymaps.ansi.layers[0].keyActions[2].keyCode === "KC_S")).toBe(true),
    );

    fireEvent.click(screen.getByText("Export Keyglass Bundle..."));
    await waitFor(() => expect(client.exportBundle).toHaveBeenCalled());
  });

  it("computes center-key parallax with the effect off and at each foot stand angle", () => {
    const baseSettings = { ...TEST_CONFIG.settings };
    const offRes = computeCenterKeyParallax({
      ...baseSettings,
      parallaxActive: false,
      displayOffsetX: 3,
      displayOffsetY: -2,
    });
    expect(offRes.dxPx).toBe(3);
    expect(offRes.dyPx).toBe(-2);
    const flatRes = computeCenterKeyParallax({ ...baseSettings, parallaxActive: true, footStandMode: "none" });
    const lowRes = computeCenterKeyParallax({ ...baseSettings, parallaxActive: true, footStandMode: "low" });
    const highRes = computeCenterKeyParallax({ ...baseSettings, parallaxActive: true, footStandMode: "high" });
    expect(flatRes.standDeg).toBe(0);
    expect(lowRes.standDeg).toBe(4);
    expect(highRes.standDeg).toBe(8);
    expect(Math.hypot(highRes.dxPx, highRes.dyPx)).toBeLessThanOrEqual(28.0001);
    expect(lowRes.dyPx).not.toBe(flatRes.dyPx);
  });

  it("keeps a font edit made while a media import runs", async () => {
    const { client } = createMockClient();
    const updates: Partial<OpenFluxConfig>[] = [];
    const origUpdate = client.updateConfig;
    client.updateConfig = vi.fn(async (patch: Partial<OpenFluxConfig>) => {
      updates.push(patch);
      return origUpdate(patch);
    });
    let resolveImport!: (res: { assetPath: string; isVideo: boolean; transcoded: boolean }) => void;
    client.importMedia = vi.fn(
      () =>
        new Promise<{ assetPath: string; isVideo: boolean; transcoded: boolean }>((resolve) => {
          resolveImport = resolve;
        }),
    );

    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());

    client.pickFile = vi.fn(async () => "/tmp/slow_clip.mov");
    fireEvent.click(screen.getByText("+ Import Image / Video"));
    await waitFor(() => expect(resolveImport).toBeDefined());

    // Change the font while the import is still running.
    fireEvent.change(screen.getByLabelText(/Font Family/i), {
      target: { value: "RobotoMono" },
    });
    await waitFor(() => expect(updates.at(-1)?.appearance?.defaultTheme.fontFamily).toBe("RobotoMono"));

    resolveImport({ assetPath: "transcoded_clip.mp4", isVideo: true, transcoded: true });
    await waitFor(() =>
      expect(updates.at(-1)?.appearance?.defaultTheme.backgroundImagePath).toBe("transcoded_clip.mp4"),
    );
    expect(updates.at(-1)?.appearance?.defaultTheme.fontFamily).toBe("RobotoMono");
  });

  it("exports only after pending writes land and refuses export after a failed write", async () => {
    const { client } = createMockClient();
    const events: string[] = [];
    let releasePut!: () => void;
    let failNextPut = false;
    client.updateConfig = vi.fn(
      (patch: Partial<OpenFluxConfig>) =>
        new Promise<void>((resolve, reject) => {
          if (failNextPut) {
            events.push("put:fail");
            reject(new Error("PUT failed"));
            return;
          }
          releasePut = () => {
            events.push(`put:${patch.appearance?.defaultTheme.name ?? "other"}`);
            resolve();
          };
        }),
    );
    client.exportBundle = vi.fn(async (outPath: string) => {
      events.push(`export:${outPath}`);
      return { message: "Exported", outPath };
    });

    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());

    // Export waits for the pending write.
    fireEvent.change(screen.getByDisplayValue("Test Theme"), { target: { value: "ExportSynced" } });
    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    fireEvent.click(screen.getByText("Export Keyglass Bundle..."));
    await waitFor(() => expect(releasePut).toBeDefined());
    expect(client.exportBundle).not.toHaveBeenCalled();
    releasePut();
    await waitFor(() => expect(client.exportBundle).toHaveBeenCalled());
    expect(events).toEqual(["put:ExportSynced", "export:/tmp/export.openflux"]);

    // A failed write blocks export instead of exporting stale data.
    failNextPut = true;
    fireEvent.click(screen.getByRole("button", { name: "Themes" }));
    fireEvent.change(screen.getByDisplayValue("ExportSynced"), { target: { value: "WillFail" } });
    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    fireEvent.click(screen.getByText("Export Keyglass Bundle..."));
    await waitFor(() => expect(screen.getByText(/Cannot export: earlier config update/i)).toBeTruthy());
    expect(client.exportBundle).toHaveBeenCalledTimes(1);
  });

  it("loads icon thumbnails past the first 48 and reloads them when a filter with the same count is applied", async () => {
    const { client } = createMockClient();
    const allIcons = [
      ...Array.from({ length: 65 }, (_, i) => ({
        id: `alpha_${String(i).padStart(2, "0")}.png`,
        category: "key_icons",
        name: `alpha_${i}`,
      })),
      ...Array.from({ length: 65 }, (_, i) => ({
        id: `beta_${String(i).padStart(2, "0")}.png`,
        category: "key_icons",
        name: `beta_${i}`,
      })),
    ];
    const fetchedAssets: string[] = [];
    client.getAssetsCatalog = vi.fn(async () => ({
      fonts: [],
      keyIcons: allIcons,
      media: [],
      presets: [],
      shortcutPacks: [],
    }));
    client.getAssetDataUrl = vi.fn(async (_cat: string, name: string) => {
      fetchedAssets.push(name);
      return `data:image/png;base64,AA==`;
    });

    render(<App client={client} />);
    await waitFor(() => expect(client.getAssetsCatalog).toHaveBeenCalled());

    fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
    fireEvent.click(screen.getByText("Pick Icon..."));
    fireEvent.change(screen.getByPlaceholderText(/Filter icons/i), { target: { value: "alpha_" } });
    await waitFor(() => expect(fetchedAssets).toContain("alpha_60.png"));
    fireEvent.change(screen.getByPlaceholderText(/Filter icons/i), { target: { value: "beta_" } });
    await waitFor(() => expect(fetchedAssets).toContain("beta_60.png"));
  });

  it("shows icon thumbnails and font specimens that finish loading after the search changes", async () => {
    const { client } = createMockClient();
    client.getAssetsCatalog = vi.fn(async () => ({
      fonts: [
        { family: "SlowSans", files: ["SlowSans.ttf"], hasItalic: false, isVariable: false },
        { family: "QuickSerif", files: ["QuickSerif.ttf"], hasItalic: false, isVariable: false },
      ],
      keyIcons: [
        { id: "alpha_slow.png", category: "key_icons", name: "alpha_slow" },
        { id: "alpha_quick.png", category: "key_icons", name: "alpha_quick" },
        { id: "beta_quick.png", category: "key_icons", name: "beta_quick" },
      ],
      media: [],
      presets: [],
      shortcutPacks: [],
    }));
    const slowLoads = new Map<string, () => void>();
    client.getAssetDataUrl = vi.fn(
      (_cat: string, name: string) =>
        new Promise<string>((resolve) => {
          const done = () => resolve("data:image/png;base64,AA==");
          if (name.includes("Slow") || name.includes("slow")) slowLoads.set(name, done);
          else done();
        }),
    );
    // jsdom has no font loading API; these stand in for the browser's.
    vi.stubGlobal(
      "FontFace",
      class {
        load() {
          return Promise.resolve(this);
        }
      },
    );
    Object.defineProperty(document, "fonts", { configurable: true, value: { add: () => {} } });

    try {
      render(<App client={client} initialTab="themes" />);
      await waitFor(() => expect(client.getAssetsCatalog).toHaveBeenCalled());

      // The search narrows the list while the slow font is still loading.
      fireEvent.click(screen.getByRole("tab", { name: "Typography & Styles" }));
      fireEvent.click(await screen.findByText(/Browse All Fonts/));
      await waitFor(() => expect(slowLoads.has("SlowSans.ttf")).toBe(true));
      fireEvent.change(screen.getByPlaceholderText(/Search fonts/i), { target: { value: "slow" } });
      slowLoads.get("SlowSans.ttf")?.();
      const fontBrowser = within(screen.getByRole("dialog", { name: "Font Browser" }));
      await waitFor(() =>
        expect(fontBrowser.getByText("SlowSans").closest("button")?.textContent).not.toMatch(/Loading/),
      );
      fireEvent.click(fontBrowser.getByText("Close"));

      // The same holds for an icon thumbnail.
      fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
      fireEvent.click(screen.getByText("Pick Icon..."));
      await waitFor(() => expect(slowLoads.has("alpha_slow.png")).toBe(true));
      fireEvent.change(screen.getByPlaceholderText(/Filter icons/i), { target: { value: "alpha_" } });
      slowLoads.get("alpha_slow.png")?.();
      expect(await screen.findByAltText("alpha_slow")).toBeTruthy();
    } finally {
      vi.unstubAllGlobals();
      Reflect.deleteProperty(document, "fonts");
    }
  });

  it("shows a macro read from the keyboard as read-only and keeps it when a new macro is added", async () => {
    const { client } = createMockClient();
    const updates: Partial<OpenFluxConfig>[] = [];
    client.updateConfig = vi.fn(async (p: Partial<OpenFluxConfig>) => {
      updates.push(p);
    });
    const cfgWithRaw = structuredClone(TEST_CONFIG);
    cfgWithRaw.keymap.macros = [{ name: "RawPrintScreen", words: [0x80000046] }];
    client.getConfig = vi.fn(async () => cfgWithRaw);

    render(<App client={client} />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());

    fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
    expect(screen.getByText(/This macro came from the keyboard and can't be edited here/i)).toBeTruthy();

    fireEvent.click(screen.getByText("+ New Macro"));
    await waitFor(() =>
      expect(
        updates.some(
          (u) =>
            Array.isArray(u.keymap?.macros) &&
            u.keymap.macros.length === 2 &&
            u.keymap.macros[0].words?.[0] === 0x80000046,
        ),
      ).toBe(true),
    );
  });

  it("creates a profile, binds actuation and an app matcher, and switches profiles from the top bar", async () => {
    const { client, state } = createMockClient();
    render(<App client={client} />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());

    fireEvent.click(screen.getByRole("button", { name: "Profiles" }));
    fireEvent.change(screen.getByPlaceholderText(/New profile name/i), {
      target: { value: "Blender Sculpt" },
    });
    fireEvent.click(screen.getByRole("button", { name: "New Profile" }));

    await waitFor(() => {
      expect(state.profiles?.length).toBe(3);
      expect(state.profiles?.[2].name).toBe("Blender Sculpt");
      expect(state.desiredProfileIndex).toBe(2);
    });

    fireEvent.change(screen.getByLabelText("Bound Actuation Mode"), { target: { value: "1" } });
    fireEvent.change(screen.getByLabelText("Profile App Matcher"), {
      target: { value: "org.blenderfoundation.blender" },
    });

    await waitFor(() => {
      expect(state.profiles?.[2].actuation_mode_index).toBe(1);
      expect(state.profiles?.[2].app_matcher).toBe("org.blenderfoundation.blender");
    });

    fireEvent.change(screen.getByLabelText("Active Profile"), { target: { value: "0" } });
    await waitFor(() => {
      expect(state.desiredProfileIndex).toBe(0);
    });
  });

  it("remaps existing appRules when a preceding profile is deleted before later app-rule edits", async () => {
    const initial = structuredClone(TEST_CONFIG);
    initial.appRules = [{ matcher: "com.manual.rule", profile_index: 1, language_code: 471 }];
    const { client, state } = createMockClient(initial);
    render(<App client={client} />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());

    // Deleting profile 0 moves "VS Code" to index 0, and its rule follows.
    fireEvent.click(screen.getByRole("button", { name: "Profiles" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => {
      expect(state.profiles?.length).toBe(1);
      expect(state.profiles?.[0].id).toBe("coding");
      expect(state.appRules.find((r) => r.matcher === "com.manual.rule")?.profile_index).toBe(0);
    });

    // A later rule edit keeps the remapped rule at index 0.
    fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
    fireEvent.change(screen.getByPlaceholderText("com.microsoft.VSCode"), {
      target: { value: "com.apple.Safari" },
    });
    fireEvent.click(screen.getByText("Add App Rule"));
    await waitFor(() => {
      expect(state.appRules.find((r) => r.matcher === "com.manual.rule")?.profile_index).toBe(0);
      expect(state.appRules.some((r) => r.matcher === "com.apple.Safari")).toBe(true);
    });
  });

  it("applies imported media to the theme that was selected when the import started", async () => {
    const initial = structuredClone(TEST_CONFIG);
    initial.appearance.customThemes = [
      {
        ...structuredClone(TEST_CONFIG.appearance.defaultTheme),
        id: "theme_a",
        name: "Theme A",
        backgroundImagePath: "orig_a.mp4",
      },
      {
        ...structuredClone(TEST_CONFIG.appearance.defaultTheme),
        id: "theme_b",
        name: "Theme B",
        backgroundImagePath: "orig_b.mp4",
      },
    ];
    initial.appearance.selectedId = "theme_a";
    let releaseImport!: (r: { assetPath: string; isVideo: boolean; transcoded: boolean; sizeBytes: number }) => void;
    const { client, state } = createMockClient(initial);
    client.importMedia = vi.fn(
      () =>
        new Promise<{ assetPath: string; isVideo: boolean; transcoded: boolean; sizeBytes: number }>((resolve) => {
          releaseImport = resolve;
        }),
    );

    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());

    // Start importing media while Theme A is selected
    client.pickFile = vi.fn(async () => "/tmp/clip_for_a.mov");
    fireEvent.click(screen.getByRole("button", { name: "+ Import Image / Video" }));
    await waitFor(() => expect(releaseImport).toBeDefined());

    // Switch to Theme B before the import finishes.
    fireEvent.change(screen.getByLabelText("Editing Theme"), { target: { value: "theme_b" } });
    await waitFor(() => expect(state.appearance.selectedId).toBe("theme_b"));

    // Theme A gets the imported file; Theme B and the default theme are unchanged.
    releaseImport({ assetPath: "imported_a.mp4", isVideo: true, transcoded: true, sizeBytes: 1024 });
    await waitFor(() => {
      expect(state.appearance.customThemes.find((t) => t.id === "theme_a")?.backgroundImagePath).toBe("imported_a.mp4");
    });
    expect(state.appearance.customThemes.find((t) => t.id === "theme_b")?.backgroundImagePath).toBe("orig_b.mp4");
    expect(state.appearance.defaultTheme.backgroundImagePath).toBe("aurora_drift.frag");
  });

  it("follows a profile switched by an app rule on the keyboard while the chosen profile stays the same", async () => {
    vi.useFakeTimers();
    try {
      const initial = structuredClone(TEST_CONFIG);
      initial.appearance.customThemes = [
        { ...structuredClone(TEST_CONFIG.appearance.defaultTheme), id: "theme_coding", name: "Coding Theme" },
      ];
      const { client } = createMockClient(initial);
      let currentApplied = 0;
      let currentThemeId = "default";
      client.getConfig = vi.fn(async () => {
        const c = structuredClone(initial);
        c.appearance.selectedId = currentThemeId;
        return c;
      });
      client.getStatus = vi.fn(async () => ({
        connected: { bridge: true, firmware: true },
        versions: { firmware: "openflux-firmware 1.0.0", bridge: "openflux-bridge 1.0.0" },
        desiredProfileIndex: 0,
        appliedProfileIndex: currentApplied,
      }));

      render(<App client={client} initialTab="themes" />);
      await vi.advanceTimersByTimeAsync(100);
      expect((screen.getByLabelText("Active Profile") as HTMLSelectElement).value).toBe("0");
      expect((screen.getByLabelText("Editing Theme") as HTMLSelectElement).value).toBe("default");
      expect(client.getConfig).toHaveBeenCalledTimes(1);

      // An app rule on the daemon switches the applied profile and theme; the desired profile stays 0.
      currentApplied = 1;
      currentThemeId = "theme_coding";
      await vi.advanceTimersByTimeAsync(2100);
      expect((screen.getByLabelText("Active Profile") as HTMLSelectElement).value).toBe("1");
      expect(client.getConfig).toHaveBeenCalledTimes(2);
      expect((screen.getByLabelText("Editing Theme") as HTMLSelectElement).value).toBe("theme_coding");
    } finally {
      vi.useRealTimers();
    }
  });

  it("refreshes after a poll only once pending edits are written", async () => {
    vi.useFakeTimers();
    try {
      const initial = structuredClone(TEST_CONFIG);
      const { client, calls } = createMockClient(initial);
      let releasePut!: () => void;
      let currentApplied = 0;
      let putCount = 0;
      let serverThemeName = "Test Theme";

      client.updateConfig = vi.fn(async (patch) => {
        putCount++;
        if (putCount === 1) {
          await new Promise<void>((resolve) => {
            releasePut = resolve;
          });
        }
        if (patch.appearance) {
          serverThemeName = patch.appearance.defaultTheme.name;
        }
        calls.push(`put:${serverThemeName}`);
      });
      client.getConfig = vi.fn(async () => {
        calls.push(`get:${serverThemeName}`);
        const c = structuredClone(TEST_CONFIG);
        c.appearance.defaultTheme.name = serverThemeName;
        return c;
      });
      client.getStatus = vi.fn(async () => ({
        connected: { bridge: true, firmware: true },
        versions: { firmware: "openflux-firmware 1.0.0", bridge: "openflux-bridge 1.0.0" },
        desiredProfileIndex: 0,
        appliedProfileIndex: currentApplied,
      }));

      render(<App client={client} initialTab="themes" />);
      await vi.advanceTimersByTimeAsync(100);
      expect(calls).toContain("get:Test Theme");

      // Hold an edit in flight.
      fireEvent.change(screen.getByDisplayValue("Test Theme"), {
        target: { value: "PendingEdit" },
      });
      await vi.advanceTimersByTimeAsync(10);
      expect(releasePut).toBeDefined();

      // The poll sees a profile switch while the write is pending.
      currentApplied = 1;
      await vi.advanceTimersByTimeAsync(2100);
      // The refresh waits for the pending write.
      expect(calls.filter((c) => c.startsWith("get:")).length).toBe(1);

      releasePut();
      await vi.advanceTimersByTimeAsync(50);
      expect(calls.indexOf("put:PendingEdit")).toBeLessThan(calls.lastIndexOf("get:PendingEdit"));
      expect(screen.getByDisplayValue("PendingEdit")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the manually chosen profile when the auto-switched profile is deleted", async () => {
    const initial = structuredClone(TEST_CONFIG);
    initial.profiles = [
      {
        id: "c",
        name: "C",
        theme_id: "default",
        contexts: ["Main"],
        active_context: "Main",
        actuation_mode_index: 0,
        default_layer: 0,
      },
      {
        id: "a",
        name: "A Manual",
        theme_id: "default",
        contexts: ["Main"],
        active_context: "Main",
        actuation_mode_index: 0,
        default_layer: 0,
      },
      {
        id: "b",
        name: "B Auto",
        theme_id: "default",
        contexts: ["Main"],
        active_context: "Main",
        actuation_mode_index: 0,
        default_layer: 0,
      },
    ];
    initial.desiredProfileIndex = 1;
    const { client, state } = createMockClient(initial);
    client.getStatus = vi.fn(async () => ({
      connected: { bridge: true, firmware: true },
      versions: { firmware: "openflux-firmware 1.0.0", bridge: "openflux-bridge 1.0.0" },
      desiredProfileIndex: 1,
      appliedProfileIndex: 2,
    }));

    render(<App client={client} />);
    await waitFor(() => expect((screen.getByLabelText("Active Profile") as HTMLSelectElement).value).toBe("2"));
    fireEvent.click(screen.getByRole("button", { name: "Profiles" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(state.profiles?.map((p) => p.id)).toEqual(["c", "a"]);
      expect(state.desiredProfileIndex).toBe(1);
    });
  });

  it("renders again with new state once an in-flight preview frame finishes", async () => {
    let releaseFirst!: (buf: Uint8Array) => void;
    let callCount = 0;
    const { client } = createMockClient();
    client.renderPreviewRgba = vi.fn(
      () =>
        new Promise<Uint8Array>((resolve) => {
          callCount++;
          if (callCount === 1) {
            releaseFirst = resolve;
          } else {
            resolve(new Uint8Array(960 * 540 * 4));
          }
        }),
    );

    render(
      <React.StrictMode>
        <App client={client} />
      </React.StrictMode>,
    );
    await waitFor(() => expect(callCount).toBe(1));

    // Toggle sleep while the first frame is still rendering.
    fireEvent.click(screen.getByRole("button", { name: "Preview Sleep" }));
    expect(callCount).toBe(1);

    // The next frame starts right away and carries the new state.
    releaseFirst(new Uint8Array(960 * 540 * 4));
    await waitFor(() => expect(callCount).toBeGreaterThanOrEqual(2));
    const lastOpts = (client.renderPreviewRgba as ReturnType<typeof vi.fn>).mock.calls.at(-1)?.[0];
    expect(lastOpts?.suspended).toBe(true);
  });

  it("locks keymap and actuation edits until the keyboard's config is loaded, then shows it", async () => {
    vi.useFakeTimers();
    try {
      const unhydrated = structuredClone(TEST_CONFIG);
      delete (unhydrated.appearance.defaultTheme as { stylemaps?: unknown }).stylemaps;
      unhydrated.hydrated = { appearance: false, settings: false, mosaic: false, keymap: false };
      unhydrated.keymap.keymaps.ansi.layers[0].keyActions = Array.from({ length: 85 }, (_, i) => ({
        type: 0,
        keyCode: i === 0 ? "KC_ESCAPE" : "KC_A",
      }));

      const hydrated = structuredClone(TEST_CONFIG);
      hydrated.hydrated = { appearance: true, settings: true, mosaic: true, keymap: true };
      hydrated.actuationModes = [
        { id: "m0", name: "Typing", type: 1, actuationPointMm: 1.2, rapidTrigger: false },
        {
          id: "m1",
          name: "Gaming",
          type: 2,
          actuationPointMm: 1.0,
          rapidTrigger: true,
          rapidTriggerSensitivityMm: 0.3,
        },
      ];

      let phase = 0;
      const { client } = createMockClient(unhydrated);
      client.getConfig = vi.fn(async () => structuredClone(phase === 0 ? unhydrated : hydrated));
      client.getShortcutPack = vi.fn(async () => ({
        pack: "VSCode.csv",
        application: "Visual Studio Code",
        rows: [
          {
            context: "Edit",
            shortcut: "Cmd+C",
            label: "Copy",
            icon: "",
            macString: "Cmd + C",
            winString: "Ctrl + C",
            macKeyCode: "KC_C",
            macHasSecondKey: false,
            macModifiers: 8,
            winKeyCode: "KC_C",
            winHasSecondKey: false,
            winModifiers: 1,
          },
        ],
      }));
      client.getAssetsCatalog = vi.fn(async () => ({
        fonts: [],
        media: [],
        keyIcons: [],
        presets: [],
        shortcutPacks: [{ id: "VSCode.csv", name: "Visual Studio Code", rowCount: 1 }],
      }));
      client.getStatus = vi.fn(async () => ({
        connected: { bridge: true, firmware: true },
        versions: { firmware: "openflux-firmware 1.0.0", bridge: "openflux-bridge 1.0.0" },
        desiredProfileIndex: 0,
        appliedProfileIndex: 0,
      }));

      render(<App client={client} />);
      await vi.advanceTimersByTimeAsync(100);

      // Keymap opens without stylemaps and shows the sync lock.
      fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
      expect(screen.getByText(/Loading the keymap from the keyboard/i)).toBeTruthy();

      // ISO and + New Macro write nothing before the keymap is loaded.
      const isoBtn = screen.getByRole("button", { name: "ISO" }) as HTMLButtonElement;
      expect(isoBtn.disabled).toBe(true);
      fireEvent.click(isoBtn);
      fireEvent.click(screen.getByRole("button", { name: "+ New Macro" }));
      // Shortcuts keeps Apply disabled and writes nothing before then.
      fireEvent.click(screen.getByRole("button", { name: "Shortcuts" }));
      await vi.advanceTimersByTimeAsync(50);
      const applyBtn = screen.getByRole("button", { name: /Apply →/i }) as HTMLButtonElement;
      expect(applyBtn.disabled).toBe(true);
      fireEvent.click(applyBtn);
      await vi.advanceTimersByTimeAsync(50);
      expect(client.updateConfig).not.toHaveBeenCalled();

      // Actuation shows the unloaded values and can't overwrite the keyboard.
      fireEvent.click(screen.getByRole("button", { name: "Actuation" }));
      expect(screen.getAllByText(/Typing\/General/i).length).toBeGreaterThan(0);

      // The daemon loads the keyboard's config without a profile change.
      phase = 1;
      await vi.advanceTimersByTimeAsync(2100);
      expect(screen.getByRole("heading", { name: "Typing" })).toBeTruthy();
      expect(screen.getByText(/Actuation Point \(1\.2 mm\)/i)).toBeTruthy();
      // Keymap unlocks.
      fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
      expect(screen.queryByText(/Loading the keymap from the keyboard/i)).toBeNull();
      expect((screen.getByRole("button", { name: "ISO" }) as HTMLButtonElement).disabled).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  describe("when the keyboard's keymap can't be loaded", () => {
    const MISMATCH_REASON = "layer 1 key 20 uses KC_FF_PROFILE_CYCLE_NEXT, which Keyglass doesn't know";

    /** A daemon that loaded everything but the keymap, with the given keymap sync state. */
    function keymapStuck(keymapSync: NonNullable<DeviceStatus["keymapSync"]>) {
      const cfg = structuredClone(TEST_CONFIG);
      cfg.hydrated = { appearance: true, settings: true, mosaic: true, keymap: false };
      const mock = createMockClient(cfg);
      mock.keyboard.status.keymapSync = keymapSync;
      return mock;
    }

    it("explains a mismatch and unlocks editing once the firmware keymap is adopted", async () => {
      const { client, calls } = keymapStuck({ state: "mismatch", reason: MISMATCH_REASON });
      render(<App client={client} />);
      await waitFor(() => expect(calls).toContain("getStatus"));

      fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
      const message = `The keyboard's keymap file and its firmware keymap don't match: ${MISMATCH_REASON}`;
      await waitFor(() => expect(screen.getByText(message)).toBeTruthy());
      expect(screen.queryByText(/Loading the keymap from the keyboard/i)).toBeNull();
      expect((screen.getByRole("button", { name: "ISO" }) as HTMLButtonElement).disabled).toBe(true);

      // Actuation lives in the same keymap, so it says the same.
      fireEvent.click(screen.getByRole("button", { name: "Actuation" }));
      expect(screen.getByText(message)).toBeTruthy();

      fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
      fireEvent.click(screen.getByRole("button", { name: "Use the keyboard's firmware keymap" }));
      await waitFor(() => expect(screen.queryByText(message)).toBeNull());
      expect(calls).toContain("adoptFirmwareKeymap");
      expect(screen.queryByRole("button", { name: "Use the keyboard's firmware keymap" })).toBeNull();

      // Editing is open again: switching the layout writes the keymap.
      const iso = screen.getByRole("button", { name: "ISO" }) as HTMLButtonElement;
      expect(iso.disabled).toBe(false);
      fireEvent.click(iso);
      await waitFor(() => expect(calls).toContain("updateConfig:keymap"));
    });

    it("shows the daemon's refusal and keeps editing locked", async () => {
      const { client, calls, keyboard } = keymapStuck({ state: "mismatch", reason: MISMATCH_REASON });
      keyboard.adoptRefusal = new DaemonError({
        kind: "daemon",
        status: 409,
        code: "busy",
        message: "A keyboard update is running; try again when it finishes.",
      });
      render(<App client={client} />);
      await waitFor(() => expect(calls).toContain("getStatus"));

      fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
      fireEvent.click(await screen.findByRole("button", { name: "Use the keyboard's firmware keymap" }));
      const alert = await screen.findByRole("alert");
      expect(alert.textContent).toContain("A keyboard update is running; try again when it finishes.");
      expect(screen.getByText(/don't match: layer 1 key 20/)).toBeTruthy();
      expect((screen.getByRole("button", { name: "ISO" }) as HTMLButtonElement).disabled).toBe(true);
      expect(
        (screen.getByRole("button", { name: "Use the keyboard's firmware keymap" }) as HTMLButtonElement).disabled,
      ).toBe(false);
    });

    it("offers the same way out when reading the keymap failed", async () => {
      const { client, calls } = keymapStuck({ state: "error", reason: "keymap.json couldn't be read" });
      render(<App client={client} />);
      await waitFor(() => expect(calls).toContain("getStatus"));

      fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
      expect(
        await screen.findByText("Keyglass couldn't load the keyboard's keymap: keymap.json couldn't be read"),
      ).toBeTruthy();
      fireEvent.click(screen.getByRole("button", { name: "Use the keyboard's firmware keymap" }));
      await waitFor(() =>
        expect((screen.getByRole("button", { name: "ISO" }) as HTMLButtonElement).disabled).toBe(false),
      );
    });

    it("still shows the syncing message while the keymap is being read", async () => {
      const { client, calls } = keymapStuck({ state: "pulling", reason: null });
      render(<App client={client} />);
      await waitFor(() => expect(calls).toContain("getStatus"));

      fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
      expect(await screen.findByText(/Loading the keymap from the keyboard/i)).toBeTruthy();
      expect(screen.queryByRole("button", { name: "Use the keyboard's firmware keymap" })).toBeNull();
      expect((screen.getByRole("button", { name: "ISO" }) as HTMLButtonElement).disabled).toBe(true);
    });

    it("says the keyboard isn't connected when there is none", async () => {
      const { client, calls, keyboard } = keymapStuck({ state: "disconnected", reason: null });
      keyboard.status.connected = { bridge: false, firmware: false };
      render(<App client={client} />);
      await waitFor(() => expect(calls).toContain("getStatus"));

      fireEvent.click(screen.getByRole("button", { name: "Keymap" }));
      expect(await screen.findByText(/The keyboard isn't connected/)).toBeTruthy();
      expect(screen.queryByText(/Loading the keymap from the keyboard/i)).toBeNull();
    });
  });

  it("retries loading the config on the next status poll after the first attempt fails", async () => {
    vi.useFakeTimers();
    try {
      const realCfg = structuredClone(TEST_CONFIG);
      realCfg.actuationModes = [
        { id: "m0", name: "RecoveredTyping", type: 1, actuationPointMm: 1.4, rapidTrigger: false },
      ];
      let failFirst = true;
      const { client } = createMockClient(realCfg);
      client.getConfig = vi.fn(async () => {
        if (failFirst) {
          failFirst = false;
          throw new Error("daemon starting up");
        }
        return structuredClone(realCfg);
      });
      client.getStatus = vi.fn(async () => ({
        connected: { bridge: true, firmware: true },
        versions: { firmware: "openflux-firmware 1.0.0", bridge: "openflux-bridge 1.0.0" },
        desiredProfileIndex: 0,
        appliedProfileIndex: 0,
      }));

      render(<App client={client} />);
      await vi.advanceTimersByTimeAsync(100);
      fireEvent.click(screen.getByRole("button", { name: "Actuation" }));
      expect(screen.queryByRole("heading", { name: "RecoveredTyping" })).toBeNull();

      await vi.advanceTimersByTimeAsync(2100);
      expect(screen.getByRole("heading", { name: "RecoveredTyping" })).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows the brand, switches clock styles, drives the Pomodoro timer, and runs a backup from the Device tab", async () => {
    const { client, calls, state } = createMockClient();
    render(<App client={client} />);
    await waitFor(() => expect(calls).toContain("getConfig"));

    // Brand is shown; parallax lives on the Device tab, not Themes.
    expect(screen.getByText("Keyglass Studio")).toBeTruthy();
    expect(screen.queryByRole("tab", { name: /Parallax/i })).toBeNull();

    // Modules: one Clock card whose style switcher changes the tile type (12 -> 10 for Analog).
    fireEvent.click(screen.getByRole("button", { name: "Modules" }));
    expect(screen.queryByText("Digital Clock")).toBeNull();
    expect(screen.queryByText("Flip Clock")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Analog" }));
    await waitFor(() => expect(state.mosaic.tileTypes[0]).toBe(10));

    // The Pomodoro tile (slot 4) drives the timer through the daemon.
    fireEvent.click(screen.getByLabelText("Mosaic Slot 4"));
    fireEvent.click(screen.getByRole("button", { name: "Start / Pause Timer" }));
    await waitFor(() => expect(calls).toContain("pomodoroAction:toggle"));
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    await waitFor(() => expect(calls).toContain("pomodoroAction:reset"));

    // Device: parallax, SoC stats and a backup through the folder picker.
    fireEvent.click(screen.getByRole("button", { name: "Device" }));
    expect(screen.getByText(/Parallax & Optical Alignment/i)).toBeTruthy();
    expect(screen.getByText(/Keyboard SoC Performance/i)).toBeTruthy();
    await waitFor(() => expect(screen.getByText("19%")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: /Choose Folder & Run Device Backup/i }));
    await waitFor(() => expect(calls).toContain("backupDevice:/tmp/backup"));
  });

  it("imports Aerial videos without a length setting and reports the resolution and bitrate chosen", async () => {
    const { client } = createMockClient();
    client.getAerialVideos = vi.fn(async () => [
      {
        id: "a1",
        title: "Tokyo Night",
        quality: "4K",
        thumbnailUrl: "data:,",
        videoUrl: "https://example.test/tokyo.mov",
      },
    ]);
    client.importAerialVideo = vi.fn(async () => ({
      assetPath: "tokyo.mp4",
      isVideo: true,
      transcoded: true,
      resolution: "1280x720",
      bitrateKbps: 2400,
    }));
    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Aerial Videos" }));
    await waitFor(() => screen.getByTitle("Download & apply Tokyo Night"));
    expect(screen.getByText(/Videos keep their full length/)).toBeTruthy();
    expect(screen.queryByLabelText(/length/i)).toBeNull();

    fireEvent.click(screen.getByTitle("Download & apply Tokyo Night"));
    await waitFor(() =>
      expect(client.importAerialVideo).toHaveBeenCalledWith("https://example.test/tokyo.mov", "Tokyo Night", {
        loopReverse: false,
      }),
    );
    await waitFor(() => screen.getByText("Applied Aerial video: Tokyo Night (1280x720 · 2.4 Mbit/s)"));
  });

  it("shows the daemon's reason when an Aerial video can't fit", async () => {
    const { client } = createMockClient();
    client.getAerialVideos = vi.fn(async () => [
      {
        id: "a2",
        title: "Long Flight",
        quality: "4K",
        thumbnailUrl: "data:,",
        videoUrl: "https://example.test/long.mov",
      },
    ]);
    client.importAerialVideo = vi.fn(async () => {
      throw new DaemonError({
        kind: "daemon",
        status: 413,
        message: "This video is too long to fit at acceptable quality; trim it before importing",
      });
    });
    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Aerial Videos" }));
    await waitFor(() => screen.getByTitle("Download & apply Long Flight"));
    fireEvent.click(screen.getByTitle("Download & apply Long Flight"));
    await waitFor(() =>
      screen.getByText(
        'Couldn\'t import "Long Flight": This video is too long to fit at acceptable quality; trim it before importing',
      ),
    );
  });

  it("marks a theme picked on the Themes tab as a pick, even the one already selected", async () => {
    const initial = structuredClone(TEST_CONFIG);
    initial.appearance.customThemes = [
      { ...structuredClone(initial.appearance.defaultTheme), id: "theme_b", name: "Theme B" },
    ];
    const { client, calls } = createMockClient(initial);
    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());
    fireEvent.change(screen.getByLabelText("Editing Theme"), { target: { value: "theme_b" } });
    await waitFor(() => expect(calls).toContain("updateConfig:appearance,themeSelection"));
    expect(client.updateConfig).toHaveBeenLastCalledWith(
      expect.objectContaining({ themeSelection: true, appearance: expect.objectContaining({ selectedId: "theme_b" }) }),
    );
  });

  it("shows whether automatic profile switching is locked or held and locks it from the top bar", async () => {
    const { client, calls, keyboard } = createMockClient();
    keyboard.status.autoProfileSwitch = { locked: false, heldUntilAppChange: true };
    render(<App client={client} />);
    const button = await screen.findByRole("button", { name: "Auto-switch held" });
    expect(button.getAttribute("aria-pressed")).toBe("false");

    fireEvent.click(button);
    const locked = await screen.findByRole("button", { name: "Auto-switch locked" });
    expect(locked.getAttribute("aria-pressed")).toBe("true");
    expect(calls).toContain("setAutoProfileLock:true");

    fireEvent.click(locked);
    await screen.findByRole("button", { name: "Auto-switch on" });
    expect(calls).toContain("setAutoProfileLock:false");
  });

  it("explains the media limit for local videos and shows the daemon's reason when a file can't fit", async () => {
    const { client } = createMockClient();
    client.pickFile = vi.fn(async () => "/tmp/long.mov");
    client.importMedia = vi.fn(async () => {
      throw new DaemonError({
        kind: "daemon",
        status: 413,
        message: "This video is too long to fit at acceptable quality; trim it before importing",
      });
    });
    render(<App client={client} initialTab="themes" />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());
    expect(screen.getByText(/keyboard's 96 MiB media limit/)).toBeTruthy();
    fireEvent.click(screen.getByText("+ Import Image / Video"));
    await waitFor(() =>
      screen.getByText(
        "Couldn't import the file: This video is too long to fit at acceptable quality; trim it before importing",
      ),
    );
  });

  it("lands on Profiles with the live keyboard preview and profile binding controls", async () => {
    const { client } = createMockClient();
    render(<App client={client} />);
    await waitFor(() => expect(client.getConfig).toHaveBeenCalled());
    expect(screen.getByLabelText("Profile Settings")).toBeTruthy();
    expect(screen.getByLabelText("Bound Shortcut Pack")).toBeTruthy();
    expect(screen.getByLabelText("Bound Theme")).toBeTruthy();
    expect(screen.getByLabelText("Bound Actuation Mode")).toBeTruthy();
    expect(screen.getByLabelText("Bound Default Layer")).toBeTruthy();
    await waitFor(() => expect(client.renderPreviewRgba).toHaveBeenCalled());
  });

  it("surfaces a factory keyboard conversion banner on the landing page and jumps to Updates", async () => {
    const { client } = createMockClient();
    const daemon = new MockDaemon(FACTORY_KEYBOARD);
    render(<App client={client} services={mockServices(daemon)} />);
    const callout = await screen.findByLabelText("Convert keyboard to Keyglass");
    expect(callout.textContent).toContain("Factory Flux keyboard detected");
    expect(screen.getByText("Setup Required")).toBeTruthy();
    fireEvent.click(within(callout).getByRole("button", { name: "Convert to Keyglass…" }));
    await waitFor(() => expect(screen.getByRole("heading", { name: "Updates" })).toBeTruthy());
  });
});
