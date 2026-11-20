import type { ImportedMedia } from "../api/daemonClient";

/** One line describing an import result, with the resolution and bitrate the daemon chose. */
export function describeImport(prefix: string, res: ImportedMedia): string {
  const parts: string[] = [];
  if (res.resolution && res.resolution !== "original") parts.push(res.resolution);
  if (res.bitrateKbps) {
    parts.push(res.bitrateKbps >= 1000 ? `${(res.bitrateKbps / 1000).toFixed(1)} Mbit/s` : `${res.bitrateKbps} kbit/s`);
  }
  return parts.length ? `${prefix} (${parts.join(" · ")})` : prefix;
}

/** The daemon's reason for a failed import, without request details. */
export function importErrorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
