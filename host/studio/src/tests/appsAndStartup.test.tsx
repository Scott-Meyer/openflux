import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import type { DaemonClient } from "../api/daemonClient";
import type { ServiceInspection, ServiceStatus } from "../api/daemonService";
import { DaemonStartup } from "../components/DaemonStartup";
import { ThirdPartyAppsPage } from "../components/ThirdPartyAppsPage";
import { TEST_CONFIG } from "./fixtures";
import { MockDaemon, MockThirdParty, mockDaemonService, mockServices, refusal } from "./mockDaemon";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("Apps page", () => {
  it("approves and denies waiting apps, removes access, and lets a denied app ask again", async () => {
    const tp = new MockThirdParty();
    tp.regs.pending.push({ id: "def456", app: "Window Helper", requestedAt: Math.floor(Date.now() / 1000) });
    const client = tp.client();
    const onPending = vi.fn();
    render(<ThirdPartyAppsPage client={client} onPendingCount={onPending} pollMs={50} />);
    await waitFor(() => screen.getByText("Editor Plugin"));
    expect(onPending).toHaveBeenLastCalledWith(2);

    const editor = document.querySelector('[data-pending="Editor Plugin"]') as HTMLElement;
    fireEvent.click(within(editor).getByText("Allow"));
    await waitFor(() => expect(document.querySelector('[data-approved="Editor Plugin"]')).toBeTruthy());
    expect(client.decide).toHaveBeenCalledWith("abc123", "approve");
    expect(screen.getByText("Editor Plugin approved")).toBeTruthy();

    const helper = document.querySelector('[data-pending="Window Helper"]') as HTMLElement;
    fireEvent.click(within(helper).getByText("Deny"));
    await waitFor(() => expect(document.querySelector('[data-denied="Window Helper"]')).toBeTruthy());
    expect(onPending).toHaveBeenLastCalledWith(0);
    expect(screen.getByText("No app is asking for access.")).toBeTruthy();

    const status = document.querySelector('[data-approved="Status Helper"]') as HTMLElement;
    fireEvent.click(within(status).getByText("Remove access…"));
    fireEvent.click(within(screen.getByRole("dialog")).getByText("Remove and block"));
    await waitFor(() => expect(document.querySelector('[data-denied="Status Helper"]')).toBeTruthy());
    expect(client.revoke).toHaveBeenCalledWith("Status Helper", true);

    const old = document.querySelector('[data-denied="Old Tool"]') as HTMLElement;
    fireEvent.click(within(old).getByText("Let it ask again"));
    await waitFor(() => expect(document.querySelector('[data-denied="Old Tool"]')).toBeNull());
    expect(client.forgetDenied).toHaveBeenCalledWith("Old Tool");
  });

  it("shows an app that starts waiting while the page is open", async () => {
    const tp = new MockThirdParty();
    tp.regs.pending = [];
    render(<ThirdPartyAppsPage client={tp.client()} pollMs={20} />);
    await waitFor(() => screen.getByText("No app is asking for access."));
    tp.regs.pending.push({ id: "n1", app: "Late App", requestedAt: Math.floor(Date.now() / 1000) });
    await waitFor(() => expect(screen.getByText("Late App")).toBeTruthy());
  });

  it("changes the API switch and permissions, and reports discovery files it couldn't write", async () => {
    const tp = new MockThirdParty();
    tp.discoveryFailures = ["/home/u/Documents/flux/config/keys.json: permission denied"];
    const client = tp.client();
    render(<ThirdPartyAppsPage client={client} platform="macos" />);
    await waitFor(() => screen.getByLabelText("Allow apps to use the Keyglass API"));
    fireEvent.click(screen.getByLabelText(/Switch profiles/));
    await waitFor(() => expect(client.putSettings).toHaveBeenCalledWith({ profileChange: false }));
    expect(await screen.findByText(/permission denied/)).toBeTruthy();
    expect(screen.getByText(/macOS asks whether Keyglass may access the Documents folder/)).toBeTruthy();

    fireEvent.click(screen.getByLabelText("Allow apps to use the Keyglass API"));
    await waitFor(() => expect(client.putSettings).toHaveBeenCalledWith({ apiActive: false }));
    await waitFor(() =>
      expect((screen.getByLabelText(/Save to the keyboard/) as HTMLInputElement).closest("fieldset")?.disabled).toBe(
        true,
      ),
    );
  });

  it("waits for one permission change before allowing the next", async () => {
    const tp = new MockThirdParty();
    const client = tp.client();
    const put = client.putSettings;
    let release: () => void = () => {};
    client.putSettings = vi.fn(async (patch) => {
      await new Promise<void>((r) => (release = r));
      return put(patch);
    });
    render(<ThirdPartyAppsPage client={client} />);
    await waitFor(() => screen.getByLabelText(/Import settings/));
    const box = screen.getByLabelText(/Import settings/) as HTMLInputElement;
    fireEvent.click(box);
    await waitFor(() => expect(box.closest("fieldset")?.disabled).toBe(true));
    expect((screen.getByLabelText("Allow apps to use the Keyglass API") as HTMLInputElement).disabled).toBe(true);
    await act(async () => release());
    await waitFor(() => expect(box.closest("fieldset")?.disabled).toBe(false));
    expect(client.putSettings).toHaveBeenCalledTimes(1);
  });

  it("puts a setting back when the daemon refuses it", async () => {
    const tp = new MockThirdParty();
    const client = tp.client();
    client.putSettings = vi.fn(async () => {
      throw refusal("invalid", "Failed to save settings: disk full", 500);
    });
    render(<ThirdPartyAppsPage client={client} />);
    await waitFor(() => screen.getByLabelText(/Import settings/));
    const box = screen.getByLabelText(/Import settings/) as HTMLInputElement;
    fireEvent.click(box);
    expect(await screen.findByText("Failed to save settings: disk full")).toBeTruthy();
    expect(box.checked).toBe(true);
  });
});

function inspection(patch: Partial<ServiceInspection> = {}): ServiceInspection {
  return {
    platform: "macos",
    manager: "launchAgent",
    registration: "notRegistered",
    daemonBinary: "/Applications/OpenFlux Studio.app/Contents/MacOS/openfluxd",
    detail: null,
    ...patch,
  };
}

function statusOf(reachable: boolean, svc: ServiceInspection, notRunning = !reachable): ServiceStatus {
  return { reachable, notRunning, unreachableReason: reachable ? null : "connection refused", service: svc };
}

describe("First launch", () => {
  it("opens Studio at once when the daemon answers", async () => {
    const service = mockDaemonService(null);
    const onReady = vi.fn();
    render(<DaemonStartup service={service} onReady={onReady} pollMs={5} />);
    await waitFor(() => expect(onReady).toHaveBeenCalled());
    expect(service.start).not.toHaveBeenCalled();
  });

  it("registers the login item, shows progress and the Documents prompt, then opens Studio", async () => {
    let running = false;
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => {
      release = r;
    });
    const service = mockDaemonService(null, {
      status: vi.fn(async () => statusOf(running, inspection({ registration: running ? "enabled" : "notRegistered" }))),
      start: vi.fn(async () => {
        await gate;
        return {
          before: inspection(),
          steps: [{ step: "registerLaunchAgent" as const, ok: true, detail: null }],
          after: inspection({ registration: "enabled" }),
        };
      }),
    });
    const onReady = vi.fn();
    render(<DaemonStartup service={service} onReady={onReady} pollMs={5} />);
    await waitFor(() => screen.getByText("Starting the Keyglass background service…"));
    expect(screen.getByRole("progressbar")).toBeTruthy();
    await act(async () => {
      release();
    });
    await waitFor(() => screen.getByText("Registering the background service as a login item"));
    expect(screen.getByText("Login item: on")).toBeTruthy();
    expect(screen.getByText(/access your Documents folder/)).toBeTruthy();
    expect(service.start).toHaveBeenCalledWith("auto");
    running = true;
    await waitFor(() => expect(onReady).toHaveBeenCalled());
  });

  it("explains Login Items approval and keeps waiting for it", async () => {
    let approved = false;
    const service = mockDaemonService(null, {
      status: vi.fn(async () =>
        statusOf(approved, inspection({ registration: approved ? "enabled" : "requiresApproval" })),
      ),
    });
    const onReady = vi.fn();
    render(<DaemonStartup service={service} onReady={onReady} pollMs={5} timeoutMs={20} />);
    await waitFor(() => screen.getByText("Open Login Items"));
    expect(service.start).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Open Login Items"));
    expect(service.openLoginItems).toHaveBeenCalled();
    // Longer than the start timeout: approval waits for the user.
    await new Promise((r) => setTimeout(r, 60));
    expect(screen.getByText("Open Login Items")).toBeTruthy();
    approved = true;
    await waitFor(() => expect(onReady).toHaveBeenCalled());
  });

  it("can start the daemon for this session while the login item waits for approval", async () => {
    let running = false;
    const service = mockDaemonService(null, {
      status: vi.fn(async () => statusOf(running, inspection({ registration: "requiresApproval" }))),
      start: vi.fn(async (mode) => {
        running = mode === "direct";
        return {
          before: inspection(),
          steps: [{ step: "startBundledDaemon" as const, ok: true, detail: null }],
          after: inspection(),
        };
      }),
    });
    const onReady = vi.fn();
    render(<DaemonStartup service={service} onReady={onReady} pollMs={5} />);
    await waitFor(() => screen.getByText("Start for this session only"));
    fireEvent.click(screen.getByText("Start for this session only"));
    await waitFor(() => expect(onReady).toHaveBeenCalled());
    expect(service.start).toHaveBeenCalledWith("direct");
  });

  it("reports a daemon that doesn't start, with retry and continue", async () => {
    const svc = inspection({ platform: "linux", manager: "systemdUser", registration: "notRegistered" });
    const service = mockDaemonService(null, {
      status: vi.fn(async () => statusOf(false, svc)),
      start: vi.fn(async () => ({
        before: svc,
        steps: [
          { step: "startSystemdUnit" as const, ok: false, detail: "Failed to connect to bus" },
          { step: "startBundledDaemon" as const, ok: true, detail: null },
        ],
        after: svc,
      })),
    });
    const onReady = vi.fn();
    render(<DaemonStartup service={service} onReady={onReady} pollMs={5} timeoutMs={30} />);
    await waitFor(() => screen.getByText("The Keyglass background service isn't running."));
    expect(screen.getByText(/Starting the background service: Failed to connect to bus/)).toBeTruthy();
    expect(screen.queryByText(/Documents folder/)).toBeNull();
    fireEvent.click(screen.getByText("Try again"));
    await waitFor(() => expect(service.start).toHaveBeenCalledTimes(2));
    await waitFor(() => screen.getByText("Continue without it"));
    fireEvent.click(screen.getByText("Continue without it"));
    expect(onReady).toHaveBeenCalledWith(null);
  });

  it("doesn't start a second daemon when one is listening but not answering", async () => {
    const service = mockDaemonService(null, {
      status: vi.fn(async () => ({ ...statusOf(false, inspection({ registration: "enabled" })), notRunning: false })),
    });
    render(<DaemonStartup service={service} onReady={() => {}} pollMs={5} timeoutMs={30} />);
    await waitFor(() => screen.getByText("Waiting for the Keyglass background service to answer…"));
    await waitFor(() => screen.getByText("Couldn't connect to the Keyglass background service."));
    expect(service.start).not.toHaveBeenCalled();
  });

  it("gives up at the deadline when status checks keep failing, and says why", async () => {
    let calls = 0;
    const service = mockDaemonService(null, {
      status: vi.fn(async () => {
        calls += 1;
        if (calls === 1) return { ...statusOf(false, inspection({ registration: "enabled" })), notRunning: false };
        throw new Error("the shell stopped answering");
      }),
    });
    render(<DaemonStartup service={service} onReady={() => {}} pollMs={5} timeoutMs={30} />);
    await waitFor(() => screen.getByText("Couldn't connect to the Keyglass background service."));
    expect(screen.getByText("the shell stopped answering")).toBeTruthy();
    expect(screen.getByText("Try again")).toBeTruthy();
  });

  it("times the start once Login Items approval is given", async () => {
    let approved = false;
    const service = mockDaemonService(null, {
      status: vi.fn(async () =>
        statusOf(false, inspection({ registration: approved ? "enabled" : "requiresApproval" })),
      ),
    });
    render(<DaemonStartup service={service} onReady={() => {}} pollMs={5} timeoutMs={30} />);
    await waitFor(() => screen.getByText("Open Login Items"));
    approved = true;
    await waitFor(() => screen.getByText("The Keyglass background service isn't running."));
    expect(screen.queryByText("Open Login Items")).toBeNull();
  });
});

describe("Studio navigation", () => {
  function configClient(): DaemonClient {
    const fail = async () => {
      throw new Error("not used");
    };
    return new Proxy({} as DaemonClient, {
      get: (_t, prop) => {
        if (prop === "getConfig") return async () => structuredClone(TEST_CONFIG);
        if (prop === "getStatus")
          return async () => ({ connected: { bridge: true, firmware: true }, versions: {}, desiredProfileIndex: 0 });
        if (prop === "getPreviewRects") return async () => [];
        if (prop === "closePreview") return async () => {};
        return fail;
      },
    });
  }

  it("badges Updates and Apps, and opens the page a launcher asks for", async () => {
    const daemon = new MockDaemon();
    let navigate: (p: string) => void = () => {};
    const services = mockServices(daemon, {
      navigation: {
        initialPage: async () => "apps",
        subscribe: (fn) => {
          navigate = fn;
          return () => {};
        },
      },
    });
    render(<App client={configClient()} services={services} initialConfig={structuredClone(TEST_CONFIG)} />);
    await waitFor(() => screen.getByLabelText("App access"));
    await waitFor(() => screen.getByRole("button", { name: "Updates (Updates available)" }));
    expect(screen.getByRole("button", { name: "Apps (1 waiting for approval)" })).toBeTruthy();
    act(() => navigate("updates"));
    await waitFor(() => screen.getByText("Check now"));
    await waitFor(() => screen.getByLabelText("Background service"));
    expect(screen.getByText("Login item: on")).toBeTruthy();
    act(() => navigate("nowhere"));
    expect(screen.getByText("Check now")).toBeTruthy();
  });
});
