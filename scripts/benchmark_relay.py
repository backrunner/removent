#!/usr/bin/env python3
"""Measure the actual relay PID (not its load generator), on macOS or Linux.

Build first: cargo build --release -p removent-relay --bin removent-relay --example resource_benchmark
Run: python3 scripts/benchmark_relay.py --seconds 10 > /tmp/relay-resources.jsonl
CPU 100% = one logical core. RSS is sampled, not an allocator or cgroup limit.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import platform
import queue
import signal
import subprocess
import sys
import threading
import time


class TaskInfo(ctypes.Structure):
    _fields_ = [(name, ctypes.c_uint64) for name in (
        "virtual", "resident", "user", "system", "threads_user", "threads_system"
    )] + [(name, ctypes.c_int32) for name in (
        "policy", "faults", "pageins", "cow_faults", "messages_sent", "messages_received",
        "syscalls_mach", "syscalls_unix", "csw", "threads", "running", "priority"
    )]


def sampler():
    if sys.platform == "darwin":
        lib = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        lib.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
        lib.proc_pidinfo.restype = ctypes.c_int
        # proc_taskinfo uses Mach absolute ticks, not nanoseconds (1 tick =
        # 125/3 ns on this Apple Silicon Mac). Intel often masks this mistake.
        class Timebase(ctypes.Structure):
            _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]
        system = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
        system.mach_timebase_info.argtypes = [ctypes.POINTER(Timebase)]
        scale = Timebase()
        if system.mach_timebase_info(ctypes.byref(scale)) != 0 or not scale.denom:
            raise RuntimeError("mach_timebase_info failed")
        seconds_per_tick = scale.numer / scale.denom / 1e9

        def sample(pid):
            info = TaskInfo()
            if lib.proc_pidinfo(pid, 4, 0, ctypes.byref(info), ctypes.sizeof(info)) != ctypes.sizeof(info):
                raise OSError(ctypes.get_errno(), "proc_pidinfo failed")
            return (info.user + info.system) * seconds_per_tick, info.resident, info.threads
        return sample
    if sys.platform == "linux":
        ticks, page_size = os.sysconf("SC_CLK_TCK"), os.sysconf("SC_PAGE_SIZE")

        def sample(pid):
            fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
            return (int(fields[11]) + int(fields[12])) / ticks, int(fields[21]) * page_size, int(fields[17])
        return sample
    raise RuntimeError("Only macOS and Linux are supported")


def run(args, transport):
    sample = sampler()
    proc = subprocess.Popen([str(args.benchmark.resolve()), str(args.relay.resolve()), transport, str(args.seconds)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, start_new_session=True)
    messages = queue.Queue()

    def read():
        for line in proc.stdout:
            messages.put(line)
        messages.put(None)

    threading.Thread(target=read, daemon=True).start()
    active = None
    try:
        while True:
            try:
                line = messages.get(timeout=0.25)
            except queue.Empty:
                if active:
                    samples.append(sample(active["pid"]))
                continue
            if line is None:
                break
            event = json.loads(line)
            if event["event"] == "start":
                active = event
                started = time.monotonic()
                first = sample(event["pid"])
                samples = [first]
            else:
                if not active or active["phase"] != event["phase"]:
                    raise RuntimeError("Unmatched benchmark phase")
                last = sample(active["pid"])
                elapsed = time.monotonic() - started
                samples.append(last)
                rss = [s[1] / 1024**2 for s in samples]
                print(json.dumps({"transport": transport, "phase":active["phase"], "seconds":elapsed,
                    "cpu_one_core_pct":(last[0] - first[0]) / elapsed * 100,
                    "rss_mean_mib":sum(rss)/len(rss), "rss_peak_mib":max(rss),
                    "threads_peak":max(s[2] for s in samples),
                    **{k:v for k,v in event.items() if k not in ("event", "phase")}}), flush=True)
                active = None
                proc.stdin.write("next\n")
                proc.stdin.flush()
        if proc.wait() != 0:
            raise RuntimeError(f"{transport} benchmark failed")
    finally:
        # The benchmark's child relay must also stop if the driver is interrupted.
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        if proc.poll() is None:
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--relay", type=Path, default=Path("target/release/removent-relay"))
    parser.add_argument("--benchmark", type=Path, default=Path("target/release/examples/resource_benchmark"))
    parser.add_argument("--seconds", type=int, default=10)
    parser.add_argument("--transport", choices=["quic", "websocket", "both"], default="both")
    args = parser.parse_args()
    if not 2 <= args.seconds <= 300:
        parser.error("--seconds must be 2..300")
    print(json.dumps({"system":platform.platform(), "logical_cpus":os.cpu_count(),
        "tokio_worker_threads":os.environ.get("TOKIO_WORKER_THREADS", "default"),
        "note":"loopback, relay PID only; WebSocket excludes edge TLS/Worker/Container overhead; 100% CPU = one core"}), flush=True)
    for transport in (["quic", "websocket"] if args.transport == "both" else [args.transport]):
        run(args, transport)


if __name__ == "__main__":
    main()
