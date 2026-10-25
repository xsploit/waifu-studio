#!/usr/bin/env python3
"""MIT-licensed one-invocation hook; stdout is reserved for API-v1 JSONL."""
import json
import os
from pathlib import Path
import sys

handshake = json.loads(sys.stdin.readline())
settings = handshake["message"].get("settings", {})
data = Path(os.environ["WAIFU_EXTENSION_DATA"])
receipt = data / "latest-hook.json"
target = None
requested = False
action_id = "hook-nod"


def record(value):
    receipt.write_text(json.dumps(value))


motion = settings.get("motion", "studio.example.hook-gesture/nod")
try:
    valid_motion = isinstance(motion, str) and len(motion.encode("utf-8")) <= 193 and not any(
        char.isspace() or ord(char) < 32 or 127 <= ord(char) <= 159 for char in motion)
except UnicodeEncodeError:
    valid_motion = False
if not valid_motion:
    record({"state": "error", "reason": "Invalid motion setting"})
    print("Invalid motion setting", file=sys.stderr)
    sys.exit(1)

record({"state": "started" if motion else "disabled", "session_id": handshake["session_id"], "generation": handshake["process_generation"]})
if not motion or not settings.get("active", True):
    sys.exit(0)
for line in sys.stdin:
    message = json.loads(line)["message"]
    if message["type"] == "result" and message["id"] == action_id:
        record(message)
        if message["result"]["status"] == "error":
            break
    if message["type"] != "event":
        continue
    event = message["event"]
    if event["type"] == "target.changed":
        target = event["target"]
    elif event["type"] == "motion.finished" and event["action_id"] == action_id:
        record(event)
        break
    elif event["type"] == "reply.finished" and not requested:
        if not target or event.get("character") != target["uuid"] or event["outcome"] != "completed":
            record({"state": "ignored", "reason": "Reply does not match the captured loaded character"})
            break
        requested = True
        print(json.dumps({
            "api_version": 1, "type": "action", "id": action_id,
            "session_id": handshake["session_id"], "process_generation": handshake["process_generation"],
            "sequence": 1, "target": target,
            "action": {"type": "motion.play", "motion_id": motion, "mode": "one_shot"},
        }), flush=True)
