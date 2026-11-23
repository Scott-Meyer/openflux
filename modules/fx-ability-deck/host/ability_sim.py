#!/usr/bin/env python3
"""Host-side fake game/StreamDeck telemetry simulator for `fx-ability-deck`.

Periodically simulates real-world events coming from a computer game or app (e.g. an ability
triggering a cooldown, gaining a charge, toggling mic/scene state, or charging an ultimate),
publishing structured updates onto `openfluxd`'s v2 bus (`modules/ability-deck/state`) which
streams straight to both Studio preview and the physical Flux keyboard!
"""

import json
import os
import random
import ssl
import time
import urllib.request

BUS_URL = os.environ.get("OPENFLUX_BUS_URL", "https://127.0.0.1:52323").rstrip("/")
BUS_TOKEN = os.environ.get("OPENFLUX_BUS_TOKEN", "").strip()
CFG = json.loads(os.environ.get("OPENFLUX_CONFIG_JSON", "{}") or "{}")
SSL_CTX = ssl._create_unverified_context()


def publish(topic: str, payload: dict, retain: bool = True):
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
        with urllib.request.urlopen(req, context=SSL_CTX, timeout=3.0):
            pass
    except Exception as e:
        print(f"[ability-deck] publish failed: {e}", flush=True)


def main():
    print("[ability-deck] host telemetry simulator started", flush=True)
    tick = 0
    ult_pct = 86
    while True:
        time.sleep(4.0)
        if CFG.get("demo_activity") is False:
            continue
        tick += 1
        ult_pct = 100 if ult_pct >= 96 else ult_pct + 7
        patch = {
            "decks": {
                "base": {
                    "35": {"ult_pct": ult_pct},
                }
            }
        }
        # Periodically simulate a cooldown or charge event from the "game"
        if tick % 3 == 1:
            patch["decks"]["base"]["18"] = {"cd": 4.8, "charges": 1}
        elif tick % 3 == 2:
            patch["decks"]["base"]["20"] = {"cd": 7.5}
        else:
            patch["decks"]["base"]["19"] = {"charges": random.choice([2, 3]), "cd": 3.2}
        publish("modules/ability-deck/state", patch, retain=True)


if __name__ == "__main__":
    main()
