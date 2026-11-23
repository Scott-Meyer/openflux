import type React from "react";
import { useEffect, useRef, useState } from "react";
import { v2Client } from "../api/v2Client";

interface VialKeymapTabProps {
  /** Optional legacy styling view; omitted in pure OpenFlux v2. */
  stylingView?: React.ReactNode;
}

export const VialKeymapTab: React.FC<VialKeymapTabProps> = ({ stylingView }) => {
  const [subTab, setSubTab] = useState<"vial" | "styling">("vial");
  const [connected, setConnected] = useState<boolean | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [syncNote, setSyncNote] = useState<string | null>(null);
  const [frameKey, setFrameKey] = useState(0);
  const iframeRef = useRef<HTMLIFrameElement | null>(null);

  useEffect(() => {
    let active = true;
    const poll = () => {
      void v2Client
        .vialStatus()
        .then((st) => {
          if (active) setConnected(st.connected);
        })
        .catch(() => {
          if (active) setConnected(false);
        });
    };
    poll();
    const id = window.setInterval(poll, 3000);
    return () => {
      active = false;
      window.clearInterval(id);
    };
  }, []);

  useEffect(() => {
    const onMessage = (ev: MessageEvent) => {
      const iframeWin = iframeRef.current?.contentWindow;
      if (!iframeWin || ev.source !== iframeWin) return;
      const msg = ev.data as { type?: string; reqId?: number; data?: number[] } | null;
      if (!msg || typeof msg !== "object" || typeof msg.reqId !== "number") return;
      const reqId = msg.reqId;

      if (msg.type === "openflux:vial:status") {
        void v2Client
          .vialStatus()
          .then((result) => {
            setConnected(result.connected);
            iframeWin.postMessage({ type: "openflux:vial:res", reqId, result }, "*");
          })
          .catch((err: unknown) => {
            iframeWin.postMessage(
              { type: "openflux:vial:res", reqId, error: err instanceof Error ? err.message : String(err) },
              "*",
            );
          });
      } else if (msg.type === "openflux:vial:xfer" && Array.isArray(msg.data)) {
        void v2Client
          .vialXfer(msg.data)
          .then((result) => {
            iframeWin.postMessage({ type: "openflux:vial:res", reqId, result }, "*");
          })
          .catch((err: unknown) => {
            iframeWin.postMessage(
              { type: "openflux:vial:res", reqId, error: err instanceof Error ? err.message : String(err) },
              "*",
            );
          });
      }
    };

    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);

  const handleSyncActions = async () => {
    setSyncing(true);
    setSyncNote(null);
    try {
      const res = await v2Client.vialSyncActions();
      if (res.firmware_synced) {
        setSyncNote(`Synced ${res.slot_count} actions (${res.xz_bytes}B) to keyboard.`);
        // Reload Vialite so it fetches the updated customKeycodes from the keyboard.
        setFrameKey((k) => k + 1);
      } else {
        setSyncNote(res.warnings[0] ?? "Saved locally (keyboard not connected).");
      }
    } catch (err) {
      setSyncNote(err instanceof Error ? err.message : String(err));
    } finally {
      setSyncing(false);
    }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: "76vh", gap: 10 }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 12,
          flexWrap: "wrap",
          padding: "8px 12px",
          borderRadius: 10,
          background: "rgba(255,255,255,0.04)",
          border: "1px solid rgba(255,255,255,0.08)",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <button
            type="button"
            className={`nav-btn ${subTab === "vial" ? "active" : ""}`}
            onClick={() => setSubTab("vial")}
          >
            QMK / Vial Keymap
          </button>
          {stylingView && (
            <button
              type="button"
              className={`nav-btn ${subTab === "styling" ? "active" : ""}`}
              onClick={() => setSubTab("styling")}
            >
              Keycap Styling
            </button>
          )}
          <span
            style={{
              fontSize: 12,
              opacity: 0.85,
              padding: "2px 8px",
              borderRadius: 999,
              background: connected ? "rgba(34,197,94,0.18)" : "rgba(234,179,8,0.16)",
              color: connected ? "#86efac" : "#fde047",
            }}
          >
            {connected === null ? "Checking Vial…" : connected ? "Vial Connected" : "Vial Offline (flash QMK firmware)"}
          </span>
        </div>

        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          {syncNote && <span style={{ fontSize: 12, opacity: 0.8 }}>{syncNote}</span>}
          <button
            type="button"
            className="btn-secondary"
            disabled={syncing}
            onClick={() => void handleSyncActions()}
            title="Push enabled module [[actions]] to keyboard vial.json so they appear in Vial's User tab"
          >
            {syncing ? "Syncing…" : "Sync Module Actions"}
          </button>
          {subTab === "vial" && (
            <button
              type="button"
              className="btn-secondary"
              onClick={() => setFrameKey((k) => k + 1)}
              title="Reload embedded Vial configurator"
            >
              Reload
            </button>
          )}
        </div>
      </div>

      {subTab === "vial" ? (
        <iframe
          key={frameKey}
          ref={iframeRef}
          src="./vialite/index.html"
          sandbox="allow-scripts allow-downloads"
          title="OpenFlux QMK / Vial Configurator"
          style={{
            flex: 1,
            width: "100%",
            minHeight: "72vh",
            border: "1px solid rgba(255,255,255,0.08)",
            borderRadius: 12,
            background: "#111318",
          }}
        />
      ) : (
        stylingView
      )}
    </div>
  );
};
