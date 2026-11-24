import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { V2ModuleRecord } from "../api/v2Client";
import { App } from "../App";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("shows module installs and settings from the daemon without reopening Studio", async () => {
  vi.useFakeTimers();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  const id = "test-key-faces";
  const name = "Test Key Faces";
  const label = "Demo in browser";
  const mod: V2ModuleRecord = {
    id,
    manifest: {
      id,
      name,
      version: "1.0.0",
      description: "Shows sample key faces",
      author: "Test",
      category: "companion",
      stages: [],
      host_requirement: "none",
      slot_capable: false,
      stackable: true,
      entrypoints: {},
      channels: { subscribes: [], publishes: [] },
      settings: [{ key: "demo", kind: "boolean", label, default: false }],
    },
    tier: "local",
    path: `/modules/${id}`,
    healthy: true,
    state: { enabled: true, config: { demo: false } },
  };
  let modules: V2ModuleRecord[] = [];
  let revision = 1;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => {
      if (url === "/v2/status") return Response.json({ pid: 123, modules_rev: revision, usb_protocol: "ofv2" });
      if (url === "/v2/modules") return Response.json({ modules, modules_rev: revision });
      return Response.json({ error: "Not available in this test" }, { status: 404 });
    }),
  );

  await act(async () => {
    render(<App />);
  });
  const sidebar = within(screen.getByRole("navigation"));
  expect(sidebar.queryByRole("button", { name })).toBeNull();

  modules = [mod];
  revision += 1;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  fireEvent.click(sidebar.getByRole("button", { name }));
  expect((screen.getByRole("checkbox", { name: label }) as HTMLInputElement).checked).toBe(false);

  modules = [{ ...mod, state: { enabled: true, config: { demo: true } } }];
  revision += 1;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect((screen.getByRole("checkbox", { name: label }) as HTMLInputElement).checked).toBe(true);
});

it("distinguishes readback from display activation and reports a failed verification", async () => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  let readbackError: string | null = null;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => {
      if (url === "/v2/status") return Response.json({ pid: 123, modules_rev: 1, usb_protocol: "ofv2" });
      if (url === "/v2/modules") return Response.json({ modules: [], modules_rev: 1 });
      if (url === "/v2/keyboard/sync") {
        return Response.json({
          ok: !readbackError,
          uploaded_files: ["/modules_state.json"],
          skipped_unchanged: 77,
          persisted_verified: !readbackError,
          persisted_error: readbackError,
          runtime_applied: null,
        });
      }
      return Response.json({ error: "Not available in this test" }, { status: 404 });
    }),
  );

  await act(async () => {
    render(<App />);
  });
  const sync = screen.getByRole("button", { name: "Sync to Keyboard" });
  fireEvent.click(sync);
  await screen.findByText(/verified on keyboard; display activation not confirmed/);

  readbackError = "Keyboard disconnected before settings could be verified.";
  fireEvent.click(sync);
  await screen.findByText(readbackError);
});
