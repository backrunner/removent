"""Check the held and updated pictures across the 10 s theme change.

python3 scripts/qa/region_late_edit.py <late-edit replay directory> > late-edit.json
Requires Pillow. Input: the 1280x720, 30 fps late-edit experiment.
"""
import argparse
from bisect import bisect_right
import csv
import json
from pathlib import Path

from PIL import Image, ImageChops

WIDTH, HEIGHT, FPS = 1280, 720, 30
VARIANTS = ("baseline", "hybrid", "paced")
TICKS = (299, 300, 301)


def pictures(path):
    timestamps = list(map(int, path.with_suffix(".pts").read_text().split()))
    size = WIDTH * HEIGHT * 4
    bgra = path.with_suffix(".bgra")
    if sorted(set(timestamps)) != timestamps or bgra.stat().st_size != size * len(timestamps):
        raise ValueError(f"bad replay index for {path}")
    result = {}
    with bgra.open("rb") as stream:
        for tick in TICKS:
            index = bisect_right(timestamps, tick * 1_000_000 // FPS) - 1
            if index < 0:
                raise ValueError(f"no visible picture at tick {tick}")
            stream.seek(index * size)
            image = Image.frombytes("RGBA", (WIDTH, HEIGHT), stream.read(size), "raw", "BGRA")
            result[tick] = timestamps[index], image
    return result


def records(path):
    result = {}
    with path.with_suffix(".csv").open(newline="") as source, path.with_suffix(".media").open("rb") as wire:
        for tick, row in enumerate(csv.DictReader(source)):
            video, patch, pts = (int(row[k]) for k in ("video_bytes", "patch_bytes", "pts_us"))
            start = wire.tell()
            if tick in TICKS:
                if pts != tick * 1_000_000 // FPS:
                    raise ValueError(f"unexpected timestamp in {path}")
                flags = None
                if video:
                    # RVP video header, followed by one HEVC access unit.
                    header = wire.read(27)
                    if (len(header) != 27 or header[0] != 0x01
                            or int.from_bytes(header[9:17], "big", signed=True) != pts
                            or int.from_bytes(header[23:27], "little") != video - 27):
                        raise ValueError(f"mismatched video header in {path}")
                    flags = header[17]
                reset = bool((flags or 0) & 0x80)
                if reset != bool(int(row["reset_patches"])):
                    raise ValueError(f"trace/reset flag mismatch in {path}")
                result[tick] = dict(pts_us=pts, video_bytes=video, patch_bytes=patch,
                                    patch_budget_bytes=int(row["patch_budget_bytes"]) if row["patch_budget_bytes"] else None,
                                    invalidations=int(row["invalidated"]), video_header_flags=flags,
                                    reset_in_video_header=reset)
            wire.seek(start + video + patch)
        if wire.tell() != path.with_suffix(".media").stat().st_size:
            raise ValueError(f"media length mismatch in {path}")
    return result


def changed_pixels(a, b):
    channels = ImageChops.difference(a, b).split()
    delta = channels[0]
    for channel in channels[1:]:
        delta = ImageChops.lighter(delta, channel)
    return WIDTH * HEIGHT - delta.histogram()[0]


def main(directory):
    result, baseline = {}, None
    for variant in VARIANTS:
        path = directory / f"late-edit-{variant}"
        displayed = pictures(path)
        observed = records(path)
        if baseline is None:
            baseline = displayed
        for tick, sample in observed.items():
            shown_pts, frame = displayed[tick]
            sample["displayed_pts_us"] = shown_pts
            sample["pixels_different_from_baseline"] = changed_pixels(frame, baseline[tick][1])
        result[variant] = observed
    changed = result["paced"][300]
    if (result["paced"][299]["pixels_different_from_baseline"] == 0
            or changed["invalidations"] == 0 or changed["patch_budget_bytes"] != 0
            or not changed["reset_in_video_header"]
            or changed["patch_bytes"] or changed["pixels_different_from_baseline"]
            or result["paced"][301]["pixels_different_from_baseline"]):
        raise ValueError("active patches failed to clear on the replacement video frame")
    return dict(width=WIDTH, height=HEIGHT, fps=FPS, edit_tick=300,
                comparison="All four BGRA channels; each display holds the last emitted picture at or before source PTS.",
                observations=result)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("replay_dir", type=Path)
    print(json.dumps(main(parser.parse_args().replay_dir), indent=2))
