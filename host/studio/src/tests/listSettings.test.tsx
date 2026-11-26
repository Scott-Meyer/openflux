import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "../App";
import type { V2ModuleRecord } from "../api/v2Client";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const id = "test-mqtt-buttons";
const name = "Test MQTT Buttons";

/** A daemon holding one module with a `buttons` list setting; PUT /state replaces config keys like the real one. */
function stubDaemon(config: Record<string, unknown>) {
  const mod: V2ModuleRecord = {
    id,
    manifest: {
      id,
      name,
      version: "1.0.0",
      description: "Buttons that publish MQTT",
      author: "Test",
      category: "companion",
      stages: [],
      host_requirement: "none",
      slot_capable: false,
      stackable: true,
      entrypoints: {},
      channels: { subscribes: [], publishes: [] },
      settings: [
        {
          key: "buttons",
          label: "Buttons",
          kind: "list",
          default: [],
          fields: [
            { key: "label", label: "Label", kind: "string", default: "" },
            { key: "mode", label: "Mode", kind: "select", options: ["tap", "hold"], default: "tap" },
            { key: "topic", label: "Topic", kind: "string", default: "" },
          ],
        },
      ],
    },
    tier: "local",
    path: `/modules/${id}`,
    healthy: true,
    state: { enabled: true, config },
  };
  let rev = 1;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init?: RequestInit) => {
      if (url === "/v2/status") return Response.json({ pid: 123, modules_rev: rev, usb_protocol: "ofv2" });
      if (url === "/v2/modules") return Response.json({ modules: [structuredClone(mod)], modules_rev: rev });
      if (url === `/v2/modules/${id}/state` && init?.method === "PUT") {
        const patch = JSON.parse(String(init.body)) as { config?: Record<string, unknown> };
        mod.state.config = { ...mod.state.config, ...patch.config };
        rev += 1;
        return Response.json({ ok: true });
      }
      return Response.json({ error: "Not available in this test" }, { status: 404 });
    }),
  );
  return {
    stored: () => mod.state.config.buttons as Array<Record<string, unknown>>,
    /** A change made outside Studio, like `fluxctl modules config`. */
    setFromCli: (buttons: unknown[]) => {
      mod.state.config = { ...mod.state.config, buttons };
      rev += 1;
    },
  };
}

async function openModule() {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  await act(async () => {
    render(<App />);
  });
  fireEvent.click(await within(screen.getByRole("navigation")).findByRole("button", { name }));
}

it("edits list rows in place and keeps keys the manifest doesn't declare", async () => {
  const daemon = stubDaemon({
    buttons: [{ id: "btn-1", label: "Lamp", mode: "tap", topic: "home/lamp", icon_extra: 7 }],
  });
  await openModule();

  // Collapsed to a summary until opened.
  const toggle = await screen.findByRole("button", { name: /^Lamp/ });
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByLabelText("Topic")).toBeNull();

  fireEvent.click(toggle);
  fireEvent.change(screen.getByLabelText("Mode"), { target: { value: "hold" } });
  await waitFor(() => expect(daemon.stored()[0].mode).toBe("hold"));
  expect(daemon.stored()).toEqual([{ id: "btn-1", label: "Lamp", mode: "hold", topic: "home/lamp", icon_extra: 7 }]);

  // Typing stays responsive and lands once typing pauses.
  const topic = screen.getByLabelText("Topic") as HTMLInputElement;
  fireEvent.change(topic, { target: { value: "home/lamp/" } });
  fireEvent.change(topic, { target: { value: "home/lamp/set" } });
  expect(topic.value).toBe("home/lamp/set");
  await waitFor(() => expect(daemon.stored()[0].topic).toBe("home/lamp/set"));
  expect(daemon.stored()[0].id).toBe("btn-1");
});

it("adds, reorders and removes rows", async () => {
  const daemon = stubDaemon({ buttons: [{ id: "btn-1", label: "Lamp", mode: "tap", topic: "home/lamp" }] });
  await openModule();
  await screen.findByRole("button", { name: /^Lamp/ });

  fireEvent.click(screen.getByRole("button", { name: "Add Buttons item" }));
  await waitFor(() => expect(daemon.stored()).toHaveLength(2));
  expect(daemon.stored()[1]).toEqual({ label: "", mode: "tap", topic: "" });

  // The new row opens for editing; name it.
  fireEvent.change(screen.getByLabelText("Label"), { target: { value: "Fan" } });
  await waitFor(() => expect(daemon.stored()[1].label).toBe("Fan"));

  fireEvent.click(screen.getByRole("button", { name: "Move Fan up" }));
  await waitFor(() => expect(daemon.stored().map((b) => b.label)).toEqual(["Fan", "Lamp"]));
  expect(daemon.stored()[1].id).toBe("btn-1");

  fireEvent.click(screen.getByRole("button", { name: "Remove Lamp" }));
  expect(daemon.stored()).toHaveLength(2); // asks first
  fireEvent.click(screen.getByRole("button", { name: "Confirm removing Lamp" }));
  await waitFor(() => expect(daemon.stored().map((b) => b.label)).toEqual(["Fan"]));
});

it("shows a list changed elsewhere, e.g. by fluxctl", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  try {
    const daemon = stubDaemon({ buttons: [{ id: "btn-1", label: "Lamp", mode: "tap", topic: "" }] });
    await openModule();
    await screen.findByRole("button", { name: /^Lamp/ });

    daemon.setFromCli([...daemon.stored(), { id: "btn-2", label: "Garage", mode: "hold", topic: "home/garage" }]);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });
    expect(await screen.findByRole("button", { name: /^Garage/ })).toBeTruthy();
  } finally {
    vi.useRealTimers();
  }
});
