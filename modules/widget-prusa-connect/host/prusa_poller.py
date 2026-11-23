#!/usr/bin/env python3
"""OpenFlux v2 host-side poller for Prusa Connect Cloud and local PrusaLink printers.

Polls printer telemetry (nozzle/bed temps, job progress, ETA) and fetches camera snapshots
(faster while the user holds the glance key!), publishing retained status onto `openfluxd`'s
v2 bus and uploading the live camera JPEG to `/home/root/openflux/media/prusa_cam_live.jpg`.
"""

import json
import os
import shutil
import ssl
import subprocess
import tempfile
import threading
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
    thumb_url = ""
    try:
        jinfo = json.loads(http_get(f"{base}/api/v1/job", headers).decode("utf-8"))
        fobj = jinfo.get("file") or {}
        job_name = fobj.get("display_name") or fobj.get("name") or job_name
        tpath = (fobj.get("refs") or {}).get("thumbnail") or (fobj.get("refs") or {}).get("icon") or ""
        if tpath:
            thumb_url = f"{base}{tpath}" if tpath.startswith("/") else tpath
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
        "thumb_url": thumb_url,
        "cam_headers": headers,
    }


def find_ffmpeg() -> str:
    for cand in ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", shutil.which("ffmpeg") or ""]:
        if cand and os.path.isfile(cand):
            return cand
    return ""


def fetch_and_upload_snapshot(cam_url: str, headers: dict, is_live_cam: bool = True):
    if not cam_url:
        return False
    try:
        tmp = CAM_LOCAL_PATH + ".tmp.jpg"
        if cam_url.lower().startswith("rtsp://"):
            ff = find_ffmpeg()
            if not ff:
                print("[prusa] ffmpeg not found for RTSP capture", flush=True)
                return False
            res = subprocess.run(
                [
                    ff, "-y", "-rtsp_transport", "tcp", "-i", cam_url,
                    "-frames:v", "1", "-vf", "scale=960:-2", "-q:v", "4", tmp,
                ],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                timeout=8.0,
            )
            if res.returncode != 0 or not os.path.isfile(tmp):
                err_tail = res.stderr.decode("utf-8", "replace").strip().splitlines()[-1:]
                print(f"[prusa] RTSP capture failed: {err_tail}", flush=True)
                return False
        else:
            img_bytes = http_get(cam_url, headers, timeout=5.0)
            if not img_bytes or len(img_bytes) < 256:
                return False
            with open(tmp, "wb") as f:
                f.write(img_bytes)
        os.replace(tmp, CAM_LOCAL_PATH)
        bus_post(
            "/v2/keyboard/upload-media",
            {"local_path": CAM_LOCAL_PATH, "remote_name": "prusa_cam_live.jpg"},
            timeout=5.0,
        )
        return True
    except Exception as e:
        print(f"[prusa] {'camera' if is_live_cam else 'thumbnail'} fetch failed: {e}", flush=True)
        return False


class RtspFrameGrabber:
    """Streams live Buddy3D RTSP video directly over OpenFlux v2's `OFV2 CH_MEDIA` hardware
    H.264 pipeline (`POST /v2/streams/prusa-cam`, -bf 0, -flush_packets 1, unbuffered os.read)
    with ZERO temp files and ZERO disk writes!"""

    def __init__(self):
        self._proc = None
        self._thread = None
        self._url = ""
        self._stop = threading.Event()
        self.frames_uploaded = 0

    def ensure_running(self, url: str):
        if self._url == url and self._thread and self._thread.is_alive():
            return
        self.stop()
        if not url or not url.lower().startswith("rtsp://"):
            return
        self._url = url
        self._stop.clear()
        self._thread = threading.Thread(target=self._run, args=(url,), daemon=True)
        self._thread.start()

    def _close_stream(self):
        import http.client
        from urllib.parse import urlparse
        parsed = urlparse(BUS_URL)
        try:
            conn = (
                http.client.HTTPSConnection(parsed.hostname, parsed.port, context=SSL_CTX, timeout=3)
                if parsed.scheme == "https"
                else http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=3)
            )
            conn.request("DELETE", "/v2/streams/prusa-cam")
            conn.getresponse().read()
            conn.close()
        except Exception:
            pass

    def stop(self):
        self._stop.set()
        if self._proc is not None:
            try:
                self._proc.terminate()
                self._proc.wait(timeout=2.0)
            except Exception:
                try:
                    self._proc.kill()
                except Exception:
                    pass
            self._proc = None
        self._close_stream()
        self._url = ""

    def _run(self, url: str):
        import http.client
        from urllib.parse import urlparse

        ff = find_ffmpeg()
        if not ff:
            print("[prusa] ffmpeg not found for live H.264 RTSP stream", flush=True)
            return
        parsed = urlparse(BUS_URL)
        print(f"[prusa] starting live OFV2 H.264 RTSP streamer ({url}) -> /v2/streams/prusa-cam", flush=True)
        vcodec = (
            [
                "-c:v", "h264_videotoolbox",
                "-realtime", "true",
                "-bf", "0",
                "-b:v", "2400k",
                "-maxrate", "3200k",
                "-bufsize", "4000k",
                "-g", "20",
            ]
            if os.uname().sysname == "Darwin"
            else [
                "-c:v", "libx264",
                "-preset", "ultrafast",
                "-tune", "zerolatency",
                "-bf", "0",
                "-b:v", "2400k",
                "-g", "20",
            ]
        )
        while not self._stop.is_set():
            conn = None
            try:
                # Grab one fast preview snapshot first so quick clicks on PRT immediately show a picture!
                try:
                    snap_tmp = CAM_LOCAL_PATH + ".snap.jpg"
                    snap_res = subprocess.run(
                        [
                            ff, "-hide_banner", "-loglevel", "error",
                            "-rtsp_transport", "tcp",
                            "-i", url,
                            "-frames:v", "1",
                            "-vf", "scale=960:-2",
                            "-q:v", "5",
                            "-y", snap_tmp,
                        ],
                        timeout=8,
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                    )
                    if snap_res.returncode == 0 and os.path.isfile(snap_tmp) and os.path.getsize(snap_tmp) >= 1024:
                        os.replace(snap_tmp, CAM_LOCAL_PATH)
                        bus_post(
                            "/v2/keyboard/upload-media",
                            {"local_path": CAM_LOCAL_PATH, "remote_name": "prusa_cam_live.jpg"},
                            timeout=4.0,
                        )
                except Exception:
                    pass

                cmd = [
                    ff, "-hide_banner", "-loglevel", "error",
                    "-rtsp_transport", "tcp",
                    "-fflags", "nobuffer",
                    "-flags", "low_delay",
                    "-i", url,
                    "-an",
                    "-vf", "fps=20,scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,format=yuv420p",
                    *vcodec,
                    "-flush_packets", "1",
                    "-f", "h264",
                    "pipe:1",
                ]
                proc = subprocess.Popen(
                    cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0
                )
                self._proc = proc

                # Wait for the FIRST real H.264 chunk BEFORE opening POST /v2/streams/prusa-cam!
                # If the RTSP camera is offline/unreachable, ffmpeg exits with 0 bytes and we
                # never trigger a false `active: true` flash on the keyboard!
                out_fd = proc.stdout.fileno()
                first_chunk = os.read(out_fd, 16384)
                if not first_chunk:
                    proc.wait(timeout=2.0)
                    if not self._stop.is_set():
                        time.sleep(8.0)
                    continue

                conn = (
                    http.client.HTTPSConnection(parsed.hostname, parsed.port, context=SSL_CTX, timeout=15)
                    if parsed.scheme == "https"
                    else http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=15)
                )
                conn.putrequest("POST", "/v2/streams/prusa-cam")
                conn.putheader("Content-Type", "video/h264")
                conn.putheader("Transfer-Encoding", "chunked")
                conn.endheaders()

                hdr = f"{len(first_chunk):x}\r\n".encode("ascii")
                conn.send(hdr + first_chunk + b"\r\n")
                self.frames_uploaded += 1

                while not self._stop.is_set():
                    chunk = os.read(out_fd, 16384)
                    if not chunk:
                        break
                    hdr = f"{len(chunk):x}\r\n".encode("ascii")
                    conn.send(hdr + chunk + b"\r\n")
                    self.frames_uploaded += 1
                conn.send(b"0\r\n\r\n")
                try:
                    conn.getresponse().read()
                except Exception:
                    pass
                proc.wait(timeout=2.0)
            except Exception as e:
                print(f"[prusa] RTSP H.264 stream reconnect: {e}", flush=True)
            finally:
                if conn is not None:
                    try:
                        conn.close()
                    except Exception:
                        pass
                    self._close_stream()
            if not self._stop.is_set():
                time.sleep(6.0)


def main():
    printer_url = str(CFG.get("printer_url") or "https://connect.prusa3d.com")
    printer_uuid = str(CFG.get("printer_uuid") or "")
    api_token = str(CFG.get("api_token") or "")
    custom_cam = str(CFG.get("camera_url") or "")
    poll_sec = max(2.0, float(CFG.get("poll_seconds") or 5.0))

    print(f"[prusa] poller started (url={printer_url}, token_set={bool(api_token)}, cam={custom_cam or 'auto'})", flush=True)
    last_tel = 0.0
    last_cam = 0.0
    cam_url = custom_cam
    thumb_url = ""
    last_thumb_uploaded = ""
    cam_headers = {}
    rtsp_grabber = RtspFrameGrabber()
    printer_active = False

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
                            "camera_mode": "none",
                        },
                    },
                )
            time.sleep(1.0)
            continue

        if api_token and (now - last_tel >= poll_sec):
            last_tel = now
            try:
                tel = fetch_prusa_telemetry(printer_url, api_token, printer_uuid)
                default_cam = tel.pop("default_cam_url", "")
                thumb_url = tel.pop("thumb_url", "")
                default_headers = tel.pop("cam_headers", {})
                cam_url = custom_cam or default_cam
                cam_headers = {} if custom_cam else default_headers
                printer_active = tel.get("state") in ("PRINTING", "BUSY", "ATTENTION", "PAUSED")
                # If no live camera URL is set yet, upload the active print's thumbnail once.
                if not cam_url and thumb_url and thumb_url != last_thumb_uploaded:
                    if fetch_and_upload_snapshot(thumb_url, default_headers, is_live_cam=False):
                        last_thumb_uploaded = thumb_url
                        print(f"[prusa] uploaded print preview thumbnail: {thumb_url}", flush=True)
                tel["camera_path"] = CAM_LOCAL_PATH if os.path.isfile(CAM_LOCAL_PATH) else ""
                tel["camera_kb_path"] = "/home/root/openflux/media/prusa_cam_live.jpg"
                tel["camera_mode"] = "live" if cam_url else ("thumbnail" if last_thumb_uploaded else "none")
                tel["camera_frames"] = rtsp_grabber.frames_uploaded
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
                printer_active = False
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

        # Persistent ~2.5 FPS RTSP stream while printing or peeking; periodic snapshot for HTTP cams
        if cam_url and cam_url.lower().startswith("rtsp://"):
            if printer_active or peeking or not api_token:
                rtsp_grabber.ensure_running(cam_url)
            else:
                rtsp_grabber.stop()
        else:
            rtsp_grabber.stop()
            cam_interval = 0.55 if peeking else 20.0
            if cam_url and (now - last_cam >= cam_interval):
                last_cam = now
                fetch_and_upload_snapshot(cam_url, cam_headers, is_live_cam=True)

        time.sleep(0.35 if peeking else 0.75)


if __name__ == "__main__":
    main()
