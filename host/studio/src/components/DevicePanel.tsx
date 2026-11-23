import React, { useEffect, useState } from "react";
import type { DaemonClient, SocStatsSample } from "../api/daemonClient";
import type { DeviceStatus, SettingsConfig } from "../types/openflux";
import { ParallaxDiagram } from "./ParallaxDiagram";
import { PressRefinementRow } from "./PressRefinementRow";
import { v2Client } from "../api/v2Client";

interface DevicePanelProps {
  client: DaemonClient;
  status: DeviceStatus | null;
  settings?: SettingsConfig;
  onChangeSettings?: (next: SettingsConfig) => void;
  onRestoreDefaults: () => void;
  /** `macos`, `windows`, `linux`. Following the computer's display sleep works on macOS. */
  platform?: string;
}

const Sparkline: React.FC<{ values: number[]; maxVal: number; color: string }> = ({ values, maxVal, color }) => {
  const pts =
    values.length >= 2
      ? values
          .map((v, i) => {
            const x = (i / Math.max(1, values.length - 1)) * 120;
            const y = 26 - (Math.min(maxVal, Math.max(0, v)) / Math.max(1, maxVal)) * 24;
            return `${x.toFixed(1)},${y.toFixed(1)}`;
          })
          .join(" ")
      : "0,25 120,25";
  return (
    <svg width="100%" height="28" viewBox="0 0 120 28" preserveAspectRatio="none" aria-hidden="true">
      <polyline fill="none" stroke={color} strokeWidth="2" strokeLinejoin="round" points={pts} />
    </svg>
  );
};

export const DevicePanel: React.FC<DevicePanelProps> = ({
  client,
  status,
  settings,
  onChangeSettings,
  onRestoreDefaults,
  platform,
}) => {
  const followsDisplaySleep = platform === "macos";
  const [log, setLog] = useState<string | null>(null);
  const [socHistory, setSocHistory] = useState<SocStatsSample[]>([]);
  const [socFreshness, setSocFreshness] = useState<"waiting" | "live" | "stale" | "offline">("waiting");
  const lastSampleKeyRef = React.useRef<string>("");
  const lastFreshAtRef = React.useRef<number>(0);
  const [brightness, setBrightness] = useState<number>(80);
  const [v2Timeout, setV2Timeout] = useState<number>(settings?.timeout ?? 300);
  const [swMode, setSwMode] = useState<"offline" | "stock" | "keyglass">("offline");
  const [switchingSw, setSwitchingSw] = useState(false);

  useEffect(() => {
    let active = true;
    void v2Client
      .getDisplayPrefs()
      .then((prefs) => {
        if (!active) return;
        if (typeof prefs?.power?.brightness === "number") setBrightness(prefs.power.brightness);
        if (typeof prefs?.power?.timeoutSec === "number") setV2Timeout(prefs.power.timeoutSec);
      })
      .catch(() => {});
    void v2Client
      .getKeyboardSoftwareStatus()
      .then((st) => {
        if (active) setSwMode(st.mode);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);

  const patchSettings = (partial: Partial<SettingsConfig>) => {
    if (settings && onChangeSettings) {
      const next = { ...settings, ...partial };
      onChangeSettings(next);
      void v2Client.putDisplayParallax({
        parallaxActive: next.parallaxActive,
        eyeHeight: next.eyeHeight,
        eyeDistanceFromFront: next.eyeDistanceFromFront,
        footStandMode: next.footStandMode,
        displayOffsetX: next.displayOffsetX,
        displayOffsetY: next.displayOffsetY,
        showAlignmentGrid: next.showAlignmentGrid,
      });
      if (typeof partial.timeout === "number") {
        setV2Timeout(partial.timeout);
        void v2Client.putDisplayPower({ brightness, timeoutSec: partial.timeout });
      }
    }
  };

  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      try {
        const s = await client.getSocStats();
        if (cancelled) return;
        if (!s || s.offline || typeof s.cpuPct !== "number") {
          setSocFreshness("offline");
          return;
        }
        const key = `${s.sampleMs ?? ""}:${s.tsSec ?? ""}`;
        const now = Date.now();
        if (key !== lastSampleKeyRef.current) {
          lastSampleKeyRef.current = key;
          lastFreshAtRef.current = now;
          setSocFreshness("live");
          setSocHistory((prev) => [...prev.slice(-59), s]);
        } else if (lastFreshAtRef.current > 0 && now - lastFreshAtRef.current > 8000) {
          setSocFreshness("stale");
        }
      } catch {
        if (!cancelled) setSocFreshness("offline");
      }
    };
    void poll();
    const id = window.setInterval(poll, 2500);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [client]);

  // The last sample is shown while it is live or recently stale; offline shows no readings.
  const latestSoc = socFreshness === "live" || socFreshness === "stale" ? socHistory.at(-1) : undefined;
  const staleNote = socFreshness === "stale" ? " (stale)" : "";
  const gpuSamples = socHistory.map((s) => s.gpuPct).filter((v): v is number => typeof v === "number");
  const latestGpu = latestSoc?.gpuPct;

  return (
    <div className="panel-grid device-grid">
      <div className="card">
        <h3>Keyboard Hardware &amp; Display Power</h3>
        <div className="kv-list">
          <div>
            <span>Keyboard USB Bridge</span>
            <strong>{status?.versions.bridge ?? (status?.connected.bridge ? "Connected" : "Offline")}</strong>
          </div>
          <div>
            <span>Key Scanner MCU (RP2040)</span>
            <strong>{status?.versions.firmware ?? (status?.connected.firmware ? "Connected" : "Offline")}</strong>
          </div>
          <div>
            <span>Display software</span>
            <strong>{status?.versions.display ?? "Offline"}</strong>
          </div>
          <div>
            <span>Keyboard Linux OS</span>
            <strong>{status?.versions.os ?? "Offline"}</strong>
          </div>
        </div>
        <PressRefinementRow client={client} firmwareConnected={!!status?.connected.firmware} />

        <div className="rail-divider" style={{ margin: "14px 0" }} />
        <div className="toolbar-between" style={{ marginBottom: 8 }}>
          <h3 style={{ margin: 0 }}>Keyboard SoC Performance</h3>
          <span className="pill">
            {socFreshness === "live" && latestSoc
              ? `${latestSoc.renderMs ?? 0}ms frame${latestSoc.rkmpp ? " · HW Video" : ""}`
              : socFreshness === "stale"
                ? "Not updating"
                : socFreshness === "offline"
                  ? "Offline"
                  : "Waiting for the keyboard"}
          </span>
        </div>
        <div className="row-3" style={{ gap: 8 }}>
          <div className="kv-card" style={{ padding: "8px 10px", background: "var(--bg-input)", borderRadius: 8 }}>
            <small className="muted">SoC CPU</small>
            <strong style={{ display: "block", fontSize: 15 }}>
              {latestSoc ? `${latestSoc.cpuPct?.toFixed(0) ?? 0}%${staleNote}` : "--"}
            </strong>
            <Sparkline values={socHistory.map((s) => s.cpuPct ?? 0)} maxVal={100} color="#2dd4bf" />
          </div>
          <div className="kv-card" style={{ padding: "8px 10px", background: "var(--bg-input)", borderRadius: 8 }}>
            <small className="muted">GPU</small>
            <strong style={{ display: "block", fontSize: 15 }}>
              {latestSoc && typeof latestGpu === "number"
                ? `${latestGpu.toFixed(0)}%${staleNote}`
                : latestSoc
                  ? "Unavailable"
                  : "--"}
            </strong>
            <Sparkline values={gpuSamples} maxVal={100} color="#60a5fa" />
          </div>
          <div className="kv-card" style={{ padding: "8px 10px", background: "var(--bg-input)", borderRadius: 8 }}>
            <small className="muted">SoC RAM</small>
            <strong style={{ display: "block", fontSize: 15 }}>
              {latestSoc ? `${latestSoc.memUsedMb ?? 0}/${latestSoc.memTotalMb ?? 0}M${staleNote}` : "--"}
            </strong>
            <Sparkline
              values={socHistory.map((s) => s.memUsedMb ?? 0)}
              maxVal={socHistory.at(-1)?.memTotalMb || 2048}
              color="#c084fc"
            />
          </div>
        </div>

        {settings && onChangeSettings && (
          <>
            <label className="field" style={{ marginTop: 12 }}>
              <span>Display Backlight Brightness ({brightness}%)</span>
              <input
                type="range"
                min={5}
                max={100}
                step={5}
                value={brightness}
                onChange={(e) => {
                  const b = Number(e.target.value);
                  setBrightness(b);
                  void v2Client.putDisplayPower({ brightness: b, timeoutSec: v2Timeout });
                }}
              />
            </label>
            <label className="field" style={{ marginTop: 10 }}>
              <span>
                Display Sleep After Inactivity{followsDisplaySleep ? " (also follows the Mac's display sleep)" : ""}
              </span>
              <select value={v2Timeout} onChange={(e) => patchSettings({ timeout: Number(e.target.value) })}>
                <option value={0}>{followsDisplaySleep ? "With the Mac's display only" : "Never"}</option>
                <option value={300}>5 minutes</option>
                <option value={900}>15 minutes (Default)</option>
                <option value={1800}>30 minutes</option>
                <option value={3600}>60 minutes</option>
              </select>
            </label>

            <div className="rail-divider" style={{ margin: "14px 0" }} />
            <h3>Parallax &amp; Optical Alignment</h3>
            <ParallaxDiagram settings={settings} />
            <div className="checkbox-row" style={{ marginTop: 10, marginBottom: 10 }}>
              <label>
                <input
                  type="checkbox"
                  checked={settings.parallaxActive}
                  onChange={(e) => patchSettings({ parallaxActive: e.target.checked })}
                />
                3D Optical Parallax Active
              </label>
              <label>
                <input
                  type="checkbox"
                  checked={settings.showAlignmentGrid}
                  onChange={(e) => patchSettings({ showAlignmentGrid: e.target.checked })}
                />
                Alignment Grid
              </label>
            </div>
            <div className="row-2">
              <label className="field">
                <span>Eye Height ({settings.eyeHeight} mm)</span>
                <input
                  type="number"
                  value={settings.eyeHeight}
                  onChange={(e) => patchSettings({ eyeHeight: Number(e.target.value) })}
                />
              </label>
              <label className="field">
                <span>Front Distance ({settings.eyeDistanceFromFront} mm)</span>
                <input
                  type="number"
                  value={settings.eyeDistanceFromFront}
                  onChange={(e) => patchSettings({ eyeDistanceFromFront: Number(e.target.value) })}
                />
              </label>
            </div>
            <div className="row-3">
              <label className="field">
                <span>Foot Stand</span>
                <select
                  value={settings.footStandMode}
                  onChange={(e) => patchSettings({ footStandMode: e.target.value as SettingsConfig["footStandMode"] })}
                >
                  <option value="none">None (0°)</option>
                  <option value="low">Low (4°)</option>
                  <option value="high">High (8°)</option>
                </select>
              </label>
              <label className="field">
                <span>Offset X (px)</span>
                <input
                  type="number"
                  value={settings.displayOffsetX}
                  onChange={(e) => patchSettings({ displayOffsetX: Number(e.target.value) })}
                />
              </label>
              <label className="field">
                <span>Offset Y (px)</span>
                <input
                  type="number"
                  value={settings.displayOffsetY}
                  onChange={(e) => patchSettings({ displayOffsetY: Number(e.target.value) })}
                />
              </label>
            </div>
          </>
        )}
      </div>

      <div className="card">
        <h3>Backup &amp; Recovery</h3>
        <p className="muted" style={{ fontSize: 12, marginTop: 4 }}>
          Export a complete backup of keyboard configs, themes, and the RP2040 key calibration.
        </p>
        <button
          type="button"
          className="btn-secondary"
          onClick={async () => {
            const folder = await client.pickFolder("Select Backup Destination Folder");
            if (!folder?.trim()) return;
            try {
              const res = await client.backupDevice(folder.trim());
              setLog(res.message);
            } catch (e) {
              setLog(String(e));
            }
          }}
        >
          Choose Folder &amp; Run Device Backup...
        </button>

        <div className="danger-zone">
          <span className="danger-label">Advanced Firmware, Recovery &amp; Reset (Caution)</span>
          <div style={{ marginTop: 10, display: "flex", flexDirection: "column", gap: 10 }}>
            <button
              type="button"
              className="btn-danger"
              onClick={() => {
                if (window.confirm("Restore all keyboard theme, font, and layout settings to the OpenFlux defaults?")) {
                  onRestoreDefaults();
                }
              }}
            >
              Restore Default Settings
            </button>

            <button
              type="button"
              className="btn-danger"
              onClick={async () => {
                const uf2 = await client.pickFile("Select Key Scanner Firmware (.uf2)", ["uf2"]);
                if (!uf2?.trim()) return;
                if (
                  !window.confirm(
                    `Install the key scanner firmware ${uf2}? Keep the keyboard connected until it finishes.`,
                  )
                )
                  return;
                try {
                  const res = await client.flashFirmwareUf2(uf2.trim());
                  setLog(res.message);
                } catch (e) {
                  setLog(String(e));
                }
              }}
            >
              Install Key Scanner Firmware from File…
            </button>

            <button
              type="button"
              className="btn-secondary"
              disabled={switchingSw}
              onClick={async () => {
                const ok = window.confirm(
                  "Switch a Stock keyboard to KeyGlass?\n\nThis installs KeyGlass packages cleanly (factory Hall-effect calibration is preserved; stock profiles & settings are NOT migrated).",
                );
                if (!ok) return;
                setSwitchingSw(true);
                try {
                  const pkg = await client.pickFile("Select KeyGlass Keyboard Package (.ipk or .uf2)", ["ipk", "uf2"]);
                  if (!pkg?.trim()) return;
                  const res = pkg.toLowerCase().endsWith(".uf2")
                    ? await client.flashFirmwareUf2(pkg.trim())
                    : await client.sideloadIpk(pkg.trim());
                  setLog(res.message);
                } catch (e) {
                  setLog(String(e));
                } finally {
                  setSwitchingSw(false);
                }
              }}
            >
              Switch Stock → KeyGlass… (Current: {swMode === "keyglass" ? "KeyGlass" : swMode === "stock" ? "Stock" : "Offline"})
            </button>

            <button
              type="button"
              className="btn-danger"
              disabled={switchingSw}
              onClick={async () => {
                const ok = window.confirm(
                  "Return this keyboard to Stock Factory software?\n\nRestores signed factory bridge, display, and firmware (preserving calibration; KeyGlass modules & settings will not carry over).",
                );
                if (!ok) return;
                setSwitchingSw(true);
                try {
                  const res = await v2Client.returnKeyboardToStock();
                  setLog(res.message);
                } catch (e) {
                  setLog(String(e));
                } finally {
                  setSwitchingSw(false);
                }
              }}
            >
              Return Keyboard → Stock Factory Software…
            </button>

            <button
              type="button"
              className="btn-danger"
              onClick={async () => {
                const ipk = await client.pickFile("Select a Keyboard Package (.ipk)", ["ipk"]);
                if (!ipk?.trim()) return;
                if (!window.confirm(`Install the keyboard package ${ipk}?`)) return;
                try {
                  const res = await client.sideloadIpk(ipk.trim());
                  setLog(res.message);
                } catch (e) {
                  setLog(String(e));
                }
              }}
            >
              Install Keyboard Package from File…
            </button>
          </div>
        </div>
        {log && <p className="status-msg">{log}</p>}

        <div className="rail-divider" style={{ margin: "18px 0 12px" }} />
        <div className="kg-about-disclaimer">
          <strong>About KeyGlass</strong>
          <p className="muted" style={{ margin: "6px 0 0", fontSize: 12, lineHeight: 1.5 }}>
            KeyGlass is an independent, community-driven open-source modular keyboard software project. It is not
            affiliated with, endorsed by, sponsored by, or an official product of Flux.
          </p>
        </div>
      </div>
    </div>
  );
};
