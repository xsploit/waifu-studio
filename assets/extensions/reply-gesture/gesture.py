#!/usr/bin/env python3
"""MIT-licensed persistent event/action example. Stdout is JSONL only."""
import json
import os
from pathlib import Path
import sys
import time

handshake = json.loads(sys.stdin.readline())
settings = handshake["message"].get("settings", {})
grants = set(handshake["message"]["capabilities"])
target = None
pending = None
sequence = 0
last_request = float("-inf")
receipt = Path(os.environ["WAIFU_EXTENSION_DATA"]) / "latest.json"


def record(value):
    receipt.write_text(json.dumps(value))


motion = settings.get("motion", "studio.example.reply-gesture/nod")
try:
    valid_motion = isinstance(motion, str) and len(motion.encode("utf-8")) <= 193 and not any(
        char.isspace() or ord(char) < 32 or 127 <= ord(char) <= 159 for char in motion)
except UnicodeEncodeError:
    valid_motion = False
if not valid_motion:
    record({"state": "error", "reason": "Invalid motion setting"})
    print("Invalid motion setting", file=sys.stderr)
    sys.exit(1)

record({"state": "waiting" if motion else "disabled", "session_id": handshake["session_id"]})
for line in sys.stdin:
    message = json.loads(line)["message"]
    if message["type"] == "result":
        if message["id"] == pending:
            record({"action_id": pending, "result": message["result"]})
            if message["result"]["status"] == "error":
                pending = None
        continue
    if message["type"] != "event":
        continue
    event = message["event"]
    if event["type"] == "target.changed":
        target = event["target"]
        continue
    if event["type"] == "motion.finished":
        if event["action_id"] == pending:
            record(event)
            pending = None
        continue
    if not motion or not settings.get("active", True) or pending or not target:
        continue
    if not {"lifecycle.observe", "motion.play"}.issubset(grants):
        continue
    trigger = {"Reply completed": "reply.finished", "Speech started": "speech.started"}.get(
        settings.get("trigger", "Reply completed"), "reply.finished")
    if event["type"] != trigger:
        continue
    if event.get("character") != target["uuid"]:
        continue
    if event["type"] == "reply.finished" and event["outcome"] != "completed":
        continue
    now = time.monotonic()
    if now - last_request < settings.get("cooldown_seconds", 5):
        continue
    sequence += 1
    pending = f"gesture-{sequence}"
    last_request = now
    print(json.dumps({
        "api_version": 1, "type": "action", "id": pending,
        "session_id": handshake["session_id"],
        "process_generation": handshake["process_generation"],
        "sequence": sequence, "target": target,
        "action": {"type": "motion.play", "motion_id": motion, "mode": "one_shot"},
    }), flush=True)
