#!/usr/bin/env python3
"""Replay encoder traces over local TCP with an application-payload rate limit.

The producer releases ticks on their source PTS. A FIFO writer serializes video
before patches and paces TCP writes against a monotonic clock. An independent
reader timestamps complete records and verifies their SHA-256. Times are in
microseconds relative to the scheduled start. The socket receives exactly the
.media contents; it does not receive the CSV or extra test framing.
"""

import argparse
import bisect
import csv
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
import hashlib
import math
from pathlib import Path
import queue
import socket
import time


clock = time.monotonic_ns


@dataclass
class Record:
    kind: str
    data: memoryview
    frame_id: int | None = None
    last_due: int = 0
    last_send: int = 0
    received: int = 0
    digest: str = ""


@dataclass
class Tick:
    pts: int
    video: int
    patch: int
    offset: int
    records: list[Record] = field(default_factory=list)
    released: int = 0
    service: int = 0


def load(stem: Path):
    data = memoryview(stem.with_suffix(".media").read_bytes())
    pos = 0
    ticks = []
    with stem.with_suffix(".csv").open() as f:
        for row in csv.DictReader(f):
            pts, video, patch = (int(row[k]) for k in ("pts_us", "video_bytes", "patch_bytes"))
            if min(video, patch) < 0 or (ticks and pts <= ticks[-1].pts):
                raise ValueError(f"invalid lengths or PTS at {pts}")
            tick = Tick(pts, video, patch, pos)
            end = pos + video
            if end + patch > len(data):
                raise ValueError(f"truncated media at {pts}")
            while pos < end:
                hdr = data[pos : pos + 27]
                if len(hdr) != 27 or hdr[0] != 1 or int.from_bytes(hdr[9:17], "big", signed=True) != pts:
                    raise ValueError(f"invalid RVP video header at {pts}")
                size = 27 + int.from_bytes(hdr[23:27], "little")
                if size == 27 or pos + size > end:
                    raise ValueError(f"invalid RVP video payload at {pts}")
                tick.records.append(Record("video", data[pos : pos + size], int.from_bytes(hdr[1:9], "big")))
                pos += size
            if patch:
                batch = data[pos : pos + patch]
                if (patch < 24 or int.from_bytes(batch[:4], "little") + 24 != patch
                        or int.from_bytes(batch[4:12], "little", signed=True) != pts):
                    raise ValueError(f"invalid patch header at {pts}")
                tick.records.append(Record("patch", batch))
                pos += patch
            ticks.append(tick)
    if not ticks or pos != len(data):
        raise ValueError("empty trace or unaccounted media bytes")
    return ticks


def wait_until(target):
    while (remaining := target - clock()) > 0:
        time.sleep(remaining / 1e9)


def send(sock, ready, kbps, quantum):
    # Absolute deadlines avoid adding a sleep overshoot to every chunk. Limit
    # catch-up after a scheduling stall to one write quantum of credit.
    quantum_ns = math.ceil(quantum * 8_000_000 / kbps)
    due = 0
    sent = 0
    writes = []
    with sock:
        while (tick := ready.get()) is not None:
            tick.service = clock()
            due = max(due, tick.released)
            for record in tick.records:
                for start in range(0, len(record.data), quantum):
                    chunk = record.data[start : start + quantum]
                    duration = (len(chunk) * 8_000_000 + kbps - 1) // kbps
                    due = max(due, clock() - quantum_ns) + duration
                    wait_until(due)
                    sock.sendall(chunk)
                    end = clock()
                    sent += len(chunk)
                    writes.append((end, sent))
                record.last_due, record.last_send = due, end
        sock.shutdown(socket.SHUT_WR)
    return writes


def receive(sock, ticks):
    with sock:
        for tick in ticks:
            for record in tick.records:
                data = bytearray(len(record.data))
                pending = memoryview(data)
                while pending:
                    count = sock.recv_into(pending)
                    if not count:
                        raise ValueError(f"early EOF at {tick.pts}")
                    pending = pending[count:]
                record.received = clock()
                record.digest = hashlib.sha256(data).hexdigest()
                if record.digest != hashlib.sha256(record.data).hexdigest():
                    raise ValueError(f"SHA-256 mismatch at {tick.pts}")
        if sock.recv(1):
            raise ValueError("extra bytes after final record")


def replay(stem, kbps, quantum):
    ticks = load(stem)
    ready = queue.SimpleQueue()
    with socket.create_server(("127.0.0.1", 0)) as listener:
        sender = socket.create_connection(listener.getsockname())
        receiver, _ = listener.accept()
    sender.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    with ThreadPoolExecutor(max_workers=2) as pool:
        reader = pool.submit(receive, receiver, ticks)
        writer = pool.submit(send, sender, ready, kbps, quantum)
        epoch = clock() + 50_000_000
        try:
            for tick in ticks:
                wait_until(epoch + (tick.pts - ticks[0].pts) * 1000)
                tick.released = clock()
                ready.put(tick)
        finally:
            ready.put(None)
        writes = writer.result()
        reader.result()
    return ticks, writes, epoch


FIELDS = (
    "scene variant kbps chunk_bytes tick pts_us deadline_us released_us queued_bytes "
    "writer_start_us video_bytes patch_bytes kind frame_id record_bytes sha256 "
    "last_write_due_us last_write_us complete_us age_us deadline_late_us measured_burst_bytes"
).split()


def measured_burst(writes, kbps):
    # For every subinterval, application bytes accepted by sendall are at most
    # its nominal service plus this measured burst (integer microbit accounting).
    debt = peak = previous_bytes = 0
    previous_time = writes[0][0] if writes else 0
    for now, total_bytes in writes:
        debt = max(0, debt - (now - previous_time) * kbps)
        debt += (total_bytes - previous_bytes) * 8_000_000
        peak = max(peak, debt)
        previous_time, previous_bytes = now, total_bytes
    return (peak + 7_999_999) // 8_000_000


def rows(scene, variant, kbps, quantum, ticks, writes, epoch):
    burst = measured_burst(writes, kbps)

    def us(stamp):
        return (stamp - epoch) // 1000

    for i, tick in enumerate(ticks):
        due = tick.pts - ticks[0].pts
        # The final tick has no measured next source PTS.
        deadline = ticks[i + 1].pts - ticks[0].pts if i + 1 < len(ticks) else None
        index = bisect.bisect_right(writes, (tick.released, math.inf))
        queued = tick.offset - (writes[index - 1][1] if index else 0)
        common = [scene, variant, kbps, quantum, i, tick.pts, deadline, us(tick.released),
                  queued, us(tick.service), tick.video, tick.patch]
        if not tick.records:
            yield common + ["idle", None, 0, "", None, None, None, None, None, burst]
        for record in tick.records:
            complete = us(record.received)
            yield common + [
                record.kind, record.frame_id, len(record.data), record.digest,
                us(record.last_due), us(record.last_send), complete, complete - due,
                max(0, complete - deadline) if deadline is not None else None, burst,
            ]


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[math.ceil(len(ordered) * fraction) - 1]


def summarize(trial, variant, ticks, rows):
    values = [dict(zip(FIELDS, row)) for row in rows]
    videos = [r for r in values if r["kind"] == "video"]
    if not videos:
        raise ValueError(f"{variant} has no video records")
    # Separate ideal source-PTS FIFO serialization from measured receipt. This
    # residual also contains the sender timer, socket calls and reader timing.
    first_pts = ticks[0].pts
    ideal_us = 0.0
    residuals = []
    for r in values:
        ideal_us = max(ideal_us, r["pts_us"] - first_pts)
        ideal_us += 8_000 * r["record_bytes"] / r["kbps"]
        if r["kind"] == "video":
            residuals.append(r["complete_us"] - ideal_us)
    ages = [r["age_us"] for r in videos]
    queue_us = max(r["writer_start_us"] - r["released_us"] for r in values)
    print(f"trial={trial} {variant} frames={len(ages)} verified_bytes={sum(r['record_bytes'] for r in values)} "
          f"video_age_p95_ms={percentile(ages, .95) / 1000:.3f} "
          f"video_age_max_ms={max(ages) / 1000:.3f} max_fifo_wait_ms={queue_us / 1000:.3f} "
          f"video_rx_minus_ideal_p95_ms={percentile(residuals, .95) / 1000:.3f} "
          f"max_queue_bytes={max(r['queued_bytes'] for r in values)} "
          f"max_patch_bytes={max(t.patch for t in ticks)} "
          f"measured_rate_envelope_burst_bytes={values[0]['measured_burst_bytes']}", flush=True)


def preserved_trial_zero(path, variants):
    # The initial run predates replicated trials. Preserve its actual data when
    # requested; a repeat of this command replaces just the crossover trials.
    with path.open(newline="") as f:
        reader = csv.DictReader(f)
        if reader.fieldnames not in (FIELDS, ["trial", "run_order", "context", *FIELDS]):
            raise ValueError("unrecognized initial evidence schema")
        return [
            [0, variants.index(row["variant"]) + 1, "initial_high_load", *[row[k] for k in FIELDS]]
            for row in reader if row.get("trial", "0") == "0"
        ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trace-dir", required=True, type=Path)
    parser.add_argument("--scene", default="mixed")
    parser.add_argument("--variants", nargs="+", default=["baseline", "hybrid", "paced"])
    parser.add_argument("--kbps", type=int, default=2000)
    parser.add_argument("--chunk-bytes", type=int, default=1024)
    parser.add_argument("--trials", type=int, default=1,
                        help="rotate variant order by one position per trial")
    parser.add_argument("--keep-trial-zero", action="store_true",
                        help="retain the previous high-load run in the output as trial 0")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if min(args.kbps, args.chunk_bytes, args.trials) <= 0:
        parser.error("rate, chunk size and trials must be positive")
    if len(set(args.variants)) != len(args.variants):
        parser.error("variants must be unique")

    evidence = preserved_trial_zero(args.output, args.variants) if args.keep_trial_zero else []
    fingerprints = {}
    for run in range(args.trials):
        split = run % len(args.variants)
        order = args.variants[split:] + args.variants[:split]
        for position, variant in enumerate(order, start=1):
            stem = args.trace_dir / f"{args.scene}-{variant}"
            ticks, writes, epoch = replay(stem, args.kbps, args.chunk_bytes)
            records = list(rows(args.scene, variant, args.kbps, args.chunk_bytes, ticks, writes, epoch))
            fingerprint = [(t.pts, t.video, t.patch, [r.digest for r in t.records]) for t in ticks]
            if fingerprints.setdefault(variant, fingerprint) != fingerprint:
                raise ValueError(f"{variant} source changed between trials")
            summarize(run + 1, variant, ticks, records)
            evidence.extend([run + 1, position, "crossover", *row] for row in records)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", newline="") as f:
        writer = csv.writer(f, lineterminator="\n")
        writer.writerow(["trial", "run_order", "context", *FIELDS])
        writer.writerows(evidence)


if __name__ == "__main__":
    main()
