#!/usr/bin/env python3
"""Linux integration tests exercising the compiled openflux-bridge binary."""

import fcntl
import os
import select
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "tools/openflux-dev"))
import simple_hdlc

BRIDGE_BIN = Path(os.environ.get("OPENFLUX_TEST_BRIDGE_BIN") or (ROOT / "keyboard/bridge/build/openflux-bridge"))


def is_blocked_in_poll(pid: int) -> bool:
    syscall_path = Path(f"/proc/{pid}/syscall")
    if syscall_path.exists():
        text = syscall_path.read_text().strip()
        # poll (7) and ppoll (271) on x86_64, ppoll (73) on aarch64; other architectures fall
        # back to wchan below.
        if text.startswith(("73 ", "271 ", "7 ")):
            return True
    wchan_path = Path(f"/proc/{pid}/wchan")
    if wchan_path.exists():
        wchan = wchan_path.read_text().strip()
        if "poll" in wchan:
            return True
    return False


def test_blocked_poll_sigterm() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ready_file = Path(tmp) / "openflux_bridge_ready"
        in_r, in_w = os.pipe()
        out_r, out_w = os.pipe()
        os.set_inheritable(out_w, True)
        os.set_inheritable(in_r, True)

        env = os.environ.copy()
        env["OPENFLUX_SYSROOT"] = str(tmp)
        env["OPENFLUX_READY_FILE"] = str(ready_file)
        env["OPENFLUX_TEST_EP_FDS"] = f"{out_w},{in_r}"

        proc = subprocess.Popen([str(BRIDGE_BIN)], env=env, pass_fds=(out_w, in_r))
        os.close(out_w)
        os.close(in_r)

        try:
            deadline = time.monotonic() + 3.0
            blocked = False
            while time.monotonic() < deadline:
                if ready_file.exists() and is_blocked_in_poll(proc.pid):
                    blocked = True
                    break
                time.sleep(0.02)

            assert ready_file.exists(), "bridge did not write readiness stamp"
            assert blocked, "bridge did not enter blocking poll() before SIGTERM"

            hdr = bytearray(512)
            struct.pack_into("<BII", hdr, 0, 1, 0, 0)  # CMD_REQUEST_BRIDGE_VERSION
            os.write(in_w, bytes(hdr))
            rtype, resp_data = read_response(out_r)
            assert rtype == 1, f"Bad response type {rtype}"
            assert resp_data.startswith(b"openflux-bridge "), f"Unexpected version: {resp_data[:32]!r}"

            put_payload = b"/home/root/openflux/test_split.txt\x00hello_split"
            put_pkt = put_payload.ljust(512, b"\x00")
            put_hdr = bytearray(512)
            struct.pack_into("<BII", put_hdr, 0, 102, 1, len(put_payload))
            os.write(in_w, bytes(put_hdr) + put_pkt)
            put_type, _ = read_response(out_r)
            assert put_type == 1, f"PUT transfer failed: type {put_type}"
            assert (Path(tmp) / "home/root/openflux/test_split.txt").read_bytes() == b"hello_split"

            bad_hdr = bytearray(512)
            struct.pack_into("<BII", bad_hdr, 0, 102, 2, 700)
            os.write(in_w, bytes(bad_hdr) + put_pkt)
            bad_type, _ = read_response(out_r)
            assert bad_type == 0, f"Expected NAK on short-read, got type {bad_type}"

            blocked = False
            deadline = time.monotonic() + 2.0
            while time.monotonic() < deadline:
                if is_blocked_in_poll(proc.pid):
                    blocked = True
                    break
                time.sleep(0.01)
            assert blocked, "bridge did not return to poll() after command"

            t0 = time.monotonic()
            proc.send_signal(signal.SIGTERM)
            rc = proc.wait(timeout=2.0)
            elapsed_ms = (time.monotonic() - t0) * 1000.0
            assert rc == 0, f"Expected exit 0 on SIGTERM, got {rc}"
            assert not ready_file.exists(), "bridge did not unlink ready stamp on SIGTERM"
            print(f"bridge poll/SIGTERM + USB-loop exit OK in {elapsed_ms:.1f} ms")
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=2.0)
            os.close(in_w)
            os.close(out_r)


def test_sigterm_before_read_race_window() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ready_file = Path(tmp) / "openflux_bridge_ready"
        in_r, in_w = os.pipe()
        out_r, out_w = os.pipe()
        os.set_inheritable(out_w, True)
        os.set_inheritable(in_r, True)

        env = os.environ.copy()
        env["OPENFLUX_READY_FILE"] = str(ready_file)
        env["OPENFLUX_TEST_EP_FDS"] = f"{out_w},{in_r}"

        proc = subprocess.Popen([str(BRIDGE_BIN)], env=env, pass_fds=(out_w, in_r))
        os.close(out_w)
        os.close(in_r)

        try:
            deadline = time.monotonic() + 3.0
            while time.monotonic() < deadline and not ready_file.exists():
                time.sleep(0.005)
            assert ready_file.exists(), "bridge did not write readiness stamp"
            proc.send_signal(signal.SIGTERM)
            rc = proc.wait(timeout=2.0)
            assert rc == 0, f"Expected exit 0, got {rc}"
            assert not ready_file.exists(), "ready stamp leaked"
            print("bridge immediate SIGTERM race-window test OK")
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=2.0)
            os.close(in_w)
            os.close(out_r)


def wait_pipe_drained(fd: int, timeout_s: float = 2.0) -> None:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        avail = struct.unpack("i", fcntl.ioctl(fd, termios.FIONREAD, b"\x00\x00\x00\x00"))[0]
        if avail == 0:
            return
        time.sleep(0.005)
    raise AssertionError(f"Pipe fd {fd} still had unread bytes after {timeout_s}s")


def read_with_deadline(fd: int, nbytes: int, timeout_s: float = 2.0) -> bytes:
    deadline = time.monotonic() + timeout_s
    out = bytearray()
    while len(out) < nbytes:
        rem = deadline - time.monotonic()
        assert rem > 0, f"Timed out reading {nbytes} bytes (got {len(out)})"
        rdy, _, _ = select.select([fd], [], [], rem)
        assert rdy, f"select() timed out reading {nbytes} bytes"
        chunk = os.read(fd, nbytes - len(out))
        assert chunk, "Unexpected EOF reading pipe"
        out.extend(chunk)
    return bytes(out)


def read_some_with_deadline(fd: int, max_bytes: int, timeout_s: float = 2.0) -> bytes:
    rdy, _, _ = select.select([fd], [], [], timeout_s)
    assert rdy, f"select() timed out waiting for data on fd {fd}"
    chunk = os.read(fd, max_bytes)
    assert chunk, "Unexpected EOF reading pipe"
    return chunk


def read_response(fd: int, timeout_s: float = 2.0) -> tuple[int, bytes]:
    """Reads one bridge response: the 512-byte header and its packets. Returns the type and payload."""
    hdr = read_with_deadline(fd, 512, timeout_s)
    rtype, rpkts, rbytes = struct.unpack_from("<BII", hdr, 0)
    body = read_with_deadline(fd, rpkts * 512, timeout_s) if rpkts else b""
    return rtype, body[:rbytes]


def read_ndjson_line(sock: socket.socket, buf: bytearray) -> str:
    while b"\n" not in buf:
        chunk = sock.recv(4096)
        assert chunk, "Unexpected EOF on IPC socket"
        buf.extend(chunk)
    nl = buf.index(b"\n")
    line = bytes(buf[:nl]).decode("utf-8")
    del buf[: nl + 1]
    return line


def connect_and_sync_ipc(sock_path: Path, expected_seq: int) -> tuple[socket.socket, bytearray]:
    cli = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    cli.settimeout(2.0)
    cli.connect(str(sock_path))
    cli.sendall(b'{"cmd":"get_state"}\n')
    buf = bytearray()
    line = read_ndjson_line(cli, buf)
    assert f'"seq":{expected_seq}' in line, f"Unexpected sync line: {line}"
    return cli, buf


def test_uart_and_display_ipc_roundtrip() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ready_file = Path(tmp) / "openflux_bridge_ready"
        sock_path = Path(tmp) / "bridge.sock"
        in_r, in_w = os.pipe()
        out_r, out_w = os.pipe()
        uart_rx_r, uart_rx_w = os.pipe()
        uart_tx_r, uart_tx_w = os.pipe()
        for fd in (out_w, in_r, uart_rx_r, uart_tx_w):
            os.set_inheritable(fd, True)

        env = os.environ.copy()
        env["OPENFLUX_READY_FILE"] = str(ready_file)
        env["OPENFLUX_TEST_EP_FDS"] = f"{out_w},{in_r}"
        env["OPENFLUX_TEST_UART_FDS"] = f"{uart_rx_r},{uart_tx_w}"
        env["OPENFLUX_BRIDGE_SOCK"] = str(sock_path)

        proc = subprocess.Popen([str(BRIDGE_BIN)], env=env, pass_fds=(out_w, in_r, uart_rx_r, uart_tx_w))
        os.close(out_w)
        os.close(in_r)
        os.close(uart_rx_r)
        os.close(uart_tx_w)

        try:
            deadline = time.monotonic() + 3.0
            while time.monotonic() < deadline and not (ready_file.exists() and sock_path.exists()):
                time.sleep(0.01)
            assert ready_file.exists() and sock_path.exists(), "bridge failed to create ready/socket"

            prime = bytearray(139)
            prime[0] = 2
            struct.pack_into("<I", prime, 1, 1)
            os.write(uart_rx_w, simple_hdlc.encode_frame(bytes(prime)))
            assert simple_hdlc.decode_frame(read_some_with_deadline(uart_tx_r, 64)) == bytes([2, 0, 0, 0])

            cli, cli_buf = connect_and_sync_ipc(sock_path, expected_seq=1)

            msg = bytearray(139)
            msg[0] = 2
            struct.pack_into("<I", msg, 1, 77)
            struct.pack_into("<I", msg, 5 + 0x20, 9999)
            msg[5 + 0x24] = 1
            msg[5 + 0x25] = 1
            os.write(uart_rx_w, simple_hdlc.encode_frame(bytes(msg)))

            ack_bytes = read_some_with_deadline(uart_tx_r, 64)
            assert simple_hdlc.decode_frame(ack_bytes) == bytes([2, 0, 0, 0])
            line1 = read_ndjson_line(cli, cli_buf)
            assert '"type":"key_state","seq":77,"timestamp_ms":9999' in line1, line1

            for frame in simple_hdlc.encode_live_json({"hardwareData": {"cpu": 25}}):
                hdr = bytearray(512)
                struct.pack_into("<BII", hdr, 0, 200, 1, len(frame))
                pkt = frame.ljust(512, b"\x00")
                os.write(in_w, bytes(hdr) + pkt)
                live_type, _ = read_response(out_r)
                assert live_type == 1

            line2 = read_ndjson_line(cli, cli_buf)
            assert '"type":"live_data","data_type":0,"payload":{"hardwareData":{"cpu":25}}' in line2, line2

            cli.close()
            proc.send_signal(signal.SIGTERM)
            assert proc.wait(timeout=2.0) == 0
            print("bridge UART + Display IPC + CMD 200 roundtrip OK")
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=2.0)
            for fd in (in_w, out_r, uart_rx_w, uart_tx_r):
                os.close(fd)


def test_max_ipc_clients_with_backpressured_split_uart() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ready_file = Path(tmp) / "openflux_bridge_ready"
        sock_path = Path(tmp) / "bridge.sock"
        in_r, in_w = os.pipe()
        out_r, out_w = os.pipe()
        uart_rx_r, uart_rx_w = os.pipe()
        uart_tx_r, uart_tx_w = os.pipe()
        for fd in (out_w, in_r, uart_rx_r, uart_tx_w):
            os.set_inheritable(fd, True)

        os.set_blocking(uart_tx_w, False)
        filler = b"\xAA" * 4096
        filled = 0
        while True:
            try:
                filled += os.write(uart_tx_w, filler)
            except BlockingIOError:
                break

        os.set_blocking(out_w, False)
        filler_usb = b"\x55" * 4096
        filled_usb = 0
        while True:
            try:
                filled_usb += os.write(out_w, filler_usb)
            except BlockingIOError:
                break

        env = os.environ.copy()
        env["OPENFLUX_READY_FILE"] = str(ready_file)
        env["OPENFLUX_TEST_EP_FDS"] = f"{out_w},{in_r}"
        env["OPENFLUX_TEST_UART_FDS"] = f"{uart_rx_r},{uart_tx_w}"
        env["OPENFLUX_BRIDGE_SOCK"] = str(sock_path)

        proc = subprocess.Popen([str(BRIDGE_BIN)], env=env, pass_fds=(out_w, in_r, uart_rx_r, uart_tx_w))
        os.close(out_w)
        os.close(in_r)
        os.close(uart_rx_r)
        os.close(uart_tx_w)

        clients = []
        bufs = []
        try:
            deadline = time.monotonic() + 3.0
            while time.monotonic() < deadline and not (ready_file.exists() and sock_path.exists()):
                time.sleep(0.01)
            assert ready_file.exists() and sock_path.exists()

            prime = bytearray(139)
            prime[0] = 2
            struct.pack_into("<I", prime, 1, 1)
            os.write(uart_rx_w, simple_hdlc.encode_frame(bytes(prime)))
            wait_pipe_drained(uart_rx_w)

            for _ in range(8):
                c, b = connect_and_sync_ipc(sock_path, expected_seq=1)
                clients.append(c)
                bufs.append(b)

            hdr_usb = bytearray(512)
            struct.pack_into("<BII", hdr_usb, 0, 1, 0, 0)
            os.write(in_w, bytes(hdr_usb))
            wait_pipe_drained(in_w)

            msg = bytearray(139)
            msg[0] = 2
            struct.pack_into("<I", msg, 1, 108)
            os.write(uart_rx_w, simple_hdlc.encode_frame(bytes(msg)))

            for c, b in zip(clients, bufs):
                line = read_ndjson_line(c, b)
                assert '"type":"key_state","seq":108' in line

            drained_usb = 0
            while drained_usb < filled_usb:
                drained_usb += len(read_with_deadline(out_r, min(4096, filled_usb - drained_usb)))
            usb_resp = read_with_deadline(out_r, 1024)
            assert usb_resp[0] == 1 and usb_resp[512:].startswith(b"openflux-bridge ")

            drained = 0
            while drained < filled:
                drained += len(read_with_deadline(uart_tx_r, min(4096, filled - drained)))
            ack_buf = b""
            while len(ack_buf) < 22:
                ack_buf += read_with_deadline(uart_tx_r, 22 - len(ack_buf))
            dec = simple_hdlc.SimpleHdlcDecoder()
            frames = dec.feed(ack_buf)
            assert frames == [bytes([2, 0, 0, 0]), bytes([2, 0, 0, 0])], frames

            proc.send_signal(signal.SIGTERM)
            assert proc.wait(timeout=2.0) == 0
            print("bridge 14-fd split-UART backpressure + 8 IPC clients OK")
        finally:
            for c in clients:
                c.close()
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=2.0)
            for fd in (in_w, out_r, uart_rx_w, uart_tx_r):
                os.close(fd)


def test_usb_in_backpressure_does_not_stall_uart_or_ipc() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ready_file = Path(tmp) / "openflux_bridge_ready"
        sock_path = Path(tmp) / "bridge.sock"
        in_r, in_w = os.pipe()
        out_r, out_w = os.pipe()
        uart_rx_r, uart_rx_w = os.pipe()
        uart_tx_r, uart_tx_w = os.pipe()
        for fd in (out_w, in_r, uart_rx_r, uart_tx_w):
            os.set_inheritable(fd, True)

        os.set_blocking(out_w, False)
        filler = b"\x55" * 4096
        filled = 0
        while True:
            try:
                filled += os.write(out_w, filler)
            except BlockingIOError:
                break

        env = os.environ.copy()
        env["OPENFLUX_READY_FILE"] = str(ready_file)
        env["OPENFLUX_TEST_EP_FDS"] = f"{out_w},{in_r}"
        env["OPENFLUX_TEST_UART_FDS"] = f"{uart_rx_r},{uart_tx_w}"
        env["OPENFLUX_BRIDGE_SOCK"] = str(sock_path)

        proc = subprocess.Popen([str(BRIDGE_BIN)], env=env, pass_fds=(out_w, in_r, uart_rx_r, uart_tx_w))
        os.close(out_w)
        os.close(in_r)
        os.close(uart_rx_r)
        os.close(uart_tx_w)

        try:
            deadline = time.monotonic() + 3.0
            while time.monotonic() < deadline and not (ready_file.exists() and sock_path.exists()):
                time.sleep(0.01)
            assert ready_file.exists() and sock_path.exists()

            prime = bytearray(139)
            prime[0] = 2
            struct.pack_into("<I", prime, 1, 1)
            os.write(uart_rx_w, simple_hdlc.encode_frame(bytes(prime)))
            assert simple_hdlc.decode_frame(read_some_with_deadline(uart_tx_r, 64)) == bytes([2, 0, 0, 0])
            cli, cli_buf = connect_and_sync_ipc(sock_path, expected_seq=1)

            hdr = bytearray(512)
            struct.pack_into("<BII", hdr, 0, 1, 0, 0)
            os.write(in_w, bytes(hdr))
            wait_pipe_drained(in_w)

            msg = bytearray(139)
            msg[0] = 2
            struct.pack_into("<I", msg, 1, 222)
            os.write(uart_rx_w, simple_hdlc.encode_frame(bytes(msg)))
            ack_bytes = read_with_deadline(uart_tx_r, 11)
            assert simple_hdlc.decode_frame(ack_bytes) == bytes([2, 0, 0, 0])
            assert '"seq":222' in read_ndjson_line(cli, cli_buf)

            proc.send_signal(signal.SIGTERM)
            assert proc.wait(timeout=2.0) == 0
            print("bridge USB-IN backpressure isolation + SIGTERM OK")
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=2.0)
            for fd in (in_w, out_r, uart_rx_w, uart_tx_r):
                os.close(fd)


def test_large_get_file_over_64kib_under_backpressure() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ready_file = Path(tmp) / "openflux_bridge_ready"
        in_r, in_w = os.pipe()
        out_r, out_w = os.pipe()
        os.set_inheritable(out_w, True)
        os.set_inheritable(in_r, True)

        root_dir = Path(tmp) / "home/root"
        root_dir.mkdir(parents=True, exist_ok=True)
        large_file = root_dir / ".openflux_test_large_80k.bin"
        payload_bytes = bytes((i * 13 + 7) & 0xFF for i in range(80 * 1024))
        large_file.write_bytes(payload_bytes)

        env = os.environ.copy()
        env["OPENFLUX_READY_FILE"] = str(ready_file)
        env["OPENFLUX_TEST_EP_FDS"] = f"{out_w},{in_r}"
        env["OPENFLUX_SYSROOT"] = tmp

        proc = subprocess.Popen([str(BRIDGE_BIN)], env=env, pass_fds=(out_w, in_r))
        os.close(out_w)
        os.close(in_r)

        try:
            deadline = time.monotonic() + 3.0
            while time.monotonic() < deadline and not ready_file.exists():
                time.sleep(0.01)
            assert ready_file.exists()

            path_bytes = b"/home/root/.openflux_test_large_80k.bin\x00"
            hdr = bytearray(512)
            struct.pack_into("<BII", hdr, 0, 101, 1, len(path_bytes))
            pkt = path_bytes.ljust(512, b"\x00")
            os.write(in_w, bytes(hdr) + pkt)

            resp_hdr = read_with_deadline(out_r, 512)
            rtype, rpkts, rbytes = struct.unpack_from("<BII", resp_hdr, 0)
            assert rtype == 1 and rbytes == len(payload_bytes) and rpkts == 160, (rtype, rpkts, rbytes)

            total_pkt_bytes = rpkts * 512
            body = bytearray()
            while len(body) < total_pkt_bytes:
                time.sleep(0.002)  # read slowly, so the bridge sees backpressure
                body.extend(read_some_with_deadline(out_r, min(2048, total_pkt_bytes - len(body))))
            assert bytes(body[:rbytes]) == payload_bytes

            proc.send_signal(signal.SIGTERM)
            assert proc.wait(timeout=2.0) == 0
            print("bridge >64 KiB GET_FILE under backpressure OK")
        finally:
            if large_file.exists():
                large_file.unlink()
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=2.0)
            os.close(in_w)
            os.close(out_r)


def main() -> None:
    if not BRIDGE_BIN.exists():
        raise FileNotFoundError(f"Missing compiled binary at {BRIDGE_BIN}")
    test_blocked_poll_sigterm()
    test_sigterm_before_read_race_window()
    test_uart_and_display_ipc_roundtrip()
    test_max_ipc_clients_with_backpressured_split_uart()
    test_usb_in_backpressure_does_not_stall_uart_or_ipc()
    test_large_get_file_over_64kib_under_backpressure()


if __name__ == "__main__":
    main()
