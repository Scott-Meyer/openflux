/**
 * Errors from the daemon and the desktop shell, as reported by the update, conversion,
 * lifecycle and third-party commands.
 */
export type DaemonErrorKind = "notRunning" | "unreachable" | "indeterminate" | "daemon" | "local";

export class DaemonError extends Error {
  readonly kind: DaemonErrorKind;
  readonly status: number | null;
  readonly code: string | null;
  readonly recovery: string | null;
  readonly detail: string | null;

  constructor(init: {
    kind: DaemonErrorKind;
    message: string;
    status?: number | null;
    code?: string | null;
    recovery?: string | null;
    detail?: string | null;
  }) {
    super(init.message);
    this.name = "DaemonError";
    this.kind = init.kind;
    this.status = init.status ?? null;
    this.code = init.code ?? null;
    this.recovery = init.recovery ?? null;
    this.detail = init.detail ?? null;
  }
}

/** Turns whatever a command rejected with into a `DaemonError`. */
export function toDaemonError(e: unknown): DaemonError {
  if (e instanceof DaemonError) return e;
  if (e && typeof e === "object" && "message" in e && "kind" in e) {
    const o = e as Record<string, unknown>;
    return new DaemonError({
      kind: (o.kind as DaemonErrorKind) ?? "local",
      message: String(o.message),
      status: typeof o.status === "number" ? o.status : null,
      code: typeof o.code === "string" ? o.code : null,
      recovery: typeof o.recovery === "string" ? o.recovery : null,
      detail: typeof o.detail === "string" ? o.detail : null,
    });
  }
  if (e instanceof Error) return new DaemonError({ kind: "local", message: e.message });
  return new DaemonError({ kind: "local", message: String(e) });
}

/** What an error code means to the user, and what they can do about it. */
export interface ErrorExplanation {
  message: string;
  recovery: string;
}

const EXPLANATIONS: Record<string, ErrorExplanation> = {
  offline: {
    message: "OpenFlux couldn't reach the update server.",
    recovery: "Check the internet connection and try again.",
  },
  untrusted: {
    message: "The update information failed its signature check, so OpenFlux didn't use it.",
    recovery: "Try again later. If it keeps happening, download OpenFlux from the project's release page.",
  },
  expired: {
    message: "The update information has expired.",
    recovery: "Check that this computer's date and time are correct, then check again.",
  },
  replayed: {
    message: "The update server sent older update information than OpenFlux has already seen, so it was ignored.",
    recovery: "Try again later.",
  },
  malformed: {
    message: "The update information couldn't be read.",
    recovery: "Try again later.",
  },
  verification: {
    message: "A downloaded file didn't match its signature, so nothing was installed from it.",
    recovery: "Check for updates and try again.",
  },
  versionMismatch: {
    message: "After installing, the keyboard reported a different version than expected.",
    recovery: "Reconnect the keyboard and check for updates. If it happens again, install the update again.",
  },
  keyboardDisconnected: {
    message: "The keyboard was disconnected.",
    recovery: "Reconnect the keyboard. OpenFlux resumes or restores the interrupted step when it's back.",
  },
  interrupted: {
    message: "The operation stopped before it finished.",
    recovery:
      "Keep the keyboard connected and leave OpenFlux running. It resumes or restores the interrupted step on its own.",
  },
  busy: {
    message: "OpenFlux is already working on the keyboard.",
    recovery: "Wait for the current operation to finish, then try again.",
  },
};

/**
 * Explains an error for display. Studio's own wording comes first; the daemon's message and
 * recovery hint are kept when they add something.
 */
export function explainError(
  code: string | null | undefined,
  message: string | null | undefined,
  recovery?: string | null,
): ErrorExplanation & { detail: string | null } {
  const known = code ? EXPLANATIONS[code] : undefined;
  if (known) {
    return {
      message: known.message,
      recovery: recovery || known.recovery,
      detail: message && message !== known.message ? message : null,
    };
  }
  return { message: message || "Something went wrong.", recovery: recovery ?? "", detail: null };
}
