#!/usr/bin/env python3
"""Rename Kimodo SOMA BVH joints to VRM humanoid bone names.

Waifu Studio's BVH importer matches VRM bone names exactly, but SOMA uses
names that collide with its Mixamo aliases (SOMA `LeftLeg` is the thigh, its
fingers have four joints, it has three spine joints). Joints with no VRM
counterpart get an `x_` prefix so they never match a bone. Only names change;
hierarchy, offsets and motion data are copied verbatim.

Usage: kimodo_to_vrm_bvh.py INPUT.bvh OUTPUT.bvh
"""
import re
import sys

FIXED = {
    "Hips": "hips",
    "Spine1": "spine",
    "Spine2": "chest",
    "Chest": "upperChest",
    "Neck1": "neck",
    "Head": "head",
    "Jaw": "jaw",
    "LeftEye": "leftEye",
    "RightEye": "rightEye",
}
LIMBS = {
    "Shoulder": "Shoulder",
    "Arm": "UpperArm",
    "ForeArm": "LowerArm",
    "Hand": "Hand",
    "Leg": "UpperLeg",
    "Shin": "LowerLeg",
    "Foot": "Foot",
    "ToeBase": "Toes",
}
FINGERS = {"Thumb": "Thumb", "Index": "Index", "Middle": "Middle", "Ring": "Ring", "Pinky": "Little"}
# SOMA thumbs have 3 joints (metacarpal first); other fingers have 4 (metacarpal unused by VRM).
THUMB_SEGMENTS = {"1": "Metacarpal", "2": "Proximal", "3": "Distal"}
FINGER_SEGMENTS = {"2": "Proximal", "3": "Intermediate", "4": "Distal"}


def vrm_name(joint: str) -> str:
    if joint in FIXED:
        return FIXED[joint]
    for side in ("Left", "Right"):
        if not joint.startswith(side):
            continue
        rest = joint[len(side):]
        prefix = side.lower()
        if rest in LIMBS:
            return prefix + LIMBS[rest]
        match = re.fullmatch(r"Hand(Thumb|Index|Middle|Ring|Pinky)(\d)", rest)
        if match:
            finger, number = match.groups()
            segments = THUMB_SEGMENTS if finger == "Thumb" else FINGER_SEGMENTS
            if number in segments:
                return prefix + FINGERS[finger] + segments[number]
    return "x_" + joint


def convert(text: str) -> str:
    def rename(match: re.Match) -> str:
        return f"{match.group(1)} {vrm_name(match.group(2))}"

    header, separator, motion = text.partition("MOTION")
    header = re.sub(r"^(\s*(?:ROOT|JOINT))\s+(\S+)", rename, header, flags=re.M)
    return header + separator + motion


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    with open(sys.argv[1], encoding="utf-8") as source:
        text = source.read()
    if "MOTION" not in text or "ROOT" not in text:
        sys.exit("Not a BVH file")
    with open(sys.argv[2], "w", encoding="utf-8") as target:
        target.write(convert(text))


if __name__ == "__main__":
    main()
