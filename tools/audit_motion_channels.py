"""Inventory every BVH, without claiming geometric or visual clearance."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import re


def inspect(path):
    raw = path.read_bytes()
    header, motion = raw.decode("utf-8").split("MOTION", 1)
    joint = None
    columns = []
    for line in header.splitlines():
        match = re.search(r"(?:ROOT|JOINT)\s+(\S+)", line)
        if match:
            joint = match[1]
        if "CHANNELS" in line:
            channels = line.split("CHANNELS", 1)[1].split()
            columns.extend((joint, channel) for channel in channels[1:])
    lines = [line.strip() for line in motion.splitlines() if line.strip()]
    count = int(lines[0].split(":", 1)[1])
    cadence = float(lines[1].split(":", 1)[1])
    rows = [[float(v) for v in line.split()] for line in lines[2:]]
    if len(rows) != count or any(len(row) != len(columns) for row in rows):
        raise ValueError("Frame/channel count mismatch")
    if not all(math.isfinite(v) for row in rows for v in row):
        raise ValueError("Non-finite motion sample")
    eye = []
    for index, (name, channel) in enumerate(columns):
        if name.lower() in ("lefteye", "righteye"):
            values = [row[index] for row in rows]
            eye.append({"joint": name, "channel": channel, "min": min(values), "max": max(values)})
    return {"clip": path.name, "sha256": hashlib.sha256(raw).hexdigest(), "frames": count, "fps": 1/cadence, "duration": count*cadence, "channels": len(columns), "eye_tracks": eye, "visual_status": "needs_review", "collision_status": "not_validated"}


def run(root, output):
    clips = []
    for path in sorted(root.glob("*.bvh")):
        try:
            clips.append(inspect(path))
        except Exception as error:
            clips.append({"clip": path.name, "error": str(error), "visual_status": "rejected_import"})
    result = {"scope": "Full source-frame/channel inventory; no target-avatar collision acceptance", "clips": clips, "total": len(clips), "total_source_frames": sum(c.get("frames", 0) for c in clips), "clips_with_eye_channels": sum(bool(c.get("eye_tracks")) for c in clips)}
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps({k:v for k,v in result.items() if k != "clips"}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    run(args.root, args.output)
