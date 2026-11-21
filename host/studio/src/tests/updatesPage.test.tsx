import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostUpdateFlow } from "../api/hostUpdateFlow";
import { UpdatesPage } from "../components/updates/UpdatesPage";
import { FACTORY_KEYBOARD, MockDaemon, mockDaemonService, mockHostUpdater, refusal } from "./mockDaemon";

afterEach(() => cleanup());

function setup(daemon: MockDaemon, props: { onKeyboardChanged?: () => void } = {}) {
  const updates = daemon.updatesClient();
  const updater = mockHostUpdater(daemon);
  const flow = new HostUpdateFlow({
    updater,
    lifecycle: (a) => updates.lifecycle(a),
    currentJob: async () => (await updates.getUpdates()).job,
    startDaemon: (m) => mockDaemonService(daemon).start(m),
  });
  const utils = render(<UpdatesPage updates={updates} hostFlow={flow} pollMs={10} jobPollMs={10} {...props} />);
  return { updates, updater, flow, ...utils };
}

const keyboardCard = () => screen.getByLabelText("Keyboard");
const row = (component: string) => keyboardCard().querySelector(`tr[data-component="${component}"]`) as HTMLElement;

describe("Updates page", () => {
  it("shows every keyboard component's state, including blocked, incompatible and required updates", async () => {
    const daemon = new MockDaemon();
    daemon.status.keyboard.components[0] = {
      ...daemon.status.keyboard.components[0],
      state: "blocked",
      reason: "display needs display-ipc >=2.0, <3.0 but bridge provides 1.4",
    };
    setup(daemon);
    await waitFor(() => expect(row("bridge")).toBeTruthy());
    expect(row("firmware").textContent).toContain("Update blocked");
    expect(row("firmware").textContent).toContain("display needs display-ipc");
    expect(row("bridge").textContent).toContain("Update available: 1.0.1");
    expect(row("display").textContent).toContain("Required");
    expect(screen.getByText(/Last checked/)).toBeTruthy();
  });

  it("explains a version that isn't in the release index instead of offering a plan", async () => {
    const daemon = new MockDaemon();
    daemon.status.keyboard.components[1] = {
      ...daemon.status.keyboard.components[1],
      installed: "1.0.7",
      latest: null,
      state: "unknown",
    };
    daemon.status.keyboard.plan = null;
    setup(daemon);
    await waitFor(() => expect(row("bridge").textContent).toContain("Not in the release index"));
    expect(screen.getByText(/isn't in the release index/)).toBeTruthy();
    expect(screen.queryByText("Install keyboard updates…")).toBeNull();
  });

  it("checks now, follows the running check, and saves the channel and auto-check", async () => {
    const daemon = new MockDaemon();
    daemon.checkReads = 2;
    const { updates, updater } = setup(daemon);
    await waitFor(() => screen.getByText("Check now"));
    fireEvent.click(screen.getByText("Check now"));
    await waitFor(() => expect(screen.getByText("Checking for updates…")).toBeTruthy());
    await waitFor(() => expect(screen.getByText("Check now")).toBeTruthy());
    expect(updates.checkUpdates).toHaveBeenCalledTimes(1);
    expect(updater.check).toHaveBeenCalledWith("stable");

    fireEvent.change(screen.getByLabelText("Update channel"), { target: { value: "beta" } });
    await waitFor(() => expect(updates.putUpdateSettings).toHaveBeenCalledWith({ channel: "beta" }));
    await waitFor(() => expect(updater.check).toHaveBeenCalledWith("beta"));
    fireEvent.click(screen.getByLabelText("Check for updates automatically"));
    await waitFor(() => expect(updates.putUpdateSettings).toHaveBeenCalledWith({ autoCheck: false }));
    expect(daemon.status.autoCheck).toBe(false);
  });

  it("keeps following a running check through a failed poll", async () => {
    const daemon = new MockDaemon();
    daemon.checkReads = 2;
    const { updates } = setup(daemon);
    await waitFor(() => screen.getByText("Check now"));
    const getUpdates = updates.getUpdates as ReturnType<typeof vi.fn>;
    getUpdates.mockImplementationOnce(async () => {
      throw new Error("connection reset");
    });
    fireEvent.click(screen.getByText("Check now"));
    await waitFor(() => screen.getByText("Couldn't follow the running check. Trying again…"));
    expect(screen.getByText("Checking…")).toBeTruthy();
    await waitFor(() => expect(screen.getByText("Check now")).toBeTruthy());
    expect(screen.queryByText("Couldn't follow the running check. Trying again…")).toBeNull();
  });

  it("explains a failed check with what to do", async () => {
    const daemon = new MockDaemon();
    daemon.status.error = { code: "offline", message: "dns error" };
    daemon.status.indexSource = "bundled";
    setup(daemon);
    await waitFor(() => screen.getByText("The last check failed"));
    expect(screen.getByText("OpenFlux couldn't reach the update server.")).toBeTruthy();
    expect(screen.getByText("Check the internet connection and try again.")).toBeTruthy();
    expect(screen.getByText(/came with this installation/)).toBeTruthy();
  });

  it("installs keyboard updates after a confirmation about typing, and shows each step", async () => {
    const daemon = new MockDaemon();
    const onKeyboardChanged = vi.fn();
    const { updates } = setup(daemon, { onKeyboardChanged });
    await waitFor(() => screen.getByText("Install keyboard updates…"));
    fireEvent.click(screen.getByText("Install keyboard updates…"));
    const dialog = screen.getByRole("dialog", { name: "Install keyboard updates?" });
    expect(dialog.textContent).toContain("the keyboard can't type");
    expect(dialog.textContent).toContain("Keyboard bridge 1.0.1");
    expect(updates.applyUpdates).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByText("Install"));

    await waitFor(() => expect(screen.getByText("Step 1 of 2: Installing bridge 1.0.1")).toBeTruthy());
    expect(updates.applyUpdates).toHaveBeenCalledWith(undefined, [
      { component: "bridge", to: "1.0.1" },
      { component: "display", to: "1.0.1" },
    ]);
    expect(screen.getByText("Install keyboard updates…").closest("button")?.disabled).toBe(true);

    act(() =>
      daemon.progress("job-1", { step: { index: 2, count: 2, label: "Installing display 1.0.1" }, progress: 0.5 }),
    );
    await waitFor(() => expect(screen.getByText("Step 2 of 2: Installing display 1.0.1")).toBeTruthy());
    expect(screen.getByRole("progressbar", { name: "Installing display 1.0.1" }).getAttribute("aria-valuenow")).toBe(
      "50",
    );

    act(() => daemon.finish("job-1", "succeeded"));
    await waitFor(() => expect(screen.getByText("Done")).toBeTruthy());
    expect(onKeyboardChanged).toHaveBeenCalledTimes(1);
  });

  it("installs only the selected components and confirms the subset plan", async () => {
    const daemon = new MockDaemon();
    const { updates } = setup(daemon);
    await waitFor(() => screen.getByText("Install keyboard updates…"));
    fireEvent.click(screen.getByLabelText(/Display 1.0.1/));
    expect(screen.getByText("OpenFlux adds any other component your selection needs.")).toBeTruthy();
    await waitFor(() => expect(updates.planUpdates).toHaveBeenCalledWith(["bridge"]));
    fireEvent.click(screen.getByText("Install keyboard updates…"));
    const dialog = screen.getByRole("dialog", { name: "Install keyboard updates?" });
    expect(dialog.textContent).toContain("Keyboard bridge 1.0.1");
    expect(dialog.textContent).not.toContain("Display 1.0.1");
    fireEvent.click(within(dialog).getByText("Install"));
    await waitFor(() =>
      expect(updates.applyUpdates).toHaveBeenCalledWith(["bridge"], [{ component: "bridge", to: "1.0.1" }]),
    );
  });

  it("shows a refusal from the daemon with its recovery hint", async () => {
    const daemon = new MockDaemon();
    const { updates } = setup(daemon);
    updates.applyUpdates = vi.fn(async () => {
      throw refusal("busy", "Another job is running", 409, "Wait for the backup to finish.");
    });
    await waitFor(() => screen.getByText("Install keyboard updates…"));
    fireEvent.click(screen.getByText("Install keyboard updates…"));
    fireEvent.click(within(screen.getByRole("dialog")).getByText("Install"));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("The update didn't start");
    expect(alert.textContent).toContain("OpenFlux is already working on the keyboard.");
    expect(alert.textContent).toContain("Wait for the backup to finish.");
  });

  it("explains a failed job and how to recover", async () => {
    const daemon = new MockDaemon();
    setup(daemon);
    await waitFor(() => screen.getByText("Install keyboard updates…"));
    fireEvent.click(screen.getByText("Install keyboard updates…"));
    fireEvent.click(within(screen.getByRole("dialog")).getByText("Install"));
    await waitFor(() => screen.getByText("Step 1 of 2: Installing bridge 1.0.1"));
    act(() =>
      daemon.finish("job-1", "failed", {
        code: "keyboardDisconnected",
        message: "bridge stopped answering during transfer",
        recovery: "Reconnect the keyboard; the bridge is restored from its rollback copy.",
      }),
    );
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("The keyboard was disconnected.");
    expect(alert.textContent).toContain("Reconnect the keyboard; the bridge is restored from its rollback copy.");
    expect(alert.textContent).toContain("bridge stopped answering during transfer");
    expect(screen.getByText("Failed")).toBeTruthy();
  });

  it("follows a job that was already running when the page opened", async () => {
    const daemon = new MockDaemon();
    daemon.startJob("update", "Installing bridge 1.0.1");
    const { updates } = setup(daemon);
    await waitFor(() => expect(screen.getByText("Step 1 of 2: Installing bridge 1.0.1")).toBeTruthy());
    expect(updates.applyUpdates).not.toHaveBeenCalled();
    expect(screen.getByText(/Closing Studio doesn't stop it/)).toBeTruthy();
  });

  it("asks to connect the keyboard when it is disconnected", async () => {
    const daemon = new MockDaemon();
    daemon.status.keyboard = { ...daemon.status.keyboard, connected: false, mode: "disconnected", plan: null };
    setup(daemon);
    await waitFor(() => screen.getByText("Connect the keyboard to see and install its updates."));
    expect(screen.queryByText("Install keyboard updates…")).toBeNull();
  });
});

describe("Conversion", () => {
  it("explains, confirms once, converts with progress, and reports the imported settings", async () => {
    const daemon = new MockDaemon(FACTORY_KEYBOARD);
    const onKeyboardChanged = vi.fn();
    const { updates } = setup(daemon, { onKeyboardChanged });
    await waitFor(() => screen.getByText("Convert this keyboard to OpenFlux"));
    const wizard = screen.getByLabelText("Convert to OpenFlux");
    expect(wizard.textContent).toContain("key calibration are kept");
    expect(wizard.textContent).toContain("reset the keyboard to its factory software later");
    expect(wizard.textContent).toContain("Factory software 1.4.28");
    expect(screen.queryByText("Install keyboard updates…")).toBeNull();

    fireEvent.click(screen.getByText("Convert to OpenFlux…"));
    const dialog = screen.getByRole("dialog", { name: "Convert to OpenFlux?" });
    expect(updates.startConversion).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByText("Cancel"));
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.click(screen.getByText("Convert to OpenFlux…"));
    fireEvent.click(within(screen.getByRole("dialog")).getByText("Convert"));
    await waitFor(() => expect(screen.getByText("Step 1 of 4: Installing firmware 1.0.0")).toBeTruthy());
    expect(updates.startConversion).toHaveBeenCalledWith("convert");
    expect(screen.getByText("Converting the keyboard to OpenFlux")).toBeTruthy();

    act(() => {
      daemon.progress("job-1", { step: { index: 4, count: 4, label: "Importing your settings" } });
    });
    await waitFor(() => screen.getByText("Step 4 of 4: Importing your settings"));
    act(() => {
      daemon.status.keyboard = structuredClone(new MockDaemon().status.keyboard);
      daemon.progress("job-1", { pressRefinement: { installed: false, code: "offline", message: "unreachable" } });
      daemon.finish("job-1", "succeeded");
    });
    await waitFor(() => screen.getByText("The keyboard runs OpenFlux, and your settings were imported."));
    expect(screen.getByText(/Press-refinement data isn't installed/).textContent).toContain(
      "matches the keyboard maker's original 1.4 software",
    );
    expect(onKeyboardChanged).toHaveBeenCalled();
  });

  it("shows why conversion isn't possible", async () => {
    const daemon = new MockDaemon(FACTORY_KEYBOARD);
    daemon.conversion = {
      ...daemon.conversion,
      canConvert: false,
      reason: "Connect the keyboard directly, not through a hub.",
    };
    setup(daemon);
    await waitFor(() => screen.getByText("Connect the keyboard directly, not through a hub."));
    expect(screen.getByText("Convert to OpenFlux…").closest("button")?.disabled).toBe(true);
  });

  it("factory reset is unavailable with the daemon's reason", async () => {
    const daemon = new MockDaemon();
    daemon.conversion = {
      canConvert: false,
      canRestore: false,
      reason: "Factory software isn't in the release index.",
      job: null,
    };
    setup(daemon);
    await waitFor(() => screen.getByText("Advanced"));
    expect(screen.getByText("Not available: Factory software isn't in the release index.")).toBeTruthy();
    expect(screen.getByText("Reset to factory software…").closest("button")?.disabled).toBe(true);
  });

  it("factory reset needs two confirmations and reports a 501 as unavailable", async () => {
    const daemon = new MockDaemon();
    daemon.conversion = { canConvert: false, canRestore: true, reason: null, job: null };
    daemon.restoreError = refusal("notImplemented", "Factory software isn't available for this keyboard", 501);
    const { updates } = setup(daemon);
    await waitFor(() => screen.getByText("Reset to factory software…"));
    fireEvent.click(screen.getByText("Reset to factory software…"));
    fireEvent.click(within(screen.getByRole("dialog", { name: "Reset to factory software?" })).getByText("Continue"));
    expect(updates.startConversion).not.toHaveBeenCalled();
    fireEvent.click(within(screen.getByRole("dialog", { name: "Reset the keyboard now?" })).getByText("Reset now"));
    await waitFor(() =>
      expect(screen.getByText("Not available: Factory software isn't available for this keyboard")).toBeTruthy(),
    );
    expect(updates.startConversion).toHaveBeenCalledWith("restore");
    expect(screen.getByText("Reset to factory software…").closest("button")?.disabled).toBe(true);
  });

  it("factory reset runs as a job when the daemon accepts it", async () => {
    const daemon = new MockDaemon();
    daemon.conversion = { canConvert: false, canRestore: true, reason: null, job: null };
    setup(daemon);
    await waitFor(() => screen.getByText("Reset to factory software…"));
    fireEvent.click(screen.getByText("Reset to factory software…"));
    fireEvent.click(within(screen.getByRole("dialog")).getByText("Continue"));
    fireEvent.click(within(screen.getByRole("dialog")).getByText("Reset now"));
    await waitFor(() => screen.getByText("Restoring the factory software"));
    expect(screen.getByText("Step 1 of 4: Installing factory software")).toBeTruthy();
  });

  it("lets developers switch to Local builds, scan a folder, and confirm an unsigned local install", async () => {
    const daemon = new MockDaemon();
    const { updates } = setup(daemon);
    const installLocalUpdates = vi.fn(async () => ({ jobId: "local-job-1" }));
    updates.installLocalUpdates = installLocalUpdates;
    const origPut = updates.putUpdateSettings;
    updates.putUpdateSettings = vi.fn(async (patch) => {
      const next = await origPut(patch);
      if (patch.channel === "local" || patch.localDir) {
        const localDir = patch.localDir ?? "/tmp/openflux-dev";
        return {
          ...next,
          channel: "local",
          localDir,
          localBuilds: {
            dir: localDir,
            exists: true,
            error: null,
            firmware: null,
            bridge: {
              component: "bridge",
              name: "openflux-bridge_1.0.3-dev_aarch64.ipk",
              path: "/tmp/openflux-dev/openflux-bridge_1.0.3-dev_aarch64.ipk",
              version: "1.0.3-dev",
              sizeBytes: 340000,
              modifiedAt: "2026-11-21T19:00:00Z",
            },
            display: null,
            host: null,
          },
        };
      }
      return next;
    });

    await waitFor(() => screen.getByLabelText("Update channel"));
    fireEvent.change(screen.getByLabelText("Update channel"), { target: { value: "local" } });
    await waitFor(() => screen.getByText("Local builds in folder"));
    expect(screen.getByText("openflux-bridge_1.0.3-dev_aarch64.ipk")).toBeTruthy();

    fireEvent.click(screen.getByText("Install local builds…"));
    const dlg = screen.getByRole("dialog", { name: "Install unsigned local builds?" });
    expect(dlg.textContent).toContain("skip release signature verification");
    fireEvent.click(within(dlg).getByText("Install unsigned builds"));
    await waitFor(() => expect(installLocalUpdates).toHaveBeenCalledWith(["bridge"]));
  });
});
