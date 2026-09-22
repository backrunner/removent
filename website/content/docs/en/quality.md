---
title: "Picture & sound"
description: "How native sessions balance clarity, motion, and network conditions."
order: 8
---

## Native media

Removent uses ScreenCaptureKit for screen capture, VideoToolbox for hardware HEVC/H.264 encoding, and Opus for native audio. HEVC is the default; software AV1 is an opt-in development path with higher CPU cost.

These capabilities describe native RVP sessions. [VNC](/docs/vnc) and [RDP](/docs/rdp) use their own graphics paths and do not provide native RVP audio.

## Readability first

When a connection becomes constrained, the native host adjusts frame rate and bitrate using receiver feedback and sender pressure. It preserves negotiated capture dimensions and a minimum per-frame detail budget. A congested connection may refresh more slowly to keep text readable.

The current native capture fits within a **1920 × 1080 bounding box**. Preserving negotiated dimensions is not a promise of native 4K capture. Display scaling and viewer scaling also affect text size.

## Static screens

A frame is skipped when its pixels are identical to the previous successfully sent frame. Keyframe requests bypass this check. The relay forwards encrypted data without decoding or transcoding.

## Inspect a session

Press **Control + Command + I**, or use the gauge icon in the session toolbar, to toggle performance information. Frame rate and throughput vary with screen content and network conditions.

Local encode time and input send time are not end-to-end screen latency. First-frame delivery also includes network serialization, round-trip time, decoding, and possible retransmission.

## What to expect on a slow connection

Prefer stable links and readable font sizes. Full-screen motion, small colored text, and very low bandwidth are more demanding than a mostly static desktop. At very low bandwidth, waiting for a clear frame can make interaction visibly slower.

The repository contains [reproducible quality measurements](https://github.com/backrunner/removent/blob/main/docs/readable-quality-2026-09-20.md) with their test environment and limits; they are not performance guarantees for every Mac or network.
