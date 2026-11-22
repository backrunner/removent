#!/usr/bin/env python3
"""Score fixed native-pixel lines from the mixed scene on macOS (Pillow + Vision).

python3 scripts/qa/region_compression_ocr.py <replay-dir> <output-dir>
Uses the 1280x720, 120-tick, 30 fps experiment. Samples require an exact PTS
match in every replay. Rerunning produces ocr.json and mixed-text-crops.png.
"""

import argparse
import csv
from collections import defaultdict
import json
from pathlib import Path
import subprocess
import tempfile
import unicodedata

from PIL import Image, ImageChops, ImageDraw, ImageStat

ROOT = Path(__file__).resolve().parents[2]
W, H, FPS = 1280, 720, 30
CROP_W, LINE_H = 520, 30
EDIT_TICK = 24
NETWORK = "网络较慢时保留文字细节，降低画面刷新频率。"
SETTINGS = "连接设置：中继地址、端口、凭据与受信任的设备"


def lines(tick):
    # Each row is (name, output y, original PNG y, transcription).
    return [
        ("edited", 30, 110 if tick < EDIT_TICK else 160, SETTINGS if tick < EDIT_TICK else NETWORK),
        ("stable", 60, 160, NETWORK),
        ("code", 120, 260, "let status = connection.is_ready() && daemon.running;"),
        ("small10", 210, 483, "字号 10 px — 状态正常 / Ready 1920 x 1080"),
        ("small12", 240, 540, "字号 12 px — 状态正常 / Ready 1920 x 1080"),
    ]


def normalize(text):
    return "".join(unicodedata.normalize("NFKC", text).split())


def distance(left, right):
    row = list(range(len(right) + 1))
    for i, a in enumerate(left, 1):
        next_row = [i]
        for j, b in enumerate(right, 1):
            next_row.append(min(next_row[-1] + 1, row[j] + 1, row[j - 1] + (a != b)))
        row = next_row
    return row[-1]


def read_replay(directory, variant, frames):
    path = directory / f"mixed-{variant}"
    timestamps = list(map(int, path.with_suffix(".pts").read_text().split()))
    if timestamps != sorted(set(timestamps)):
        raise ValueError(f"invalid timestamp order for {variant}")
    bgra = path.with_suffix(".bgra")
    stride = W * H * 4
    if bgra.stat().st_size != stride * len(timestamps):
        raise ValueError(f"incomplete or unexpected resolution in {bgra}")
    result = {}
    with bgra.open("rb") as stream:
        for tick in frames:
            frame_index = timestamps.index(tick * 1_000_000 // FPS)
            stream.seek(frame_index * stride)
            result[tick] = Image.frombytes("RGBA", (W, H), stream.read(stride), "raw", "BGRA")
    return result


def any_channel(image):
    red, green, blue = image.split()
    return ImageChops.lighter(red, ImageChops.lighter(green, blue))


def repair_times(directory, variants, output, original):
    """Audit the changed source pixels on every actual mixed replay output."""
    with Image.open(original) as source:
        before, after = [source.crop((22, y, 22 + CROP_W, y + LINE_H)).convert("RGB")
                         for y in (110, 160)]
    changed = any_channel(ImageChops.difference(before, after)).point(lambda x: 255 if x else 0)
    count = changed.histogram()[255]
    summary = {}
    with (output / "edit-repair.csv").open("w", newline="") as file:
        rows = csv.writer(file, lineterminator="\n")
        rows.writerow(["variant", "tick", "pts_us", "changed_source_pixels", "exact_current_pixels",
                       "changed_pixel_rgb_mae", "full_line_exact"])
        for variant in variants:
            if variant == "source":
                continue
            path = directory / f"mixed-{variant}"
            timestamps = list(map(int, path.with_suffix(".pts").read_text().split()))
            first_before = first_after = first_line_after = None
            exact_count = 0
            with path.with_suffix(".bgra").open("rb") as stream:
                for idx, pts in enumerate(timestamps):
                    tick = (pts * FPS + 500_000) // 1_000_000
                    if tick * 1_000_000 // FPS != pts:
                        raise ValueError(f"unexpected timestamp {pts} in {path}")
                    # Read only the 30 rows needed for the edited line.
                    stream.seek((idx * H + LINE_H) * W * 4)
                    strip = Image.frombytes("RGBA", (W, LINE_H), stream.read(W * LINE_H * 4),
                                            "raw", "BGRA")
                    crop = strip.crop((0, 0, CROP_W, LINE_H)).convert("RGB")
                    reference = after if tick >= EDIT_TICK else before
                    delta = ImageChops.difference(reference, crop)
                    exact_count = any_channel(delta).histogram(mask=changed)[0]
                    mae = sum(ImageStat.Stat(delta, changed).sum) / (count * 3)
                    line_exact = delta.getbbox() is None
                    rows.writerow([variant, tick, pts, count, exact_count, round(mae, 4),
                                   int(line_exact)])
                    if exact_count == count:
                        if tick < EDIT_TICK and first_before is None:
                            first_before = tick
                        if tick >= EDIT_TICK and first_after is None:
                            first_after = tick
                    if line_exact and tick >= EDIT_TICK and first_line_after is None:
                        first_line_after = tick
            summary[variant] = dict(first_exact_changed_pixels_before_edit_tick=first_before,
                                    first_exact_changed_pixels_after_edit_tick=first_after,
                                    first_exact_full_line_after_edit_tick=first_line_after,
                                    source_time_to_exact_changed_ms=(
                                        round((first_after - EDIT_TICK) * 1000 / FPS, 3)
                                        if first_after is not None else None),
                                    final_exact_changed_pixels=exact_count, changed_pixels=count)
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("replay_dir", type=Path)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--variants", nargs="+", default=["baseline", "hybrid", "matched"],
                        help="mixed replay variants; the source reference is always included")
    parser.add_argument("--ticks", type=int, nargs="+", default=[23, 24, 119])
    parser.add_argument("--repair", action="store_true",
                        help="also measure changed-glyph pixel convergence across all outputs")
    args = parser.parse_args()
    variants = ["source"] + args.variants
    if len(set(variants)) != len(variants):
        parser.error("duplicate variants")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    original = ROOT / "docs/benchmarks/2026-09-20-readability/source.png"
    records = []
    groups = defaultdict(list)
    with tempfile.TemporaryDirectory(prefix="removent-ocr-") as tmp:
        tmp = Path(tmp)
        images = []
        for variant in variants:
            frames = {} if variant == "source" else read_replay(args.replay_dir, variant, args.ticks)
            with Image.open(original) as reference:
                for tick in args.ticks:
                    for label, y, original_y, expected in lines(tick):
                        if variant == "source":
                            crop = reference.crop((22, original_y, 22 + CROP_W, original_y + LINE_H))
                        else:
                            crop = frames[tick].crop((0, y, CROP_W, y + LINE_H))
                        path = tmp / f"{tick}-{variant}-{label}.png"
                        crop.convert("RGB").save(path)
                        images.append(str(path))
                        records.append(dict(tick=tick, pts_us=tick * 1_000_000 // FPS,
                                            variant=variant, line=label, expected=expected,
                                            crop=[0, y, CROP_W, LINE_H], file=path.name))
        swift = ROOT / "scripts/fixtures/readability_fixture.swift"
        raw = subprocess.check_output(["swift", str(swift), "recognize", *images], text=True)
        readings = {row["image"]: row["lines"] for row in map(json.loads, raw.splitlines())}
        source_readings = {}
        for record in records:
            record["recognized"] = readings[record.pop("file")]
            observed = normalize("".join(record["recognized"]))
            expected = normalize(record["expected"])
            key = (record["tick"], record["line"])
            if record["variant"] == "source":
                source_readings[key] = observed
            record["edits"] = distance(expected, observed)
            record["reference_chars"] = len(expected)
            record["edits_vs_source_ocr"] = distance(source_readings[key], observed)
            groups[record["variant"]].append(record)
        # Native-size changed line, sampled before and after the edit. Labels
        # are outside every crop, so pixels remain comparable at 1:1 zoom.
        margin, line_spacing = 86, 32
        sheet = Image.new("RGB", (margin + CROP_W, len(args.ticks) * (24 + len(variants) * line_spacing)), "white")
        draw = ImageDraw.Draw(sheet)
        top = 0
        for tick in args.ticks:
            draw.text((5, top + 4), f"tick {tick:03}  |  {tick * 1_000_000 // FPS} us", fill="black")
            top += 24
            for variant in variants:
                draw.text((5, top + 8), variant, fill="black")
                with Image.open(tmp / f"{tick}-{variant}-edited.png") as crop:
                    sheet.paste(crop, (margin, top))
                top += line_spacing
        sheet.save(args.output_dir / "mixed-text-crops.png")
    summary = {}
    for variant, rows in groups.items():
        edits = sum(r["edits"] for r in rows)
        chars = sum(r["reference_chars"] for r in rows)
        summary[variant] = dict(samples=len(rows), exact=sum(r["edits"] == 0 for r in rows),
                                edits=edits, reference_chars=chars, cer=round(edits / chars, 5),
                                exact_vs_source_ocr=sum(r["edits_vs_source_ocr"] == 0 for r in rows))
    report = dict(recognizer="Apple Vision accurate zh-Hans/en-US; no language correction",
                  source="docs/benchmarks/2026-09-20-readability/source.png",
                  metric="Character edits after NFKC and whitespace removal. Counts include repeated lines across timestamps.",
                  frame_format="1280x720 BGRA; exact presentation PTS in microseconds",
                  summary=summary, samples=records)
    if args.repair:
        report["edit_tick"] = EDIT_TICK
        report["pixel_repair"] = repair_times(args.replay_dir, variants, args.output_dir, original)
    (args.output_dir / "ocr.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps(summary, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
