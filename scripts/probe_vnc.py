#!/usr/bin/env python3
"""Probe an RFB greeting and authentication challenge without sending credentials."""

import argparse
import socket
import struct
import time


def read_exact(stream, size):
    result = bytearray()
    while len(result) < size:
        data = stream.recv(size - len(result))
        if not data:
            raise ConnectionError("Server closed the connection")
        result.extend(data)
    return bytes(result)


def probe(host, port, security, timeout):
    stage = "TCP connection"
    started = time.monotonic()
    try:
        with socket.create_connection((host, port), timeout) as stream:
            stream.settimeout(timeout)
            stage = "RFB greeting"
            banner = read_exact(stream, 12)
            print(f"Server: {banner.decode('ascii', errors='replace').strip()}")
            if not banner.startswith(b"RFB 003.") or int(banner[8:11]) < 8:
                raise ValueError("This diagnostic requires an RFB 3.8-compatible server")
            stream.sendall(b"RFB 003.008\n")
            stage = "security offer"
            types = list(read_exact(stream, read_exact(stream, 1)[0]))
            print(f"Offered authentication types: {types}")
            if security not in types:
                raise ValueError(f"Server does not offer type {security}")
            stream.sendall(bytes([security]))
            stage = f"type-{security} challenge"
            # Only read public challenge parameters; never send account credentials.
            if security in (30, 35):
                generator, key_bytes = struct.unpack(">HH", read_exact(stream, 4))
                if not 16 <= key_bytes <= 512:
                    raise ValueError(f"Unexpected DH key length: {key_bytes}")
                read_exact(stream, key_bytes * 2)
                print(f"ARD DH challenge received: generator={generator}, key={key_bytes * 8} bits")
            else:
                read_exact(stream, 16)
                print("VNC password challenge received")
            print(f"Challenge ready in {time.monotonic() - started:.3f}s; no login attempted.")
            return 0
    except (OSError, ValueError) as error:
        print(f"Failed during {stage} after {time.monotonic() - started:.3f}s: {error}")
        return 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("host")
    parser.add_argument("--port", type=int, default=5900)
    parser.add_argument("--security", type=int, choices=(2, 30, 35), default=30)
    parser.add_argument("--timeout", type=float, default=5)
    args = parser.parse_args()
    raise SystemExit(probe(args.host, args.port, args.security, args.timeout))
