#!/usr/bin/env python3
"""Require real multicast loopback for both IP families before mDNS CI tests."""

import socket
import struct


def check(family: int) -> None:
    index = socket.if_nametoindex("lo0")
    with socket.socket(family, socket.SOCK_DGRAM) as sock:
        if family == socket.AF_INET:
            source = socket.inet_aton("127.0.0.1")
            group = socket.inet_aton("224.0.0.251")
            sock.bind(("0.0.0.0", 0))
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, source)
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_LOOP, 1)
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, group + source)
            destination = ("224.0.0.251", sock.getsockname()[1])
        else:
            group = socket.inet_pton(socket.AF_INET6, "ff02::fb")
            sock.bind(("::", 0))
            sock.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_MULTICAST_IF, index)
            sock.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_MULTICAST_LOOP, 1)
            sock.setsockopt(
                socket.IPPROTO_IPV6, socket.IPV6_JOIN_GROUP, group + struct.pack("@I", index)
            )
            destination = ("ff02::fb", sock.getsockname()[1], 0, index)
        payload = b"removent-mdns-ci-loopback"
        sock.settimeout(3)
        sock.sendto(payload, destination)
        received, sender = sock.recvfrom(128)
        if received != payload:
            raise RuntimeError(f"unexpected loopback payload: {received!r}")
        version = 6 if family == socket.AF_INET6 else 4
        print(f"IPv{version} multicast loopback OK: {sender}", flush=True)


if __name__ == "__main__":
    check(socket.AF_INET)
    check(socket.AF_INET6)
