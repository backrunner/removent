"""Replay measured media bytes through a shared FIFO in source time.

Video bytes enter first at each tick, followed by that tick's patch bytes. A
parallel queue receives only the identical video bytes at the identical PTS.
The per-video completion difference isolates time spent behind earlier patches.
An optional link trace changes the service rate or blocks it at specified
timestamps. The model omits RTT, loss and encoder delay.
"""

import argparse
from bisect import bisect_left, bisect_right
import csv
from fractions import Fraction
import math
from pathlib import Path
import sys


def load(path):
    rows = []
    with path.open(newline="") as stream:
        for row in csv.DictReader(stream):
            sample = tuple(int(row[key]) for key in ("pts_us", "video_bytes", "patch_bytes"))
            if min(sample) < 0 or (rows and sample[0] <= rows[-1][0]):
                raise ValueError(f"{path}: negative value or non-increasing PTS at {sample[0]}")
            rows.append(sample)
    if not any(video or patch for _, video, patch in rows):
        raise ValueError(f"empty media trace: {path}")
    return rows


def require_same_video(traces):
    groups = {}
    for path, rows in traces.items():
        scene, _, variant = path.stem.rpartition("-")
        if variant in ("baseline", "hybrid", "paced"):
            group = groups.setdefault((path.parent, scene), {})
            group[variant] = [(pts, video) for pts, video, _ in rows]
    if not groups:
        raise ValueError("no baseline, hybrid or paced traces found")
    for (_, scene), group in groups.items():
        if set(group) != {"baseline", "hybrid", "paced"}:
            raise ValueError(f"{scene}: require baseline, hybrid and paced from the same directory")
        for variant in ("hybrid", "paced"):
            if group[variant] != group["baseline"]:
                raise ValueError(f"{scene}-{variant}: different per-frame PTS or video byte count")


class Service:
    """Cumulative capacity in (kbps × microseconds), exactly 8000 units/byte."""

    def __init__(self, kbps, changes):
        self.times, self.rates, self.units = [0], [kbps], [0]
        for pts, rate in changes:
            if pts == 0:
                self.rates[0] = rate
                continue
            self.units.append(self.at(pts))
            self.times.append(pts)
            self.rates.append(rate)

    def at(self, time_us):
        i = bisect_right(self.times, time_us) - 1
        return self.units[i] + (time_us - self.times[i]) * self.rates[i]

    def time(self, units, *, next_service=False):
        # At a plateau boundary, draining the last bit completes at the
        # beginning of the outage. The following bit starts after the outage.
        i = bisect_left(self.units, units)
        if not next_service and i < len(self.units) and self.units[i] == units:
            return self.times[i]
        i = bisect_right(self.units, units) - 1
        if not self.rates[i]:
            raise ValueError("trace ends blocked with outstanding media")
        return self.times[i] + Fraction(units - self.units[i], self.rates[i])


def read_link(path):
    changes = []
    with path.open(newline="") as source:
        for row in csv.DictReader(source):
            pts, rate, blocked = int(row["pts_us"]), int(row["kbps"]), row["blocked"]
            if (pts < 0 or rate < 0 or blocked not in ("true", "false")
                    or (changes and pts <= changes[-1][0])):
                raise ValueError(f"{path}: invalid or unordered link change {row}")
            changes.append((pts, 0 if blocked == "true" else rate))
    return changes


def summarize(path, rows, kbps, detail, changes=()):
    service = Service(kbps, changes)
    finish = video_only_finish = 0
    updates, videos, extra = [], [], []
    total_patches = peak_patch = 0
    longest_patch_service = 0

    def ms(us):
        return round(float(us) / 1000, 3)

    def percentile(values, q):
        return ms(sorted(values)[math.ceil(q * len(values)) - 1]) if values else ""

    for pts, video, patch in rows:
        arrival = service.at(pts)
        start = max(finish, arrival)
        video_finish = start + 8000 * video
        finish = video_finish + 8000 * patch
        video_only_finish = max(video_only_finish, arrival) + 8000 * video
        if video or patch:
            updates.append(service.time(finish) - pts)
        if video:
            videos.append(service.time(video_finish) - pts)
            extra.append(service.time(video_finish) - service.time(video_only_finish))
        if patch:
            # Wall time from the patch's service opportunity until completion;
            # includes any delivery outage while it is next in line.
            patch_start = service.time(video_finish) if video else max(pts, service.time(start))
            longest_patch_service = max(longest_patch_service, service.time(finish) - patch_start)
        total_patches += patch
        peak_patch = max(peak_patch, patch)
        if detail is not None:
            next_start = max(pts, service.time(start, next_service=bool(video or patch)))
            detail.writerow([path.stem, kbps, pts, video, patch, ms(next_start - pts),
                             ms(videos[-1]) if video else "",
                             ms(service.time(video_only_finish) - pts) if video else "",
                             ms(extra[-1]) if video else "",
                             ms(updates[-1]) if video or patch else ""])
    return [
        path.stem, kbps, len(updates), *(percentile(updates, q) for q in (0.95, 0.99, 1)),
        len(videos), *(percentile(videos, q) for q in (0.95, 0.99, 1)),
        sum(x > 0 for x in extra), percentile(extra, 0.95), percentile(extra, 1),
        total_patches, peak_patch, ms(longest_patch_service),
    ]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kbps", type=int, help="link payload capacity in decimal kilobits/s")
    parser.add_argument("trace", nargs="+", type=Path)
    parser.add_argument("--require-identical-video", action="store_true",
                        help="check PTS and video bytes across all three policy traces")
    parser.add_argument("--per-frame", type=Path,
                        help="optional per-source-tick timing CSV, including no-update ticks")
    parser.add_argument("--link-trace", type=Path,
                        help="pts_us,kbps,blocked CSV; apply changes at their timestamp, including mid-frame")
    args = parser.parse_args()
    if args.kbps < 1:
        parser.error("the link capacity must be positive")
    traces = {path: load(path) for path in args.trace}
    changes = read_link(args.link_trace) if args.link_trace else []
    if args.require_identical_video:
        require_same_video(traces)
    detail_file = args.per_frame.open("w", newline="") if args.per_frame else None
    try:
        detail = csv.writer(detail_file, lineterminator="\n") if detail_file else None
        if detail is not None:
            detail.writerow(["clip", "link_kbps", "pts_us", "video_bytes", "patch_bytes",
                             "queue_wait_ms", "video_delay_ms", "same_video_only_delay_ms",
                             "video_extra_wait_ms", "update_delay_ms"])
        out = csv.writer(sys.stdout, lineterminator="\n")
        # The original CLI and first six columns retain their meaning.
        out.writerow(("clip", "link_kbps", "updates", "delay_p95_ms", "delay_p99_ms", "delay_max_ms",
                      "videos", "video_p95_ms", "video_p99_ms", "video_max_ms", "videos_delayed_by_patch",
                      "video_extra_p95_ms", "video_extra_max_ms", "total_patch_bytes",
                      "peak_patch_bytes", "peak_patch_service_ms"))
        for trace, rows in traces.items():
            out.writerow(summarize(trace, rows, args.kbps, detail, changes))
    finally:
        if detail_file is not None:
            detail_file.close()
