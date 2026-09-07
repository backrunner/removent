#!/usr/bin/env python3
"""Fake removentd: answers status requests per the IPC protocol and pushes
events, used by the tray integration test.

Usage: fake_daemon.py <data-dir>
Listens on {data-dir}/run/removentd.sock.
"""
import json
import os
import socket
import sys
import threading
import time

STATUS = {
    "type": "status",
    "running": True,
    "port": 7999,
    "device_name": "测试 Mac",
    "fp_short": "ab12cd34",
    "sessions": [
        {
            "id": 1,
            "peer_name": "客厅 iPad",
            "peer_fp16": "0011223344556677",
            "since_unix": int(time.time()) - 125,
            "video_codec": "h264",
        }
    ],
    "pending_pin": "482913",
    "tray_connected": True,
    "screen_recording_granted": True,
    "accessibility_granted": True,
}


def handle(conn):
    f = conn.makefile("rw", encoding="utf-8", newline="\n")

    def push(msg):
        f.write(json.dumps(msg, ensure_ascii=False) + "\n")
        f.flush()

    # Push events proactively once the connection is established
    push({"type": "pairing_pin", "pin": "482913"})
    time.sleep(0.5)
    push({
        "type": "admission_request",
        "request_id": 42,
        "peer_name": "客厅 iPad",
        "peer_fp16": "0011223344556677",
    })
    time.sleep(0.5)
    push({"type": "admission_resolved", "request_id": 42, "allow": False})

    while True:
        line = f.readline()
        if not line:
            break
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        mtype = msg.get("type")
        print(f"[fake-daemon] recv: {line.strip()}", flush=True)
        if mtype == "status":
            push(STATUS)
        elif mtype == "set_enabled":
            STATUS["running"] = bool(msg.get("on"))
            push({"type": "ok"})
            push({"type": "state_changed", "running": STATUS["running"]})
        elif mtype == "shutdown":
            push({"type": "ok"})
            break
    conn.close()


def main():
    data_dir = sys.argv[1]
    sock_path = os.path.join(data_dir, "run", "removentd.sock")
    os.makedirs(os.path.dirname(sock_path), exist_ok=True)
    if os.path.exists(sock_path):
        os.unlink(sock_path)

    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(sock_path)
    server.listen(4)
    print(f"[fake-daemon] listening on {sock_path}", flush=True)

    while True:
        conn, _ = server.accept()
        print("[fake-daemon] client connected", flush=True)
        threading.Thread(target=handle, args=(conn,), daemon=True).start()


if __name__ == "__main__":
    main()
