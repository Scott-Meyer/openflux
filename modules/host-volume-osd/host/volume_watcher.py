#!/usr/bin/env python3
"""OpenFlux v2 macOS CoreAudio Volume & Mute HUD Publisher (`host-volume-osd`).

Reads macOS default output device volume & mute directly via CoreAudio C API (`ctypes`,
zero subprocess forks, <0.1ms per poll) and publishes macOS-style meter toasts onto `ui/toast`
whenever volume or mute changes!
"""
import ctypes
import json
import os
import ssl
import sys
import time
import urllib.request

BUS_URL = os.environ.get("OPENFLUX_BUS_URL", "http://127.0.0.1:52324").rstrip("/")
BUS_TOKEN = os.environ.get("OPENFLUX_BUS_TOKEN", "").strip()
CFG = json.loads(os.environ.get("OPENFLUX_CONFIG_JSON", "{}") or "{}")
SSL_CTX = ssl._create_unverified_context() if BUS_URL.startswith("https://") else None


def bus_pub(topic: str, payload: dict, retain: bool = False):
    data = json.dumps({"topic": topic, "payload": payload, "retain": retain}).encode("utf-8")
    headers = {"Content-Type": "application/json"}
    if BUS_TOKEN:
        headers["Authorization"] = f"Bearer {BUS_TOKEN}"
    req = urllib.request.Request(
        f"{BUS_URL}/v2/bus/pub",
        data=data,
        headers=headers,
        method="POST",
    )
    try:
        if SSL_CTX is not None:
            with urllib.request.urlopen(req, context=SSL_CTX, timeout=2.0):
                pass
        else:
            with urllib.request.urlopen(req, timeout=2.0):
                pass
    except Exception:
        pass


class AudioObjectPropertyAddress(ctypes.Structure):
    _fields_ = [
        ("mSelector", ctypes.c_uint32),
        ("mScope", ctypes.c_uint32),
        ("mElement", ctypes.c_uint32),
    ]


def fourcc(s: str) -> int:
    return int.from_bytes(s.encode("ascii"), "big")


def main():
    if sys.platform != "darwin":
        print("[volume-osd] macOS CoreAudio only; idling cleanly", flush=True)
        while True:
            time.sleep(30)

    ca = ctypes.CDLL("/System/Library/Frameworks/CoreAudio.framework/CoreAudio")
    kSystem = 1
    kDefaultOut = fourcc("dOut")
    kScopeGlobal = fourcc("glob")
    kScopeOut = fourcc("outp")
    kVolScalar = fourcc("volm")
    kVirtualMainVol = fourcc("vmvc")
    kMute = fourcc("mute")

    addr_out = AudioObjectPropertyAddress(kDefaultOut, kScopeGlobal, 0)
    dev_buf = ctypes.c_uint32(0)
    dev_sz = ctypes.c_uint32(ctypes.sizeof(dev_buf))
    vol_buf = ctypes.c_float(0.0)
    vol_sz = ctypes.c_uint32(ctypes.sizeof(vol_buf))
    mute_buf = ctypes.c_uint32(0)
    mute_sz = ctypes.c_uint32(ctypes.sizeof(mute_buf))

    def read_audio():
        dev_sz.value = ctypes.sizeof(dev_buf)
        if ca.AudioObjectGetPropertyData(kSystem, ctypes.byref(addr_out), 0, None, ctypes.byref(dev_sz), ctypes.byref(dev_buf)) != 0:
            return None, False
        dev = dev_buf.value
        if not dev:
            return None, False
        got_vol = False
        for sel, elem in ((kVirtualMainVol, 0), (kVolScalar, 0), (kVolScalar, 1)):
            a = AudioObjectPropertyAddress(sel, kScopeOut, elem)
            if ca.AudioObjectHasProperty(dev, ctypes.byref(a)):
                vol_sz.value = ctypes.sizeof(vol_buf)
                if ca.AudioObjectGetPropertyData(dev, ctypes.byref(a), 0, None, ctypes.byref(vol_sz), ctypes.byref(vol_buf)) == 0:
                    got_vol = True
                    break
        muted = False
        for elem in (0, 1):
            a = AudioObjectPropertyAddress(kMute, kScopeOut, elem)
            if ca.AudioObjectHasProperty(dev, ctypes.byref(a)):
                mute_sz.value = ctypes.sizeof(mute_buf)
                if ca.AudioObjectGetPropertyData(dev, ctypes.byref(a), 0, None, ctypes.byref(mute_sz), ctypes.byref(mute_buf)) == 0:
                    muted = mute_buf.value != 0
                    break
        return (round(float(vol_buf.value), 3) if got_vol else None), muted

    accent = str(CFG.get("accent") or "#5ce1ff")
    dur_ms = int(float(CFG.get("duration_ms") or 1400))

    last_vol, last_mute = read_audio()
    if last_vol is not None:
        bus_pub("host/audio/volume", {"level": last_vol, "muted": last_mute}, retain=True)
    print(f"[volume-osd] watching CoreAudio output (initial={last_vol}, muted={last_mute})", flush=True)

    while True:
        time.sleep(0.06)
        vol, muted = read_audio()
        if vol is None:
            continue
        changed = (last_vol is None) or (abs(vol - last_vol) >= 0.005) or (muted != last_mute)
        if changed:
            last_vol, last_mute = vol, muted
            bus_pub("host/audio/volume", {"level": vol, "muted": muted}, retain=True)
            is_muted = muted or vol <= 0.001
            pct = int(round(vol * 100))
            bus_pub(
                "ui/toast",
                {
                    "id": "volume",
                    "icon": "mute" if is_muted else "volume",
                    "title": "Volume",
                    "level": 0.0 if is_muted else vol,
                    "value_text": "Muted" if is_muted else f"{pct}%",
                    "color": "#ff6b74" if is_muted else accent,
                    "duration_ms": dur_ms,
                },
                retain=False,
            )


if __name__ == "__main__":
    main()
