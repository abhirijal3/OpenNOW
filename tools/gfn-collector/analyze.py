#!/usr/bin/env python3
import json
import os
import statistics
import struct
import sys
from collections import Counter

FRAME_TYPES = {1: "P", 2: "IDR", 4: "intra-refresh", 5: "post-invalidate", 6: "non-ref P"}


def records(path):
    with open(path, "rb") as handle:
        data = handle.read()
    offset = 0
    while offset + 12 <= len(data):
        stamp, length = struct.unpack_from("<QI", data, offset)
        offset += 12
        yield stamp, data[offset : offset + length]
        offset += length


def percentile(values, share):
    if not values:
        return 0
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(share * len(ordered)))]


def video(path):
    packets = list(records(path))
    if not packets:
        print("video: no packets")
        return
    start, end = packets[0][0], packets[-1][0]
    seconds = max((end - start) / 1e6, 1e-6)
    total = sum(len(p) for _, p in packets)
    sizes = Counter(len(p) for _, p in packets)
    frames = {}
    parity = 0
    reordered = 0
    previous_sequence = None
    for stamp, packet in packets:
        if len(packet) < 32:
            continue
        sequence = struct.unpack_from(">H", packet, 2)[0]
        if previous_sequence is not None:
            step = (sequence - previous_sequence) & 0xFFFF
            if step == 0 or step > 0x8000:
                reordered += 1
        previous_sequence = sequence
        _, frame, flags, fec = struct.unpack_from("<IIII", packet, 16)
        index, sources = (fec >> 12) & 0x3FF, (fec >> 22) & 0x3FF
        entry = frames.setdefault(frame, {"first": stamp, "last": stamp, "bytes": 0, "packets": 0, "type": (flags >> 20) & 0xF})
        entry["last"] = stamp
        entry["bytes"] += len(packet)
        entry["packets"] += 1
        if sources and index >= sources:
            parity += 1
    ordered = sorted(frames)
    spread = [frames[f]["last"] - frames[f]["first"] for f in ordered]
    done = [frames[f]["last"] for f in ordered]
    intervals = [b - a for a, b in zip(done, done[1:])]
    missing = (ordered[-1] - ordered[0] + 1 - len(ordered)) if ordered else 0
    types = Counter(FRAME_TYPES.get(frames[f]["type"], str(frames[f]["type"])) for f in ordered)
    frame_bytes = [frames[f]["bytes"] for f in ordered]
    print(f"video: {len(packets)} packets, {len(ordered)} frames in {seconds:.1f} s")
    print(f"  rate: {len(packets) / seconds:.0f} pkt/s, {total * 8 / seconds / 1e6:.2f} Mbps, {len(ordered) / seconds:.1f} fps")
    print(f"  packet sizes: {dict(sizes.most_common(4))}")
    print(f"  frame types: {dict(types)}; frame numbers skipped: {missing}")
    print(f"  frame bytes: median {statistics.median(frame_bytes):.0f}, p99 {percentile(frame_bytes, 0.99)}, max {max(frame_bytes)}")
    extended = []
    base = 0
    last = None
    for _, packet in packets:
        if len(packet) < 32:
            continue
        sequence = struct.unpack_from(">H", packet, 2)[0]
        if last is not None:
            delta = (sequence - last) & 0xFFFF
            if delta > 0x8000:
                delta -= 0x10000
            base += delta
        else:
            base = sequence
        last = sequence
        extended.append(base)
    unique = set(extended)
    lost = (max(unique) - min(unique) + 1) - len(unique)
    print(f"  parity packets: {parity} ({parity / len(packets) * 100:.1f}%); packets never received (GFN-leg loss): {lost} ({lost / (len(unique) + lost) * 100:.2f}%); arrived out of order: {reordered}; duplicates: {len(extended) - len(unique)}")
    print(f"  frame arrival spread (first to last packet): median {statistics.median(spread)} us, p99 {percentile(spread, 0.99)} us")
    if intervals:
        print(f"  frame-to-frame interval: median {statistics.median(intervals)} us, p99 {percentile(intervals, 0.99)} us, max {max(intervals)} us")


def audio(path):
    packets = list(records(path))
    if not packets:
        print("audio: no packets")
        return
    timestamps = [t for t, _ in packets]
    steps = Counter(b - a for a, b in zip(timestamps, timestamps[1:]))
    sizes = [len(p) for _, p in packets]
    duration = (timestamps[-1] - timestamps[0]) / 48000 if len(timestamps) > 1 else 0
    print(f"audio: {len(packets)} Opus packets over {duration:.1f} s of RTP time")
    print(f"  bytes: median {statistics.median(sizes):.0f}, max {max(sizes)}; RTP timestamp steps: {dict(steps.most_common(4))}")


def events(path):
    kinds = Counter()
    with open(path) as handle:
        for line in handle:
            try:
                kinds[json.loads(line).get("type", "?")] += 1
            except json.JSONDecodeError:
                kinds["unparsed"] += 1
    print(f"events: {dict(kinds)}")


def main():
    run = sys.argv[1]
    video(os.path.join(run, "video.bin"))
    audio(os.path.join(run, "audio.bin"))
    events(os.path.join(run, "events.jsonl"))


if __name__ == "__main__":
    main()
