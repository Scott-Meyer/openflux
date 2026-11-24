#!/usr/bin/env python3
"""
KeyGlass Stream Deck SDK v2 Host Bridge (`modules/streamdeck-bridge/host/sd_host.py`)

Runs real Elgato `.sdPlugin` packages using Elgato's official Stream Deck WebSocket JSON protocol
(zero third-party pip dependencies — pure Python stdlib RFC 6455 WebSocket server!) AND native
built-in starter actions (`builtin.open_url`, `builtin.launch_app`, `builtin.run_command`,
`builtin.toggle_command`, `builtin.counter`).

Flow:
  1. Reads configured buttons from `~/.local/share/openflux/v2/streamdeck/buttons.json`
     and watches `modules/streamdeck/buttons` + `modules/streamdeck/key` on the authenticated v2 bus.
  2. Spawns any referenced `.sdPlugin` `CodePathMac`/`CodePath` binaries with official Elgato flags:
     `-port <ws_port> -pluginUUID <uuid> -registerEvent registerPlugin -info <json>`.
  3. Sends `willAppear`, `keyDown`, and `keyUp` JSON events over WebSocket; receives live `setImage`,
     `setTitle`, `setState`, `showAlert`, `showOk`, and `setSettings` from plugins!
  4. Saves live PNGs to `~/.local/share/openflux/media/sd_<btn_id>.png`, uploads to the keyboard
     over OFV2 `/v2/keyboard/upload-media`, and publishes retained `modules/streamdeck/faces`!
"""

import base64
import hashlib
import json
import os
import shutil
import socket
import ssl
import struct
import subprocess
import threading
import time
import urllib.request

BUS_URL = os.environ.get("OPENFLUX_BUS_URL", "http://127.0.0.1:52324").rstrip("/")
BUS_TOKEN = os.environ.get("OPENFLUX_BUS_TOKEN", "").strip()
SSL_CTX = ssl._create_unverified_context() if BUS_URL.startswith("https://") else None

DATA_ROOT = os.path.expanduser("~/.local/share/openflux")
SD_ROOT = os.path.join(DATA_ROOT, "v2", "streamdeck")
PLUGINS_DIR = os.path.join(SD_ROOT, "plugins")
BUTTONS_PATH = os.path.join(SD_ROOT, "buttons.json")
MEDIA_DIR = os.path.join(DATA_ROOT, "media")
os.makedirs(PLUGINS_DIR, exist_ok=True)
os.makedirs(MEDIA_DIR, exist_ok=True)

WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def _auth_headers(extra=None):
    h = dict(extra or {})
    if BUS_TOKEN:
        h["Authorization"] = f"Bearer {BUS_TOKEN}"
    return h


def bus_post(path: str, payload: dict, timeout: float = 4.0):
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        f"{BUS_URL}{path}",
        data=data,
        headers=_auth_headers({"Content-Type": "application/json"}),
        method="POST",
    )
    try:
        if SSL_CTX is not None:
            with urllib.request.urlopen(req, context=SSL_CTX, timeout=timeout) as r:
                return json.loads(r.read().decode("utf-8"))
        else:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read().decode("utf-8"))
    except Exception:
        return None


def bus_poll_events(pattern: str, since_seq: int, wait_ms: int = 1500):
    url = f"{BUS_URL}/v2/bus/events?pattern={urllib.request.quote(pattern)}&since_seq={since_seq}&wait_ms={wait_ms}"
    req = urllib.request.Request(url, headers=_auth_headers())
    try:
        if SSL_CTX is not None:
            with urllib.request.urlopen(req, context=SSL_CTX, timeout=5.0) as r:
                return json.loads(r.read().decode("utf-8"))
        else:
            with urllib.request.urlopen(req, timeout=5.0) as r:
                return json.loads(r.read().decode("utf-8"))
    except Exception:
        return None


# ---- Minimal RFC 6455 WebSocket Server (Pure Python stdlib) ----

def ws_send_text(conn: socket.socket, text: str):
    payload = text.encode("utf-8")
    n = len(payload)
    if n < 126:
        hdr = bytes([0x81, n])
    elif n < 65536:
        hdr = struct.pack("!BBH", 0x81, 126, n)
    else:
        hdr = struct.pack("!BBQ", 0x81, 127, n)
    conn.sendall(hdr + payload)


def ws_recv_exact(conn: socket.socket, n: int) -> bytes | None:
    buf = bytearray()
    while len(buf) < n:
        chunk = conn.recv(n - len(buf))
        if not chunk:
            return None
        buf.extend(chunk)
    return bytes(buf)


def ws_recv_frame(conn: socket.socket) -> str | None:
    while True:
        hdr = ws_recv_exact(conn, 2)
        if not hdr:
            return None
        opcode = hdr[0] & 0x0F
        masked = (hdr[1] & 0x80) != 0
        length = hdr[1] & 0x7F
        if length == 126:
            ext = ws_recv_exact(conn, 2)
            if not ext:
                return None
            length = struct.unpack("!H", ext)[0]
        elif length == 127:
            ext = ws_recv_exact(conn, 8)
            if not ext:
                return None
            length = struct.unpack("!Q", ext)[0]
        mask_key = ws_recv_exact(conn, 4) if masked else None
        data = ws_recv_exact(conn, length) if length > 0 else b""
        if data is None:
            return None
        if masked and mask_key:
            data = bytes(b ^ mask_key[i % 4] for i, b in enumerate(data))
        if opcode == 0x8:  # Close
            return None
        if opcode == 0x9:  # Ping -> Pong
            conn.sendall(bytes([0x8A, len(data)]) + data)
            continue
        if opcode in (0x1, 0x2):
            return data.decode("utf-8", "ignore")


class StreamDeckHost:
    def __init__(self):
        self.lock = threading.Lock()
        self.buttons: dict[str, dict] = {}
        self.faces: dict[str, dict] = {}
        self.toggle_states: dict[str, bool] = {}
        self.counters: dict[str, int] = {}
        self.plugin_sockets: dict[str, socket.socket] = {}  # plugin_uuid -> ws socket
        self.plugin_procs: dict[str, subprocess.Popen] = {}
        self.last_img_hash: dict[str, str] = {}
        self.ws_port = 0

    def start_ws_server(self):
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind(("127.0.0.1", 0))
        self.ws_port = srv.getsockname()[1]
        srv.listen(16)
        print(f"[streamdeck] SDK WebSocket host listening on ws://127.0.0.1:{self.ws_port}", flush=True)

        def accept_loop():
            while True:
                try:
                    conn, _ = srv.accept()
                    threading.Thread(target=self._handle_ws_client, args=(conn,), daemon=True).start()
                except Exception:
                    time.sleep(0.2)

        threading.Thread(target=accept_loop, daemon=True).start()

    def _handle_ws_client(self, conn: socket.socket):
        plugin_uuid = None
        try:
            req = b""
            while b"\r\n\r\n" not in req and len(req) < 8192:
                chunk = conn.recv(1024)
                if not chunk:
                    return
                req += chunk
            key = None
            for line in req.decode("utf-8", "ignore").splitlines():
                if line.lower().startswith("sec-websocket-key:"):
                    key = line.split(":", 1)[1].strip()
                    break
            if not key:
                return
            accept = base64.b64encode(hashlib.sha1((key + WS_GUID).encode("ascii")).digest()).decode("ascii")
            resp = (
                "HTTP/1.1 101 Switching Protocols\r\n"
                "Upgrade: websocket\r\n"
                "Connection: Upgrade\r\n"
                f"Sec-WebSocket-Accept: {accept}\r\n\r\n"
            )
            conn.sendall(resp.encode("ascii"))

            while True:
                raw = ws_recv_frame(conn)
                if raw is None:
                    break
                try:
                    msg = json.loads(raw)
                except Exception:
                    continue
                ev = msg.get("event", "")
                if ev == "registerPlugin":
                    plugin_uuid = msg.get("uuid", "")
                    with self.lock:
                        self.plugin_sockets[plugin_uuid] = conn
                    print(f"[streamdeck] plugin registered: {plugin_uuid}", flush=True)
                    self._send_will_appear_for_plugin(plugin_uuid, conn)
                else:
                    self._handle_plugin_event(msg)
        except Exception:
            pass
        finally:
            if plugin_uuid:
                with self.lock:
                    if self.plugin_sockets.get(plugin_uuid) is conn:
                        del self.plugin_sockets[plugin_uuid]
            try:
                conn.close()
            except Exception:
                pass

    def _send_will_appear_for_plugin(self, plugin_uuid: str, conn: socket.socket):
        with self.lock:
            btns = [b for b in self.buttons.values() if b.get("plugin_id") == plugin_uuid]
        for idx, b in enumerate(btns):
            ev = {
                "action": b.get("action_uuid", ""),
                "event": "willAppear",
                "context": b["id"],
                "device": "keyglass-main",
                "payload": {
                    "settings": b.get("settings") or {},
                    "coordinates": {"column": idx % 8, "row": idx // 8},
                    "state": 0,
                    "isInMultiAction": False,
                },
            }
            try:
                ws_send_text(conn, json.dumps(ev))
            except Exception:
                pass

    def _handle_plugin_event(self, msg: dict):
        ev = msg.get("event", "")
        ctx_id = msg.get("context", "")
        payload = msg.get("payload") or {}
        if not ctx_id:
            return
        changed = False
        with self.lock:
            face = self.faces.setdefault(ctx_id, {})
            if ev == "setTitle":
                face["title"] = str(payload.get("title", ""))
                changed = True
            elif ev == "setState":
                face["state"] = int(payload.get("state", 0))
                changed = True
            elif ev == "showAlert":
                face["alert"] = True
                face["ok"] = False
                changed = True
            elif ev == "showOk":
                face["ok"] = True
                face["alert"] = False
                changed = True
            elif ev == "setImage":
                img_data = payload.get("image", "")
                if isinstance(img_data, str) and "base64," in img_data:
                    b64 = img_data.split("base64,", 1)[1]
                    try:
                        raw = base64.b64decode(b64)
                        h = hashlib.sha256(raw).hexdigest()[:16]
                        if self.last_img_hash.get(ctx_id) != h:
                            self.last_img_hash[ctx_id] = h
                            fname = f"sd_live_{ctx_id}.png"
                            out_p = os.path.join(MEDIA_DIR, fname)
                            with open(out_p, "wb") as f:
                                f.write(raw)
                            bus_post(
                                "/v2/keyboard/upload-media",
                                {"local_path": out_p, "remote_name": fname},
                                timeout=4.0,
                            )
                            face["image"] = f"media://{fname}"
                            changed = True
                    except Exception:
                        pass
        if changed:
            self.publish_faces()

    def publish_faces(self):
        with self.lock:
            snap = dict(self.faces)
        bus_post(
            "/v2/bus/pub",
            {"topic": "modules/streamdeck/faces", "payload": snap, "retain": True},
        )

    def load_buttons(self):
        try:
            with open(BUTTONS_PATH, "r", encoding="utf-8") as f:
                doc = json.load(f)
            arr = doc.get("buttons", [])
        except Exception:
            arr = []
        with self.lock:
            self.buttons = {b["id"]: b for b in arr if isinstance(b, dict) and b.get("id")}
            for bid, b in self.buttons.items():
                face = self.faces.setdefault(bid, {})
                face.setdefault("title", b.get("title", ""))
                if b.get("icon_media"):
                    face.setdefault("image", f"media://{b['icon_media']}")
                if b.get("color"):
                    face["color"] = b["color"]
        self.publish_faces()
        self._ensure_plugins_running()

    def _ensure_plugins_running(self):
        needed_plugins = {
            b.get("plugin_id")
            for b in self.buttons.values()
            if b.get("plugin_id") and b.get("plugin_id") != "builtin"
        }
        for pid in needed_plugins:
            if pid in self.plugin_procs and self.plugin_procs[pid].poll() is None:
                continue
            pdir = os.path.join(PLUGINS_DIR, f"{pid}.sdPlugin")
            mpath = os.path.join(pdir, "manifest.json")
            if not os.path.isfile(mpath):
                continue
            try:
                with open(mpath, "r", encoding="utf-8") as f:
                    m = json.load(f)
                code_rel = m.get("CodePathMac") or m.get("CodePath")
                if not code_rel:
                    continue
                code_abs = os.path.join(pdir, code_rel)
                if not os.path.isfile(code_abs):
                    continue
                info_json = json.dumps({
                    "application": {"language": "en", "platform": "mac", "platformVersion": "14.0", "version": "6.5.0"},
                    "plugin": {"uuid": pid, "version": m.get("Version", "1.0.0")},
                    "devicePixelRatio": 2,
                    "devices": [{"id": "keyglass-main", "name": "KeyGlass", "size": {"columns": 15, "rows": 6}, "type": 0}],
                })
                args = [
                    "-port", str(self.ws_port),
                    "-pluginUUID", pid,
                    "-registerEvent", "registerPlugin",
                    "-info", info_json,
                ]
                if code_abs.endswith(".js") or code_abs.endswith(".mjs"):
                    node = shutil.which("node") or "/opt/homebrew/bin/node"
                    cmd = [node, code_abs] + args
                elif code_abs.endswith(".py"):
                    cmd = ["python3", code_abs] + args
                else:
                    os.chmod(code_abs, 0o755)
                    cmd = [code_abs] + args
                proc = subprocess.Popen(cmd, cwd=pdir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                self.plugin_procs[pid] = proc
                print(f"[streamdeck] launched .sdPlugin '{pid}' (pid={proc.pid})", flush=True)
            except Exception as e:
                print(f"[streamdeck] failed to launch '{pid}': {e}", flush=True)

    def handle_key(self, button_id: str, pressed: bool):
        with self.lock:
            b = self.buttons.get(button_id)
        if not b:
            return
        pid = b.get("plugin_id", "builtin")
        auuid = b.get("action_uuid", "")
        settings = b.get("settings") or {}

        # Built-in native Stream Deck actions (work immediately with zero external plugins!)
        if pid == "builtin":
            if not pressed:
                return
            if auuid == "builtin.open_url":
                url = str(settings.get("url", "")).strip()
                if url:
                    subprocess.Popen(["open", url])
            elif auuid == "builtin.launch_app":
                app = str(settings.get("app", "")).strip()
                if app:
                    subprocess.Popen(["open", "-a", app])
            elif auuid == "builtin.run_command":
                cmd = str(settings.get("command", "")).strip()
                if cmd:
                    subprocess.Popen(["/bin/sh", "-c", cmd])
            elif auuid == "builtin.toggle_command":
                nxt = not self.toggle_states.get(button_id, False)
                self.toggle_states[button_id] = nxt
                cmd = str(settings.get("on_command" if nxt else "off_command", "")).strip()
                if cmd:
                    subprocess.Popen(["/bin/sh", "-c", cmd])
                with self.lock:
                    face = self.faces.setdefault(button_id, {})
                    face["title"] = str(settings.get("on_title" if nxt else "off_title", b.get("title", "")))
                    face["color"] = "#58f29c" if nxt else "#94a3b8"
                self.publish_faces()
            elif auuid == "builtin.counter":
                c = self.counters.get(button_id, 0) + 1
                self.counters[button_id] = c
                with self.lock:
                    face = self.faces.setdefault(button_id, {})
                    face["title"] = f"{b.get('title', 'Cnt')} {c}"
                self.publish_faces()
            return

        # Real Elgato .sdPlugin WebSocket event dispatch (`keyDown` / `keyUp`)!
        with self.lock:
            ws = self.plugin_sockets.get(pid)
        if ws is not None:
            ev = {
                "action": auuid,
                "event": "keyDown" if pressed else "keyUp",
                "context": button_id,
                "device": "keyglass-main",
                "payload": {
                    "settings": settings,
                    "coordinates": {"column": 0, "row": 0},
                    "state": self.faces.get(button_id, {}).get("state", 0),
                    "isInMultiAction": False,
                },
            }
            try:
                ws_send_text(ws, json.dumps(ev))
            except Exception as e:
                print(f"[streamdeck] key send to '{pid}' failed: {e}", flush=True)


def main():
    host = StreamDeckHost()
    host.start_ws_server()
    host.load_buttons()
    seq = 0
    while True:
        res = bus_poll_events("modules/streamdeck/#", seq, wait_ms=1500)
        if isinstance(res, dict):
            seq = int(res.get("latest_seq", seq))
            for ev in res.get("events", []):
                topic = ev.get("topic", "")
                p = ev.get("payload") or {}
                if topic == "modules/streamdeck/buttons":
                    host.load_buttons()
                elif topic == "modules/streamdeck/key":
                    bid = str(p.get("button_id", ""))
                    pressed = bool(p.get("pressed", False))
                    if bid:
                        host.handle_key(bid, pressed)
        else:
            time.sleep(0.5)


if __name__ == "__main__":
    main()
