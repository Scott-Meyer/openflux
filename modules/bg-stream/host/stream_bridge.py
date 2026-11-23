#!/usr/bin/env python3
"""
OpenFlux v2 Live Stream Background Host Service (`bg-stream`)
Supports:
- YouTube videos & YouTube Live (via yt-dlp + ffmpeg)
- Twitch live channels & VODs (via yt-dlp + ffmpeg)
- Kick / Vimeo / 1,000+ yt-dlp extractors
- Direct HLS (.m3u8), RTSP (rtsp://), RTMP, HTTP MP4 / MJPEG streams (via ffmpeg directly)

When idle, stopped, or offline, publishes `live = false` so `bg-stream` becomes 100% transparent
and your underlying `bg-media` background plays uninterrupted!
"""
import json
import os
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

BUS_URL = os.environ.get("OPENFLUX_BUS_URL", "https://127.0.0.1:52323")
CONFIG_JSON = os.environ.get("OPENFLUX_MODULE_CONFIG", "{}")
LOCAL_FRAME = os.path.join(tempfile.gettempdir(), "openflux_stream_live.jpg")
KB_REMOTE_NAME = "stream_live.jpg"
KB_REMOTE_PATH = f"/home/root/openflux/media/{KB_REMOTE_NAME}"

SSL_CTX = ssl._create_unverified_context()


def load_token() -> str:
    p = os.path.expanduser("~/.local/share/openflux/config/desktop_token.txt")
    try:
        with open(p, "r", encoding="utf-8") as f:
            return f.read().strip()
    except Exception:
        return ""


def api_post(path: str, payload: dict, timeout: float = 4.0) -> bool:
    tok = load_token()
    headers = {"Content-Type": "application/json"}
    if tok:
        headers["Authorization"] = f"Bearer {tok}"
    req = urllib.request.Request(
        f"{BUS_URL}{path}",
        data=json.dumps(payload).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, context=SSL_CTX, timeout=timeout):
            return True
    except Exception:
        return False


def publish_state(live: bool, url: str = "", status_text: str = "idle", fps: int = 12):
    api_post(
        "/v2/bus/pub",
        {
            "topic": "modules/bg-stream/state",
            "retain": True,
            "payload": {
                "live": live,
                "url": url,
                "status": status_text,
                "fps": fps,
                "local_path": LOCAL_FRAME,
                "kb_path": KB_REMOTE_PATH,
                "updated_ms": int(time.time() * 1000),
            },
        },
    )


def find_bin(name: str) -> str | None:
    found = shutil.which(name)
    if found:
        return found
    for prefix in ("/opt/homebrew/bin", "/usr/local/bin", os.path.expanduser("~/.local/bin")):
        cand = os.path.join(prefix, name)
        if os.path.isfile(cand) and os.access(cand, os.X_OK):
            return cand
    return None


def resolve_stream_url(raw_url: str) -> tuple[str | None, str]:
    u = raw_url.strip()
    if not u:
        return None, "empty"
    low = u.lower()
    # Direct media / streaming protocols go straight to ffmpeg!
    if (
        low.startswith("rtsp://")
        or low.startswith("rtmp://")
        or low.startswith("udp://")
        or low.endswith(".m3u8")
        or low.endswith(".mp4")
        or low.endswith(".mkv")
        or low.endswith(".webm")
        or ".m3u8?" in low
        or os.path.isfile(u)
    ):
        return u, "direct"

    ytdlp = find_bin("yt-dlp")
    if not ytdlp:
        print("[bg-stream] yt-dlp not found; install with `brew install yt-dlp` for YouTube/Twitch URLs", flush=True)
        return None, "missing_yt_dlp"

    try:
        # Resolve direct video stream URL (prefer <=720p H.264/MP4/HLS for snappy decoding)
        cmd = [
            ytdlp,
            "-g",
            "--no-warnings",
            "--no-playlist",
            "-f",
            "best[height<=720][vcodec!=none]/best[height<=1080][vcodec!=none]/best",
            u,
        ]
        res = subprocess.run(cmd, capture_output=True, text=True, timeout=20)
        if res.returncode != 0:
            err = (res.stderr or "").strip().splitlines()
            print(f"[bg-stream] yt-dlp could not resolve stream: {err[-1] if err else res.returncode}", flush=True)
            return None, "offline_or_error"
        lines = [ln.strip() for ln in res.stdout.splitlines() if ln.strip()]
        if not lines:
            return None, "no_stream_url"
        return lines[0], "yt-dlp"
    except Exception as e:
        print(f"[bg-stream] yt-dlp error: {e}", flush=True)
        return None, "error"


def main():
    try:
        cfg = json.loads(CONFIG_JSON)
    except Exception:
        cfg = {}

    raw_url = str(cfg.get("stream_url") or "").strip()
    active = cfg.get("active", True) is not False
    fps = max(2, min(24, int(float(cfg.get("fps") or 12))))
    quality = max(2, min(12, int(float(cfg.get("quality") or 6))))

    publish_state(False, raw_url, "idle", fps)
    if not active or not raw_url:
        print("[bg-stream] idle (no stream_url configured — 100% transparent)", flush=True)
        while True:
            time.sleep(5)

    ffmpeg = find_bin("ffmpeg")
    if not ffmpeg:
        print("[bg-stream] ffmpeg not found in PATH", flush=True)
        while True:
            time.sleep(10)

    # Background keyboard uploader thread so USB I/O never stalls ffmpeg frame reading!
    latest_lock = threading.Lock()
    latest_seq = [0]
    latest_bytes = [b""]
    stop_flag = [False]

    def kb_uploader():
        last_sent = 0
        while not stop_flag[0]:
            with latest_lock:
                seq = latest_seq[0]
            if seq != last_sent and os.path.isfile(LOCAL_FRAME):
                last_sent = seq
                api_post(
                    "/v2/keyboard/upload-media",
                    {"local_path": LOCAL_FRAME, "remote_name": KB_REMOTE_NAME},
                    timeout=3.0,
                )
            else:
                time.sleep(0.015)

    t_up = threading.Thread(target=kb_uploader, daemon=True)
    t_up.start()

    while not stop_flag[0]:
        print(f"[bg-stream] resolving stream: {raw_url}", flush=True)
        resolved, how = resolve_stream_url(raw_url)
        if not resolved:
            publish_state(False, raw_url, how, fps)
            time.sleep(8)
            continue

        print(f"[bg-stream] starting ffmpeg ({how}, {fps} fps, q={quality})", flush=True)
        cmd = [
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-re",
        ]
        if resolved.lower().startswith("rtsp://"):
            cmd += ["-rtsp_transport", "tcp"]
        elif resolved.lower().startswith("http://") or resolved.lower().startswith("https://"):
            cmd += ["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "5"]

        # Cover-crop cleanly to 16:9 (1280x720) and pipe raw MJPEG frames to stdout
        cmd += [
            "-i",
            resolved,
            "-an",
            "-vf",
            f"fps={fps},scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720",
            "-c:v",
            "mjpeg",
            "-q:v",
            str(quality),
            "-f",
            "image2pipe",
            "-",
        ]

        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        buf = bytearray()
        is_live = False
        tmp_path = LOCAL_FRAME + ".tmp"

        try:
            while True:
                chunk = proc.stdout.read(16384)
                if not chunk:
                    break
                buf.extend(chunk)
                # Extract complete JPEG frames (SOI 0xFFD8 .. EOI 0xFFD9)
                while True:
                    soi = buf.find(b"\xff\xd8")
                    if soi < 0:
                        buf.clear()
                        break
                    eoi = buf.find(b"\xff\xd9", soi + 2)
                    if eoi < 0:
                        if soi > 0:
                            del buf[:soi]
                        break
                    frame = bytes(buf[soi : eoi + 2])
                    del buf[: eoi + 2]

                    try:
                        with open(tmp_path, "wb") as f:
                            f.write(frame)
                        os.replace(tmp_path, LOCAL_FRAME)
                        with latest_lock:
                            latest_bytes[0] = frame
                            latest_seq[0] += 1
                        if not is_live:
                            is_live = True
                            publish_state(True, raw_url, "streaming", fps)
                            print("[bg-stream] LIVE! streaming frames to Studio + keyboard", flush=True)
                    except Exception:
                        pass
        finally:
            try:
                proc.terminate()
                proc.wait(timeout=2)
            except Exception:
                try:
                    proc.kill()
                except Exception:
                    pass
            if is_live:
                publish_state(False, raw_url, "ended", fps)
                print("[bg-stream] stream ended — returning to transparent", flush=True)
        time.sleep(4)


if __name__ == "__main__":
    main()
