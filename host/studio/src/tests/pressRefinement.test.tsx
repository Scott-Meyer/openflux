import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PressRefinementInstall, PressRefinementStatus } from "../api/daemonClient";
import {
  PRESS_REFINEMENT_INACTIVE,
  PRESS_REFINEMENT_NOT_INSTALLED,
  PressRefinementOutcome,
  PressRefinementRow,
  type PressRefinementRowClient,
} from "../components/PressRefinementRow";
import { refusal } from "./mockDaemon";

afterEach(() => {
  cleanup();
});

/**
 * A keyboard whose press-refinement storage holds `state`. An install stores the data the way
 * `install` answers; `picked` is the file the file picker returns.
 */
function keyboard(
  state: PressRefinementStatus,
  opts: { install?: (path?: string) => Promise<PressRefinementInstall>; picked?: string | null } = {},
) {
  let current = state;
  const client: PressRefinementRowClient = {
    getPressRefinement: vi.fn(async () => current),
    installPressRefinement: vi.fn(
      opts.install ??
        (async () => {
          current = { state: "installed", crc32: 0xcafe, active: true };
          return { ...current, alreadyInstalled: false, fromCache: false };
        }),
    ),
    pickFile: vi.fn(async () => opts.picked ?? null),
  };
  return client;
}

const installButton = () => screen.queryByRole("button", { name: "Install press-refinement data" });
const fileButton = () => screen.queryByRole("button", { name: "Use a file I already have…" });

describe("Press refinement", () => {
  it("shows installed data without install buttons", async () => {
    render(<PressRefinementRow client={keyboard({ state: "installed", crc32: 1, active: true })} firmwareConnected />);
    await screen.findByText("Installed");
    expect(installButton()).toBeNull();
    expect(fileButton()).toBeNull();
    expect(screen.queryByText(PRESS_REFINEMENT_NOT_INSTALLED)).toBeNull();
    expect(
      screen.getByText(/installs press-refinement data from the keyboard maker's published software/),
    ).toBeTruthy();
  });

  it("says when this firmware stores the data without using it", async () => {
    render(<PressRefinementRow client={keyboard({ state: "installed", crc32: 1, active: false })} firmwareConnected />);
    await screen.findByText(PRESS_REFINEMENT_INACTIVE);
    expect(installButton()).toBeNull();
  });

  it("explains missing data, installs it and shows the confirmed state", async () => {
    const client = keyboard({ state: "absent", crc32: null });
    render(<PressRefinementRow client={client} firmwareConnected />);
    await screen.findByText("Not installed");
    expect(screen.getByText(PRESS_REFINEMENT_NOT_INSTALLED)).toBeTruthy();
    expect(PRESS_REFINEMENT_NOT_INSTALLED).toMatch(/original 1\.4 software.*makes it match the maker.s 1\.5 software/);
    fireEvent.click(installButton() as HTMLElement);
    await screen.findByText("Installed and verified.");
    expect(screen.getByText("Installed")).toBeTruthy();
    expect(installButton()).toBeNull();
    expect(client.installPressRefinement).toHaveBeenCalledWith(undefined);
  });

  it("installs from a package or firmware file the user already has", async () => {
    const client = keyboard({ state: "absent", crc32: null }, { picked: "/Users/me/Downloads/polymath.msix" });
    render(<PressRefinementRow client={client} firmwareConnected />);
    await screen.findByText("Not installed");
    fireEvent.click(fileButton() as HTMLElement);
    await screen.findByText("Installed and verified.");
    expect(client.pickFile).toHaveBeenCalledWith(expect.stringContaining(".msix"), ["msix", "uf2"]);
    expect(client.installPressRefinement).toHaveBeenCalledWith("/Users/me/Downloads/polymath.msix");
  });

  it("does nothing when the file choice is cancelled", async () => {
    const client = keyboard({ state: "absent", crc32: null }, { picked: null });
    render(<PressRefinementRow client={client} firmwareConnected />);
    await screen.findByText("Not installed");
    fireEvent.click(fileButton() as HTMLElement);
    await waitFor(() => expect(client.pickFile).toHaveBeenCalled());
    expect(client.installPressRefinement).not.toHaveBeenCalled();
  });

  it("offers an install over damaged data and reports why an install failed", async () => {
    const client = keyboard(
      { state: "invalid", crc32: null },
      {
        install: async () => {
          throw refusal("hashMismatch", "the file doesn't match the published software", 502);
        },
        picked: "/tmp/other.uf2",
      },
    );
    render(<PressRefinementRow client={client} firmwareConnected />);
    await screen.findByText(/stored data is damaged/);
    fireEvent.click(fileButton() as HTMLElement);
    await screen.findByText("Not installed: the file doesn't match the published software");
    expect(installButton()).toBeTruthy();
  });

  it("doesn't offer an install when the keyboard's storage is unavailable", async () => {
    render(<PressRefinementRow client={keyboard({ state: "storageUnavailable", crc32: null })} firmwareConnected />);
    await screen.findByText("Keyboard storage unavailable");
    expect(installButton()).toBeNull();
    expect(fileButton()).toBeNull();
  });

  it("shows the daemon's refusal, and reads the state again when the firmware connects", async () => {
    let refuse = true;
    const client: PressRefinementRowClient = {
      getPressRefinement: vi.fn(async () => {
        if (refuse) throw refusal("conversionRequired", "Convert the keyboard to OpenFlux first");
        return { state: "absent" as const, crc32: null };
      }),
      installPressRefinement: vi.fn(),
      pickFile: vi.fn(),
    };
    const { rerender } = render(<PressRefinementRow client={client} firmwareConnected={false} />);
    await screen.findByText("Convert the keyboard to OpenFlux first");
    expect(installButton()).toBeNull();
    refuse = false;
    rerender(<PressRefinementRow client={client} firmwareConnected />);
    await waitFor(() => expect(installButton()).toBeTruthy());
    expect(client.getPressRefinement).toHaveBeenCalledTimes(2);
  });
});

describe("Press refinement after a conversion", () => {
  it("uses the row's wording for each outcome", () => {
    const { rerender } = render(<PressRefinementOutcome outcome={{ installed: false, code: "offline" }} />);
    expect(screen.getByText(/isn't installed/).textContent).toContain(PRESS_REFINEMENT_NOT_INSTALLED);
    rerender(<PressRefinementOutcome outcome={null} />);
    expect(screen.getByText(/isn't installed/).textContent).toContain(PRESS_REFINEMENT_NOT_INSTALLED);
    rerender(<PressRefinementOutcome outcome={{ installed: true, active: false }} />);
    expect(screen.getByText(/Press-refinement data:/).textContent).toContain(PRESS_REFINEMENT_INACTIVE);
    rerender(<PressRefinementOutcome outcome={{ installed: true, active: true }} />);
    expect(screen.getByText("Press-refinement data is installed.")).toBeTruthy();
  });
});
