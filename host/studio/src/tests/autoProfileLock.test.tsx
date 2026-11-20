import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AutoProfileLockButton } from "../components/AutoProfileLockButton";

afterEach(() => {
  cleanup();
});

describe("AutoProfileLockButton", () => {
  it("renders nothing for a daemon that doesn't report the lock", () => {
    const client = { setAutoProfileLock: vi.fn() };
    const { container } = render(
      <AutoProfileLockButton client={client} state={undefined} onChange={vi.fn()} onError={vi.fn()} />,
    );
    expect(container.textContent).toBe("");
  });

  it("unlocks a locked switch and passes on the daemon's answer", async () => {
    const answer = { locked: false, heldUntilAppChange: false };
    const client = { setAutoProfileLock: vi.fn(async () => answer) };
    const onChange = vi.fn();
    render(
      <AutoProfileLockButton
        client={client}
        state={{ locked: true, heldUntilAppChange: false }}
        onChange={onChange}
        onError={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Auto-switch locked" }));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith(answer));
    expect(client.setAutoProfileLock).toHaveBeenCalledWith(false);
  });

  it("reports a failed change and keeps the state", async () => {
    const client = {
      setAutoProfileLock: vi.fn(async () => {
        throw "Daemon unreachable";
      }),
    };
    const onChange = vi.fn();
    const onError = vi.fn();
    render(
      <AutoProfileLockButton
        client={client}
        state={{ locked: false, heldUntilAppChange: false }}
        onChange={onChange}
        onError={onError}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Auto-switch on" }));
    await waitFor(() => expect(onError).toHaveBeenCalledWith("Couldn't change the profile lock: Daemon unreachable"));
    expect(client.setAutoProfileLock).toHaveBeenCalledWith(true);
    expect(onChange).not.toHaveBeenCalled();
  });
});
