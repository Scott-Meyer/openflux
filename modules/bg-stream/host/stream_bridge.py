#!/usr/bin/env python3
"""
OpenFlux v2 Live Stream Background Host Service (`bg-stream`)
Streams YouTube, Twitch, HLS (.m3u8), RTSP, or any ffmpeg-readable video directly over
OpenFlux v2's `CH_MEDIA` live H.264 channel (`POST /v2/streams/bg-stream`) into the
keyboard's RK3566 hardware `rkmpp` VPU decoder at 1080p 30-60 FPS with ZERO disk writes!

When idle, stopped, or offline, closes the stream immediately so `bg-stream` is 100%
transparent and your underlying `bg-media` ocean/Patagonia video plays uninterrupted!
"""
import http.client
import json
import os
import shutil
import ssl
import subprocess
import time
from urllib.parse import urlparse

BUS_URL = os.environ.get("OPENFLUX_BUS_URL", "https://127.0.0.1:52323")
CONFIG_JSON = os.environ.get("OPENFLUX_CONFIG_JSON") or os.environ.get("OPENFLUX_MODULE_CONFIG") or "{}"
STREAM_NAME = "bg-stream"
SSL_CTX = ssl._create_unverified_context()


def load_token() -> str:
    p = os.path.expanduser("~/.local/share/openflux/config/desktop_token.txt")
    try:
        with open(p, "r", encoding="utf-8") as f:
            return f.read().strip()
    except Exception:
        return ""


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
        print("[bg-stream] yt-dlp not found (brew install yt-dlp)", flush=True)
        return None, "missing_yt_dlp"
    try:
        res = subprocess.run(
            [
                ytdlp,
                "-g",
                "--no-warnings",
                "--no-playlist",
                "-f",
                "bv*[height<=1080][vcodec^=avc1]/bv*[height<=1080]/best[height<=1080][vcodec!=none]/best",
                u,
            ],
            capture_output=True,
            text=True,
            timeout=20,
        )
        if res.returncode != 0:
            err = (res.stderr or "").strip().splitlines()
            print(f"[bg-stream] yt-dlp failed: {err[-1] if err else res.returncode}", flush=True)
            return None, "offline_or_error"
        lines = [ln.strip() for ln in res.stdout.splitlines() if ln.strip()]
        return (lines[0], "yt-dlp") if lines else (None, "no_url")
    except Exception as e:
        print(f"[bg-stream] yt-dlp error: {e}", flush=True)
        return None, "error"


def close_remote_stream():
    parsed = urlparse(BUS_URL)
    tok = load_token()
    try:
        conn = (
            http.client.HTTPSConnection(parsed.hostname, parsed.port, context=SSL_CTX, timeout=3)
            if parsed.scheme == "https"
            else http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=3)
        )
        headers = {"Authorization": f"Bearer {tok}"} if tok else {}
        conn.request("DELETE", f"/v2/streams/{STREAM_NAME}", headers=headers)
        conn.getresponse().read()
        conn.close()
    except Exception:
        pass


def main():
    try:
        cfg = json.loads(CONFIG_JSON)
    except Exception:
        cfg = {}

    raw_url = str(cfg.get("stream_url") or "").strip()
    active = cfg.get("active", True) is not False
    fps = max(10, min(60, int(float(cfg.get("fps") or 30))))

    close_remote_stream()
    if not active or not raw_url:
        print("[bg-stream] idle (no stream_url — 100% transparent)", flush=True)
        while True:
            time.sleep(5)

    ffmpeg = find_bin("ffmpeg")
    if not ffmpeg:
        print("[bg-stream] ffmpeg not found", flush=True)
        while True:
            time.sleep(10)

    parsed = urlparse(BUS_URL)

    while True:
        print(f"[bg-stream] resolving: {raw_url}", flush=True)
        resolved, how = resolve_stream_url(raw_url)
        if not resolved:
            close_remote_stream()
            time.sleep(8)
            continue

        # Hardware Apple VideoToolbox H.264 on macOS (<1% CPU!), fallback to ultrafast libx264
        vcodec = (
            ["-c:v", "h264_videotoolbox", "-realtime", "true", "-b:v", "4500k", "-g", str(fps)]
            if os.uname().sysname == "Darwin"
            else ["-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-b:v", "4500k", "-g", str(fps)]
        )
        cmd = [ffmpeg, "-hide_banner", "-loglevel", "error", "-re"]
        if resolved.lower().startswith("rtsp://"):
            cmd += ["-rtsp_transport", "tcp"]
        elif resolved.lower().startswith("http://") or resolved.lower().startswith("https://"):
            cmd += ["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "5"]

        cmd += [
            "-i",
            resolved,
            "-an",
            "-vf",
            f"fps={fps},scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080,format=yuv420p",
            *vcodec,
            "-bsf:v",
            "h264_mp4toannexb",
            "-f",
            "h264",
            "-",
        ]

        print(f"[bg-stream] streaming live 1080p@{fps}fps H.264 Annex-B ({how}) -> /v2/streams/{STREAM_NAME}", flush=True)
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

        tok = load_token()
        conn = None
        try:
            conn = (
                http.client.HTTPSConnection(parsed.hostname, parsed.port, context=SSL_CTX, timeout=15)
                if parsed.scheme == "https"
                else http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=15)
            )
            conn.putrequest("POST", f"/v2/streams/{STREAM_NAME}")
            conn.putheader("Content-Type", "video/h264")
            conn.putheader("Transfer-Encoding", "chunked")
            if tok:
                conn.putheader("Authorization", f"Bearer {tok}")
            conn.endheaders()

            while True:
                chunk = proc.stdout.read(32768)
                if not chunk:
                    break
                header = f"{len(chunk):x}\r\n".encode("ascii")
                conn.send(header + chunk + b"\r\n")
            conn.send(b"0\r\n\r\n")
            try:
                conn.getresponse().read()
            except Exception:
                pass
        except Exception as e:
            print(f"[bg-stream] stream pipe ended: {e}", flush=True)
        finally:
            if conn is not None:
                try:
                    conn.close()
                except Exception:
                    pass
            try:
                proc.terminate()
                proc.wait(timeout=2)
            except Exception:
                try:
                    proc.kill()
                except Exception:
                    pass
            close_remote_stream()
            print("[bg-stream] stream closed — returning to 100% transparent", flush=True)
        time.sleep(4)


if __name__ == "__main__":
    main()
