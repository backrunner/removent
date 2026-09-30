#!/usr/bin/env python3
"""Opt-in loopback RFB fixture for the mobile UI suite; synthetic pixels/input only."""
import http.server
import json
import socketserver
import struct
import threading
import time

inputs = 0
lock = threading.Lock()


class VNC(socketserver.BaseRequestHandler):
    def read(self, length):
        data = bytearray()
        while len(data) < length:
            chunk = self.request.recv(length - len(data))
            if not chunk:
                raise EOFError
            data.extend(chunk)
        return bytes(data)

    def handle(self):
        global inputs
        try:
            self.request.settimeout(30)
            self.request.sendall(b"RFB 003.008\n")
            self.read(12)
            self.request.sendall(b"\x01\x01")  # No authentication on loopback.
            if self.read(1) != b"\x01":
                return
            self.request.sendall(struct.pack(">I", 0))
            self.read(1)
            width, height = 640, 360
            fmt = struct.pack(">BBBBHHHBBB3x", 32, 24, 0, 1, 255, 255, 255, 16, 8, 0)
            name = b"Synthetic VNC computer"
            self.request.sendall(struct.pack(">HH", width, height) + fmt + struct.pack(">I", len(name)) + name)
            frame = bytes(component for y in range(height) for x in range(width)
                          for component in (x % 256, y % 256, 180, 0))
            while True:
                kind = self.read(1)[0]
                if kind == 0:
                    self.read(3)
                    fmt = self.read(16)
                    if fmt != struct.pack(">BBBBHHHBBB3x", 32, 24, 0, 1, 255, 255, 255, 16, 8, 0):
                        raise ValueError("Fixture expects BGRA true color")
                elif kind == 2:
                    header = self.read(3)
                    self.read(struct.unpack(">H", header[1:])[0] * 4)
                elif kind == 3:
                    self.read(9)
                    time.sleep(1 / 30)
                    self.request.sendall(struct.pack(">BBHHHHHi", 0, 0, 1, 0, 0, width, height, 0) + frame)
                elif kind in (4, 5):
                    self.read(7 if kind == 4 else 5)
                    with lock:
                        inputs += 1
                elif kind == 6:
                    header = self.read(7)
                    length = struct.unpack(">I", header[3:])[0]
                    if length > 1024 * 1024:
                        return
                    self.read(length)
                else:
                    raise ValueError(f"Unsupported client message {kind}")
        except (EOFError, ConnectionError, TimeoutError):
            pass


class Status(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with lock:
            data = json.dumps({"vnc": inputs}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_):
        pass


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


if __name__ == "__main__":
    with Server(("127.0.0.1", 5901), VNC) as server:
        status = http.server.ThreadingHTTPServer(("127.0.0.1", 3392), Status)
        threading.Thread(target=status.serve_forever, daemon=True).start()
        print("Synthetic VNC: 127.0.0.1:5901; status: http://127.0.0.1:3392", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass
        finally:
            status.shutdown()
