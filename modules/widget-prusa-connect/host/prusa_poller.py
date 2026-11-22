#!/usr/bin/env python3
"""OpenFlux v2 host-side poller for Prusa Connect Cloud and local PrusaLink printers.

Polls printer telemetry (nozzle/bed temps, job progress, ETA) and fetches camera snapshots
(faster while the user holds the glance key!), publishing retained status onto `openfluxd`'s
v2 bus and uploading the live camera JPEG to `/home/root/openflux/media/prusa_cam_live.jpg`.
"""

import json
import os
import ssl
import tempfile
import time
import urllib.error
import urllib.request

BUS_URL = os.environ.get("OPENFLUX_BUS_URL", "https://127.0.0.1:52323").rstrip("/")
CFG = json.loads(os.environ.get("OPENFLUX_CONFIG_JSON", "{}") or "{}")
SSL_CTX = ssl._create_unverified_context()
CAM_LOCAL_PATH = os.path.join(tempfile.gettempdir(), "openflux_prusa_cam_live.jpg")


def bus_post(path: str, payload: dict, timeout: float = 5.0):
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        f"{BUS_URL}{path}",
        data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, context=SSL_CTX, timeout=timeout) as r:
            return json.loads(r.read().decode("utf-8"))
    except Exception as e:
        print(f"[prusa] bus_post {path} failed: {e}", flush=True)
        return None


def bus_get_retained(topic: str):
    url = f"{BUS_URL}/v2/bus/retained?pattern={topic}"
    try:
        with urllib.request.urlopen(url, context=SSL_CTX, timeout=3.0) as r:
            msgs = json.loads(r.read().decode("utf-8")).get("messages", [])
            if msgs:
                return msgs[0].get("payload")
    except Exception:
        pass
    return None


def http_get(url: str, headers: dict, timeout: float = 6.0) -> bytes:
    req = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(req, context=SSL_CTX, timeout=timeout) as r:
        return r.read()


def fetch_prusa_telemetry(base_url: str, token: str, uuid: str) -> dict:
    base = base_url.rstrip("/")
    is_cloud = "prusa3d.com" in base.lower()

    if is_cloud:
        headers = {"Token": token, "Accept": "application/json"}
        if not uuid:
            raw = json.loads(http_get(f"{base}/app/printers", headers).decode("utf-8"))
            printers = raw.get("printers") if isinstance(raw, dict) else raw
            if not printers:
                raise RuntimeError("No printers returned from Prusa Connect")
            p = printers[0]
            uuid = p.get("uuid", "")
        else:
            p = json.loads(http_get(f"{base}/app/printers/{uuid}", headers).decode("utf-8"))
        tel = p.get("telemetry", p)
        job = p.get("job") or {}
        return {
            "name": p.get("name") or "Prusa Connect",
            "state": str(p.get("state") or tel.get("state") or "IDLE").upper(),
            "progress": float(job.get("progress") or tel.get("progress") or 0.0),
            "time_remaining_min": float(job.get("time_remaining") or tel.get("time_remaining") or 0.0) / 60.0,
            "nozzle_temp": float(tel.get("temp_nozzle") or 0.0),
            "nozzle_target": float(tel.get("target_nozzle") or 0.0),
            "bed_temp": float(tel.get("temp_bed") or 0.0),
            "bed_target": float(tel.get("target_bed") or 0.0),
            "job_name": str(job.get("file", {}).get("display_name") or job.get("name") or "Ready"),
            "default_cam_url": f"{base}/app/printers/{uuid}/camera/snapshot" if uuid else "",
            "cam_headers": headers,
        }

    # Local PrusaLink LAN API (`/api/v1/status` + `/api/v1/job` + `/api/v1/info`)
    headers = {"X-Api-Key": token, "Accept": "application/json"}
    st = json.loads(http_get(f"{base}/api/v1/status", headers).decode("utf-8"))
    printer = st.get("printer", {})
    job = st.get("job") or {}
    printer_name = "Prusa CORE One"
    try:
        info = json.loads(http_get(f"{base}/api/v1/info", headers).decode("utf-8"))
        printer_name = info.get("name") or info.get("hostname") or printer_name
    except Exception:
        pass
    job_name = "Ready"
    try:
        jinfo = json.loads(http_get(f"{base}/api/v1/job", headers).decode("utf-8"))
        job_name = (jinfo.get("file") or {}).get("display_name") or (jinfo.get("file") or {}).get("name") or job_name
    except Exception:
        pass
    cam_snap = ""
    try:
        cams = json.loads(http_get(f"{base}/api/v1/cameras", headers).decode("utf-8"))
        clist = cams.get("camera_list") if isinstance(cams, dict) else cams
        if isinstance(clist, list) and clist:
            cid = clist[0].get("camera_id") or clist[0].get("id")
            if cid:
                cam_snap = f"{base}/api/v1/cameras/{cid}/snap"
    except Exception:
        pass
    return {
        "name": str(printer_name),
        "state": str(printer.get("state") or "IDLE").upper(),
        "progress": float(job.get("progress") or 0.0),
        "time_remaining_min": float(job.get("time_remaining") or 0.0) / 60.0,
        "nozzle_temp": float(printer.get("temp_nozzle") or 0.0),
        "nozzle_target": float(printer.get("target_nozzle") or 0.0),
        "bed_temp": float(printer.get("temp_bed") or 0.0),
        "bed_target": float(printer.get("target_bed") or 0.0),
        "job_name": str(job_name),
        "default_cam_url": cam_snap,
        "cam_headers": headers,
    }


def fetch_and_upload_snapshot(cam_url: str, headers: dict):
    if not cam_url:
        return False
    try:
        jpg = http_get(cam_url, headers, timeout=4.0)
        if not jpg or len(jpg) < 512:
            return False
        tmp = CAM_LOCAL_PATH + ".tmp"
        with open(tmp, "wb") as f:
            f.write(jpg)
        os.replace(tmp, CAM_LOCAL_PATH)
        bus_post(
            "/v2/keyboard/upload-media",
            {"local_path": CAM_LOCAL_PATH, "remote_name": "prusa_cam_live.jpg"},
            timeout=5.0,
        )
        return True
    except Exception as e:
        print(f"[prusa] camera fetch failed: {e}", flush=True)
        return False


def main():
    printer_url = str(CFG.get("printer_url") or "https://connect.prusa3d.com")
    printer_uuid = str(CFG.get("printer_uuid") or "")
    api_token = str(CFG.get("api_token") or "")
    custom_cam = str(CFG.get("camera_url") or "")
    poll_sec = max(2.0, float(CFG.get("poll_seconds") or 5.0))

    print(f"[prusa] poller started (url={printer_url}, token_set={bool(api_token)})", flush=True)
    last_tel = 0.0
    last_cam = 0.0
    cam_url = custom_cam
    cam_headers = {}

    while True:
        now = time.monotonic()
        peek_state = bus_get_retained("modules/prusa-connect/peek") or {}
        peeking = bool(peek_state.get("active"))

        if not api_token and not custom_cam:
            if now - last_tel >= 10.0:
                last_tel = now
                bus_post(
                    "/v2/bus/pub",
                    {
                        "topic": "modules/prusa-connect/status",
                        "retain": True,
                        "payload": {
                            "name": "Prusa Connect",
                            "state": "SETUP",
                            "progress": 0,
                            "time_remaining_min": 0,
                            "nozzle_temp": 0,
                            "nozzle_target": 0,
                            "bed_temp": 0,
                            "bed_target": 0,
                            "job_name": "Add API token in Studio",
                            "camera_path": "",
                            "camera_kb_path": "/home/root/openflux/media/prusa_cam_live.jpg",
                        },
                    },
                )
            time.sleep(1.0)
            continue

        if api_token and (now - last_tel >= poll_sec):
            last_tel = now
            try:
                tel = fetch_prusa_telemetry(printer_url, api_token, printer_uuid)
                cam_url = custom_cam or tel.pop("default_cam_url", "")
                cam_headers = {} if custom_cam else tel.pop("cam_headers", {})
                tel["camera_path"] = CAM_LOCAL_PATH if os.path.isfile(CAM_LOCAL_PATH) else ""
                tel["camera_kb_path"] = "/home/root/openflux/media/prusa_cam_live.jpg"
                bus_post(
                    "/v2/bus/pub",
                    {
                        "topic": "modules/prusa-connect/status",
                        "retain": True,
                        "payload": tel,
                    },
                )
            except Exception as e:
                err_txt = str(e)
                print(f"[prusa] telemetry error: {err_txt}", flush=True)
                is_sleeping = any(k in err_txt.lower() for k in ("no route", "timed out", "refused", "unreachable"))
                bus_post(
                    "/v2/bus/pub",
                    {
                        "topic": "modules/prusa-connect/status",
                        "retain": True,
                        "payload": {
                            "name": "Prusa CORE One",
                            "state": "SLEEPING" if is_sleeping else "OFFLINE",
                            "job_name": "Printer powered off" if is_sleeping else err_txt[:36],
                            "progress": 0,
                            "nozzle_temp": 0,
                            "nozzle_target": 0,
                            "bed_temp": 0,
                            "bed_target": 0,
                        },
                    },
                )

        # Refresh camera rapidly (~2 fps) while holding the glance key, or every 20s idle
        cam_interval = 0.55 if peeking else 20.0
        if cam_url and (now - last_cam >= cam_interval):
            last_cam = now
            fetch_and_upload_snapshot(cam_url, cam_headers)

        time.sleep(0.25 if peeking else 0.75)


if __name__ == "__main__":
    main()
