import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DaemonError } from "../api/daemonError";
import { HostUpdateFlow, type HostUpdateState } from "../api/hostUpdateFlow";
import { HostUpdateCard } from "../components/updates/HostUpdateCard";
import { MockDaemon, mockDaemonService, mockHostUpdater } from "./mockDaemon";

afterEach(() => cleanup());

/** A flow whose waits run `onPoll` instead of sleeping, so tests decide what happens meanwhile. */
function makeFlow(
  daemon: MockDaemon,
  opts: { onPoll?: (n: number) => void; updater?: ReturnType<typeof mockHostUpdater> } = {},
) {
  const updates = daemon.updatesClient();
  const updater = opts.updater ?? mockHostUpdater(daemon);
  const service = mockDaemonService(daemon);
  let polls = 0;
  const states: HostUpdateState[] = [];
  const flow = new HostUpdateFlow({
    updater,
    lifecycle: (a) => updates.lifecycle(a),
    currentJob: async () => (await updates.getUpdates()).job,
    startDaemon: (mode) => service.start(mode),
    sleep: async () => {
      polls++;
      opts.onPoll?.(polls);
    },
    pollMs: 10,
    exitTimeoutMs: 100,
  });
  flow.subscribe((s) => states.push(s));
  return { flow, updater, service, states, updates };
}

describe("host update", () => {
  it("waits for keyboard work that started just before the stop, and refuses work after it", async () => {
    const daemon = new MockDaemon();
    const { flow, updater, states } = makeFlow(daemon, {
      onPoll: (n) => {
        if (n === 1) {
          // Work requested after the stop was honored is refused, never started.
          expect(() => daemon.beginWork("flash")).toThrow(DaemonError);
        }
        if (n === 3) daemon.endWork("save");
      },
    });
    // An idle status snapshot, which on its own doesn't make the install safe.
    expect((await daemon.updatesClient().lifecycle("status")).busy).toEqual([]);
    // A save arrives between that snapshot and the stop request.
    daemon.beforeStop = () => daemon.beginWork("save");

    await flow.check("stable");
    await flow.install();

    expect(states.some((s) => s.phase === "waitingForKeyboard" && s.busy.includes("save"))).toBe(true);
    expect(daemon.events).toContain("refused:flash");
    const end = daemon.events.indexOf("end:save");
    const exited = daemon.events.indexOf("exited");
    const install = daemon.events.indexOf("install");
    expect(end).toBeGreaterThan(-1);
    expect(exited).toBeGreaterThan(end);
    expect(install).toBeGreaterThan(exited);
    expect(daemon.events).not.toContain("install-while-daemon-running");
    expect(updater.install).toHaveBeenCalledTimes(1);
    expect(daemon.events.slice(-3)).toEqual(["start:serviceOnly", "started", "relaunch"]);
  });

  it("runs checks one at a time and shows the last one asked for", async () => {
    const daemon = new MockDaemon();
    let releaseStable: () => void = () => {};
    const order: string[] = [];
    const updater = mockHostUpdater(daemon, {
      check: vi.fn(async (channel: string) => {
        order.push(`start:${channel}`);
        if (channel === "stable") await new Promise<void>((r) => (releaseStable = r));
        order.push(`end:${channel}`);
        return channel === "beta"
          ? { available: true, currentVersion: "1.0.0", version: "1.1.0-beta.1" }
          : { available: false, currentVersion: "1.0.0" };
      }),
    });
    const { flow } = makeFlow(daemon, { updater });
    const stable = flow.check("stable");
    const beta = flow.check("beta");
    await vi.waitFor(() => expect(order).toEqual(["start:stable"]));
    releaseStable();
    await Promise.all([stable, beta]);
    expect(order).toEqual(["start:stable", "end:stable", "start:beta", "end:beta"]);
    expect(flow.state).toMatchObject({ phase: "available", update: { version: "1.1.0-beta.1" } });

    // A check asked for after the install began leaves it alone.
    const installing = flow.install();
    await flow.check("stable");
    expect(updater.check).toHaveBeenCalledTimes(2);
    await installing;
  });

  it("downloads and verifies before it involves the daemon", async () => {
    const daemon = new MockDaemon();
    const updater = mockHostUpdater(daemon, {
      download: vi.fn(async () => {
        expect(daemon.events.filter((e) => e.startsWith("lifecycle"))).toEqual([]);
        throw new DaemonError({ kind: "local", message: "signature mismatch" });
      }),
    });
    const { flow } = makeFlow(daemon, { updater });
    await flow.check("beta");
    expect(updater.check).toHaveBeenCalledWith("beta");
    const s = await flow.install();
    expect(s).toMatchObject({ phase: "failed", stage: "download", daemonRestarted: null });
    expect(daemon.events).toEqual([]);
    expect(updater.install).not.toHaveBeenCalled();
  });

  it("doesn't install while the daemon's exit can't be confirmed", async () => {
    const daemon = new MockDaemon();
    const updates = daemon.updatesClient();
    const unknown = new DaemonError({
      kind: "unreachable",
      code: "daemonStateUnknown",
      message:
        "Keyglass couldn't confirm whether the background service is still running: 127.0.0.1:5 refuses connections, but a daemon process still holds the instance lock",
    });
    const updater = mockHostUpdater(daemon);
    const service = mockDaemonService(daemon);
    const flow = new HostUpdateFlow({
      updater,
      lifecycle: async (a) => {
        if (a === "stop") return updates.lifecycle(a);
        throw unknown;
      },
      currentJob: async () => null,
      startDaemon: (m) => service.start(m),
      sleep: async () => {},
      pollMs: 10,
      exitTimeoutMs: 50,
    });
    await flow.check("stable");
    const s = await flow.install();
    expect(s).toMatchObject({ phase: "failed", stage: "stop", daemonRestarted: null });
    expect(s.phase === "failed" && s.error.code).toBe("daemonStateUnknown");
    expect(updater.install).not.toHaveBeenCalled();
    expect(updater.discard).toHaveBeenCalled();
    expect(service.start).not.toHaveBeenCalled();
  });

  it("starts the existing daemon again when the install fails", async () => {
    const daemon = new MockDaemon();
    const updater = mockHostUpdater(daemon, {
      install: vi.fn(async () => {
        throw new DaemonError({ kind: "local", message: "The installer was cancelled" });
      }),
    });
    const { flow, service } = makeFlow(daemon, { updater });
    await flow.check("stable");
    const s = await flow.install();
    expect(s).toMatchObject({ phase: "failed", stage: "install", daemonRestarted: true });
    expect(service.start).toHaveBeenCalledWith("auto");
    expect(daemon.exited).toBe(false);
    expect(updater.relaunch).not.toHaveBeenCalled();
  });

  it("cancelled while waiting: the daemon finishes its work, exits, and is started again without installing", async () => {
    const daemon = new MockDaemon();
    const job = daemon.startJob("update", "Installing bridge 1.0.1");
    let flowRef: HostUpdateFlow | null = null;
    const { flow, updater, service } = makeFlow(daemon, {
      onPoll: (n) => {
        if (n === 1) flowRef?.cancel();
        if (n === 2) daemon.finish(job.id, "succeeded");
      },
    });
    flowRef = flow;
    await flow.check("stable");
    const s = await flow.install();
    expect(s).toMatchObject({ phase: "cancelled", daemonRestarted: true });
    expect(updater.install).not.toHaveBeenCalled();
    expect(updater.discard).toHaveBeenCalled();
    expect(service.start).toHaveBeenCalledWith("auto");
    expect(daemon.events.indexOf(`finished:${job.id}:succeeded`)).toBeLessThan(daemon.events.indexOf("started"));
  });

  it("shows which job a deferred stop is waiting for", async () => {
    const daemon = new MockDaemon();
    const job = daemon.startJob("update", "Installing bridge 1.0.1");
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => {
      release = r;
    });
    const updates = daemon.updatesClient();
    const updater = mockHostUpdater(daemon);
    const flow = new HostUpdateFlow({
      updater,
      lifecycle: (a) => updates.lifecycle(a),
      currentJob: async () => (await updates.getUpdates()).job,
      startDaemon: (mode) => mockDaemonService(daemon).start(mode),
      sleep: () => gate,
      pollMs: 10,
    });
    await flow.check("stable");
    render(<HostUpdateCard flow={flow} host={null} onOpenUrl={() => {}} />);
    let done: Promise<HostUpdateState> = Promise.resolve(flow.state);
    await act(async () => {
      done = flow.install();
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(screen.getByRole("status").textContent).toContain("Waiting for “Installing bridge 1.0.1” to finish.");
    expect(updater.install).not.toHaveBeenCalled();
    await act(async () => {
      daemon.finish(job.id, "succeeded");
      release();
      await done;
    });
    expect(updater.install).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Restarting Keyglass Studio…")).toBeTruthy();
  });

  it("links to the release on installations that update through the package manager", async () => {
    const daemon = new MockDaemon();
    daemon.status.host = {
      installed: "1.0.0",
      latest: "1.1.0",
      state: "updateAvailable",
      reason: null,
      notesUrl: "https://example.test/host-1.1.0",
    };
    const updater = mockHostUpdater(daemon, {
      support: vi.fn(async () => ({
        mode: "notify" as const,
        reason: "Installed with the package manager.",
        installerExitsApp: false,
      })),
    });
    const { flow } = makeFlow(daemon, { updater });
    await flow.check("stable");
    const onOpenUrl = vi.fn();
    render(<HostUpdateCard flow={flow} host={daemon.status.host} onOpenUrl={onOpenUrl} />);
    expect(screen.getByText("Installed with the package manager.")).toBeTruthy();
    expect(updater.check).not.toHaveBeenCalled();
    act(() => screen.getByText("Release notes").click());
    expect(onOpenUrl).toHaveBeenCalledWith("https://example.test/host-1.1.0");
  });
});
